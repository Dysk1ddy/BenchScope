#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RamPhaseStatus {
    Complete,
    TimeBudget,
}

struct RamProgressEmitter {
    tx: Sender<RamWorkerEvent>,
    start: Instant,
    total_units: u128,
    completed_units: u128,
    phase: String,
    pass: usize,
    tested_bytes: u64,
    last_emit: Instant,
}

impl RamProgressEmitter {
    fn new(
        tx: Sender<RamWorkerEvent>,
        start: Instant,
        total_units: u128,
        tested_bytes: u64,
    ) -> Self {
        Self {
            tx,
            start,
            total_units: total_units.max(1),
            completed_units: 0,
            phase: "Allocating test buffer".to_owned(),
            pass: 0,
            tested_bytes,
            last_emit: start,
        }
    }

    fn begin_phase(&mut self, phase: impl Into<String>, pass: usize, checks: u64, errors: usize) {
        self.phase = phase.into();
        self.pass = pass;
        let _ = self.tx.send(RamWorkerEvent::Log(format!(
            "RAM phase {pass}: {}",
            self.phase
        )));
        self.emit(0, checks, errors, true);
    }

    fn complete_phase(&mut self, weight: u128, checks: u64, errors: usize) {
        self.completed_units = self
            .completed_units
            .saturating_add(weight)
            .min(self.total_units);
        self.emit(0, checks, errors, true);
    }

    fn finish(&mut self, checks: u64, errors: usize) {
        self.completed_units = self.total_units;
        self.phase = "RAM test complete".to_owned();
        self.pass = 0;
        self.emit(0, checks, errors, true);
    }

    fn emit(&mut self, current_units: u128, checks: u64, errors: usize, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_emit) < Duration::from_millis(PROGRESS_SAMPLE_MS)
        {
            return;
        }
        self.last_emit = now;
        let done = self
            .completed_units
            .saturating_add(current_units)
            .min(self.total_units);
        let progress = (done as f64 / self.total_units as f64).clamp(0.0, 1.0);
        let elapsed_s = now.duration_since(self.start).as_secs_f64();
        let eta_s = if progress > 0.001 && progress < 1.0 {
            Some((elapsed_s / progress) - elapsed_s)
        } else {
            None
        };
        let _ = self.tx.send(RamWorkerEvent::Progress(RamTestProgress {
            phase: self.phase.clone(),
            pass: self.pass,
            progress: progress as f32,
            elapsed_s,
            eta_s,
            tested_bytes: self.tested_bytes,
            checks,
            errors,
        }));
    }
}

fn run_ram_test(
    config: RamTestConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<RamWorkerEvent>,
) -> Result<RamTestResult> {
    let start = Instant::now();
    let memory_info = detect_ram_memory_info().unwrap_or(config.memory_info);
    let tested_bytes = planned_ram_test_bytes(memory_info, config.allocation);
    if tested_bytes < RAM_MIN_TEST_BYTES {
        return Err(anyhow!(
            "planned RAM test allocation is too small: {}",
            format_bytes(tested_bytes)
        ));
    }
    let word_count = usize::try_from(tested_bytes / RAM_WORD_BYTES)
        .context("RAM test allocation is too large for this process")?;
    if word_count == 0 {
        return Err(anyhow!("RAM test allocation has no testable words"));
    }

    let budget_seconds = ram_time_budget_seconds(memory_info.total_physical_bytes);
    let deadline = start + Duration::from_secs_f64(budget_seconds);
    let total_phases = ram_total_phase_count();
    let total_units = ram_total_units(word_count);
    let mut result = RamTestResult {
        status: RamTestStatus::Passed,
        tested_bytes,
        installed_bytes: memory_info.total_physical_bytes,
        available_at_start_bytes: memory_info.available_physical_bytes,
        elapsed_ms: 0.0,
        budget_seconds,
        checks: 0,
        error_count: 0,
        completed_phases: 0,
        total_phases,
        first_failure: None,
        notes: vec![
            "User-mode RAM test: covers committed process memory, not every physical address."
                .to_owned(),
        ],
    };
    if let Some(requested) = config.allocation.requested_bytes() {
        if tested_bytes < requested {
            result.notes.push(format!(
                "Requested allocation {} was clamped to {} to leave OS headroom.",
                format_bytes(requested),
                format_bytes(tested_bytes)
            ));
        }
    }

    let _ = tx.send(RamWorkerEvent::Log(format!(
        "Allocating {} RAM test buffer ({} words)",
        format_bytes(tested_bytes),
        word_count
    )));
    let mut buffer = Vec::<u64>::new();
    buffer
        .try_reserve_exact(word_count)
        .with_context(|| format!("could not reserve {}", format_bytes(tested_bytes)))?;
    buffer.resize(word_count, 0);

    let mut emitter = RamProgressEmitter::new(tx.clone(), start, total_units, tested_bytes);
    emitter.emit(0, result.checks, result.error_count, true);

    if ram_boundary_touch(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }
    if ram_data_bus_sample(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }
    if ram_address_alias_sample(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }
    for (index, pattern) in RAM_FIXED_PATTERNS.iter().copied().enumerate() {
        let phase = format!("Moving inversions {}", format_ram_word(pattern));
        if ram_moving_inversions(
            &mut buffer,
            pattern,
            index + 1,
            &phase,
            &mut result,
            &mut emitter,
            &tx,
            &cancel,
            deadline,
        )? == RamPhaseStatus::TimeBudget
        {
            return Ok(ram_finish_result(result, start, true, &mut emitter));
        }
    }
    if ram_own_address_pattern(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }
    if ram_random_sequence(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }
    for phase in RAM_MODULO_PHASES {
        if ram_modulo_stride(
            &mut buffer,
            phase,
            &mut result,
            &mut emitter,
            &tx,
            &cancel,
            deadline,
        )? == RamPhaseStatus::TimeBudget
        {
            return Ok(ram_finish_result(result, start, true, &mut emitter));
        }
    }
    if ram_block_move_stress(
        &mut buffer,
        &mut result,
        &mut emitter,
        &tx,
        &cancel,
        deadline,
    )? == RamPhaseStatus::TimeBudget
    {
        return Ok(ram_finish_result(result, start, true, &mut emitter));
    }

    Ok(ram_finish_result(result, start, false, &mut emitter))
}

fn ram_finish_result(
    mut result: RamTestResult,
    start: Instant,
    time_limited: bool,
    emitter: &mut RamProgressEmitter,
) -> RamTestResult {
    result.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    result.status = if result.error_count > 0 {
        RamTestStatus::Failed
    } else if time_limited {
        result
            .notes
            .push("Stopped at the configured time budget before every phase completed.".to_owned());
        RamTestStatus::TimeLimited
    } else {
        RamTestStatus::Passed
    };
    emitter.finish(result.checks, result.error_count);
    result
}

fn ram_boundary_touch(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Boundary and page-stride touch";
    let indices = ram_boundary_indices(buffer.len());
    let weight = indices.len() as u128;
    emitter.begin_phase(phase, 1, result.checks, result.error_count);
    for (pos, index) in indices.into_iter().enumerate() {
        if ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let expected = ram_address_pattern(index) ^ 0xBADC_0FFE_E0DD_F00D;
        buffer[index] = expected;
        let actual = buffer[index];
        ram_record_check(result, tx, phase, 1, index, expected, actual, buffer[index]);
        emitter.emit((pos + 1) as u128, result.checks, result.error_count, false);
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_data_bus_sample(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Data bus walking bits";
    let indices = ram_sample_indices(buffer.len(), 16);
    let weight = (indices.len() * 128) as u128;
    let mut units = 0_u128;
    emitter.begin_phase(phase, 2, result.checks, result.error_count);
    for index in indices {
        for bit in 0..64 {
            if ram_time_or_cancel(cancel, deadline)? {
                return Ok(RamPhaseStatus::TimeBudget);
            }
            let one = 1_u64 << bit;
            buffer[index] = one;
            let actual = buffer[index];
            ram_record_check(result, tx, phase, 2, index, one, actual, buffer[index]);
            units += 1;
            let zero = !one;
            buffer[index] = zero;
            let actual = buffer[index];
            ram_record_check(result, tx, phase, 2, index, zero, actual, buffer[index]);
            units += 1;
            emitter.emit(units, result.checks, result.error_count, false);
        }
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_address_alias_sample(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Address alias sample";
    let indices = ram_power_two_indices(buffer.len());
    let weight = (indices.len() + indices.len() * indices.len()) as u128;
    let base = 0x0123_4567_89AB_CDEF;
    let inverse = !base;
    let mut units = 0_u128;
    emitter.begin_phase(phase, 3, result.checks, result.error_count);
    for &index in &indices {
        if ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = base;
        units += 1;
    }
    for &victim in &indices {
        if ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[victim] = inverse;
        for &index in &indices {
            let expected = if index == victim { inverse } else { base };
            let actual = buffer[index];
            ram_record_check(result, tx, phase, 3, index, expected, actual, buffer[index]);
            units += 1;
        }
        buffer[victim] = base;
        emitter.emit(units, result.checks, result.error_count, false);
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

#[allow(clippy::too_many_arguments)]
fn ram_moving_inversions(
    buffer: &mut [u64],
    pattern: u64,
    pass: usize,
    phase: &str,
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let len = buffer.len();
    let weight = (len as u128).saturating_mul(3);
    emitter.begin_phase(phase, pass, result.checks, result.error_count);
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = pattern;
        emitter.emit(index as u128 + 1, result.checks, result.error_count, false);
    }
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let actual = buffer[index];
        ram_record_check(
            result,
            tx,
            phase,
            pass,
            index,
            pattern,
            actual,
            buffer[index],
        );
        buffer[index] = !pattern;
        emitter.emit(
            len as u128 + index as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    for reverse_pos in 0..len {
        if reverse_pos % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let index = len - 1 - reverse_pos;
        let expected = !pattern;
        let actual = buffer[index];
        ram_record_check(
            result,
            tx,
            phase,
            pass,
            index,
            expected,
            actual,
            buffer[index],
        );
        buffer[index] = pattern;
        emitter.emit(
            (len as u128 * 2) + reverse_pos as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_own_address_pattern(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Own-address pattern";
    let len = buffer.len();
    let weight = (len as u128).saturating_mul(2);
    emitter.begin_phase(
        phase,
        RAM_FIXED_PATTERNS.len() + 1,
        result.checks,
        result.error_count,
    );
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = ram_address_pattern(index);
        emitter.emit(index as u128 + 1, result.checks, result.error_count, false);
    }
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let expected = ram_address_pattern(index);
        let actual = buffer[index];
        ram_record_check(result, tx, phase, 1, index, expected, actual, buffer[index]);
        emitter.emit(
            len as u128 + index as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_random_sequence(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Pseudo-random sequence";
    let len = buffer.len();
    let weight = (len as u128).saturating_mul(3);
    let seed = 0xC0DE_CAFE_5EED_2026;
    emitter.begin_phase(phase, 1, result.checks, result.error_count);
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = ram_random_pattern(index, seed);
        emitter.emit(index as u128 + 1, result.checks, result.error_count, false);
    }
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let expected = ram_random_pattern(index, seed);
        let actual = buffer[index];
        ram_record_check(result, tx, phase, 1, index, expected, actual, buffer[index]);
        buffer[index] = !expected;
        emitter.emit(
            len as u128 + index as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    for reverse_pos in 0..len {
        if reverse_pos % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let index = len - 1 - reverse_pos;
        let expected = !ram_random_pattern(index, seed);
        let actual = buffer[index];
        ram_record_check(result, tx, phase, 1, index, expected, actual, buffer[index]);
        emitter.emit(
            len as u128 * 2 + reverse_pos as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_modulo_stride(
    buffer: &mut [u64],
    modulo_phase: usize,
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = format!("Modulo-{} phase {}", RAM_MODULO_STRIDE, modulo_phase);
    let len = buffer.len();
    let weight = (len as u128).saturating_mul(2);
    let pattern = 0x6DB6_DB6D_B6DB_6DB6 ^ modulo_phase as u64;
    emitter.begin_phase(&phase, modulo_phase, result.checks, result.error_count);
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = if index % RAM_MODULO_STRIDE == modulo_phase {
            pattern
        } else {
            !pattern
        };
        emitter.emit(index as u128 + 1, result.checks, result.error_count, false);
    }
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        let expected = if index % RAM_MODULO_STRIDE == modulo_phase {
            pattern
        } else {
            !pattern
        };
        let actual = buffer[index];
        ram_record_check(
            result,
            tx,
            &phase,
            modulo_phase,
            index,
            expected,
            actual,
            buffer[index],
        );
        emitter.emit(
            len as u128 + index as u128 + 1,
            result.checks,
            result.error_count,
            false,
        );
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_block_move_stress(
    buffer: &mut [u64],
    result: &mut RamTestResult,
    emitter: &mut RamProgressEmitter,
    tx: &Sender<RamWorkerEvent>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<RamPhaseStatus> {
    let phase = "Block move stress";
    let len = buffer.len();
    let half = len / 2;
    let weight = len as u128 + (half as u128 * 2);
    emitter.begin_phase(phase, 1, result.checks, result.error_count);
    for index in 0..len {
        if index % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
            return Ok(RamPhaseStatus::TimeBudget);
        }
        buffer[index] = ram_address_pattern(index);
        emitter.emit(index as u128 + 1, result.checks, result.error_count, false);
    }
    if half > 0 {
        let chunk_words = ram_block_move_chunk_words(half);
        let mut copied = 0_usize;
        while copied < half {
            if ram_time_or_cancel(cancel, deadline)? {
                return Ok(RamPhaseStatus::TimeBudget);
            }
            let count = (half - copied).min(chunk_words);
            buffer.copy_within(copied..copied + count, half + copied);
            copied += count;
            emitter.emit(
                len as u128 + copied as u128,
                result.checks,
                result.error_count,
                false,
            );
        }
        for offset in 0..half {
            if offset % RAM_CHECK_INTERVAL_WORDS == 0 && ram_time_or_cancel(cancel, deadline)? {
                return Ok(RamPhaseStatus::TimeBudget);
            }
            let index = half + offset;
            let expected = ram_address_pattern(offset);
            let actual = buffer[index];
            ram_record_check(result, tx, phase, 1, index, expected, actual, buffer[index]);
            emitter.emit(
                len as u128 + half as u128 + offset as u128 + 1,
                result.checks,
                result.error_count,
                false,
            );
        }
    }
    result.completed_phases += 1;
    emitter.complete_phase(weight, result.checks, result.error_count);
    Ok(RamPhaseStatus::Complete)
}

fn ram_record_check(
    result: &mut RamTestResult,
    tx: &Sender<RamWorkerEvent>,
    test: &str,
    pass: usize,
    word_index: usize,
    expected: u64,
    actual: u64,
    reread: u64,
) {
    result.checks = result.checks.saturating_add(1);
    if actual == expected {
        return;
    }
    result.error_count = result.error_count.saturating_add(1);
    let diff = expected ^ actual;
    let failure = RamFailure {
        test: test.to_owned(),
        pass,
        byte_offset: (word_index as u64).saturating_mul(RAM_WORD_BYTES),
        word_index,
        expected,
        actual,
        diff,
        failed_bit: (diff != 0).then(|| diff.trailing_zeros()),
        repeatable: reread != expected,
    };
    if result.first_failure.is_none() {
        result.first_failure = Some(failure.clone());
    }
    if result.error_count <= 8 {
        let _ = tx.send(RamWorkerEvent::Log(format!(
            "RAM failure {}: {}",
            result.error_count,
            format_ram_failure(&failure)
        )));
    }
}

fn ram_time_or_cancel(cancel: &AtomicBool, deadline: Instant) -> Result<bool> {
    check_canceled_with(Some(cancel), "RAM test canceled")?;
    Ok(Instant::now() >= deadline)
}

fn ram_total_phase_count() -> usize {
    3 + RAM_FIXED_PATTERNS.len() + 1 + 1 + RAM_MODULO_PHASES.len() + 1
}

fn ram_total_units(word_count: usize) -> u128 {
    let boundary = ram_boundary_indices(word_count).len() as u128;
    let data_bus = (ram_sample_indices(word_count, 16).len() * 128) as u128;
    let address = {
        let count = ram_power_two_indices(word_count).len();
        (count + count * count) as u128
    };
    let fixed = RAM_FIXED_PATTERNS.len() as u128 * word_count as u128 * 3;
    let own_address = word_count as u128 * 2;
    let random = word_count as u128 * 3;
    let modulo = RAM_MODULO_PHASES.len() as u128 * word_count as u128 * 2;
    let block = word_count as u128 + (word_count / 2) as u128 * 2;
    boundary + data_bus + address + fixed + own_address + random + modulo + block
}

fn ram_boundary_indices(word_count: usize) -> Vec<usize> {
    if word_count == 0 {
        return Vec::new();
    }
    let mut indices = ram_sample_indices(word_count, 5);
    let stride_words = ((2 * 1024 * 1024) / std::mem::size_of::<u64>()).max(1);
    let mut index = 0_usize;
    while index < word_count {
        indices.push(index);
        index = index.saturating_add(stride_words);
    }
    indices.push(word_count - 1);
    sort_dedup_indices(&mut indices);
    indices
}

fn ram_sample_indices(word_count: usize, sample_count: usize) -> Vec<usize> {
    if word_count == 0 || sample_count == 0 {
        return Vec::new();
    }
    if sample_count == 1 || word_count == 1 {
        return vec![0];
    }
    let mut indices = Vec::with_capacity(sample_count);
    for sample in 0..sample_count {
        indices.push(sample * (word_count - 1) / (sample_count - 1));
    }
    sort_dedup_indices(&mut indices);
    indices
}

fn ram_power_two_indices(word_count: usize) -> Vec<usize> {
    if word_count == 0 {
        return Vec::new();
    }
    let mut indices = vec![0];
    let mut index = 1_usize;
    while index < word_count {
        indices.push(index);
        match index.checked_mul(2) {
            Some(next) => index = next,
            None => break,
        }
    }
    indices.push(word_count - 1);
    sort_dedup_indices(&mut indices);
    indices
}

fn sort_dedup_indices(indices: &mut Vec<usize>) {
    indices.sort_unstable();
    indices.dedup();
}

fn ram_block_move_chunk_words(available_words: usize) -> usize {
    available_words.min(1_048_576).max(1)
}

fn ram_address_pattern(index: usize) -> u64 {
    let value = index as u64;
    value
        .wrapping_mul(0xD6E8_FD9D_50A7_5A5B)
        .rotate_left((value & 63) as u32)
        ^ (!value).rotate_right(17)
}

fn ram_random_pattern(index: usize, seed: u64) -> u64 {
    let mut state = seed ^ (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    splitmix64(&mut state)
}

fn planned_ram_test_bytes(info: RamMemoryInfo, allocation: RamAllocation) -> u64 {
    let safe = safe_ram_auto_bytes(info);
    let requested = allocation.requested_bytes().unwrap_or(safe);
    align_down_u64(requested.min(safe), RAM_WORD_BYTES)
}

fn safe_ram_auto_bytes(info: RamMemoryInfo) -> u64 {
    if info.total_physical_bytes == 0 || info.available_physical_bytes == 0 {
        return 0;
    }
    let available_target = info
        .available_physical_bytes
        .saturating_mul(RAM_AUTO_AVAILABLE_PERCENT)
        / 100;
    let installed_cap = info
        .total_physical_bytes
        .saturating_mul(RAM_AUTO_INSTALLED_PERCENT)
        / 100;
    let headroom_cap = info
        .available_physical_bytes
        .saturating_sub(RAM_OS_HEADROOM_BYTES);
    align_down_u64(
        available_target.min(installed_cap).min(headroom_cap),
        RAM_WORD_BYTES,
    )
}

fn ram_time_budget_seconds(total_physical_bytes: u64) -> f64 {
    if total_physical_bytes == 0 {
        return RAM_SECONDS_PER_8_GIB;
    }
    total_physical_bytes as f64 / RAM_BUDGET_UNIT_BYTES as f64 * RAM_SECONDS_PER_8_GIB
}

fn align_down_u64(value: u64, alignment: u64) -> u64 {
    if alignment == 0 {
        value
    } else {
        value / alignment * alignment
    }
}

#[cfg(windows)]
fn detect_ram_memory_info() -> Result<RamMemoryInfo> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe {
        GlobalMemoryStatusEx(&mut status).context("GlobalMemoryStatusEx failed")?;
    }
    Ok(RamMemoryInfo {
        total_physical_bytes: status.ullTotalPhys,
        available_physical_bytes: status.ullAvailPhys,
        memory_load_percent: status.dwMemoryLoad,
    })
}

#[cfg(not(windows))]
fn detect_ram_memory_info() -> Result<RamMemoryInfo> {
    Err(anyhow!(
        "RAM memory status detection is currently implemented for Windows"
    ))
}

fn format_ram_word(value: u64) -> String {
    format!("0x{value:016X}")
}

fn format_ram_failure(failure: &RamFailure) -> String {
    let bit = failure
        .failed_bit
        .map(|bit| format!("D{bit}"))
        .unwrap_or_else(|| "unknown".to_owned());
    format!(
        "{} pass {} at byte 0x{:X} (word {}): expected {}, actual {}, diff {}, bit {}, repeatable {}",
        failure.test,
        failure.pass,
        failure.byte_offset,
        failure.word_index,
        format_ram_word(failure.expected),
        format_ram_word(failure.actual),
        format_ram_word(failure.diff),
        bit,
        if failure.repeatable { "yes" } else { "no" }
    )
}

fn format_ram_first_failure(result: &RamTestResult) -> String {
    result
        .first_failure
        .as_ref()
        .map(format_ram_failure)
        .unwrap_or_else(|| "None".to_owned())
}

fn parse_ram_allocation(value: &str) -> Result<RamAllocation> {
    RamAllocation::parse(value).ok_or_else(|| {
        anyhow!("--ram-size must be one of auto, 256m, 512m, 1g, 2g, 4g, 8g, 16g, or 32g")
    })
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn open_drive_file_direct_preferred(
    path: &PathBuf,
    read: bool,
    write: bool,
    create: bool,
    truncate: bool,
    sequential: bool,
) -> Result<DriveOpenFile> {
    #[cfg(windows)]
    {
        let mut direct_options = OpenOptions::new();
        direct_options
            .read(read)
            .write(write)
            .create(create)
            .truncate(truncate);
        let access_hint = if sequential {
            FILE_FLAG_SEQUENTIAL_SCAN_RAW
        } else {
            FILE_FLAG_RANDOM_ACCESS_RAW
        };
        let write_hint = if write {
            FILE_FLAG_WRITE_THROUGH_RAW
        } else {
            0
        };
        direct_options.custom_flags(FILE_FLAG_NO_BUFFERING_RAW | write_hint | access_hint);
        match direct_options.open(path) {
            Ok(file) => {
                return Ok(DriveOpenFile {
                    file,
                    io_mode: DriveIoMode::Direct,
                    fallback_note: None,
                });
            }
            Err(err) => {
                let file = OpenOptions::new()
                    .read(read)
                    .write(write)
                    .create(create)
                    .truncate(truncate)
                    .open(path)
                    .with_context(|| {
                        format!(
                            "failed to open benchmark file {} after direct I/O failed",
                            path.display()
                        )
                    })?;
                return Ok(DriveOpenFile {
                    file,
                    io_mode: DriveIoMode::Cached,
                    fallback_note: Some(format!("Direct I/O unavailable: {err}")),
                });
            }
        }
    }

    #[cfg(not(windows))]
    {
        let file = OpenOptions::new()
            .read(read)
            .write(write)
            .create(create)
            .truncate(truncate)
            .open(path)
            .with_context(|| format!("failed to open benchmark file {}", path.display()))?;
        Ok(DriveOpenFile {
            file,
            io_mode: DriveIoMode::Cached,
            fallback_note: Some("Direct I/O is only implemented on Windows".to_owned()),
        })
    }
}

fn run_drive_benchmark(
    config: DriveBenchmarkConfig,
    cancel: Arc<AtomicBool>,
    tx: Sender<DriveWorkerEvent>,
) -> Result<Vec<DriveBenchmarkResult>> {
    let test_path = config.target_folder.join(DRIVE_BENCHMARK_FILE_NAME);
    let _ = tx.send(DriveWorkerEvent::Log(format!(
        "Using direct file I/O when available, with cached fallback."
    )));
    let _ = tx.send(DriveWorkerEvent::Log(format!(
        "Temporary benchmark file: {}",
        test_path.display()
    )));

    if config.selected_tests.iter().any(|test| test.is_read()) {
        prepare_drive_benchmark_file(&test_path, config.file_size_bytes, &cancel, &tx)?;
    }

    let suite_started = Instant::now();
    let mut results = Vec::new();
    let total_tests = config.selected_tests.len().max(1);
    for (index, test) in config.selected_tests.iter().copied().enumerate() {
        check_canceled_with(Some(&cancel), "Drive benchmark canceled")?;
        let result = run_drive_test(
            &test_path,
            config.file_size_bytes,
            config.profile,
            test,
            index,
            total_tests,
            suite_started,
            &cancel,
            &tx,
        )?;
        let _ = tx.send(DriveWorkerEvent::Log(format!(
            "{} complete: read {}, write {}, IOPS {}, duration {} ms",
            result.test,
            format_optional_rate(result.read_mbps),
            format_optional_rate(result.write_mbps),
            format_optional_iops(result.iops),
            format_ms(Some(result.duration_ms))
        )));
        results.push(result);
    }

    if let Err(err) = fs::remove_file(&test_path) {
        let _ = tx.send(DriveWorkerEvent::Log(format!(
            "Could not delete temporary benchmark file {}: {err}",
            test_path.display()
        )));
    }

    Ok(results)
}

fn prepare_drive_benchmark_file(
    path: &PathBuf,
    file_size_bytes: u64,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<()> {
    check_canceled_with(
        Some(cancel),
        "Drive benchmark canceled during file preparation",
    )?;
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("failed to create benchmark file {}", path.display()))?;
    file.set_len(file_size_bytes)
        .with_context(|| format!("failed to size benchmark file {}", path.display()))?;

    let mut buffer = vec![0_u8; DRIVE_SEQUENTIAL_BLOCK_BYTES];
    let mut written = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);
    let mut block_seed = 0xA5A5_5A5A_D15C_BEAD_u64;

    while written < file_size_bytes {
        check_canceled_with(
            Some(cancel),
            "Drive benchmark canceled during file preparation",
        )?;
        fill_drive_buffer(&mut buffer, block_seed);
        block_seed = splitmix64(&mut block_seed);
        let remaining = (file_size_bytes - written) as usize;
        let len = remaining.min(buffer.len());
        file.write_all(&buffer[..len])
            .context("failed to initialize benchmark file")?;
        written += len as u64;

        let now = Instant::now();
        if now.duration_since(last_emit) >= Duration::from_millis(PROGRESS_SAMPLE_MS) {
            last_emit = now;
            let progress = (written as f32 / file_size_bytes.max(1) as f32).clamp(0.0, 1.0);
            let elapsed_s = started.elapsed().as_secs_f64();
            let eta_s = if progress > 0.001 && progress < 1.0 {
                let total = elapsed_s / progress as f64;
                Some((total - elapsed_s).max(0.0))
            } else {
                None
            };
            let _ = tx.send(DriveWorkerEvent::Progress(DriveProgress {
                current_test: "Preparing read file".to_owned(),
                current_progress: progress,
                suite_progress: 0.0,
                elapsed_s,
                eta_s,
                bytes_processed: written,
                operations: 0,
            }));
        }
    }

    file.sync_all()
        .context("failed to flush prepared benchmark file")?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run_drive_test(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    match test {
        DriveTestKind::SequentialRead => run_sequential_drive_read(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
        ),
        DriveTestKind::SequentialWrite => run_sequential_drive_write(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
        ),
        DriveTestKind::RandomRead4K => run_random_drive_test(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
            false,
        ),
        DriveTestKind::RandomWrite4K => run_random_drive_test(
            path,
            file_size_bytes,
            profile,
            test,
            test_index,
            total_tests,
            suite_started,
            cancel,
            tx,
            true,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_sequential_drive_write(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    let opened = open_drive_file_direct_preferred(path, true, true, true, false, true)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    file.set_len(file_size_bytes)
        .context("failed to size benchmark file for sequential write")?;
    file.seek(SeekFrom::Start(0))
        .context("failed to seek benchmark file")?;

    let target_duration = profile.target_duration();
    let mut buffer = AlignedBuffer::new(DRIVE_SEQUENTIAL_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    let mut offset = 0_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut seed = 0x5151_5151_BEEF_CAFE_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        if offset >= file_size_bytes {
            offset = 0;
            file.seek(SeekFrom::Start(0))
                .context("failed to rewind benchmark file")?;
        }
        fill_drive_buffer(buffer.as_mut_slice(), seed);
        seed = splitmix64(&mut seed);
        let len = ((file_size_bytes - offset) as usize).min(buffer.len());
        let op_started = Instant::now();
        file.write_all(&buffer.as_slice()[..len])
            .context("sequential write failed")?;
        let _op_elapsed = op_started.elapsed();
        offset += len as u64;
        bytes_processed += len as u64;
        operations += 1;
        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    check_canceled_with(Some(cancel), "Drive benchmark canceled before flush")?;
    file.sync_all().context("sequential write flush failed")?;
    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    notes.push("Flush included".to_owned());
    Ok(make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        Vec::new(),
        notes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_sequential_drive_read(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
) -> Result<DriveBenchmarkResult> {
    let opened = open_drive_file_direct_preferred(path, true, false, false, false, true)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    let target_duration = profile.target_duration();
    let mut buffer = AlignedBuffer::new(DRIVE_SEQUENTIAL_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    let mut offset = 0_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut checksum = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        if offset >= file_size_bytes {
            offset = 0;
            file.seek(SeekFrom::Start(0))
                .context("failed to rewind benchmark file")?;
        }
        let len = ((file_size_bytes - offset) as usize).min(buffer.len());
        file.read_exact(&mut buffer.as_mut_slice()[..len])
            .context("sequential read failed")?;
        checksum = checksum
            .wrapping_add(buffer.as_slice()[0] as u64)
            .wrapping_add(buffer.as_slice()[len - 1] as u64);
        offset += len as u64;
        bytes_processed += len as u64;
        operations += 1;
        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    notes.push(format!("Checksum {checksum:016X}"));
    Ok(make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        Vec::new(),
        notes,
    ))
}

#[allow(clippy::too_many_arguments)]
fn run_random_drive_test(
    path: &PathBuf,
    file_size_bytes: u64,
    profile: DriveProfile,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    suite_started: Instant,
    cancel: &AtomicBool,
    tx: &Sender<DriveWorkerEvent>,
    write_mode: bool,
) -> Result<DriveBenchmarkResult> {
    let opened =
        open_drive_file_direct_preferred(path, true, write_mode, write_mode, false, false)?;
    let mut file = opened.file;
    let mut notes = Vec::new();
    if let Some(note) = opened.fallback_note {
        notes.push(note);
    }
    if write_mode {
        file.set_len(file_size_bytes)
            .context("failed to size benchmark file for random write")?;
    }

    let target_duration = profile.target_duration();
    let block_count = (file_size_bytes / DRIVE_RANDOM_BLOCK_BYTES as u64).max(1);
    let mut buffer = AlignedBuffer::new(DRIVE_RANDOM_BLOCK_BYTES, DRIVE_RANDOM_BLOCK_BYTES);
    fill_drive_buffer(buffer.as_mut_slice(), 0x4449_534B_524E_4434_u64);
    let mut rng = 0xC001_D00D_F00D_BAAD_u64;
    let mut bytes_processed = 0_u64;
    let mut operations = 0_u64;
    let mut latency_samples_ms = Vec::new();
    let mut latency_total_ms = 0.0_f64;
    let mut checksum = 0_u64;
    let started = Instant::now();
    let mut last_emit = Instant::now() - Duration::from_secs(1);

    while should_continue_drive_test(started, target_duration) {
        check_canceled_with(Some(cancel), "Drive benchmark canceled")?;
        let block_index = splitmix64(&mut rng) % block_count;
        let offset = block_index * DRIVE_RANDOM_BLOCK_BYTES as u64;
        file.seek(SeekFrom::Start(offset))
            .context("random seek failed")?;
        if write_mode {
            buffer.as_mut_slice()[..8].copy_from_slice(&operations.to_le_bytes());
        }

        let op_started = Instant::now();
        if write_mode {
            file.write_all(buffer.as_slice())
                .context("random write failed")?;
        } else {
            file.read_exact(buffer.as_mut_slice())
                .context("random read failed")?;
            checksum = checksum
                .wrapping_add(buffer.as_slice()[0] as u64)
                .wrapping_add(buffer.as_slice()[DRIVE_RANDOM_BLOCK_BYTES - 1] as u64);
        }
        let op_ms = op_started.elapsed().as_secs_f64() * 1000.0;
        latency_total_ms += op_ms;
        record_latency_sample(&mut latency_samples_ms, operations, op_ms);
        bytes_processed += DRIVE_RANDOM_BLOCK_BYTES as u64;
        operations += 1;

        emit_drive_progress(
            tx,
            test,
            test_index,
            total_tests,
            started,
            suite_started,
            target_duration,
            bytes_processed,
            operations,
            &mut last_emit,
            false,
        );
    }

    if write_mode {
        check_canceled_with(Some(cancel), "Drive benchmark canceled before flush")?;
        file.sync_all().context("random write flush failed")?;
    }

    emit_drive_progress(
        tx,
        test,
        test_index,
        total_tests,
        started,
        suite_started,
        target_duration,
        bytes_processed,
        operations,
        &mut last_emit,
        true,
    );

    if write_mode {
        notes.push("Flush included".to_owned());
    } else {
        notes.push(format!("Checksum {checksum:016X}"));
    }

    let mut result = make_drive_result(
        test,
        bytes_processed,
        operations,
        started.elapsed(),
        file_size_bytes,
        opened.io_mode,
        latency_samples_ms,
        notes,
    );
    result.avg_latency_ms = (operations > 0).then_some(latency_total_ms / operations as f64);
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn emit_drive_progress(
    tx: &Sender<DriveWorkerEvent>,
    test: DriveTestKind,
    test_index: usize,
    total_tests: usize,
    test_started: Instant,
    suite_started: Instant,
    target_duration: Duration,
    bytes_processed: u64,
    operations: u64,
    last_emit: &mut Instant,
    force: bool,
) {
    let now = Instant::now();
    if !force && now.duration_since(*last_emit) < Duration::from_millis(PROGRESS_SAMPLE_MS) {
        return;
    }
    *last_emit = now;
    let elapsed_s = test_started.elapsed().as_secs_f64();
    let target_s = target_duration.as_secs_f64().max(0.001);
    let current_progress = (elapsed_s / target_s).clamp(0.0, 1.0) as f32;
    let suite_progress =
        ((test_index as f32 + current_progress) / total_tests.max(1) as f32).clamp(0.0, 1.0);
    let suite_elapsed_s = suite_started.elapsed().as_secs_f64();
    let eta_s = if suite_progress > 0.001 && suite_progress < 1.0 {
        let total = suite_elapsed_s / suite_progress as f64;
        Some((total - suite_elapsed_s).max(0.0))
    } else {
        None
    };

    let _ = tx.send(DriveWorkerEvent::Progress(DriveProgress {
        current_test: test.label().to_owned(),
        current_progress,
        suite_progress,
        elapsed_s: suite_elapsed_s,
        eta_s,
        bytes_processed,
        operations,
    }));
}

fn should_continue_drive_test(started: Instant, target_duration: Duration) -> bool {
    let elapsed = started.elapsed();
    elapsed < target_duration && elapsed.as_secs_f64() < DRIVE_MAX_TEST_SECONDS
}

fn make_drive_result(
    test: DriveTestKind,
    bytes_processed: u64,
    operations: u64,
    elapsed: Duration,
    file_size_bytes: u64,
    io_mode: DriveIoMode,
    latency_samples_ms: Vec<f64>,
    mut notes: Vec<String>,
) -> DriveBenchmarkResult {
    let elapsed_s = elapsed.as_secs_f64().max(0.001);
    let mbps = bytes_processed as f64 / DECIMAL_MB / elapsed_s;
    if elapsed.as_secs_f64() >= DRIVE_MAX_TEST_SECONDS {
        notes.push("Capped at 30s".to_owned());
    }
    let p95_latency_ms = percentile_latency_ms(latency_samples_ms, 0.95);
    let iops = matches!(
        test,
        DriveTestKind::RandomRead4K | DriveTestKind::RandomWrite4K
    )
    .then_some(operations as f64 / elapsed_s);

    DriveBenchmarkResult {
        test,
        read_mbps: test.is_read().then_some(mbps),
        write_mbps: test.is_write().then_some(mbps),
        iops,
        avg_latency_ms: None,
        p95_latency_ms,
        duration_ms: elapsed.as_secs_f64() * 1000.0,
        file_size_bytes,
        io_mode,
        notes,
        ssd_temperature: TemperatureSummary::default(),
    }
}

fn record_latency_sample(samples: &mut Vec<f64>, operation: u64, latency_ms: f64) {
    if samples.len() < DRIVE_LATENCY_SAMPLE_LIMIT || operation % 64 == 0 {
        if samples.len() < DRIVE_LATENCY_SAMPLE_LIMIT {
            samples.push(latency_ms);
        }
    }
}

fn percentile_latency_ms(mut samples: Vec<f64>, percentile: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let index = ((samples.len() - 1) as f64 * percentile.clamp(0.0, 1.0)).round() as usize;
    samples.get(index).copied()
}

fn fill_drive_buffer(buffer: &mut [u8], mut seed: u64) {
    for chunk in buffer.chunks_mut(8) {
        let value = splitmix64(&mut seed).to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&value[..len]);
    }
}

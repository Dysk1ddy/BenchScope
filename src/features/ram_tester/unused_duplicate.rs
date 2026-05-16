#[cfg(any())]
mod unused_ram_duplicate {
    use super::*;

    fn detect_ram_memory_info() -> Result<RamMemoryInfo> {
        #[cfg(windows)]
        {
            let mut status = MEMORYSTATUSEX {
                dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
                ..Default::default()
            };
            unsafe {
                GlobalMemoryStatusEx(&mut status).context("GlobalMemoryStatusEx failed")?;
            }
            return Ok(RamMemoryInfo {
                total_physical_bytes: status.ullTotalPhys,
                available_physical_bytes: status.ullAvailPhys,
                memory_load_percent: status.dwMemoryLoad,
            });
        }

        #[cfg(not(windows))]
        {
            Err(anyhow!(
                "RAM tester memory status is currently Windows-only"
            ))
        }
    }

    fn planned_ram_test_bytes(info: RamMemoryInfo, allocation: RamAllocation) -> u64 {
        if info.available_physical_bytes == 0 || info.total_physical_bytes == 0 {
            return 0;
        }
        let available_with_headroom = info
            .available_physical_bytes
            .saturating_sub(RAM_OS_HEADROOM_BYTES);
        let auto_target = (info.available_physical_bytes * RAM_AUTO_AVAILABLE_PERCENT / 100)
            .min(info.total_physical_bytes * RAM_AUTO_INSTALLED_PERCENT / 100)
            .min(available_with_headroom);
        let requested = allocation.requested_bytes().unwrap_or(auto_target);
        align_down_u64(requested.min(available_with_headroom), RAM_WORD_BYTES)
    }

    fn ram_time_budget_seconds(installed_bytes: u64) -> f64 {
        if installed_bytes == 0 {
            return RAM_SECONDS_PER_8_GIB;
        }
        ((installed_bytes as f64 / RAM_BUDGET_UNIT_BYTES as f64) * RAM_SECONDS_PER_8_GIB).max(30.0)
    }

    fn run_ram_test(
        config: RamTestConfig,
        cancel: Arc<AtomicBool>,
        tx: Sender<RamWorkerEvent>,
    ) -> Result<RamTestResult> {
        let planned_bytes = planned_ram_test_bytes(config.memory_info, config.allocation);
        if planned_bytes < RAM_MIN_TEST_BYTES {
            return Err(anyhow!(
                "planned RAM test size is too small: {}",
                format_bytes(planned_bytes)
            ));
        }
        check_canceled(Some(&cancel))?;

        let word_count = (planned_bytes / RAM_WORD_BYTES) as usize;
        let budget_seconds = ram_time_budget_seconds(config.memory_info.total_physical_bytes);
        let deadline = Instant::now() + Duration::from_secs_f64(budget_seconds);
        let start = Instant::now();
        let _ = tx.send(RamWorkerEvent::Log(format!(
            "Allocating RAM test buffer: {}",
            format_bytes(planned_bytes)
        )));
        let mut buffer = vec![0_u64; word_count];
        let total_phases = RAM_FIXED_PATTERNS.len() + 2 + RAM_MODULO_PHASES.len();
        let mut completed_phases = 0;
        let mut checks = 0_u64;
        let mut errors = 0_usize;
        let mut first_failure = None;
        let mut time_limited = false;

        for (pattern_index, pattern) in RAM_FIXED_PATTERNS.iter().enumerate() {
            let phase = format!("Fixed pattern {pattern:#018X}");
            let completed = ram_fill_verify_phase(
                &mut buffer,
                &phase,
                pattern_index + 1,
                completed_phases,
                total_phases,
                start,
                deadline,
                &cancel,
                &tx,
                &mut checks,
                &mut errors,
                &mut first_failure,
                |_| *pattern,
            )?;
            if !completed {
                time_limited = true;
                break;
            }
            completed_phases += 1;
            if first_failure.is_some() {
                break;
            }
        }

        if first_failure.is_none() && !time_limited {
            let completed = ram_fill_verify_phase(
                &mut buffer,
                "Own-address pattern",
                1,
                completed_phases,
                total_phases,
                start,
                deadline,
                &cancel,
                &tx,
                &mut checks,
                &mut errors,
                &mut first_failure,
                |index| own_address_pattern(index),
            )?;
            if completed {
                completed_phases += 1;
            } else {
                time_limited = true;
            }
        }

        if first_failure.is_none() && !time_limited {
            let completed = ram_fill_verify_phase(
                &mut buffer,
                "Pseudo-random sequence",
                1,
                completed_phases,
                total_phases,
                start,
                deadline,
                &cancel,
                &tx,
                &mut checks,
                &mut errors,
                &mut first_failure,
                |index| splitmix64(index as u64 ^ 0xBADC_0FFE_E0DDF00D),
            )?;
            if completed {
                completed_phases += 1;
            } else {
                time_limited = true;
            }
        }

        for phase in RAM_MODULO_PHASES {
            if first_failure.is_some() || time_limited {
                break;
            }
            let phase_name = format!("Modulo-{RAM_MODULO_STRIDE} phase {phase}");
            let completed = ram_fill_verify_phase(
                &mut buffer,
                &phase_name,
                phase,
                completed_phases,
                total_phases,
                start,
                deadline,
                &cancel,
                &tx,
                &mut checks,
                &mut errors,
                &mut first_failure,
                |index| {
                    if index % RAM_MODULO_STRIDE == phase {
                        0xFFFF_FFFF_0000_0000
                    } else {
                        0x0000_0000_FFFF_FFFF
                    }
                },
            )?;
            if completed {
                completed_phases += 1;
            } else {
                time_limited = true;
            }
        }

        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let status = if errors > 0 {
            RamTestStatus::Failed
        } else if time_limited {
            RamTestStatus::TimeLimited
        } else {
            RamTestStatus::Passed
        };
        let mut notes = Vec::new();
        if time_limited {
            notes.push("Stopped at time budget".to_owned());
        }
        if config.allocation == RamAllocation::Auto {
            notes.push("Auto allocation left OS headroom".to_owned());
        }

        Ok(RamTestResult {
            status,
            tested_bytes: planned_bytes,
            installed_bytes: config.memory_info.total_physical_bytes,
            available_at_start_bytes: config.memory_info.available_physical_bytes,
            elapsed_ms,
            budget_seconds,
            checks,
            error_count: errors,
            completed_phases,
            total_phases,
            first_failure,
            notes,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn ram_fill_verify_phase<F>(
        buffer: &mut [u64],
        phase: &str,
        pass: usize,
        completed_phases: usize,
        total_phases: usize,
        start: Instant,
        deadline: Instant,
        cancel: &Arc<AtomicBool>,
        tx: &Sender<RamWorkerEvent>,
        checks: &mut u64,
        errors: &mut usize,
        first_failure: &mut Option<RamFailure>,
        expected: F,
    ) -> Result<bool>
    where
        F: Fn(usize) -> u64,
    {
        let len = buffer.len().max(1);
        for (index, word) in buffer.iter_mut().enumerate() {
            if index % RAM_CHECK_INTERVAL_WORDS == 0 {
                check_canceled(Some(cancel))?;
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                emit_ram_progress(
                    tx,
                    phase,
                    pass,
                    completed_phases,
                    total_phases,
                    index as f32 / len as f32 * 0.5,
                    start,
                    deadline,
                    buffer.len() as u64 * RAM_WORD_BYTES,
                    *checks,
                    *errors,
                );
            }
            *word = expected(index);
        }

        for index in 0..buffer.len() {
            if index % RAM_CHECK_INTERVAL_WORDS == 0 {
                check_canceled(Some(cancel))?;
                if Instant::now() >= deadline {
                    return Ok(false);
                }
                emit_ram_progress(
                    tx,
                    phase,
                    pass,
                    completed_phases,
                    total_phases,
                    0.5 + index as f32 / len as f32 * 0.5,
                    start,
                    deadline,
                    buffer.len() as u64 * RAM_WORD_BYTES,
                    *checks,
                    *errors,
                );
            }
            let expected_value = expected(index);
            let actual = buffer[index];
            *checks += 1;
            if actual != expected_value {
                *errors += 1;
                if first_failure.is_none() {
                    let retry_actual = buffer[index];
                    *first_failure = Some(RamFailure {
                        test: phase.to_owned(),
                        pass,
                        byte_offset: index as u64 * RAM_WORD_BYTES,
                        word_index: index,
                        expected: expected_value,
                        actual,
                        diff: expected_value ^ actual,
                        failed_bit: first_failed_bit(expected_value ^ actual),
                        repeatable: retry_actual == actual,
                    });
                }
                return Ok(true);
            }
        }

        emit_ram_progress(
            tx,
            phase,
            pass,
            completed_phases,
            total_phases,
            1.0,
            start,
            deadline,
            buffer.len() as u64 * RAM_WORD_BYTES,
            *checks,
            *errors,
        );
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_ram_progress(
        tx: &Sender<RamWorkerEvent>,
        phase: &str,
        pass: usize,
        completed_phases: usize,
        total_phases: usize,
        phase_progress: f32,
        start: Instant,
        deadline: Instant,
        tested_bytes: u64,
        checks: u64,
        errors: usize,
    ) {
        let elapsed_s = start.elapsed().as_secs_f64();
        let progress = ((completed_phases as f32 + phase_progress.clamp(0.0, 1.0))
            / total_phases.max(1) as f32)
            .clamp(0.0, 1.0);
        let eta_s = if progress > 0.0 {
            Some((elapsed_s / progress as f64 - elapsed_s).max(0.0))
        } else {
            Some(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f64(),
            )
        };
        let _ = tx.send(RamWorkerEvent::Progress(RamTestProgress {
            phase: phase.to_owned(),
            pass,
            progress,
            elapsed_s,
            eta_s,
            tested_bytes,
            checks,
            errors,
        }));
    }

    fn own_address_pattern(index: usize) -> u64 {
        let value = index as u64;
        value.rotate_left(17) ^ 0xA5A5_5A5A_C3C3_3C3C
    }

    fn splitmix64(mut value: u64) -> u64 {
        value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = value;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn first_failed_bit(diff: u64) -> Option<u32> {
        (diff != 0).then(|| diff.trailing_zeros())
    }

    fn format_ram_failure(failure: &RamFailure) -> String {
        format!(
            "{} pass {} offset {} word {} expected {:#018X} actual {:#018X} diff {:#018X} bit {} repeatable {}",
            failure.test,
            failure.pass,
            format_bytes(failure.byte_offset),
            failure.word_index,
            failure.expected,
            failure.actual,
            failure.diff,
            failure
                .failed_bit
                .map(|bit| bit.to_string())
                .unwrap_or_else(|| "N/A".to_owned()),
            if failure.repeatable { "yes" } else { "no" }
        )
    }

    fn format_ram_first_failure(result: &RamTestResult) -> String {
        result
            .first_failure
            .as_ref()
            .map(format_ram_failure)
            .unwrap_or_default()
    }
}


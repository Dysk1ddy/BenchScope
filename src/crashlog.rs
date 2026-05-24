static CRASHLOG_WRITING: AtomicBool = AtomicBool::new(false);

fn install_crash_logger() {
    panic::set_hook(Box::new(|info| {
        crashlog_write_panic_info(info);
    }));
}

fn crashlog_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("BenchScope").join("crashlogs"))
        .unwrap_or_else(|| std::env::temp_dir().join("BenchScope").join("crashlogs"))
}

fn crashlog_hint() -> String {
    format!("Crash logs are saved in {}.", crashlog_dir().display())
}

fn crashlog_timestamp_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn crashlog_write_panic_info(info: &panic::PanicHookInfo<'_>) {
    if CRASHLOG_WRITING.swap(true, Ordering::Relaxed) {
        return;
    }

    let thread = thread::current();
    let thread_name = thread.name().unwrap_or("unnamed");
    let location = info
        .location()
        .map(|location| format!("{}:{}", location.file(), location.line()))
        .unwrap_or_else(|| "unknown".to_owned());
    let message = crashlog_panic_message(info.payload());
    let backtrace = std::backtrace::Backtrace::force_capture();
    let body = format!(
        "BenchScope crash log\n\
         TimestampUnixMs: {}\n\
         Version: {}\n\
         Thread: {}\n\
         Location: {}\n\
         Panic: {}\n\n\
         Backtrace:\n{}\n",
        crashlog_timestamp_ms(),
        env!("CARGO_PKG_VERSION"),
        thread_name,
        location,
        message,
        backtrace
    );
    let _ = crashlog_write_named_report("panic", "latest-crash.log", &body);
    CRASHLOG_WRITING.store(false, Ordering::Relaxed);
}

fn crashlog_panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_owned()
    } else {
        "unknown panic payload".to_owned()
    }
}

fn crashlog_write_error_report(context: &str, details: &str) -> Option<PathBuf> {
    let body = format!(
        "BenchScope error report\n\
         TimestampUnixMs: {}\n\
         Version: {}\n\
         Context: {}\n\n\
         Details:\n{}\n",
        crashlog_timestamp_ms(),
        env!("CARGO_PKG_VERSION"),
        context,
        details
    );
    crashlog_write_named_report("error", "latest-error.log", &body)
}

fn crashlog_write_operation(context: &str, details: &str) -> Option<PathBuf> {
    let body = format!(
        "BenchScope diagnostic event\n\
         TimestampUnixMs: {}\n\
         Version: {}\n\
         Context: {}\n\n\
         Details:\n{}\n\n",
        crashlog_timestamp_ms(),
        env!("CARGO_PKG_VERSION"),
        context,
        details
    );
    let dir = crashlog_dir();
    fs::create_dir_all(&dir).ok()?;
    let last_path = dir.join("last-operation.log");
    fs::write(&last_path, &body).ok()?;
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("diagnostic-events.log"))
    {
        let _ = file.write_all(body.as_bytes());
    }
    Some(last_path)
}

fn crashlog_write_named_report(prefix: &str, latest_name: &str, body: &str) -> Option<PathBuf> {
    let dir = crashlog_dir();
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{prefix}-{}.log", crashlog_timestamp_ms()));
    fs::write(&path, body).ok()?;
    let _ = fs::write(dir.join(latest_name), body);
    Some(path)
}

fn render_crashlog_report() -> String {
    let dir = crashlog_dir();
    let mut report = String::new();
    report.push_str("# BenchScope Crash Logs\n\n");
    report.push_str(&format!("- Folder: `{}`\n\n", dir.display()));
    for (title, file_name) in [
        ("Last Operation", "last-operation.log"),
        ("Latest Crash", "latest-crash.log"),
        ("Latest Error", "latest-error.log"),
    ] {
        report.push_str(&format!("## {title}\n\n"));
        let path = dir.join(file_name);
        match crashlog_read_limited(&path) {
            Some(content) if !content.trim().is_empty() => {
                report.push_str("```text\n");
                report.push_str(&content);
                if !content.ends_with('\n') {
                    report.push('\n');
                }
                report.push_str("```\n\n");
            }
            _ => report.push_str("No log captured yet.\n\n"),
        }
    }
    report
}

fn crashlog_read_limited(path: &PathBuf) -> Option<String> {
    const MAX_CRASHLOG_REPORT_BYTES: usize = 16_000;
    let mut text = fs::read_to_string(path).ok()?;
    if text.len() > MAX_CRASHLOG_REPORT_BYTES {
        let mut truncate_at = MAX_CRASHLOG_REPORT_BYTES;
        while truncate_at > 0 && !text.is_char_boundary(truncate_at) {
            truncate_at -= 1;
        }
        text.truncate(truncate_at);
        text.push_str("\n... truncated ...\n");
    }
    Some(text)
}

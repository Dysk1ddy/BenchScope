use std::{
    any::Any,
    collections::{HashMap, VecDeque},
    fmt,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::ToSocketAddrs,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(windows)]
use std::os::windows::{fs::OpenOptionsExt, process::CommandExt};

use anyhow::{Context, Result, anyhow};
use bytemuck::{Pod, Zeroable};
use eframe::egui;
use wgpu::util::DeviceExt;
#[cfg(windows)]
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

include!("crashlog.rs");
include!("constants.rs");
include!("features/matrix_benchmark/mod.rs");
include!("features/ai_training_benchmark/mod.rs");
include!("features/gpu_memory_benchmark/mod.rs");
include!("features/main_menu/mod.rs");
include!("features/drive_benchmark/mod.rs");
include!("features/storage_health/mod.rs");
include!("features/ram_tester/mod.rs");
include!("features/battery_health_diagnostic/mod.rs");
include!("features/network_diagnostic/mod.rs");
include!("features/device_info/mod.rs");
include!("sensors/mod.rs");
include!("ui/mod.rs");
include!("timeline/mod.rs");
include!("history/mod.rs");
include!("app/mod.rs");
include!("cli.rs");

fn main() -> eframe::Result<()> {
    install_crash_logger();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match run_cli(&args) {
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(err) => {
            eprintln!("{err:#}");
            std::process::exit(1);
        }
    }

    #[cfg(windows)]
    if !is_process_elevated() {
        if let Err(err) = restart_app_as_admin() {
            eprintln!(
                "BenchScope must start as administrator for Windows hardware sensors: {err:#}"
            );
            std::process::exit(1);
        }
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1320.0, 900.0])
            .with_min_inner_size([1120.0, 780.0]),
        ..Default::default()
    };
    eframe::run_native(
        "BenchScope",
        options,
        Box::new(|cc| Ok(Box::new(BenchScopeRoot::new(cc)))),
    )
}

include!("tests.rs");

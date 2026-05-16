fn auto_drive_file_size(profile: DriveProfile) -> u64 {
    match profile {
        DriveProfile::Quick => 256 * 1024 * 1024,
        DriveProfile::Balanced => 512 * 1024 * 1024,
        DriveProfile::Thorough => 1024 * 1024 * 1024,
    }
}

fn detect_drives() -> Vec<DriveInfo> {
    #[cfg(windows)]
    {
        let device_names = windows_drive_device_names();
        let mut drives = Vec::new();
        for letter in b'A'..=b'Z' {
            let letter = letter as char;
            let root = PathBuf::from(format!("{letter}:\\"));
            if root.is_dir() {
                drives.push(DriveInfo::with_device_name(
                    root,
                    device_names.get(&letter).cloned(),
                ));
            }
        }
        drives
    }

    #[cfg(not(windows))]
    {
        vec![DriveInfo::with_device_name(PathBuf::from("/"), None)]
    }
}

#[cfg(windows)]
fn windows_drive_device_names() -> HashMap<char, String> {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
Get-Volume |
    Where-Object { $_.DriveLetter } |
    Sort-Object DriveLetter |
    ForEach-Object {
        $letter = $_.DriveLetter
        $partition = Get-Partition -DriveLetter $letter -ErrorAction SilentlyContinue | Select-Object -First 1
        $disk = $null
        if ($partition) {
            $disk = $partition | Get-Disk -ErrorAction SilentlyContinue
        }
        $name = ''
        if ($disk) {
            $name = $disk.FriendlyName
            if (-not $name) {
                $name = $disk.Model
            }
        }
        if (-not $name) {
            $name = $_.FileSystemLabel
        }
        "$letter`t$name"
    }
"#;
    run_powershell_sensor_script(script)
        .ok()
        .map(|output| parse_drive_device_names(&output))
        .unwrap_or_default()
}

#[cfg(windows)]
fn parse_drive_device_names(output: &str) -> HashMap<char, String> {
    output
        .lines()
        .filter_map(|line| {
            let (letter, name) = line.split_once('\t')?;
            let letter = letter.trim().chars().next()?.to_ascii_uppercase();
            if !letter.is_ascii_alphabetic() {
                return None;
            }
            let name = name.trim();
            (!name.is_empty()).then(|| (letter, name.to_owned()))
        })
        .collect()
}

fn selected_drive_for_path(drives: &[DriveInfo], path: &PathBuf) -> Option<usize> {
    let root = drive_root_for_path(path)?;
    drives
        .iter()
        .position(|drive| same_drive_root(&drive.root, &root))
}

fn same_drive_root(left: &PathBuf, right: &PathBuf) -> bool {
    let left = left.display().to_string();
    let right = right.display().to_string();
    let left = left.trim_end_matches(|ch| ch == '\\' || ch == '/');
    let right = right.trim_end_matches(|ch| ch == '\\' || ch == '/');
    left.eq_ignore_ascii_case(right)
}

fn drive_root_for_path(path: &PathBuf) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let text = path.display().to_string();
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            let letter = (bytes[0] as char).to_ascii_uppercase();
            return Some(PathBuf::from(format!("{letter}:\\")));
        }
        None
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        Some(PathBuf::from("/"))
    }
}

fn drive_letter_for_path(path: &PathBuf) -> Option<char> {
    #[cfg(windows)]
    {
        let text = path.display().to_string();
        let bytes = text.as_bytes();
        if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
            return Some((bytes[0] as char).to_ascii_uppercase());
        }
        None
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        None
    }
}


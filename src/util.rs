use std::fs::{self, File, FileTimes};
use std::io::{self, BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::models::SourceMapping;

pub fn compute_file_crc32(path: &Path) -> io::Result<u32> {
    let mut file = File::open(path)?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0u8; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize())
}

pub fn normalize_rel_path(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub fn is_safe_relative_path(rel_str: &str) -> bool {
    if rel_str.is_empty() || rel_str.starts_with('/') || rel_str.starts_with('\\') {
        return false;
    }
    let path = Path::new(rel_str);
    if path.is_absolute() {
        return false;
    }
    let mut has_normal = false;
    for comp in path.components() {
        match comp {
            Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.is_empty()
                    || s == "."
                    || s == ".."
                    || s == ".version-history"
                    || s == ".versionhistory"
                    || s.starts_with(".version-history.")
                    || s.starts_with(".versionhistory.")
                    || s == ".vcopy.json"
                    || s == ".vcopy.lock"
                    || s == ".vcopy.pid_lock"
                    || s.starts_with(".vcopy.")
                {
                    return false;
                }
                has_normal = true;
            }
            _ => return false,
        }
    }
    has_normal
}

pub fn split_stem_ext(filename: &str) -> (&str, &str) {
    if filename.starts_with('.') && !filename[1..].contains('.') {
        // e.g. ".bashrc" -> stem=".bashrc", ext=""
        return (filename, "");
    }
    match filename.rfind('.') {
        Some(idx) if idx > 0 => (&filename[..idx], &filename[idx..]),
        _ => (filename, ""),
    }
}

pub fn parse_timestamp_segment(s: &str) -> Option<u64> {
    if s.len() != 17 {
        return None;
    }
    let b = s.as_bytes();
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'_' {
        return None;
    }
    if !b[0..4].iter().all(u8::is_ascii_digit)
        || !b[5..7].iter().all(u8::is_ascii_digit)
        || !b[8..10].iter().all(u8::is_ascii_digit)
        || !b[11..13].iter().all(u8::is_ascii_digit)
        || !b[13..15].iter().all(u8::is_ascii_digit)
        || !b[15..17].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let digits = format!(
        "{}{}{}{}{}{}",
        &s[0..4],
        &s[5..7],
        &s[8..10],
        &s[11..13],
        &s[13..15],
        &s[15..17]
    );
    digits.parse::<u64>().ok()
}

/// Parses a versioned flat filename such as "notes.v1.txt" or "notes.2026-11-12_143000.txt"
/// into the base filename ("notes.txt") and the version counter/timestamp.
/// Supports extensions, compound extensions, and extensionless files (e.g. "script.v2").
pub fn parse_version_filename(filename: &str) -> Option<(String, u64)> {
    if !is_safe_flat_filename(filename) {
        return None;
    }
    let (stem, ext) = split_stem_ext(filename);

    // Case 1: filename has extension, and stem ends with .v<N> (e.g. "notes.v1.txt")
    if let Some(v_idx) = stem.rfind(".v") {
        let version_str = &stem[v_idx + 2..];
        if !version_str.is_empty() && version_str.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(version_num) = version_str.parse::<u64>() {
                let mut base_stem = &stem[..v_idx];
                // Check if base_stem itself ends with a timestamp (e.g. "notes.2026-11-12_143000.v1.txt")
                if base_stem.len() > 18
                    && base_stem.as_bytes()[base_stem.len() - 18] == b'.'
                    && parse_timestamp_segment(&base_stem[base_stem.len() - 17..]).is_some()
                {
                    base_stem = &base_stem[..base_stem.len() - 18];
                }
                if !base_stem.is_empty() {
                    let base_name = format!("{}{}", base_stem, ext);
                    return Some((base_name, version_num));
                }
            }
        }
    }

    // Case 2: filename has no trailing extension, so split_stem_ext treated .v<N> as ext (e.g. "script.v10")
    if let Some(version_str) = ext.strip_prefix(".v") {
        if !version_str.is_empty() && version_str.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(version_num) = version_str.parse::<u64>() {
                let mut base_stem = stem;
                if base_stem.len() > 18
                    && base_stem.as_bytes()[base_stem.len() - 18] == b'.'
                    && parse_timestamp_segment(&base_stem[base_stem.len() - 17..]).is_some()
                {
                    base_stem = &base_stem[..base_stem.len() - 18];
                }
                if !base_stem.is_empty() {
                    return Some((base_stem.to_string(), version_num));
                }
            }
        }
    }

    // Case 3: filename has extension, and stem ends with .<YYYY-MM-DD_HHMMSS> (e.g. "notes.2026-11-12_143000.txt")
    if stem.len() > 18 && stem.as_bytes()[stem.len() - 18] == b'.' {
        let ts_str = &stem[stem.len() - 17..];
        if let Some(ts_val) = parse_timestamp_segment(ts_str) {
            let base_stem = &stem[..stem.len() - 18];
            if !base_stem.is_empty() {
                let base_name = format!("{}{}", base_stem, ext);
                return Some((base_name, ts_val));
            }
        }
    }

    // Case 4: extensionless file whose split_stem_ext treated .<YYYY-MM-DD_HHMMSS> as ext (e.g. "script.2026-11-12_143000")
    if let Some(ts_str) = ext.strip_prefix('.') {
        if let Some(ts_val) = parse_timestamp_segment(ts_str) {
            if !stem.is_empty() {
                return Some((stem.to_string(), ts_val));
            }
        }
    }

    None
}

pub fn is_reserved_flat_name(name: &str) -> bool {
    name == ".version-history.baseline.json"
        || name == ".version-history.manifest.jsonl"
        || name == ".version-history.lock"
        || name == ".version-history.pid_lock"
        || name == ".versionhistory.baseline.json"
        || name == ".versionhistory.manifest.jsonl"
        || name == ".versionhistory.lock"
        || name == ".versionhistory.pid_lock"
        || name.starts_with(".version-history.")
        || name.starts_with(".versionhistory.")
        || name == ".vcopy.json"
        || name == ".vcopy.lock"
        || name == ".vcopy.pid_lock"
        || name.starts_with(".vcopy.")
        || name.starts_with(".tmp.")
        || name.contains(".tmp.")
}

pub fn is_safe_flat_filename(name: &str) -> bool {
    !name.contains('/')
        && !name.contains('\\')
        && !is_reserved_flat_name(name)
        && is_safe_relative_path(name)
        && Path::new(name).components().count() == 1
}

pub fn resolve_clean_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Path cannot be empty",
        ));
    }

    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };

    // Walk up to find the longest existing ancestor that can be canonicalized
    let mut existing = abs.as_path();
    let mut tail_components = Vec::new();
    while !existing.exists() {
        if let Some(name) = existing.file_name() {
            tail_components.push(name.to_os_string());
        }
        match existing.parent() {
            Some(p) if !p.as_os_str().is_empty() => existing = p,
            _ => break,
        }
    }

    let mut base = if existing.exists() {
        fs::canonicalize(existing)?
    } else {
        existing.to_path_buf()
    };

    for comp in tail_components.into_iter().rev() {
        base.push(comp);
    }

    // Normalize any remaining "." or ".." in non-existent tail
    let mut normalized = PathBuf::new();
    for comp in base.components() {
        match comp {
            Component::Prefix(p) => normalized.push(p.as_os_str()),
            Component::RootDir => normalized.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("Invalid path traversal in {}", path.display()),
                    ));
                }
            }
            Component::Normal(c) => normalized.push(c),
        }
    }

    Ok(normalized)
}

pub fn is_system_or_root_directory(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    let p_str = path.to_string_lossy();
    let stripped = p_str.strip_prefix(r"\\?\").unwrap_or(&p_str);
    let trimmed = stripped.trim_end_matches(['/', '\\']);

    let blocked_unix = [
        "/etc",
        "/private/etc",
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/var",
        "/private/var",
        "/boot",
        "/dev",
        "/sys",
        "/proc",
        "/root",
        "/System",
        "/Applications",
        "/Library",
    ];
    for b in &blocked_unix {
        if trimmed == *b {
            return true;
        }
    }

    let lower = trimmed.to_lowercase();
    lower == "c:"
        || lower == "c:\\windows"
        || lower == "c:/windows"
        || lower.starts_with("c:\\windows\\system32")
        || lower.starts_with("c:/windows/system32")
        || lower == "c:\\program files"
        || lower == "c:/program files"
        || lower == "c:\\program files (x86)"
        || lower == "c:/program files (x86)"
        || lower == "c:\\recovery"
        || lower == "c:/recovery"
}

pub fn is_user_home_directory(path: &Path) -> bool {
    let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) else {
        return false;
    };
    if home.is_empty() {
        return false;
    }
    let home_path = Path::new(&home);
    if let (Ok(clean_p), Ok(clean_home)) = (resolve_clean_path(path), resolve_clean_path(home_path))
    {
        return clean_p == clean_home;
    }
    false
}

pub fn is_foreign_populated_directory(dest: &Path) -> bool {
    if !dest.is_dir() {
        return false;
    }
    if dest.join(".version-history").is_dir()
        || dest.join(".version-history.baseline.json").is_file()
        || dest.join(".version-history.manifest.jsonl").is_file()
        || dest.join(".versionhistory.baseline.json").is_file()
        || dest.join(".versionhistory.manifest.jsonl").is_file()
        || dest.join(".vcopy.json").is_file()
    {
        return false;
    }
    if let Ok(entries) = std::fs::read_dir(dest) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_reserved_flat_name(&name) {
                return true;
            }
        }
    }
    false
}

pub fn validate_dest_path(dest: &Path, allow_existing: bool) -> io::Result<PathBuf> {
    let clean_dest = resolve_clean_path(dest)?;
    if is_system_or_root_directory(&clean_dest) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to use system or root directory '{}' as destination",
                clean_dest.display()
            ),
        ));
    }
    if is_user_home_directory(&clean_dest) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to use home directory '{}' directly as backup destination. Please use a dedicated subdirectory (e.g. '{}/backups') instead.",
                clean_dest.display(),
                clean_dest.display()
            ),
        ));
    }
    if dest.exists() && !dest.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Destination path '{}' exists and is not a directory",
                dest.display()
            ),
        ));
    }
    if clean_dest
        .components()
        .any(|c| c.as_os_str() == ".version-history")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: destination '{}' cannot be inside a .version-history directory",
                dest.display()
            ),
        ));
    }
    if !allow_existing && is_foreign_populated_directory(&clean_dest) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: destination '{}' exists and contains non-vcopy files, but has no vcopy repository metadata. To avoid accidentally adopting or displacing files in an unrelated directory, choose an empty directory or pass --allow-existing or --force.",
                clean_dest.display()
            ),
        ));
    }
    Ok(clean_dest)
}

pub fn validate_sync_paths(
    sources: &[PathBuf],
    dest: &Path,
    allow_existing: bool,
    allow_missing_source: bool,
) -> io::Result<()> {
    let clean_dest = validate_dest_path(dest, allow_existing)?;

    for src in sources {
        if src.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Source path cannot be empty",
            ));
        }
        if !allow_missing_source && !src.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "Safety error: source path '{}' does not exist or is unavailable. Aborting sync to prevent accidental deletion of destination data. (Pass --allow-missing-source or --force to proceed anyway)",
                    src.display()
                ),
            ));
        }
        let clean_src = resolve_clean_path(src)?;
        if clean_src == clean_dest {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Safety error: source '{}' and destination '{}' cannot be the same path",
                    src.display(),
                    dest.display()
                ),
            ));
        }
        if clean_dest.starts_with(&clean_src) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Safety error: destination '{}' cannot be inside source '{}'",
                    dest.display(),
                    src.display()
                ),
            ));
        }
        if clean_src.starts_with(&clean_dest) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "Safety error: source '{}' cannot be inside destination '{}'",
                    src.display(),
                    dest.display()
                ),
            ));
        }
    }

    Ok(())
}

pub fn validate_restore_paths(
    dest: &Path,
    target_out: &Path,
    allow_existing: bool,
) -> io::Result<()> {
    let clean_dest = validate_dest_path(dest, true)?;
    let clean_out = resolve_clean_path(target_out)?;

    if is_system_or_root_directory(&clean_out) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to use system or root directory '{}' as restore output directory",
                clean_out.display()
            ),
        ));
    }
    if is_user_home_directory(&clean_out) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to restore directly into home directory '{}'. Please use a dedicated subdirectory instead.",
                clean_out.display()
            ),
        ));
    }
    if target_out.exists() && !target_out.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Restore output path '{}' exists and is not a directory",
                target_out.display()
            ),
        ));
    }
    if clean_out == clean_dest {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: restore output '{}' cannot be the same as backup destination '{}'",
                target_out.display(),
                dest.display()
            ),
        ));
    }
    if clean_out.starts_with(&clean_dest) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: restore output '{}' cannot be inside backup destination '{}'",
                target_out.display(),
                dest.display()
            ),
        ));
    }
    if clean_dest.starts_with(&clean_out) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: backup destination '{}' cannot be inside restore output '{}'",
                dest.display(),
                target_out.display()
            ),
        ));
    }
    if !allow_existing && target_out.is_dir() {
        if let Ok(mut entries) = std::fs::read_dir(target_out) {
            if entries.next().is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Safety error: restore target '{}' is not empty. Pass --allow-existing or --force to overwrite/merge into an existing directory.",
                        target_out.display()
                    ),
                ));
            }
        }
    }

    Ok(())
}

pub fn sanitize_segment(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            other => other,
        })
        .collect()
}

pub fn get_file_metadata(path: &Path) -> std::io::Result<(u64, i64, SystemTime)> {
    let meta = fs::metadata(path)?;
    let size = meta.len();
    let mtime = meta.modified()?;
    let mtime_ns = match mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i64,
        Err(e) => -(e.duration().as_nanos() as i64),
    };
    Ok((size, mtime_ns, mtime))
}

pub fn get_file_permissions(path: &Path) -> (Option<u32>, Option<bool>) {
    let Ok(meta) = fs::metadata(path) else {
        return (None, None);
    };

    let readonly = Some(meta.permissions().readonly());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = Some(meta.permissions().mode() & 0o7777);
        (mode, readonly)
    }

    #[cfg(not(unix))]
    {
        (None, readonly)
    }
}

pub fn apply_file_permissions(
    path: &Path,
    mode: Option<u32>,
    readonly: Option<bool>,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(m) = mode {
            let perms = fs::Permissions::from_mode(m);
            fs::set_permissions(path, perms)?;
        } else if let Some(ro) = readonly {
            let meta = fs::metadata(path)?;
            let mut perms = meta.permissions();
            let current_mode = perms.mode() & 0o7777;
            let new_mode = if ro {
                current_mode & !0o222
            } else {
                current_mode | 0o200
            };
            perms.set_mode(new_mode);
            fs::set_permissions(path, perms)?;
        }
    }

    #[cfg(not(unix))]
    {
        let ro_target = readonly.or_else(|| mode.map(|m| (m & 0o200) == 0));
        if let Some(ro) = ro_target {
            let meta = fs::metadata(path)?;
            let mut perms = meta.permissions();
            perms.set_readonly(ro);
            fs::set_permissions(path, perms)?;
        }
    }

    Ok(())
}

pub fn mtime_ns_to_system_time(mtime_ns: i64) -> SystemTime {
    if mtime_ns >= 0 {
        UNIX_EPOCH + Duration::from_nanos(mtime_ns as u64)
    } else {
        UNIX_EPOCH
            .checked_sub(Duration::from_nanos((-mtime_ns) as u64))
            .unwrap_or(UNIX_EPOCH)
    }
}

pub fn set_file_mtime(path: &Path, mtime: SystemTime) -> std::io::Result<()> {
    let file = match File::options().write(true).open(path) {
        Ok(f) => f,
        Err(_) => File::open(path)?,
    };
    let times = FileTimes::new().set_modified(mtime);
    file.set_times(times)?;
    Ok(())
}

pub fn copy_atomic(src: &Path, dst: &Path, pid: u32, counter: u64) -> std::io::Result<u64> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let orig_name = dst
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let tmp_name = format!(".{}.tmp.{}.{}", orig_name, pid, counter);
    let tmp_path = dst.with_file_name(tmp_name);

    // Copy contents via io::copy rather than fs::copy so that restrictive OS permissions
    // (such as chmod 0444 or read-only flags) are not applied to the backup mirror files.
    let copy_res = (|| -> std::io::Result<u64> {
        let mut reader = File::open(src)?;
        let mut writer = File::create(&tmp_path)?;
        std::io::copy(&mut reader, &mut writer)
    })();
    let bytes = match copy_res {
        Ok(b) => b,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    // Preserve modified timestamp on copied file
    if let Ok(meta) = fs::metadata(src) {
        if let Ok(mtime) = meta.modified() {
            let _ = set_file_mtime(&tmp_path, mtime);
        }
    }

    if let Err(e) = fs::rename(&tmp_path, dst) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }
    Ok(bytes)
}

pub fn cleanup_empty_parents(file_path: &Path, stop_at_root: &Path) {
    let mut current = file_path.parent();
    while let Some(dir) = current {
        if dir == stop_at_root || !dir.starts_with(stop_at_root) {
            break;
        }
        if dir.file_name().is_some_and(|n| n == ".version-history") {
            break;
        }
        // fs::remove_dir only removes an empty directory; stops at first non-empty ancestor
        if fs::remove_dir(dir).is_err() {
            break;
        }
        current = dir.parent();
    }
}

pub fn parse_sources(
    sources_in: &[PathBuf],
    paths_from: &[String],
) -> std::io::Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();

    for pf in paths_from {
        if pf == "-" {
            let stdin = io::stdin();
            let reader = stdin.lock();
            for line in reader.lines() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                resolved.push(PathBuf::from(trimmed));
            }
        } else if pf.contains('*') || pf.contains('?') || pf.contains('[') {
            let paths: Vec<PathBuf> = match glob::glob(pf) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .filter(|p| p.is_file())
                    .collect(),
                Err(e) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("Invalid glob pattern '{}': {}", pf, e),
                    ));
                }
            };

            if paths.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("No files matched pattern '{}'", pf),
                ));
            }

            let mut sorted_paths = paths;
            sorted_paths.sort();

            for path in sorted_paths {
                let file = File::open(&path)?;
                let reader = BufReader::new(file);
                for line in reader.lines() {
                    let line = line?;
                    let trimmed = line.trim();
                    if trimmed.is_empty() || trimmed.starts_with('#') {
                        continue;
                    }
                    resolved.push(PathBuf::from(trimmed));
                }
            }
        } else {
            let file = File::open(pf)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                resolved.push(PathBuf::from(trimmed));
            }
        }
    }

    for s in sources_in {
        let s_str = s.to_string_lossy();
        if s_str.contains('*') || s_str.contains('?') || s_str.contains('[') {
            if let Ok(entries) = glob::glob(&s_str) {
                let mut matched: Vec<PathBuf> = entries.filter_map(Result::ok).collect();
                matched.sort();
                if !matched.is_empty() {
                    resolved.extend(matched);
                    continue;
                }
            }
        }
        resolved.push(s.clone());
    }

    Ok(resolved)
}

pub fn build_mappings(sources: &[PathBuf], flatten: bool) -> Vec<SourceMapping> {
    if sources.len() == 1 && !flatten {
        return vec![SourceMapping {
            source_path: sources[0].to_string_lossy().to_string(),
            destination_prefix: "".to_string(),
        }];
    }

    // Count how many file sources share the same basename so colliding file sources
    // (e.g. /dir1/notes.txt and /dir2/notes.txt) get disambiguated prefixes.
    let mut file_name_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for src in sources {
        if src.is_file() {
            let fname = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            *file_name_counts.entry(fname).or_insert(0) += 1;
        }
    }

    let mut map = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for src in sources {
        let base_name = if src.is_file() {
            let fname = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "file".to_string());
            if file_name_counts.get(&fname).copied().unwrap_or(0) > 1 {
                let parent_orig = src
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "file_src".to_string());
                let count = counts.entry(parent_orig.clone()).or_insert(0);
                *count += 1;
                if *count > 1 {
                    format!("{}_{}", parent_orig, count)
                } else {
                    parent_orig
                }
            } else {
                "".to_string()
            }
        } else {
            let orig = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "source".to_string());
            let count = counts.entry(orig.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                format!("{}_{}", orig, count)
            } else if sources.len() == 1 {
                "".to_string()
            } else {
                orig
            }
        };

        map.push(SourceMapping {
            source_path: src.to_string_lossy().to_string(),
            destination_prefix: sanitize_segment(&base_name),
        });
    }
    map
}

#[allow(clippy::manual_is_multiple_of)]
pub fn format_thousand_sep(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let len = s.len();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

pub fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let mut val = bytes as f64 / 1024.0;
    let mut unit_idx = 0;
    while val >= 1024.0 && unit_idx < UNITS.len() - 1 {
        val /= 1024.0;
        unit_idx += 1;
    }
    format!(
        "{:.1} {} ({} B)",
        val,
        UNITS[unit_idx],
        format_thousand_sep(bytes)
    )
}

#[allow(dead_code)]
pub fn format_size_compact(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} B", bytes);
    }
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let mut val = bytes as f64 / 1024.0;
    let mut unit_idx = 0;
    while val >= 1024.0 && unit_idx < UNITS.len() - 1 {
        val /= 1024.0;
        unit_idx += 1;
    }
    format!("{:.1} {}", val, UNITS[unit_idx])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_thousand_sep(0), "0");
        assert_eq!(format_thousand_sep(999), "999");
        assert_eq!(format_thousand_sep(1000), "1,000");
        assert_eq!(format_thousand_sep(12491024), "12,491,024");

        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB (1,024 B)");
        assert_eq!(format_size(12491024), "11.9 MB (12,491,024 B)");
        assert_eq!(format_size_compact(12491024), "11.9 MB");
    }

    #[test]
    fn test_parse_version_filename() {
        assert_eq!(
            parse_version_filename("notes.v1.txt"),
            Some(("notes.txt".to_string(), 1))
        );
        assert_eq!(
            parse_version_filename("archive.tar.v3.gz"),
            Some(("archive.tar.gz".to_string(), 3))
        );
        assert_eq!(
            parse_version_filename("script.v10"),
            Some(("script".to_string(), 10))
        );
        assert_eq!(
            parse_version_filename("disambiguated_from_src.v0.json"),
            Some(("disambiguated_from_src.json".to_string(), 0))
        );
        assert_eq!(
            parse_version_filename("my.v1.file.v2.txt"),
            Some(("my.v1.file.txt".to_string(), 2))
        );

        // Date-based versions
        assert_eq!(
            parse_version_filename("notes.2026-11-12_143000.txt"),
            Some(("notes.txt".to_string(), 20261112143000))
        );
        assert_eq!(
            parse_version_filename("script.2026-11-12_143000"),
            Some(("script".to_string(), 20261112143000))
        );
        assert_eq!(
            parse_version_filename("report.2026-11-12_143000.v2.pdf"),
            Some(("report.pdf".to_string(), 2))
        );

        // Non-versioned files
        assert_eq!(parse_version_filename("notes.txt"), None);
        assert_eq!(parse_version_filename("script"), None);
        assert_eq!(parse_version_filename("video.mp4"), None);
        assert_eq!(parse_version_filename("release.v1.0.tar"), None);
        assert_eq!(parse_version_filename("notes.v.txt"), None);
        assert_eq!(parse_version_filename(".v1"), None);
        assert_eq!(
            parse_version_filename(".version-history.baseline.json"),
            None
        );
    }
}

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
                if s.is_empty() || s == "." || s == ".." || s == ".version-history" {
                    return false;
                }
                has_normal = true;
            }
            _ => return false,
        }
    }
    has_normal
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

pub fn validate_dest_path(dest: &Path) -> io::Result<PathBuf> {
    let clean_dest = resolve_clean_path(dest)?;
    if clean_dest.parent().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to use filesystem root '{}' as backup destination",
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
    Ok(clean_dest)
}

pub fn validate_sync_paths(sources: &[PathBuf], dest: &Path) -> io::Result<()> {
    let clean_dest = validate_dest_path(dest)?;

    for src in sources {
        if src.as_os_str().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Source path cannot be empty",
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

pub fn validate_restore_paths(dest: &Path, target_out: &Path) -> io::Result<()> {
    let clean_dest = validate_dest_path(dest)?;
    let clean_out = resolve_clean_path(target_out)?;

    if clean_out.parent().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "Safety error: refusing to use filesystem root '{}' as restore output directory",
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

    let bytes = match fs::copy(src, &tmp_path) {
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
    files_from: Option<&PathBuf>,
) -> std::io::Result<Vec<PathBuf>> {
    let mut resolved = Vec::new();
    if let Some(ff) = files_from {
        let file = File::open(ff)?;
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
    for s in sources_in {
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

    let mut map = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for src in sources {
        let base_name = if flatten || src.is_file() {
            "".to_string()
        } else {
            let orig = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "source".to_string());
            let count = counts.entry(orig.clone()).or_insert(0);
            *count += 1;
            if *count > 1 {
                format!("{}_{}", orig, count)
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

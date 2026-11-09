use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

pub enum BackupLock {
    Advisory(File),
    PidFile { path: PathBuf },
}

impl Drop for BackupLock {
    fn drop(&mut self) {
        match self {
            BackupLock::Advisory(file) => {
                let _ = file.unlock();
            }
            BackupLock::PidFile { path } => {
                let _ = fs::remove_file(path);
            }
        }
    }
}

pub fn acquire_lock(dest: &Path) -> io::Result<BackupLock> {
    acquire_lock_named(dest, ".version-history", ".lock")
}

pub fn acquire_flat_lock(dest: &Path) -> io::Result<BackupLock> {
    fs::create_dir_all(dest)?;
    let lock_path = dest.join(".vcopy.lock");

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    match file.try_lock_exclusive() {
        Ok(()) => Ok(BackupLock::Advisory(file)),
        Err(e) => {
            if e.kind() == io::ErrorKind::WouldBlock {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Destination is locked by another running process (.vcopy.lock held)",
                ));
            }
            let is_unsupported = match e.raw_os_error() {
                Some(code) => code == 95 || code == 38,
                None => false,
            };
            if is_unsupported {
                acquire_flat_pid_lock(dest)
            } else {
                Err(e)
            }
        }
    }
}

fn acquire_flat_pid_lock(dest: &Path) -> io::Result<BackupLock> {
    let pid_lock_path = dest.join(".vcopy.pid_lock");
    let current_pid = process::id();

    for _ in 0..2 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pid_lock_path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", current_pid);
                return Ok(BackupLock::PidFile {
                    path: pid_lock_path,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                if let Ok(mut f) = File::open(&pid_lock_path) {
                    let mut content = String::new();
                    if f.read_to_string(&mut content).is_ok() {
                        if let Ok(existing_pid) = content.trim().parse::<u32>() {
                            if !is_process_alive(existing_pid) {
                                let _ = fs::remove_file(&pid_lock_path);
                                continue;
                            }
                        }
                    }
                }
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Destination is locked by another running process",
                ));
            }
            Err(e) => return Err(e),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "Destination is locked by another running process",
    ))
}

fn acquire_lock_named(dest: &Path, dir_name: &str, file_name: &str) -> io::Result<BackupLock> {
    let hist_dir = dest.join(dir_name);
    fs::create_dir_all(&hist_dir)?;
    let lock_path = hist_dir.join(file_name);

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;

    match file.try_lock_exclusive() {
        Ok(()) => Ok(BackupLock::Advisory(file)),
        Err(e) => {
            // Check if error is WouldBlock (already locked by another process)
            if e.kind() == io::ErrorKind::WouldBlock {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Destination is locked by another running process (.lock file held)",
                ));
            }

            // Check if error is unsupported filesystem (e.g. exFAT/FAT32 FUSE: ENOTSUP / ENOSYS)
            let is_unsupported = match e.raw_os_error() {
                Some(code) => code == 95 || code == 38,
                None => false,
            };

            if is_unsupported {
                // Fall back to PID lock file
                acquire_pid_lock(&hist_dir)
            } else {
                Err(e)
            }
        }
    }
}

fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        Path::new(&format!("/proc/{}", pid)).exists()
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

fn acquire_pid_lock(hist_dir: &Path) -> io::Result<BackupLock> {
    let pid_lock_path = hist_dir.join(".pid_lock");
    let current_pid = process::id();

    for _ in 0..2 {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pid_lock_path)
        {
            Ok(mut f) => {
                let _ = writeln!(f, "{}", current_pid);
                return Ok(BackupLock::PidFile {
                    path: pid_lock_path,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // Read PID from lock file
                if let Ok(mut f) = File::open(&pid_lock_path) {
                    let mut content = String::new();
                    if f.read_to_string(&mut content).is_ok() {
                        if let Ok(existing_pid) = content.trim().parse::<u32>() {
                            if !is_process_alive(existing_pid) {
                                // Stale lock file, remove and retry
                                let _ = fs::remove_file(&pid_lock_path);
                                continue;
                            }
                        }
                    }
                }
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Destination is locked by another running process",
                ));
            }
            Err(e) => return Err(e),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "Destination is locked by another running process",
    ))
}

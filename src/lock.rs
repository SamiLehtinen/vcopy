use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;

pub enum BackupLock {
    Advisory { file: Option<File>, path: PathBuf },
    PidFile { path: PathBuf },
}

impl Drop for BackupLock {
    fn drop(&mut self) {
        match self {
            BackupLock::Advisory { file, path } => {
                #[cfg(unix)]
                {
                    let _ = fs::remove_file(path);
                    if let Some(f) = file.take() {
                        let _ = f.unlock();
                        drop(f);
                    }
                }
                #[cfg(not(unix))]
                {
                    if let Some(f) = file.take() {
                        let _ = f.unlock();
                        drop(f);
                    }
                    let _ = fs::remove_file(path);
                }
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
    let lock_path = dest.join(".version-history.lock");
    acquire_advisory_with_fallback(
        lock_path,
        "Destination is locked by another running process (.version-history.lock held)",
        || acquire_flat_pid_lock(dest),
    )
}

fn is_same_inode(file: &File, lock_path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), fs::symlink_metadata(lock_path)) {
            (Ok(f_meta), Ok(p_meta)) => {
                f_meta.dev() == p_meta.dev() && f_meta.ino() == p_meta.ino()
            }
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (file, lock_path);
        true
    }
}

fn acquire_advisory_with_fallback<F>(
    lock_path: PathBuf,
    held_msg: &str,
    pid_fallback: F,
) -> io::Result<BackupLock>
where
    F: FnOnce() -> io::Result<BackupLock>,
{
    for _ in 0..16 {
        let mut open_opts = OpenOptions::new();
        open_opts
            .read(true)
            .write(true)
            .create(true)
            .truncate(false);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // Omit FILE_SHARE_DELETE (0x4) so another process cannot delete the lock file
            // while this handle is open (prevents STATUS_DELETE_PENDING / PermissionDenied).
            open_opts.share_mode(0x1 | 0x2);
        }

        let file = match open_opts.open(&lock_path) {
            Ok(f) => f,
            #[cfg(windows)]
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                std::thread::yield_now();
                continue;
            }
            Err(e) => return Err(e),
        };

        match file.try_lock_exclusive() {
            Ok(()) => {
                // Verify the locked fd still corresponds to the path on disk so we
                // never enter the critical section holding a stale unlinked inode.
                if is_same_inode(&file, &lock_path) {
                    return Ok(BackupLock::Advisory {
                        file: Some(file),
                        path: lock_path,
                    });
                }
                let _ = file.unlock();
                drop(file);
            }
            Err(e) => {
                if e.kind() == io::ErrorKind::WouldBlock {
                    return Err(io::Error::new(io::ErrorKind::WouldBlock, held_msg));
                }
                let is_unsupported = e.kind() == io::ErrorKind::Unsupported
                    || matches!(e.raw_os_error(), Some(95 | 38 | 45 | 78));
                if is_unsupported {
                    drop(file);
                    let _ = fs::remove_file(&lock_path);
                    return pid_fallback();
                }
                return Err(e);
            }
        }
    }

    Err(io::Error::new(io::ErrorKind::WouldBlock, held_msg))
}

fn acquire_flat_pid_lock(dest: &Path) -> io::Result<BackupLock> {
    let pid_lock_path = dest.join(".version-history.pid_lock");
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
    acquire_advisory_with_fallback(
        lock_path,
        "Destination is locked by another running process (.lock file held)",
        || acquire_pid_lock(&hist_dir),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_lock_stale_recovery_and_active_contention() {
        let dir = std::env::temp_dir().join(format!("vcopy_pid_lock_test_{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. Write stale PID (999_999_999) and verify acquire_pid_lock recovers it
        #[cfg(target_os = "linux")]
        {
            fs::write(dir.join(".pid_lock"), "999999999\n").unwrap();
            let lock = acquire_pid_lock(&dir).expect("should recover stale pid lock");
            // 2. While `lock` is held by current process::id(), a second acquire_pid_lock must fail with WouldBlock
            let second = acquire_pid_lock(&dir);
            assert!(second.is_err());
            assert_eq!(second.err().unwrap().kind(), io::ErrorKind::WouldBlock);
            drop(lock);
            assert!(!dir.join(".pid_lock").exists());

            // 3. Same for flat pid lock
            fs::write(dir.join(".version-history.pid_lock"), "999999999\n").unwrap();
            let flat_lock =
                acquire_flat_pid_lock(&dir).expect("should recover stale flat pid lock");
            let flat_second = acquire_flat_pid_lock(&dir);
            assert!(flat_second.is_err());
            drop(flat_lock);
            assert!(!dir.join(".version-history.pid_lock").exists());
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_advisory_lock_cleanup_and_contention() {
        let dir = std::env::temp_dir().join(format!("vcopy_advisory_lock_test_{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 1. Hierarchical advisory lock
        let lock = acquire_lock(&dir).expect("should acquire hierarchical advisory lock");
        let hist_lock = dir.join(".version-history/.lock");
        assert!(hist_lock.exists());

        let second = acquire_lock(&dir);
        assert!(second.is_err());
        assert_eq!(second.err().unwrap().kind(), io::ErrorKind::WouldBlock);
        assert!(
            hist_lock.exists(),
            "failed contention attempt must not remove active lock file"
        );

        drop(lock);
        assert!(
            !hist_lock.exists(),
            "hierarchical .lock file should be removed when lock is dropped"
        );

        // 2. Flat advisory lock
        let flat_lock = acquire_flat_lock(&dir).expect("should acquire flat advisory lock");
        let flat_lock_path = dir.join(".version-history.lock");
        assert!(flat_lock_path.exists());

        let flat_second = acquire_flat_lock(&dir);
        assert!(flat_second.is_err());
        assert_eq!(flat_second.err().unwrap().kind(), io::ErrorKind::WouldBlock);
        assert!(
            flat_lock_path.exists(),
            "failed contention attempt must not remove active flat lock file"
        );

        drop(flat_lock);
        assert!(
            !flat_lock_path.exists(),
            "flat .version-history.lock file should be removed when lock is dropped"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_advisory_lock_concurrent_race_and_cleanup() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Barrier;
        use std::thread;
        use std::time::Duration;

        fn run_race(
            dir: &Path,
            acquire_fn: fn(&Path) -> io::Result<BackupLock>,
            lock_rel_path: &str,
        ) {
            let active_holders = AtomicUsize::new(0);
            let acquired_count = AtomicUsize::new(0);
            let would_block_count = AtomicUsize::new(0);
            let counter_file = dir.join("counter.txt");
            fs::write(&counter_file, "0").unwrap();

            let threads = 8;
            let iterations = 40;
            let barrier = Barrier::new(threads);

            thread::scope(|s| {
                for _ in 0..threads {
                    s.spawn(|| {
                        barrier.wait();
                        for _ in 0..iterations {
                            match acquire_fn(dir) {
                                Ok(lock) => {
                                    let prev = active_holders.fetch_add(1, Ordering::SeqCst);
                                    assert_eq!(
                                        prev, 0,
                                        "mutual exclusion violated: two threads held lock simultaneously"
                                    );

                                    // Read-modify-write shared state inside critical section
                                    let val: usize = fs::read_to_string(&counter_file)
                                        .unwrap()
                                        .trim()
                                        .parse()
                                        .unwrap();
                                    thread::sleep(Duration::from_micros(50));
                                    fs::write(&counter_file, (val + 1).to_string()).unwrap();

                                    acquired_count.fetch_add(1, Ordering::SeqCst);
                                    let released = active_holders.fetch_sub(1, Ordering::SeqCst);
                                    assert_eq!(released, 1);
                                    drop(lock);
                                }
                                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                                    would_block_count.fetch_add(1, Ordering::SeqCst);
                                    thread::yield_now();
                                }
                                Err(e) => panic!("unexpected lock error: {}", e),
                            }
                        }
                    });
                }
            });

            let total_acquired = acquired_count.load(Ordering::SeqCst);
            let file_val: usize = fs::read_to_string(&counter_file)
                .unwrap()
                .trim()
                .parse()
                .unwrap();
            assert!(total_acquired > 0, "at least one thread must acquire lock");
            assert_eq!(
                file_val, total_acquired,
                "lost update detected in critical section (file_val={}, acquired={})",
                file_val, total_acquired
            );
            assert!(
                would_block_count.load(Ordering::SeqCst) > 0,
                "expected lock contention across concurrent threads"
            );
            assert!(
                !dir.join(lock_rel_path).exists(),
                "lock file {} must be removed after all threads finish",
                lock_rel_path
            );
        }

        let dir = std::env::temp_dir().join(format!("vcopy_lock_race_test_{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        run_race(&dir, acquire_lock, ".version-history/.lock");
        run_race(&dir, acquire_flat_lock, ".version-history.lock");

        let _ = fs::remove_dir_all(&dir);
    }
}

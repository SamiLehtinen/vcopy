# vcopy

**Browseable, zero-link, multi-platform file sync with snapshot history.**

`vcopy` is a lightweight, zero-dependency CLI backup tool written in pure Rust. It creates directly browseable, uncompressed filesystem mirrors on external drives and non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS) while preserving a full historical change record in `.version-history/`.

---

## Key Features & Invariants

* **Raw, Browseable Mirrors**: Files in the destination root remain directly readable in standard file managers (Finder, Windows Explorer, Dolphin, etc.) without requiring proprietary backup viewers or archive extractors.
* **Zero Links**: Never creates hardlinks, symlinks, or block reflinks. Safely works across FAT32, exFAT, and external USB storage devices.
* **Point-in-Time History**: The live mirror state is tracked in `.version-history/baseline.json`, while historical versions of modified or deleted files are archived under `.version-history/<YYYY-MM-DD_HHMMSS>/` alongside their point-in-time `manifest.json`. Deletions are tagged with `.deleted` marker files.
* **Authoritative Nanosecond Metadata & CRC32 Integrity**: Overcomes FAT/exFAT 2-second timestamp drift by persisting exact nanosecond timestamps (`mtime_ns`) and fast SIMD-accelerated CRC32 checksums inside `baseline.json` and each snapshot's `manifest.json`.
* **Atomic Writes**: Writes files and JSON manifests through temporary staging files (`.<file>.tmp.<pid>.<id>`) and atomically renames them to prevent corruption from power failure or interruptions.
* **Strict Path & Purge Safety Guards**: Rejects dangerous overlapping source/destination paths (`src == dst`, `dst` inside `src`, `src` inside `dst`, or filesystem root `/`), validates all manifest relative paths against path traversal (`..`), and enforces strict directory/non-symlink checks before pruning any snapshot directory.
* **Non-Blocking Resilient Locking**: Advisory locking via `.version-history/.lock` prevents overlapping writes, with graceful fallback to PID-based lockfile tracking on filesystems without POSIX locking support.
* **Smart GFS Retention**: Grandfather-Father-Son retention (`--keep-hourly`, `--keep-daily`, `--keep-weekly`, `--keep-monthly`, `--keep-yearly`) or count-based pruning (`-n <N>`).
* **Pruning Integrity (Reverse-Delta Consolidation)**: Guarantees that pruning intermediate snapshots never corrupts older surviving snapshots by migrating needed displaced files to the immediately preceding kept snapshot prior to removal.

---

## Installation & Building

### Prerequisites
* Rust toolchain (1.75+ recommended; tested on 1.99)
* Standard Cargo build tools

### Build from Source
```bash
# Clone the repository
git clone https://github.com/samilehtinen/vcopy.git
cd vcopy

# Build optimized release binary
cargo build --release

# The compiled static binary is located at target/release/vcopy
./target/release/vcopy --version
```

### Running Tests
```bash
cargo test
```

---

## CLI Usage & Commands

### 1. Default (Auto Backup)
Runs a backup snapshot followed by automatic retention pruning:
```bash
# Backup single folder
vcopy /path/to/photos /mnt/usb/backup

# Backup multiple folders into prefixed subdirectories
vcopy /path/to/photos /path/to/documents /mnt/usb/backup

# Flatten multiple sources into the destination root
vcopy --flatten /path/to/photos /path/to/documents /mnt/usb/backup

# Specify source paths from a file
vcopy --files-from sources.txt /mnt/usb/backup
```

### 2. Explicit Snapshot Creation
Create a backup snapshot without triggering retention pruning:
```bash
vcopy create /path/to/src /mnt/usb/backup
```

### 3. Listing Snapshots
View all recorded snapshots and run statistics:
```bash
vcopy list /mnt/usb/backup
```
Output:
```text
Snapshot               Copied     Modified   Deleted    Transferred    
----------------------------------------------------------------------
2026-11-09_083015      142        0          0          12491024 bytes 
2026-11-09_091520      3          12         1          452010 bytes   
```

### 4. Point-in-Time Restore
Restore destination files to an output folder from a specific point in time or the latest state:
```bash
# Restore latest mirror state
vcopy restore /mnt/usb/backup /path/to/restored

# Restore from a specific historical snapshot
vcopy restore --target-time 2026-11-09_083015 /mnt/usb/backup /path/to/restored
```

### 5. Snapshot Pruning
Prune historical snapshots according to retention rules:
```bash
# Keep only the last 5 snapshots
vcopy prune -n 5 /mnt/usb/backup

# Custom GFS retention
vcopy prune --keep-hourly 24 --keep-daily 14 --keep-weekly 8 /mnt/usb/backup

# Dry run (preview without deleting files)
vcopy prune -n 2 --dry-run --verbose /mnt/usb/backup
```

---

## Retention Policy (GFS & Count)

| Flag | Default | Description |
| :--- | :--- | :--- |
| `-n, --max-versions <N>` | None | Retains only the newest $N$ snapshots (overrides GFS). |
| `--keep-hourly <N>` | 24 | Retains one snapshot per hour for the past $N$ hours. |
| `--keep-daily <N>` | 7 | Retains one snapshot per day for the past $N$ days. |
| `--keep-weekly <N>` | 4 | Retains one snapshot per week for the past $N$ weeks. |
| `--keep-monthly <N>` | 12 | Retains one snapshot per month for the past $N$ months. |
| `--keep-yearly <N>` | 1 | Retains one snapshot per year for the past $N$ years. |

---

## Architecture & Design Fixes

`vcopy` resolves several fundamental issues present in earlier backup tool prototypes:

1. **Timestamp Preservation & Drift Resilience**:
   Standard file copy (`std::fs::copy`) resets file timestamps. `vcopy` explicitly preserves source `mtime` on copied files and records exact nanosecond timestamps and CRC32 hashes in `manifest.json`.
2. **Deterministic Restore**:
   Historical displaced files are indexed and verified against manifest metadata during restore, guaranteeing exact point-in-time state recovery without false matches.
3. **Safe Historical Pruning**:
   When intermediate snapshots are pruned under GFS rules, any displaced files referenced by surviving older snapshots are preserved, preventing data loss.
4. **Resilient Non-Blocking Advisory Locking**:
   Non-blocking locks (`try_lock_exclusive`) avoid deadlocks during concurrent runs, while automatic fallback to PID tracking ensures compatibility with exFAT/FAT32 filesystems.

---

## License

Dual-licensed under either:
* Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
* MIT License ([LICENSE-MIT](LICENSE-MIT))

at your option.

# vcopy

[![CI](https://github.com/samilehtinen/vcopy/actions/workflows/ci.yml/badge.svg)](https://github.com/samilehtinen/vcopy/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/samilehtinen/vcopy?color=blue)](https://github.com/samilehtinen/vcopy/releases)
[![License](https://img.shields.io/badge/license-0BSD-green.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](https://www.rust-lang.org)

**Directly browseable, zero-link, multi-platform file sync with snapshot history.**

`vcopy` is a lightweight, zero-dependency CLI backup tool written in pure Rust. It creates directly browseable, uncompressed filesystem mirrors on external drives and non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS) while preserving a full historical change record.

The binary embeds its complete documentation in both plain text and Markdown formats, enabling full standalone operation without external manuals or extra files.

---

## Simple Examples & Quick Start

### 1. Basic Single Directory Backup
Syncs `/home/user/documents` into `/mnt/usb/backup`:

```bash
vcopy /home/user/documents /mnt/usb/backup
```
Your current files are immediately browseable in `/mnt/usb/backup/documents/` as regular, uncompressed files. Historical snapshots are archived under `.version-history/`.

---

### 2. Multiple Sources
Pass multiple source directories or files positionally or with the `-s / --sources` flag:

**Positional multiple sources (destination is always the final argument):**
```bash
vcopy /home/user/docs /home/user/photos /home/user/music /mnt/usb/backup
```
Distinct top-level source folders are mirrored in the destination:
```text
/mnt/usb/backup/
├── docs/
├── photos/
└── music/
```

**Using explicit `-s / --sources` and `-d / --dest` flags:**
```bash
vcopy -s /home/user/docs -s /home/user/photos -d /mnt/usb/backup
```

**Reading sources from files, wildcards, or stdin (`-f / --paths-from`):**
```bash
# Read paths from a list file (one path per line, # for comments)
vcopy -f project_paths.txt -d /mnt/usb/backup

# Read paths matching glob patterns
vcopy -f "backups_*.txt" -d /mnt/usb/backup

# Pipe paths directly from find or stdin ('-')
find /home/user/code -name "*.rs" | vcopy -f - -d /mnt/usb/backup
```

---

### 3. Numbered Versions in a Flat Directory (`--flat` or `--flat-date`)
When you want all files in a single flat folder without nested subfolders (ideal for flat USB thumb drives, camera ingest dropboxes, or single-folder cloud storage), use `--flat` (or `--flatten`):

Every file is placed directly in the destination root. Historical revisions receive monotonic numbered version suffixes (`.v0`, `.v1`, `.v2`, ...):

```bash
vcopy --flat /home/user/docs /home/user/photos /mnt/usb/backup
```

**Destination structure with numbered versions:**
```text
/mnt/usb/backup/
├── report.pdf                     # Current live version (latest)
├── report.v0.pdf                  # Oldest historical version (v0)
├── report.v1.pdf                  # Second historical version (v1)
├── photo.jpg                      # Current live photo
├── photo.v0.jpg                   # Prior version of photo
├── notes_from_docs.txt            # Stem collision disambiguated by source
├── notes_from_photos.txt          # Stem collision disambiguated by source
├── notes_from_photos.v0.txt       # Historical version of notes_from_photos
├── .version-history.lock          # Advisory concurrency lock
├── .version-history.baseline.json # Live state manifest & checksums
└── .version-history.manifest.jsonl# Append-only snapshot ledger
```

You can also use `--flat-date` to name displaced flat revisions with full timestamps (e.g. `report.2026-11-09_083000.pdf`) instead of monotonic version numbers.

---

### 4. Excluding Files & Directories (`-e`, `--exclude-from`, `.vcopyignore`)
Easily ignore temporary files, build caches, or system artifacts:

```bash
# Exclude specific glob patterns or folders
vcopy -e "*.tmp" -e "target/" -e ".DS_Store" /home/user/code /mnt/usb/backup

# Read ignore patterns from a file (e.g. .gitignore or custom list)
vcopy --exclude-from .gitignore /home/user/code /mnt/usb/backup
```
`vcopy` also automatically loads `.vcopyignore` files placed directly inside source directories, applying patterns relative to their directory location.

---

### 5. Zero-Obfuscation Configuration Files (`-c / --config`)
No separate manual or syntax to learn! A configuration file can literally contain the **exact same command-line flags and parameters** you already type in the terminal:

```bash
vcopy -c backup.conf
```

**Direct CLI arguments format (`backup.conf`):**
```text
# backup.conf - exact same flags as the command line!
--sources /home/user/documents
--sources /home/user/photos
--dest /mnt/usb/backup
--flat
--perms
--daily 7
--weekly 4
```

**Simple key-value format (`backup.conf`):**
```text
dest = /mnt/usb/backup
sources = /home/user/documents, /home/user/photos
flat = true
perms = true
daily = 7
weekly = 4
```

---

### 6. Listing Snapshots (`list` / `ls`)
View historical snapshots, transfer statistics, and timestamps with human-readable sizes:
```bash
vcopy ls /mnt/usb/backup
```
Example output:
```text
Snapshot                   Copied     Modified   Deleted    Transferred    
---------------------------------------------------------------------------
2026-11-09_083000          142        0          0          11.9 MB (12,491,024 B)
2026-11-09_091500          3          12         1          441.4 KB (452,010 B)
2026-11-09_100000 (live)   0          0          0          0 B
```

---

### 7. Point-in-Time & Partial Restore
Restore the entire mirror, a specific historical snapshot, or a single file / subtree:

```bash
# Restore latest mirror state
vcopy restore /mnt/usb/backup /tmp/restored

# Restore files from a specific snapshot timestamp
vcopy restore -t 2026-11-09_083000 /mnt/usb/backup /tmp/restored

# Restore a single file or directory subtree
vcopy restore /mnt/usb/backup /tmp/restored documents/report.pdf
vcopy restore /mnt/usb/backup /tmp/restored docs/

# Filter files to restore using glob patterns (-i / --include / --filter)
vcopy restore -i "*.pdf" /mnt/usb/backup /tmp/restored

# Restore with POSIX permissions and Windows readonly attributes
vcopy restore -p -t 2026-11-09_083000 /mnt/usb/backup /tmp/restored
```

---

### 8. Integrity Verification & Bit-Rot Detection (`vcopy verify` / `check`)
Verify destination files against recorded SIMD CRC32 checksums to detect disk corruption, bit-rot, or tampering:

```bash
# Full verification: recomputes CRC32 checksums for live mirror and historical snapshots
vcopy verify /mnt/usb/backup

# Fast verification: checks file sizes and nanosecond modification timestamps only
vcopy check --quick /mnt/usb/backup

# Verify files as of a specific historical snapshot timestamp
vcopy verify -t 2026-11-09_083000 /mnt/usb/backup
```

Returns exit code 0 if all files match, or non-zero if corruptions or missing files are detected.

---

### 9. Retention & Snapshot Pruning
Prune old snapshots using count limits or Grandfather-Father-Son (GFS) policies:

```bash
# Keep only the newest 5 snapshots
vcopy prune -n 5 /mnt/usb/backup

# Retain 24 hourly, 7 daily, 4 weekly, 12 monthly, and 1 yearly snapshots
vcopy prune --hourly 24 --daily 7 --weekly 4 /mnt/usb/backup

# Preview pruning actions without deleting files (dry run)
vcopy prune -n 5 --dry-run -v /mnt/usb/backup
```

---

## Parameters & CLI Options Reference

```text
vcopy [OPTIONS] [SOURCES... DEST]... [COMMAND]
```

### Core Options

| Option | Value | Default | Description |
| :--- | :--- | :--- | :--- |
| `-s, --sources` | `<PATH>` | None | Explicit source directory or file. Can be specified multiple times or comma-separated. *(Alias: `--source`)* |
| `-d, --dest` | `<PATH>` | None | Destination backup directory (overrides positional destination or config setting). *(Alias: `--destination`)* |
| `-f, --paths-from` | `<PATTERN>` or `-` | None | Read source paths from text file, file glob (e.g. `backups*`), or stdin (`-`). Can be specified multiple times or comma-separated. *(Aliases: `--files-from`, `--sources-from`)* |
| `-c, --config` | `<FILE>` | None | Path to configuration file. Accepts direct CLI arguments format (exact same flags/args as command line), key-value, TOML, or JSON. |
| `-e, --exclude` | `<PATTERN>` | None | Exclude files or directories matching glob pattern (e.g. `*.tmp`, `target/`). Can be specified multiple times. |
| `--exclude-from` | `<FILE>` | None | Read exclude patterns from a file (one per line, e.g. `.gitignore`). Can be specified multiple times. |
| `--flat` | Flag | `false` | Single-directory flat mode: stores all live and historical files in root using `.v0`, `.v1` version numbers. *(Alias: `--flatten`)* |
| `--flat-date` | Flag | `false` | Flat mode using timestamped versions (`<stem>.<TIMESTAMP>.<ext>`) instead of monotonic version numbers. |
| `-i, --include` | `<PATTERN>` | None | Partial restore filter: restore only matching files or subtrees. *(Alias: `--filter`)* |
| `--quick` | Flag | `false` | Fast verification mode: verify file existence, sizes, and mtime without recomputing CRC32. |
| `-p, --perms` | Flag | `false` | Record POSIX file modes (Unix) and readonly attributes (Windows) in manifests and restore them. *(Aliases: `--preserve-permissions`, `--permissions`)* |
| `-t, --time` | `<TIMESTAMP>` | `latest` | Point-in-time timestamp (`YYYY-MM-DD_HHMMSS` or prefix) or `latest` for `restore` and `verify`. *(Aliases: `--target-time`, `--snapshot`)* |
| `-F, --force` | Flag | `false` | Force execution, bypassing safety circuit breakers (100% loss, foreign directory, missing source). |
| `--allow-existing` | Flag | `false` | Allow destination or restore output directory to contain pre-existing foreign files without failing. |
| `--allow-empty-source` | Flag | `false` | Allow empty source without tripping the 100% deletion safety circuit breaker. |
| `--allow-missing-source` | Flag | `false` | Allow missing source paths without aborting the backup run. |
| `--dry-run` | Flag | `false` | Simulate execution and display planned operations without modifying the filesystem. |
| `-v, --verbose` | Flag | `false` | Print detailed information about copied, modified, and deleted files. |
| `--manual` | Flag | | Print the complete user manual in ASCII / man-page format. *(Alias: `--man`)* |
| `--manual-markdown` | Flag | | Print the complete user manual in Markdown format. *(Aliases: `--manual-md`, `--markdown`)* |

### Retention Policy Options (GFS & Count)

| Option | Shorthand / Aliases | Default | Description |
| :--- | :--- | :--- | :--- |
| `--max-versions` | `-n` | None | Retain only the newest $N$ snapshots (overrides GFS). |
| `--keep-hourly` | `--hourly` | 24 | Retain one snapshot per hour for the past $N$ hours. |
| `--keep-daily` | `--daily` | 7 | Retain one snapshot per day for the past $N$ days. |
| `--keep-weekly` | `--weekly` | 4 | Retain one snapshot per week for the past $N$ weeks. |
| `--keep-monthly` | `--monthly` | 12 | Retain one snapshot per month for the past $N$ months. |
| `--keep-yearly` | `--yearly` | 1 | Retain one snapshot per year for the past $N$ years. |

---

## Embedded Manual & Documentation

Because `vcopy` is distributed as a single static binary, its entire documentation is embedded right inside the executable at compile time. You never have to worry about missing manuals:

```bash
# Display the comprehensive ASCII manual (terminal / man-page format)
vcopy --manual
# or via subcommand
vcopy manual

# Dump the full Markdown manual to stdout or a file
vcopy --manual-markdown > manual.md
# or via subcommand
vcopy manual --markdown > README.md
```

A standard UNIX man page is also included at [`docs/vcopy.1`](docs/vcopy.1) and can be installed via `man docs/vcopy.1` or copied to `/usr/local/share/man/man1/`.

---

## CLI Subcommands Reference

- **`auto`** *(default when no subcommand is specified)*:
  Synchronizes source directories into destination, creates a timestamped snapshot, and automatically runs GFS retention pruning.
  ```bash
  vcopy /source /dest
  vcopy auto /source /dest
  ```
- **`create`**:
  Creates a backup snapshot without running retention pruning:
  ```bash
  vcopy create /source /dest
  ```
- **`restore`**:
  Restores files from destination to a specified target directory from `latest` or `-t / --time <TIMESTAMP>`. Optionally filter by path or pattern:
  ```bash
  vcopy restore /dest /output_dir
  vcopy restore -t 2026-11-09_083000 /dest /output_dir
  vcopy restore /dest /output_dir documents/report.pdf
  vcopy restore -i "*.pdf" /dest /output_dir
  ```
- **`verify`** *(alias: `check`)*:
  Verifies destination files against recorded SIMD CRC32 checksums, reporting verified, corrupted, missing, and untracked files:
  ```bash
  vcopy verify /dest
  vcopy check --quick /dest
  vcopy verify -t 2026-11-09_083000 /dest
  ```
- **`list`** *(alias: `ls`)*:
  Displays all historical snapshots, transfer statistics, and timestamps with formatted human-readable sizes:
  ```bash
  vcopy list /dest
  vcopy ls /dest
  ```
- **`prune`**:
  Manually prunes historical snapshots according to retention rules with reverse-delta consolidation:
  ```bash
  vcopy prune -n 5 /dest
  vcopy prune --keep-daily 7 --keep-weekly 4 /dest
  ```
- **`manual`**:
  Displays the built-in documentation:
  ```bash
  vcopy manual              # ASCII man-page text
  vcopy manual --markdown   # Full Markdown guide
  ```

---

## Configuration Files (Zero-Obfuscation)

You do **not** need to learn a separate manual or format to write a configuration file. A configuration file can literally contain the exact command-line flags and arguments you already use on the CLI:

### Option 1: Direct CLI Arguments Format (`.conf` or `.args`)
Write flags and paths exactly as you would type them in the terminal (one per line or space-separated; `#` for comments):

```text
# backup.conf - exact same flags as the command line!
--sources /home/user/documents
--sources /home/user/photos
--dest /mnt/usb/backup
--flatten
--preserve-permissions
--keep-daily 7
--keep-weekly 4
```

Positional paths can also be listed directly:
```text
# backup.conf
/home/user/documents
/home/user/photos
--dest /mnt/usb/backup
--flatten
--keep-daily 7
```

### Option 2: Simple Key-Value Format
Write plain unquoted `key = value` lines matching the exact CLI flag names:

```text
# backup.conf
dest = /mnt/usb/backup
sources = /home/user/documents, /home/user/photos
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

### Option 3: TOML or JSON Format
The exact same CLI option names (kebab-case or snake_case) work identically in TOML or JSON:

```toml
# backup.toml - uses exact CLI parameter names!
dest = "/mnt/usb/backup"
sources = ["/home/user/documents", "/home/user/photos"]
flatten = true
preserve-permissions = true
verbose = true
keep-hourly = 24
keep-daily = 7
keep-weekly = 4
keep-monthly = 12
keep-yearly = 1
```

```json
{
  "dest": "/mnt/usb/backup",
  "sources": ["/home/user/documents", "/home/user/photos"],
  "flatten": true,
  "preserve-permissions": true,
  "keep-daily": 7,
  "keep-weekly": 4
}
```

Command-line flags and positional arguments always override settings defined in any configuration file.

---

## How It Works & Storage Layouts

`vcopy` provides two distinct storage layouts optimized for different operational environments:

### 1. Hierarchical Snapshot Mode (Default)
Ideal for traditional backups where the directory tree is preserved and displaced files are archived into dated snapshot subdirectories:

```text
/mnt/usb/backup/
├── documents/
│   ├── report.pdf                      # Current live version
│   └── notes.txt                       # Current live version
└── .version-history/
    ├── .lock                           # Advisory lock / PID fallback
    ├── baseline.json                   # Current live manifest
    ├── 2026-11-09_083000/              # Historical snapshot folder
    │   ├── manifest.json               # Full manifest for this snapshot
    │   └── documents/
    │       └── report.pdf              # Displaced prior version from 08:30:00
    └── 2026-11-09_090000/              # Historical snapshot folder
        ├── manifest.json
        └── documents/
            ├── old_file.txt            # Displaced file
            └── old_file.txt.deleted    # Zero-byte deletion marker
```

### 2. Single-Directory Flat Mode (`--flat`)
Ideal when you need all files in a single flat folder (e.g. flat USB drives, photo ingest dropboxes, single-folder cloud buckets):

```text
/mnt/usb/backup/
├── report.pdf                          # Current live version
├── report.v0.pdf                       # Oldest historical version
├── report.v1.pdf                       # Second historical version
├── notes_from_work.txt                 # Disambiguated source collision
├── notes_from_personal.txt             # Disambiguated source collision
├── notes_from_personal.v0.txt          # Historical version of notes_from_personal
├── .version-history.lock               # Advisory lock
├── .version-history.baseline.json      # Latest live state manifest
└── .version-history.manifest.jsonl     # Append-only snapshot ledger
```

- **Monotonic Version Numbering:** Numbered versions (`.v0`, `.v1`, ...) increment monotonically and are never renamed when intermediate snapshots are pruned. This avoids massive metadata renames and directory table fragmentation on FAT32 and exFAT.
- **Timestamped Revisions:** Use `--flat-date` to name displaced flat revisions with their snapshot timestamp (e.g. `report.2026-11-09_083000.pdf`) instead of version numbers.

---

## Technical Invariants & Safety Guarantees

* **Zero Links**: Never creates or follows hardlinks, symlinks, or block reflinks. Operates safely across FAT32, exFAT, external USB drives, and cloud-synced folders.
* **Directly Browseable Destination**: Live mirror files remain raw, uncompressed, and directly readable in any standard OS file manager without proprietary archive extractors.
* **Atomic Staging Writes**: Writes all file copies and manifests through temporary files (`.<file>.tmp.<pid>.<id>`) and atomically renames them into place to prevent corruption upon abrupt power loss or cancellation.
* **Nanosecond Timestamps & SIMD CRC32**: Overcomes FAT/exFAT 2-second timestamp drift by persisting exact nanosecond modification timestamps (`mtime_ns`) and fast SIMD CRC32 hashes in metadata manifests.
* **Safety Circuit Breakers & Path Guards**:
  * **100% Deletion Wipeout Prevention**: If source path is empty, unmounted, or yields 0 files while the destination contains existing files, `vcopy` refuses to wipe the destination, aborting with a safety error unless explicitly overridden with `--allow-empty-source` or `-F / --force`.
  * **Missing / Unmounted Source Guard**: If any source directory does not exist or has become unavailable (e.g. unmounted network or USB drive), `vcopy` halts before executing destructive file displacements unless `--allow-missing-source` or `-F / --force` is passed.
  * **Protected Directory Guards**: Refuses to sync or restore directly into system root (`/`), core system paths (`/etc`, `/usr`, `/bin`, `C:\Windows`, etc.), user home directory root (`$HOME`, `~`), or populated foreign directories lacking `vcopy` repository metadata unless overridden with `--allow-existing` or `-F / --force`.
* **Heuristic Orphan Scan Recovery**: If metadata files (`manifest.json`, `baseline.json`, or `.version-history.manifest.jsonl`) are deleted or corrupted, `vcopy` automatically scans disk, reconstructs snapshots from displaced files, `.deleted` markers, and `.v<N>` version numbers, restoring point-in-time recovery capabilities in both hierarchical and flat modes.
* **Safe Pruning (Reverse-Delta Consolidation)**: Pruning intermediate snapshots migrates displaced files needed by surviving older snapshots before deletion, guaranteeing point-in-time restore integrity across GFS retention cycles.
* **Non-Blocking Resilient Advisory Locking**: Uses advisory locking (`.version-history/.lock` or `.version-history.lock`) to block overlapping concurrent runs, falling back automatically to an atomic PID-file lock on filesystems that do not support POSIX `flock` (e.g. FUSE mounts).

---

## Installation & Pre-Built Binaries

Pre-compiled standalone static binaries for release `v0.2.0` are available on the [GitHub Releases](https://github.com/samilehtinen/vcopy/releases) page:

| Platform | Architecture | Target Triple | Binary Name |
| :--- | :--- | :--- | :--- |
| **Linux (Static MUSL)** | x86_64 | `x86_64-unknown-linux-musl` | `vcopy` |
| **Linux (Static MUSL)** | ARM64 | `aarch64-unknown-linux-musl` | `vcopy` |
| **Windows** | x86_64 | `x86_64-pc-windows-gnullvm` | `vcopy.exe` |
| **Windows** | ARM64 | `aarch64-pc-windows-gnullvm` | `vcopy.exe` |
| **macOS** | Apple Silicon (arm64) | `aarch64-apple-darwin` | `vcopy` |
| **macOS** | Intel (x86_64) | `x86_64-apple-darwin` | `vcopy` |

### Building from Source

Ensure you have Rust 1.75+ installed:

```bash
git clone https://github.com/samilehtinen/vcopy.git
cd vcopy

# Build optimized native release binary
cargo build --release

# Binary is available at target/release/vcopy
./target/release/vcopy --version
```

### Running Tests
```bash
cargo test
```

---

## Documentation & Developer Resources

* [User Manual (`docs/MANUAL.md`)](docs/MANUAL.md): Comprehensive Markdown user guide (also embedded in the binary via `vcopy --manual-markdown`).
* [UNIX Man Page (`docs/vcopy.1`)](docs/vcopy.1): Standard troff man page for Unix/Linux man systems.
* [Engineering Specification (`docs/spec.md`)](docs/spec.md): Complete architecture specification, data schema, collision disambiguation, scan recovery, and verification details.
* [Agent Guidelines (`AGENTS.md`)](AGENTS.md): Environment configuration, cross-compilation instructions, and coding invariants for AI coding agents.

---

## License

Licensed under the Zero-Clause BSD (0BSD) License. See [LICENSE](LICENSE) for details.

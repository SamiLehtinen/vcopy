# VCOPY(1) User Manual

**vcopy** - Directly browseable, zero-link, multi-platform backup and sync tool with historical versioning.

---

## SYNOPSIS

```text
vcopy [OPTIONS] [SOURCES... DEST] [COMMAND]
vcopy manual [--markdown]
vcopy --manual | --manual-markdown
```

---

## QUICK START & SIMPLE EXAMPLES

### 1. Basic Single Directory Backup
Syncs `/home/user/documents` into `/mnt/usb/backup`:
```bash
vcopy /home/user/documents /mnt/usb/backup
```
The live files appear directly in `/mnt/usb/backup/documents/` as regular uncompressed files. Historical snapshots are archived in `.version-history/`.

---

### 2. Multiple Sources
You can pass multiple source directories or files either positionally or with the `-s / --sources` flag:

**Positional multiple sources:**
```bash
# Destination is always the final argument
vcopy /home/user/docs /home/user/photos /home/user/music /mnt/usb/backup
```
In the destination mirror, distinct top-level source folders are maintained:
```text
/mnt/usb/backup/
├── docs/
├── photos/
└── music/
```

**Using the `-s / --sources` and `-d / --dest` flags:**
```bash
vcopy -s /home/user/docs -s /home/user/photos -d /mnt/usb/backup
```

**Reading sources from files or wildcards (`-f / --paths-from`):**
```bash
# Read paths from a list file (one path per line, # for comments)
vcopy -f project_paths.txt -d /mnt/usb/backup

# Read paths matching file glob patterns
vcopy -f "backups_*.txt" -d /mnt/usb/backup

# Pipe paths directly from find or stdin ('-')
find /home/user/code -name "*.rs" | vcopy -f - -d /mnt/usb/backup
```

---

### 3. Numbered Versions in a Flat Directory (`--flatten`)
If you want all files in a single flat directory without subfolder hierarchies (ideal for flat USB thumb drives, camera card ingests, or single-folder cloud storage), use `--flatten`.

Every file is placed directly in the destination root. Historical revisions receive monotonic numbered version suffixes (`.v0`, `.v1`, `.v2`, ...):

```bash
vcopy --flatten /home/user/docs /home/user/photos /mnt/usb/backup
```

**Destination structure with numbered versions:**
```text
/mnt/usb/backup/
├── report.pdf                     # Current live version (latest)
├── report.v0.pdf                  # Oldest historical version (version 0)
├── report.v1.pdf                  # Second historical version (version 1)
├── photo.jpg                      # Current live photo
├── photo.v0.jpg                   # Prior version of photo
├── notes_from_docs.txt            # Stem collision disambiguated by source
├── notes_from_photos.txt          # Stem collision disambiguated by source
├── notes_from_photos.v0.txt       # Historical version of notes_from_photos
├── .version-history.lock          # Advisory concurrency lock
├── .version-history.baseline.json # Live state manifest & checksums
└── .version-history.manifest.jsonl# Append-only snapshot ledger
```

---

### 4. Zero-Obfuscation Configuration Files (`-c / --config`)
No separate manual or syntax to learn! A configuration file uses the **exact same command-line flags and parameters**:

```bash
vcopy -c backup.conf
```

**Direct CLI arguments format (`backup.conf`):**
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

**Simple key-value format (`backup.conf`):**
```text
dest = /mnt/usb/backup
sources = /home/user/documents, /home/user/photos
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

**TOML format (`backup.toml`):**
```toml
dest = "/mnt/usb/backup"
sources = ["/home/user/documents", "/home/user/photos"]
flatten = true
preserve-permissions = true
keep-daily = 7
keep-weekly = 4
```

---

### 5. Listing Snapshots
View historical snapshots, transfer metrics, and file change counts:
```bash
vcopy list /mnt/usb/backup
```
Output:
```text
Snapshot                   Copied     Modified   Deleted    Transferred    
---------------------------------------------------------------------------
2026-11-09_083000          142        0          0          12491024 bytes 
2026-11-09_091500          3          12         1          452010 bytes   
2026-11-09_100000 (live)   0          0          0          0 bytes        
```

---

### 6. Point-in-Time Restore
Restore the entire file tree to an exact prior snapshot or the latest mirror:

```bash
# Restore latest live mirror
vcopy restore /mnt/usb/backup /tmp/restored

# Restore files as they existed at a specific snapshot timestamp
vcopy restore --target-time 2026-11-09_083000 /mnt/usb/backup /tmp/restored

# Restore with POSIX permissions and Windows readonly attributes
vcopy restore -p --target-time 2026-11-09_083000 /mnt/usb/backup /tmp/restored
```

---

### 7. Retention & Snapshot Pruning
Prune old snapshots using count limits or Grandfather-Father-Son (GFS) schedules:

```bash
# Retain only the newest 5 snapshots
vcopy prune -n 5 /mnt/usb/backup

# Retain 24 hourly, 7 daily, 4 weekly, 12 monthly, and 1 yearly snapshots
vcopy prune --keep-hourly 24 --keep-daily 7 --keep-weekly 4 /mnt/usb/backup

# Dry run: preview which snapshots would be deleted without modifying disk
vcopy prune -n 5 --dry-run -v /mnt/usb/backup
```

---

## PARAMETER & OPTIONS REFERENCE

### Core Options

| Option | Shorthand | Value | Default | Description |
| :--- | :--- | :--- | :--- | :--- |
| `--sources` | `-s` | `<PATH>` | None | Explicit source directory or file. Can be specified multiple times or comma-separated. *(Alias: `--source`)* |
| `--dest` | `-d` | `<PATH>` | None | Destination backup directory. *(Alias: `--destination`)* |
| `--paths-from` | `-f` | `<PATTERN>` | None | Read source paths from text file, file glob (e.g. `backups*`), or stdin (`-`). *(Aliases: `--files-from`, `--sources-from`)* |
| `--config` | `-c` | `<FILE>` | None | Path to configuration file (direct CLI args format, key-value, TOML, or JSON). |
| `--flatten` | | Flag | `false` | Single-directory flat mode: stores all live and historical files in root using `.v0`, `.v1` version numbers. |
| `--preserve-permissions` | `-p` | Flag | `false` | Record POSIX mode bits (Unix) and readonly attributes (Windows) in manifests and restore them. |
| `--dry-run` | | Flag | `false` | Simulate execution and display planned operations without writing to disk. |
| `--verbose` | `-v` | Flag | `false` | Enable detailed output listing every copied, modified, or deleted file. |
| `--target-time` | | `<TIMESTAMP>` | `latest` | Snapshot timestamp (`YYYY-MM-DD_HHMMSS` or prefix) for `restore`. |
| `--manual` | | Flag | | Print this complete user manual in ASCII / man-page format. *(Alias: `--man`)* |
| `--manual-markdown` | | Flag | | Print this complete user manual in Markdown format. *(Aliases: `--manual-md`, `--markdown`)* |

### Retention Policy Options (GFS & Count)

| Option | Shorthand | Default | Description |
| :--- | :--- | :--- | :--- |
| `--max-versions` | `-n` | None | Retain only the newest $N$ snapshots (overrides GFS schedules). |
| `--keep-hourly` | | 24 | Number of hourly snapshot buckets to retain. |
| `--keep-daily` | | 7 | Number of daily snapshot buckets to retain. |
| `--keep-weekly` | | 4 | Number of weekly snapshot buckets to retain. |
| `--keep-monthly` | | 12 | Number of monthly snapshot buckets to retain. |
| `--keep-yearly` | | 1 | Number of yearly snapshot buckets to retain. |

---

## SUBCOMMANDS

- **`auto`** *(default when no subcommand is specified)*:
  Synchronizes source directories into destination, creates a timestamped snapshot, and automatically executes GFS retention pruning.
- **`create`**:
  Creates a new backup snapshot without running retention pruning.
- **`restore`**:
  Restores files from destination to a specified target directory from `latest` or `--target-time <TIMESTAMP>`.
- **`list`**:
  Lists all historical snapshots, file counts, and transferred bytes.
- **`prune`**:
  Manually prunes historical snapshots according to retention rules with reverse-delta consolidation.
- **`manual`**:
  Displays this complete user guide. Use `--markdown` for Markdown output.

---

## STORAGE ARCHITECTURE & MODES

### Mode 1: Hierarchical Snapshot Mode (Default)
The live mirror resides at `<dest>/` exactly matching the source hierarchy. When a file is modified or deleted in subsequent runs, the previous version is displaced into a dated snapshot subfolder under `.version-history/<YYYY-MM-DD_HHMMSS>/`:

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

### Mode 2: Single-Directory Flat Mode (`--flatten`)
All files (live files, disambiguated sources, and numbered versions) reside in a single flat destination directory:

- **Numbered Versions**: Historical versions receive monotonic suffixes: `<stem>.v0.<ext>`, `<stem>.v1.<ext>`, ...
- **Collision Disambiguation**: When multiple sources share identical filenames, the parent directory name is prepended (`<stem>_from_<source>.<ext>`).
- **Metadata**: Stored in root as `.version-history.baseline.json`, `.version-history.manifest.jsonl`, and `.version-history.lock`.
- **Orphan Scan Recovery**: If metadata files are missing, `vcopy` scans the directory on startup, parses all `.v<N>` version numbers, and reconstructs the version history automatically.

---

## TECHNICAL INVARIANTS & SAFETY GUARANTEES

1. **Zero Links**:
   Never creates or follows hardlinks, symlinks, or block reflinks. Destination mirrors are 100% portable and fully compatible with FAT32, exFAT, NTFS, ext4, and cloud storage providers.
2. **Directly Browseable Uncompressed Files**:
   Live mirror files and historical versions are stored as standard, uncompressed files readable by any file explorer or utility without special tools.
3. **Atomic Staging Writes**:
   All files and metadata manifests are written to temporary staging files (`.<file>.tmp.<pid>.<id>`) and renamed into place atomically to protect against power loss or mid-process termination.
4. **Nanosecond Timestamps & SIMD CRC32**:
   FAT32 and exFAT store timestamps with 2-second resolution drift. `vcopy` calculates exact nanosecond modification times and fast SIMD-accelerated CRC32 hashes stored in `manifest.json` to prevent unnecessary recopying.
5. **Reverse-Delta Pruning Consolidation**:
   When intermediate snapshots are pruned, displaced files needed by surviving older snapshots are automatically consolidated and migrated into the preceding kept snapshot before deletion. No restore point is ever broken.
6. **Advisory Locking with PID Fallback**:
   Concurrently running backups on the same destination are blocked using `.lock`. If the filesystem lacks POSIX `flock` support (e.g. FUSE mounts), an atomic PID lockfile is used automatically.

---

## AUTHORS & LICENSE

Created by **Sami Lehtinen** (`noreply@sami-lehtinen.net`).
Released under the **Zero-Clause BSD (0BSD)** License.
GitHub Repository: <https://github.com/samilehtinen/vcopy>

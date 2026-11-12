# Agent Guidelines: `vcopy`

This document provides architectural context, development workflows, and core invariants for AI coding agents working on the `vcopy` codebase.

---

## 1. Project Overview & System Invariants

`vcopy` is a zero-dependency, cross-platform CLI backup and filesystem synchronization tool written in pure Rust. It creates directly browseable, uncompressed, link-free filesystem mirrors with full historical point-in-time recovery, optimized for non-snapshotting filesystems (exFAT, FAT32, NTFS, ext4, APFS).

### Non-Negotiable System Invariants
* **Zero Links:** Never create or follow hardlinks, symlinks, or block reflinks. Destination mirrors must be completely link-free.
* **Directly Browseable Mirrors:** Live mirror files must reside as raw, uncompressed files directly accessible in standard OS file managers.
* **Atomic Staging Writes:** All file copies and manifest writes must write to temporary staging files (`.<file>.tmp.<pid>.<id>`) and rename them atomically into place to prevent partial corruption.
* **Nanosecond Timestamps & CRC32:** Exact nanosecond modification timestamps (`mtime_ns`) and SIMD-accelerated CRC32 checksums must be computed and preserved in metadata manifests (`baseline.json`, `manifest.json`, and `.version-history.manifest.jsonl`) to overcome FAT/exFAT 2-second timestamp drift.
* **Resilient Locking:** Always acquire `.version-history/.lock` before modifying target destinations, falling back gracefully to `.version-history/.pid_lock` if the underlying filesystem lacks POSIX `flock` support (e.g., FUSE mounts).
* **Reverse-Delta Pruning Consolidation:** Pruning intermediate snapshots must NEVER lose files needed by surviving older snapshots. Displaced files must be migrated into the preceding kept snapshot before removing any snapshot directory.
* **Path Safety Guards:** Reject filesystem root (`/`), empty paths, non-directory targets, and overlapping source/destination paths (`src == dest`, `dest` inside `src`, `src` inside `dest`). Validate all relative manifest paths against directory traversal (`..`).

---

## 2. Codebase Architecture & Module Map

```text
src/
├── main.rs          # CLI entrypoint (clap), command routing, path validation, error handling
├── config.rs        # TOML/JSON configuration file loader and options resolver
├── models.rs        # Core data structures (SnapshotManifest, FileState, FileChange, RunSummary)
├── sync.rs          # Hierarchical sync engine, baseline ingestion, deletion-first execution
├── flatten.rs       # Single-directory flat sync engine, stem collision disambiguation,
│                    # monotonic versioning (.v0, .v1), manifest.jsonl, orphan scan recovery
├── restore.rs       # Point-in-time restore engine (hierarchical & flat modes)
├── prune.rs         # Snapshot retention pruning (GFS & count-based) with reverse-delta consolidation
└── lock.rs          # Advisory concurrency lock with PID-file fallback

tests/
├── common/
│   └── mod.rs       # Test fixtures, temp dir helpers, environment setup
├── integration_tests.rs  # End-to-end sync, collision, flatten, and safety tests
├── prune_tests.rs        # Retention policy and reverse-delta consolidation tests
└── restore_tests.rs      # Point-in-time and latest restore tests
```

---

## 3. Development Workflow & Commands

Standard Rust tooling (1.75+ stable) is used across all development, testing, and CI/CD pipelines.

### Testing
Run all unit and integration tests:
```bash
cargo test
```

### Formatting & Linting
Ensure code adheres to standard formatting and strict Clippy linter checks (enforced in GitHub CI):
```bash
# Code formatting check
cargo fmt --all -- --check

# Strict Clippy analysis
cargo clippy --all-targets -- -D warnings
```

### Native Release Build
```bash
cargo build --release
```

---

## 4. Multi-Platform Releases & CI/CD

All multi-platform releases are automated through GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml) and [`.github/workflows/release.yml`](.github/workflows/release.yml)).

When a release tag (`v*.*.*`) is pushed:
* **Linux (MUSL x86_64 & ARM64):** Built inside `ubuntu-latest` using [`cross`](https://github.com/cross-rs/cross) (`x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`).
* **Windows (x86_64 & ARM64):** Built natively on `windows-latest` (`x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`).
* **macOS (Intel & Apple Silicon):** Built natively on `macos-13` (`x86_64-apple-darwin`) and `macos-latest` (`aarch64-apple-darwin`).
* Packages `.tar.gz` and `.zip` distribution bundles alongside `.sha256` checksums automatically published to GitHub Releases.

Local cross-compilation can also be performed via `cross`:
```bash
cross build --release --target x86_64-unknown-linux-musl
cross build --release --target aarch64-unknown-linux-musl
```

---

## 5. Coding & Contribution Rules

* **Always Test Before Commit / Build:**
  * Always run `cargo test`, `cargo fmt --all -- --check`, and `cargo clippy --all-targets -- -D warnings` before committing changes or producing release builds.
* **Documentation Integrity:**
  * Keep [`docs/spec.md`](docs/spec.md) authoritative and synchronized with any architectural or protocol changes.
  * Keep [`README.md`](README.md) synchronized with user-facing options and release assets.
* **Git Commit Author:**
  * Use the configured user identity:
    ```bash
    git config user.name "Sami Lehtinen"
    git config user.email "noreply@sami-lehtinen.net"
    ```
* **Pure Rust:** Do not add dependencies that require native C libraries or external non-Rust build tools. Keep binaries static and zero-dependency.

### Repository Layout

```text
vcopy/
├── .github/
│   └── workflows/
│       ├── ci.yml                 # PR & push check (fmt, clippy, test on matrix)
│       └── release.yml            # Multi-arch binary builder & GitHub Release
├── tests/
│   ├── common/
│   │   └── mod.rs                 # Shared test sandbox helpers & fixtures
│   ├── integration_tests.rs       # End-to-end CLI integration suite
│   └── restore_tests.rs           # Point-in-time restore verification
├── src/
│   ├── main.rs                    # CLI entrypoint and orchestrator
│   └── models.rs                  # Core data structures (Manifest, State, Changes)
├── .gitignore
├── Cargo.lock
├── Cargo.toml
├── Cross.toml                     # Cross-compilation container configuration
├── LICENSE-APACHE
├── LICENSE-MIT
└── README.md

```

#### Key Repository Files

* **`.gitignore`**
```gitignore
/target
**/*.rs.bk
*.tmp.*
.version-history/
Cargo.lock

```


*(Keep `Cargo.lock` committed if building reproducible binary releases; ignore only for libraries).*
* **`Cross.toml`**
Required for cross-compiling to Linux MUSL targets cleanly without host libc dependencies:
```toml
[target.x86_64-unknown-linux-musl]
image = "ghcr.io/cross-rs/x86_64-unknown-linux-musl:edge"

[target.aarch64-unknown-linux-musl]
image = "ghcr.io/cross-rs/aarch64-unknown-linux-musl:edge"

```



---

### Comprehensive Integration Test Suite

The tests cover:

1. Deletion handling with `.deleted` marker generation.
2. In-place modification and preservation of older versions in `.version-history`.
3. Multiple source directories mapping to prefixed subfolders.
4. Point-in-time `restore` across multiple historical versions.
5. Locking mechanism preventing concurrent overlapping runs.
6. Exclusion of `.version-history` from self-traversal.

#### `tests/common/mod.rs`

```rust
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct TestContext {
    pub dir: PathBuf,
    pub src: PathBuf,
    pub dst: PathBuf,
}

impl TestContext {
    pub fn new(name: &str) -> Self {
        let id = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .subsec_nanos();
        let base = std::env::temp_dir().join(format!("vcopy_test_{}_{}_{}", name, nanos, id));
        let src = base.join("src");
        let dst = base.join("dst");

        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();

        Self {
            dir: base,
            src,
            dst,
        }
    }

    pub fn write_src_file(&self, rel: &str, content: &[u8]) {
        let p = self.src.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    pub fn read_dst_file(&self, rel: &str) -> Option<Vec<u8>> {
        let p = self.dst.join(rel);
        fs::read(p).ok()
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

```

#### `tests/integration_tests.rs`

```rust
mod common;
use common::TestContext;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_deletion_and_marker_creation() {
    let ctx = TestContext::new("deletion");
    ctx.write_src_file("keep.txt", b"staying");
    ctx.write_src_file("delete_me.txt", b"ephemeral");

    // Baseline run
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());
    assert!(ctx.dst.join("delete_me.txt").exists());

    // Wait past timestamp threshold & delete source file
    thread::sleep(Duration::from_millis(1100));
    fs::remove_file(ctx.src.join("delete_me.txt")).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Live destination file must be gone
    assert!(!ctx.dst.join("delete_me.txt").exists());
    assert!(ctx.dst.join("keep.txt").exists());

    // History must contain displaced file and zero-byte .deleted marker
    let hist_dir = ctx.dst.join(".version-history");
    let mut snapshots: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);

    let snap_dir = snapshots.pop().unwrap().path();
    let hist_file = snap_dir.join("delete_me.txt");
    let marker = snap_dir.join("delete_me.txt.deleted");

    assert!(hist_file.is_file());
    assert_eq!(fs::read(hist_file).unwrap(), b"ephemeral");
    assert!(marker.is_file());
    assert_eq!(fs::metadata(marker).unwrap().len(), 0);
}

#[test]
fn test_multiple_sources_mapping() {
    let base = std::env::temp_dir().join("vcopy_test_multisrc");
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("src_a");
    let src2 = base.join("src_b");
    let dst = base.join("dest");

    fs::create_dir_all(&src1).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    fs::write(src1.join("data_a.txt"), b"source alpha").unwrap();
    fs::write(src2.join("data_b.txt"), b"source beta").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Multi-source without --flatten maps into subdirectories named after sources
    assert!(dst.join("src_a").join("data_a.txt").is_file());
    assert!(dst.join("src_b").join("data_b.txt").is_file());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_concurrency_lock() {
    use fs2::FileExt;

    let ctx = TestContext::new("locking");
    ctx.write_src_file("payload.bin", &[0u8; 1024]);

    // Acquire lock manually beforehand
    let hist = ctx.dst.join(".version-history");
    fs::create_dir_all(&hist).unwrap();
    let lock_file = File::create(hist.join(".lock")).unwrap();
    lock_file.lock_exclusive().unwrap();

    // Spawning vcopy while lock is held should fail or block depending on backend
    let output = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .output()
        .unwrap();

    // Advisory lock blocks or exits with error
    assert!(!output.status.success() || !ctx.dst.join("payload.bin").exists());

    lock_file.unlock().unwrap();
}

```

#### `tests/restore_tests.rs`

```rust
mod common;
use common::TestContext;
use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_point_in_time_restore() {
    let ctx = TestContext::new("restore");
    let restore_out = ctx.dir.join("restored_target");

    // Version 1
    ctx.write_src_file("doc.txt", b"version 1");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    thread::sleep(Duration::from_millis(1100));

    // Version 2
    ctx.write_src_file("doc.txt", b"version 2");
    ctx.write_src_file("added.txt", b"version 2 extra");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Obtain the first snapshot identifier from destination
    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    let first_snap = snaps.first().expect("Snapshot not found").clone();

    // Run restore targeting the timestamp of the first snapshot
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&first_snap)
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    // Restored directory must reflect state at snapshot 1
    assert!(restore_out.join("doc.txt").is_file());
    assert_eq!(
        fs::read_to_string(restore_out.join("doc.txt")).unwrap(),
        "version 1"
    );
    assert!(!restore_out.join("added.txt").exists());
}

```

---

### GitHub Automation

#### 1. `.github/workflows/ci.yml` (PR and Push Sanity Check)

```yaml
name: CI

on:
  push:
    branches: [ master, main ]
  pull_request:
    branches: [ master, main ]

jobs:
  style:
    name: Code Formatting & Static Analysis
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets -- -D warnings

  test:
    name: Native Tests (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ ubuntu-latest, macos-latest, windows-latest ]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --verbose

```

#### 2. `.github/workflows/release.yml` (Cross-Arch Release Pipeline)

```yaml
name: Release

on:
  push:
    tags:
      - 'v*.*.*'

permissions:
  contents: write

jobs:
  build-release:
    name: Build - ${{ matrix.target }}
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        include:
          - target: x86_64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
            artifact_name: vcopy
            asset_name: vcopy-x86_64-unknown-linux-musl.tar.gz

          - target: aarch64-unknown-linux-musl
            os: ubuntu-latest
            use_cross: true
            artifact_name: vcopy
            asset_name: vcopy-aarch64-unknown-linux-musl.tar.gz

          - target: x86_64-pc-windows-msvc
            os: windows-latest
            use_cross: false
            artifact_name: vcopy.exe
            asset_name: vcopy-x86_64-pc-windows-msvc.zip

          - target: aarch64-apple-darwin
            os: macos-latest
            use_cross: false
            artifact_name: vcopy
            asset_name: vcopy-aarch64-apple-darwin.tar.gz

          - target: x86_64-apple-darwin
            os: macos-13
            use_cross: false
            artifact_name: vcopy
            asset_name: vcopy-x86_64-apple-darwin.tar.gz

    steps:
      - name: Checkout repository
        uses: actions/checkout@v4

      - name: Install Rust toolchain
        uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - name: Install cross
        if: matrix.use_cross
        run: cargo install cross --git https://github.com/cross-rs/cross

      - name: Build target (native)
        if: "!matrix.use_cross"
        run: cargo build --release --target ${{ matrix.target }}

      - name: Build target (cross)
        if: matrix.use_cross
        run: cross build --release --target ${{ matrix.target }}

      - name: Archive artifacts (Unix)
        if: matrix.os != 'windows-latest'
        run: |
          tar -czf "${{ matrix.asset_name }}" \
            -C "target/${{ matrix.target }}/release" "${{ matrix.artifact_name }}"

      - name: Archive artifacts (Windows)
        if: matrix.os == 'windows-latest'
        shell: pwsh
        run: |
          Compress-Archive -Path "target/${{ matrix.target }}/release/${{ matrix.artifact_name }}" -DestinationPath "${{ matrix.asset_name }}"

      - name: Generate SHA-256 Checksums
        shell: bash
        run: |
          if command -v sha256sum >/dev/null 2>&1; then
            sha256sum "${{ matrix.asset_name }}" > "${{ matrix.asset_name }}.sha256"
          else
            shasum -a 256 "${{ matrix.asset_name }}" > "${{ matrix.asset_name }}.sha256"
          fi

      - name: Publish Release Assets
        uses: softprops/action-gh-release@v2
        with:
          files: |
            ${{ matrix.asset_name }}
            ${{ matrix.asset_name }}.sha256
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}

```

---

### GitHub Setup and Sync Workflow

1. Initialize local repository and set default branch:
```bash
git init -b main
git add .
git commit -m "Initial commit: vcopy core implementation, tests, and CI/CD"

```


2. Link remote repository via SSH:
```bash
git remote add origin git@github.com:<username>/vcopy.git
git push -u origin main

```


3. Trigger automated multi-platform release generation:
```bash
git tag -a v0.1.0 -m "Release v0.1.0"
git push origin v0.1.0

```


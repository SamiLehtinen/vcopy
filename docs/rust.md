# Rust Build and Test Environment Guide (Ubuntu 24.04 Docker)

This guide documents the runtime environment, directory layout, environment configuration, and verification procedures for compiling and testing Rust projects inside this container.

---

## 1. Environment & Architecture Overview

- **Host-synced persistent volume**: `/data` (Do **not** use `/workspace`; persistent work must reside in `/data`).
- **Container user**: `admin` (Non-root interactive shell, but sudo privileges may exist if configured).
- **Global Toolchain Locations**:
  - `CARGO_HOME`: `/usr/local/cargo`
  - `RUSTUP_HOME`: `/usr/local/rustup`
- **Toolchain Status**: Installed at system level with global write/read access (`a+w`).

---

## 2. Shell Initialization & PATH Setup

Interactive sessions spawned as `admin` might not automatically inherit `/usr/local/cargo/bin` in `$PATH` or have an active toolchain link.

Before running any Cargo or Rust commands, source the environment and confirm the default toolchain:

```bash
# 1. Source cargo environment variables
. "/usr/local/cargo/env"

# 2. Ensure the active user has a default toolchain configured
rustup default stable

# 3. Verify toolchain availability
rustc --version
cargo --version
```

### Persistence for Non-Interactive / Interactive Shells
If modifying user profiles, add the source command to `~/.bashrc`:
```bash
echo '. "/usr/local/cargo/env"' >> ~/.bashrc
```

---

## 3. Directory Conventions

Always build and store source code under `/data` so work persists across container restarts:

- Recommended code path: `/data/src/<project-name>`
- Example: `/data/src/my-rust-project`

### Build Artifact Management (Target Directory)
To avoid high I/O traffic and potential filesystem-sync overhead over the `/data` mount, you can direct build outputs to `/tmp`:

```bash
export CARGO_TARGET_DIR="/tmp/cargo-target"
```

---

## 4. Standard Workflow Commands

### Initializing a Project
```bash
cd /data/src/<project-name>
cargo init --bin .   # Or --lib for library crates
```

### Dependency Resolution & Offline Checks
```bash
cargo fetch
```

### Testing
Always compile and run all unit, integration, and doc tests:
```bash
cargo test --all-targets
```

### Building
```bash
# Debug build
cargo build

# Release build
cargo build --release
```

---

## 5. Single-File Compilation & Testing (Without Cargo)

For single standalone `.rs` files containing embedded unit tests:

```bash
# 1. Compile and execute binary
rustc main.rs -o /tmp/app-bin
/tmp/app-bin

# 2. Compile and run test runner harness
rustc --test main.rs -o /tmp/app-test
/tmp/app-test
```

---

## 6. Known Gotchas & Troubleshooting

| Issue / Error | Root Cause | Remediation |
| :--- | :--- | :--- |
| `cd: /workspace: No such file or directory` | Standard container mounts use `/data`, not `/workspace`. | Target directories inside `/data/src/...`. |
| `rustup could not choose a version of cargo to run...` | Toolchain installed as `root` without default set in user `admin` rustup settings. | Run `rustup default stable`. |
| `cargo: command not found` | `/usr/local/cargo/bin` is not in `$PATH`. | Run `. "/usr/local/cargo/env"`. |
| Excessive host sync lag / high disk usage | Heavy `target/` compilation caches synced to host via `/data`. | Set `export CARGO_TARGET_DIR="/tmp/cargo-target"`. |


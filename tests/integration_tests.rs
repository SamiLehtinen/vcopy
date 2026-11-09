mod common;
use common::TestContext;
use std::fs::{self, File};
use std::io::Write;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn test_basic_backup_and_versioning() {
    let ctx = TestContext::new("basic");
    ctx.write_src_file("test.txt", b"version 1\n");

    // Run auto backup
    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(status.success());
    assert!(ctx.dst.join("test.txt").exists());
    assert_eq!(ctx.read_dst_file("test.txt").unwrap(), b"version 1\n");

    // Modify file
    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("test.txt", b"version 2\n");

    let status = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(status.success());

    // Verify history directory exists and contains version 1
    let hist = ctx.dst.join(".version-history");
    assert!(hist.is_dir());
    let snapshots: Vec<_> = fs::read_dir(&hist)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    assert_eq!(snapshots.len(), 1);

    let hist_file = snapshots[0].path().join("test.txt");
    assert!(hist_file.exists());
    let content = fs::read_to_string(hist_file).unwrap();
    assert_eq!(content, "version 1\n");

    // Verify live mirror contains version 2
    assert_eq!(ctx.read_dst_file("test.txt").unwrap(), b"version 2\n");
}

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
    thread::sleep(Duration::from_millis(1050));
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
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_multisrc_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
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
fn test_multiple_sources_with_flatten() {
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_flatten_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("dir1");
    let src2 = base.join("dir2");
    let dst = base.join("dest");

    fs::create_dir_all(&src1).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    fs::write(src1.join("doc1.txt"), b"doc 1").unwrap();
    fs::write(src2.join("doc2.txt"), b"doc 2").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    assert!(dst.join("doc1.txt").is_file());
    assert!(dst.join("doc2.txt").is_file());

    let _ = fs::remove_dir_all(&base);
}

#[test]
fn test_files_from() {
    let ctx = TestContext::new("files_from");
    ctx.write_src_file("a.txt", b"file a");
    ctx.write_src_file("b.txt", b"file b");

    let list_file = ctx.dir.join("sources.txt");
    let mut f = File::create(&list_file).unwrap();
    writeln!(f, "# Comments should be ignored").unwrap();
    writeln!(f, "{}", ctx.src.join("a.txt").display()).unwrap();
    writeln!(f, "{}", ctx.src.join("b.txt").display()).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--files-from")
        .arg(&list_file)
        .arg("--flatten")
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(ctx.read_dst_file("a.txt").unwrap(), b"file a");
    assert_eq!(ctx.read_dst_file("b.txt").unwrap(), b"file b");
}

#[test]
fn test_concurrency_lock() {
    use fs2::FileExt;

    let ctx = TestContext::new("locking");
    ctx.write_src_file("payload.bin", &[0u8; 1024]);

    // Acquire advisory lock beforehand
    let hist = ctx.dst.join(".version-history");
    fs::create_dir_all(&hist).unwrap();
    let lock_file = File::create(hist.join(".lock")).unwrap();
    lock_file.lock_exclusive().unwrap();

    // Spawning vcopy while lock is held should fail or block
    let output = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .output()
        .unwrap();

    assert!(!output.status.success() || !ctx.dst.join("payload.bin").exists());

    lock_file.unlock().unwrap();
}

#[test]
fn test_no_changes_does_not_create_snapshot() {
    let ctx = TestContext::new("no_changes");
    ctx.write_src_file("steady.txt", b"constant");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Destination created, but no snapshot history needed for unchanged baseline initial copy
    let hist = ctx.dst.join(".version-history");
    let count1 = if hist.is_dir() {
        fs::read_dir(&hist)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .count()
    } else {
        0
    };

    // Second run with no source changes
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    let count2 = if hist.is_dir() {
        fs::read_dir(&hist)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .count()
    } else {
        0
    };

    assert_eq!(count1, count2);
}

#[test]
fn test_dangerous_overlapping_and_root_paths_rejected() {
    let ctx = TestContext::new("path_safety");
    ctx.write_src_file("important.txt", b"do not lose");

    // 1. src == dst must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.src)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert!(ctx.src.join("important.txt").is_file());

    // 2. dst inside src must fail
    let dst_in_src = ctx.src.join("nested_backup");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&dst_in_src)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert!(!dst_in_src.exists());

    // 3. src inside dst must fail and MUST NOT delete or move src files
    let sub_src = ctx.dst.join("sub_source");
    fs::create_dir_all(&sub_src).unwrap();
    fs::write(sub_src.join("precious.txt"), b"keep safe").unwrap();
    fs::write(ctx.dst.join("other.txt"), b"other file").unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&sub_src)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!st.status.success());
    assert_eq!(
        fs::read_to_string(sub_src.join("precious.txt")).unwrap(),
        "keep safe"
    );
    assert_eq!(
        fs::read_to_string(ctx.dst.join("other.txt")).unwrap(),
        "other file"
    );

    // 4. Filesystem root "/" as destination must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg("/")
        .output()
        .unwrap();
    assert!(!st.status.success());

    // 5. Restore with target_out == dst or inside dst must fail
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&ctx.dst)
        .output()
        .unwrap();
    assert!(!st.status.success());

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(ctx.dst.join("restored_inside"))
        .output()
        .unwrap();
    assert!(!st.status.success());
}

#[test]
fn test_file_to_directory_and_directory_to_file_replacement() {
    let ctx = TestContext::new("file_dir_swap");

    // Run 1: "morph" is a regular file
    ctx.write_src_file("morph", b"i am a file");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_file());

    // Run 2: "morph" becomes a directory containing "inner.txt"
    fs::remove_file(ctx.src.join("morph")).unwrap();
    ctx.write_src_file("morph/inner.txt", b"i am inside dir");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_dir());
    assert_eq!(
        ctx.read_dst_file("morph/inner.txt").unwrap(),
        b"i am inside dir"
    );

    // Run 3: "morph" becomes a regular file again
    fs::remove_dir_all(ctx.src.join("morph")).unwrap();
    ctx.write_src_file("morph", b"file again");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());
    assert!(ctx.dst.join("morph").is_file());
    assert_eq!(ctx.read_dst_file("morph").unwrap(), b"file again");
}

#[test]
fn test_manifest_path_traversal_ignored() {
    let ctx = TestContext::new("traversal_guard");
    let victim = ctx.dir.join("outside_victim.txt");
    fs::write(&victim, b"untouchable").unwrap();

    ctx.write_src_file("legit.txt", b"v1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Poison baseline.json with a "../outside_victim.txt" path entry
    let baseline_path = ctx.dst.join(".version-history").join("baseline.json");
    let content = fs::read_to_string(&baseline_path).unwrap();
    let poisoned = content.replace(
        "\"state\": [",
        "\"state\": [\n    {\n      \"path\": \"../outside_victim.txt\",\n      \"size\": 11,\n      \"mtime_ns\": 1000\n    },",
    );
    fs::write(&baseline_path, poisoned).unwrap();

    // Run sync again: must NOT delete or move ../outside_victim.txt
    ctx.write_src_file("legit.txt", b"v2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    assert!(victim.is_file());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "untouchable");
}

#[test]
fn test_flatten_collisions_versioning_and_restore() {
    let base = std::env::temp_dir().join(format!(
        "vcopy_test_flat_full_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    let _ = fs::remove_dir_all(&base);

    let src1 = base.join("dir1");
    let src2 = base.join("dir2");
    let dst = base.join("dst");
    let restore_out = base.join("restored");

    fs::create_dir_all(src1.join("sub")).unwrap();
    fs::create_dir_all(&src2).unwrap();
    fs::create_dir_all(&dst).unwrap();

    // Create overlapping filenames across directories
    fs::write(src1.join("sub").join("notes.txt"), b"dir1 sub notes v1").unwrap();
    fs::write(src2.join("notes.txt"), b"dir2 notes v1").unwrap();
    fs::write(src1.join("unique.txt"), b"unique file v1").unwrap();

    // 1. Initial backup with --flatten
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("--flatten")
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Both files must exist in flat dst without collision!
    assert!(dst.join("notes.txt").is_file());
    assert!(dst.join("notes_2.txt").is_file());
    assert!(dst.join("unique.txt").is_file());
    assert!(dst.join(".vcopy.json").is_file());
    assert!(!dst.join(".version-history").exists());

    std::thread::sleep(std::time::Duration::from_millis(1100));

    // 2. Modify notes in dir1 and delete unique.txt in dir1
    fs::write(src1.join("sub").join("notes.txt"), b"dir1 sub notes v2").unwrap();
    fs::remove_file(src1.join("unique.txt")).unwrap();

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&src1)
        .arg(&src2)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Modified version should be archived as .v1 file in the flat root
    let notes_v1_exists =
        dst.join("notes.v1.txt").is_file() || dst.join("notes_2.v1.txt").is_file();
    assert!(notes_v1_exists);
    assert!(dst.join("unique.v1.txt").is_file());
    assert!(!dst.join("unique.txt").exists());

    // 3. Restore latest: should reconstruct original directory tree
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read(restore_out.join("dir1").join("sub").join("notes.txt")).unwrap(),
        b"dir1 sub notes v2"
    );
    assert_eq!(
        fs::read(restore_out.join("dir2").join("notes.txt")).unwrap(),
        b"dir2 notes v1"
    );
    assert!(!restore_out.join("dir1").join("unique.txt").exists());

    // 4. Restore point-in-time from initial snapshot
    let flat_content = fs::read_to_string(dst.join(".vcopy.json")).unwrap();
    let flat_json: serde_json::Value = serde_json::from_str(&flat_content).unwrap();
    let first_snap_name = flat_json["snapshots"][0]["snapshot_name"].as_str().unwrap();

    let restore_v1_out = base.join("restored_v1");
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(first_snap_name)
        .arg(&dst)
        .arg(&restore_v1_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read(restore_v1_out.join("dir1").join("sub").join("notes.txt")).unwrap(),
        b"dir1 sub notes v1"
    );
    assert_eq!(
        fs::read(restore_v1_out.join("dir2").join("notes.txt")).unwrap(),
        b"dir2 notes v1"
    );
    assert_eq!(
        fs::read(restore_v1_out.join("dir1").join("unique.txt")).unwrap(),
        b"unique file v1"
    );

    let _ = fs::remove_dir_all(&base);
}

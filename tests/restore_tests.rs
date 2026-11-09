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

    // Version 1: 2 files
    ctx.write_src_file("doc.txt", b"version 1");
    ctx.write_src_file("unmodified.txt", b"never changed");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    thread::sleep(Duration::from_millis(1050));

    // Version 2: doc.txt modified, added.txt added, unmodified.txt untouched
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
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert!(
        !snaps.is_empty(),
        "Expected at least one historical snapshot"
    );
    let first_snap = snaps.first().unwrap().clone();

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

    // Unmodified file must be correctly restored
    assert!(restore_out.join("unmodified.txt").is_file());
    assert_eq!(
        fs::read_to_string(restore_out.join("unmodified.txt")).unwrap(),
        "never changed"
    );

    // File added only in snapshot 2 must NOT be in restored snapshot 1
    assert!(!restore_out.join("added.txt").exists());
}

#[test]
fn test_multi_snapshot_restore_with_later_deletions_and_modifications() {
    let ctx = TestContext::new("multi_restore");

    // Version 1: a="a1", b="b1", del="del1"
    ctx.write_src_file("a.txt", b"a1");
    ctx.write_src_file("b.txt", b"b1");
    ctx.write_src_file("del.txt", b"del1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Version 2: a="a2", b untouched ("b1"), del untouched ("del1"), c="c2" added
    ctx.write_src_file("a.txt", b"a2");
    ctx.write_src_file("c.txt", b"c2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    // Version 3: b modified to "b3", del.txt deleted!
    ctx.write_src_file("b.txt", b"b3");
    fs::remove_file(ctx.src.join("del.txt")).unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("create")
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap()
        .success());

    let hist_dir = ctx.dst.join(".version-history");
    let mut snaps: Vec<_> = fs::read_dir(&hist_dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.len() == 17 && n.contains('_'))
        .collect();
    snaps.sort();
    assert_eq!(snaps.len(), 2); // snaps[0] = v1, snaps[1] = v2, live = v3

    // Restore snaps[0] (v1): even though b.txt and del.txt were only modified/deleted in v3 (snaps[1]),
    // restoring snaps[0] must find them in snaps[1]!
    let out_v1 = ctx.dir.join("out_v1");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snaps[0])
        .arg(&ctx.dst)
        .arg(&out_v1)
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::read_to_string(out_v1.join("a.txt")).unwrap(), "a1");
    assert_eq!(fs::read_to_string(out_v1.join("b.txt")).unwrap(), "b1");
    assert_eq!(fs::read_to_string(out_v1.join("del.txt")).unwrap(), "del1");
    assert!(!out_v1.join("c.txt").exists());

    // Restore snaps[1] (v2): a="a2", b="b1", del="del1", c="c2"
    let out_v2 = ctx.dir.join("out_v2");
    assert!(Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg("--target-time")
        .arg(&snaps[1])
        .arg(&ctx.dst)
        .arg(&out_v2)
        .status()
        .unwrap()
        .success());
    assert_eq!(fs::read_to_string(out_v2.join("a.txt")).unwrap(), "a2");
    assert_eq!(fs::read_to_string(out_v2.join("b.txt")).unwrap(), "b1");
    assert_eq!(fs::read_to_string(out_v2.join("del.txt")).unwrap(), "del1");
    assert_eq!(fs::read_to_string(out_v2.join("c.txt")).unwrap(), "c2");
}

#[test]
fn test_restore_latest() {
    let ctx = TestContext::new("restore_latest");
    let restore_out = ctx.dir.join("restored_latest");

    ctx.write_src_file("hello.txt", b"hello world");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    thread::sleep(Duration::from_millis(1050));
    ctx.write_src_file("hello.txt", b"hello world v2");

    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg(&ctx.src)
        .arg(&ctx.dst)
        .status()
        .unwrap();
    assert!(st.success());

    // Restore latest
    let st = Command::new(env!("CARGO_BIN_EXE_vcopy"))
        .arg("restore")
        .arg(&ctx.dst)
        .arg(&restore_out)
        .status()
        .unwrap();
    assert!(st.success());

    assert_eq!(
        fs::read_to_string(restore_out.join("hello.txt")).unwrap(),
        "hello world v2"
    );
}

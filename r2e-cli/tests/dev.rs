use r2e_cli::commands::dev::{purge_corrupt_fat_archives, resolve_target_dir};
use serial_test::serial;
use std::fs;

#[test]
fn purge_removes_empty_and_bad_archives_keeps_valid_ones() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("aarch64-apple-darwin/desktop-dev");
    fs::create_dir_all(&profile).unwrap();

    let empty = profile.join("libdeps-2241ac6f.a");
    fs::write(&empty, b"").unwrap();
    let truncated = profile.join("libdeps-aaaa.a");
    fs::write(&truncated, b"!<ar").unwrap();
    let bad_magic = profile.join("libdeps-bbbb.a");
    fs::write(&bad_magic, b"not an archive!!").unwrap();
    let valid = profile.join("libdeps-cccc.a");
    fs::write(&valid, b"!<arch>\nsome archive contents").unwrap();
    let thin = profile.join("libdeps-dddd.a");
    fs::write(&thin, b"!<thin>\nsome archive contents").unwrap();
    let unrelated = profile.join("libother.a");
    fs::write(&unrelated, b"").unwrap();

    let mut purged = purge_corrupt_fat_archives(temp.path());
    purged.sort();

    assert_eq!(purged, vec![empty.clone(), truncated.clone(), bad_magic.clone()]);
    assert!(!empty.exists());
    assert!(!truncated.exists());
    assert!(!bad_magic.exists());
    assert!(valid.exists());
    assert!(thin.exists());
    assert!(unrelated.exists());
}

#[test]
fn purge_covers_profile_dirs_without_target_triple() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("desktop-dev");
    fs::create_dir_all(&profile).unwrap();

    let empty = profile.join("libdeps-1234.a");
    fs::write(&empty, b"").unwrap();

    assert_eq!(purge_corrupt_fat_archives(temp.path()), vec![empty.clone()]);
    assert!(!empty.exists());
}

#[test]
fn purge_on_missing_target_dir_is_a_no_op() {
    let temp = tempfile::tempdir().unwrap();
    assert!(purge_corrupt_fat_archives(&temp.path().join("does-not-exist")).is_empty());
}

#[test]
#[serial]
fn resolve_target_dir_honors_cargo_target_dir_env() {
    let temp = tempfile::tempdir().unwrap();

    std::env::set_var("CARGO_TARGET_DIR", "/tmp/custom-target");
    assert_eq!(
        resolve_target_dir(temp.path()),
        std::path::PathBuf::from("/tmp/custom-target")
    );

    std::env::set_var("CARGO_TARGET_DIR", "relative-target");
    assert_eq!(
        resolve_target_dir(temp.path()),
        temp.path().join("relative-target")
    );
    std::env::remove_var("CARGO_TARGET_DIR");
}

#[test]
#[serial]
fn resolve_target_dir_falls_back_to_project_target() {
    let temp = tempfile::tempdir().unwrap();
    std::env::remove_var("CARGO_TARGET_DIR");
    // No Cargo.toml in the temp dir, so `cargo metadata` fails and the
    // resolver falls back to `<root>/target`.
    assert_eq!(resolve_target_dir(temp.path()), temp.path().join("target"));
}

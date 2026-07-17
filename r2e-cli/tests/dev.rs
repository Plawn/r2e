use r2e_cli::commands::dev::{
    purge_corrupt_fat_archives, purge_corrupt_fat_archives_with_min_age, resolve_target_dir,
};
use serial_test::serial;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Purge with the mid-write grace period disabled — test files are always
/// freshly written.
fn purge_now(target_dir: &Path) -> Vec<PathBuf> {
    purge_corrupt_fat_archives_with_min_age(target_dir, Duration::ZERO)
}

/// A minimal valid ar archive: global magic + one well-formed member.
fn valid_archive(name: &str, content: &[u8]) -> Vec<u8> {
    let mut bytes = b"!<arch>\n".to_vec();
    bytes.extend_from_slice(
        format!(
            "{name:<16}0           0     0     100644  {size:<10}`\n",
            size = content.len()
        )
        .as_bytes(),
    );
    bytes.extend_from_slice(content);
    if content.len() % 2 == 1 {
        bytes.push(b'\n');
    }
    bytes
}

#[test]
fn purge_removes_empty_and_bad_archives_keeps_valid_ones() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("aarch64-apple-darwin/desktop-dev");
    fs::create_dir_all(&profile).unwrap();

    let empty = profile.join("libdeps-2241ac6f.a");
    fs::write(&empty, b"").unwrap();
    let truncated_magic = profile.join("libdeps-aaaa.a");
    fs::write(&truncated_magic, b"!<ar").unwrap();
    let bad_magic = profile.join("libdeps-bbbb.a");
    fs::write(&bad_magic, b"not an archive!!").unwrap();
    // Valid magic but the member data is cut short: a writer killed mid-member.
    let truncated_member = profile.join("libdeps-eeee.a");
    let mut cut = valid_archive("member.o", b"0123456789abcdef");
    cut.truncate(cut.len() - 5);
    fs::write(&truncated_member, &cut).unwrap();
    let valid = profile.join("libdeps-cccc.a");
    fs::write(&valid, valid_archive("member.o", b"0123456789abcdef")).unwrap();
    let thin = profile.join("libdeps-dddd.a");
    fs::write(&thin, b"!<thin>\nsome archive contents").unwrap();
    let unrelated = profile.join("libother.a");
    fs::write(&unrelated, b"").unwrap();

    let mut purged = purge_now(temp.path());
    purged.sort();

    assert_eq!(
        purged,
        vec![
            empty.clone(),
            truncated_magic.clone(),
            bad_magic.clone(),
            truncated_member.clone()
        ]
    );
    assert!(!empty.exists());
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

    assert_eq!(purge_now(temp.path()), vec![empty.clone()]);
    assert!(!empty.exists());
}

#[test]
fn purge_skips_cargo_internal_dirs() {
    let temp = tempfile::tempdir().unwrap();
    let deps = temp.path().join("debug/deps");
    fs::create_dir_all(&deps).unwrap();

    // Never legitimately corrupt-purged: `deps/` is cargo's own output dir,
    // not a dx profile root.
    let inside_deps = deps.join("libdeps-1234.a");
    fs::write(&inside_deps, b"").unwrap();

    assert!(purge_now(temp.path()).is_empty());
    assert!(inside_deps.exists());
}

#[test]
fn purge_leaves_fresh_archives_for_concurrent_writers() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("desktop-dev");
    fs::create_dir_all(&profile).unwrap();

    // Freshly written (mtime ≈ now): could be another session mid-write, so
    // the default entry point must not touch it.
    let fresh = profile.join("libdeps-1234.a");
    fs::write(&fresh, b"").unwrap();

    assert!(purge_corrupt_fat_archives(temp.path()).is_empty());
    assert!(fresh.exists());
}

#[test]
fn purge_on_missing_target_dir_is_a_no_op() {
    let temp = tempfile::tempdir().unwrap();
    assert!(purge_now(&temp.path().join("does-not-exist")).is_empty());
}

/// Restores any pre-existing `CARGO_TARGET_DIR` on drop so env-mutating tests
/// cannot leak into each other or drop a developer's setting.
struct EnvGuard(Option<std::ffi::OsString>);

impl EnvGuard {
    fn capture() -> Self {
        Self(std::env::var_os("CARGO_TARGET_DIR"))
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.0 {
            Some(value) => std::env::set_var("CARGO_TARGET_DIR", value),
            None => std::env::remove_var("CARGO_TARGET_DIR"),
        }
    }
}

#[test]
#[serial]
fn resolve_target_dir_honors_cargo_target_dir_env() {
    let _guard = EnvGuard::capture();
    let temp = tempfile::tempdir().unwrap();

    std::env::set_var("CARGO_TARGET_DIR", "/tmp/custom-target");
    assert_eq!(
        resolve_target_dir(temp.path()),
        PathBuf::from("/tmp/custom-target")
    );

    std::env::set_var("CARGO_TARGET_DIR", "relative-target");
    assert_eq!(
        resolve_target_dir(temp.path()),
        temp.path().join("relative-target")
    );
}

#[test]
#[serial]
fn resolve_target_dir_falls_back_to_project_target() {
    let _guard = EnvGuard::capture();
    let temp = tempfile::tempdir().unwrap();
    std::env::remove_var("CARGO_TARGET_DIR");
    // An invalid manifest makes `cargo metadata` fail deterministically (a
    // bare dir would let cargo walk up to an ancestor workspace), so the
    // resolver falls back to `<root>/target`.
    fs::write(temp.path().join("Cargo.toml"), "not a manifest").unwrap();
    assert_eq!(resolve_target_dir(temp.path()), temp.path().join("target"));
}

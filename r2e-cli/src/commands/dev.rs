use colored::Colorize;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::thread;
use std::time::{Duration, SystemTime};

const COLD_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Files whose values can survive a hot-patch and therefore cannot safely
/// change layout in place. Changes restart the complete `dx` child process.
const COLD_ROOTS: &[&str] = &["Cargo.toml", "build.rs", "src/env.rs", "src/env"];

pub fn run(
    port: Option<u16>,
    extra_features: Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    ensure_dx_installed()?;
    ensure_dioxus_config()?;

    let features = merged_features(extra_features);
    let project_root = std::env::current_dir()?;
    let mut cold_snapshot = ColdSnapshot::capture(&project_root)?;

    println!(
        "{}",
        "Starting R2E dev server with Subsecond hot-reload..."
            .blue()
            .bold()
    );
    println!(
        "{} Handler, service, and App::build edits are hot-patched",
        "->".blue()
    );
    println!(
        "{} env.rs, Cargo.toml, and build.rs edits trigger a safe full restart",
        "->".blue()
    );
    println!("{} Press {} to stop", "->".blue(), "Ctrl+C".yellow());
    println!();

    loop {
        // Re-resolved on every spawn: the cold restart below fires precisely
        // on Cargo.toml edits, which can move the target directory (and a
        // transient `cargo metadata` failure must not stick for the session).
        let target_dir = resolve_target_dir(&project_root);
        for archive in purge_corrupt_fat_archives(&target_dir) {
            println!(
                "{} removing corrupted fat-binary archive {}",
                "!".yellow(),
                archive.display()
            );
        }

        let mut child = spawn_dx(port, &features)?;

        loop {
            if let Some(status) = child.try_wait()? {
                if status.success() {
                    return Ok(());
                }
                return Err(
                    format!("dx serve exited with code {}", status.code().unwrap_or(-1)).into(),
                );
            }

            thread::sleep(COLD_POLL_INTERVAL);
            let next_snapshot = ColdSnapshot::capture(&project_root)?;
            if next_snapshot != cold_snapshot {
                println!();
                println!(
                    "{} Cold application boundary changed; restarting the process and App::Env...",
                    "->".yellow().bold()
                );
                stop_child(&mut child)?;
                cold_snapshot = next_snapshot;
                break;
            }
        }
    }
}

fn merged_features(extra_features: Vec<String>) -> String {
    let mut features = vec!["dev-reload".to_string()];
    for feature in extra_features {
        if !features.contains(&feature) {
            features.push(feature);
        }
    }
    features.join(",")
}

fn spawn_dx(port: Option<u16>, features: &str) -> Result<Child, Box<dyn std::error::Error>> {
    let mut cmd = Command::new("dx");
    cmd.args(["serve", "--hot-patch", "--features", features]);

    if let Some(port) = port {
        cmd.env("R2E_PORT", port.to_string());
    }

    Ok(cmd.spawn()?)
}

/// Magic bytes opening a valid `ar` archive (regular and thin variants).
const AR_MAGIC: &[u8] = b"!<arch>\n";
const AR_THIN_MAGIC: &[u8] = b"!<thin>\n";

/// Cargo-internal subdirectories of a profile dir that never hold dx
/// fat-binary archives; skipping them keeps the scan cheap on large shared
/// target directories (`deps/` alone can hold tens of thousands of entries).
const CARGO_INTERNAL_DIRS: &[&str] = &[
    ".fingerprint",
    "build",
    "deps",
    "doc",
    "examples",
    "incremental",
];

/// A corrupt archive younger than this is left alone: it may be mid-write by
/// a concurrent dx session sharing the target dir (e.g. a global
/// `~/.cargo/target`). A crashed writer's leftover stops aging, so it is
/// purged on the next spawn once past this threshold.
const MIN_PURGE_AGE: Duration = Duration::from_secs(2);

/// Remove empty/truncated `libdeps-*.a` fat-binary archives from the dx
/// profile directories under `target_dir`, returning the purged paths.
///
/// dx writes these archives in place and its cache accepts any existing file
/// as a hit, so a process killed mid-write (e.g. by the cold-boundary restart
/// above) leaves a zero-byte archive that fails every subsequent link with
/// `ld: file is empty` until removed by hand.
pub fn purge_corrupt_fat_archives(target_dir: &Path) -> Vec<PathBuf> {
    purge_corrupt_fat_archives_with_min_age(target_dir, MIN_PURGE_AGE)
}

/// Test seam for [`purge_corrupt_fat_archives`] — `min_age` overrides the
/// mid-write grace period.
#[doc(hidden)]
pub fn purge_corrupt_fat_archives_with_min_age(
    target_dir: &Path,
    min_age: Duration,
) -> Vec<PathBuf> {
    // Archives live at `<target>/<triple>/<profile>/libdeps-<hash>.a`;
    // depth 2 also covers layouts without the triple component.
    let mut purged = Vec::new();
    scan_for_corrupt_archives(target_dir, 2, min_age, &mut purged);
    purged
}

fn scan_for_corrupt_archives(
    dir: &Path,
    depth: usize,
    min_age: Duration,
    purged: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `fs::metadata` (not `DirEntry::file_type`) so symlinked triple or
        // profile directories are scanned too.
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            if depth > 0 && !is_cargo_internal_dir(&path) {
                scan_for_corrupt_archives(&path, depth - 1, min_age, purged);
            }
        } else if is_fat_archive_name(&path)
            && is_corrupt_archive(&path)
            && is_older_than(&metadata, min_age)
            && fs::remove_file(&path).is_ok()
        {
            purged.push(path);
        }
    }
}

fn is_cargo_internal_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| CARGO_INTERNAL_DIRS.contains(&name))
}

fn is_fat_archive_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("libdeps-") && name.ends_with(".a"))
}

fn is_older_than(metadata: &fs::Metadata, min_age: Duration) -> bool {
    metadata
        .modified()
        .ok()
        .and_then(|mtime| SystemTime::now().duration_since(mtime).ok())
        // No usable mtime → assume old, so the core fix still applies.
        .is_none_or(|age| age >= min_age)
}

fn is_corrupt_archive(path: &Path) -> bool {
    let Ok(mut file) = fs::File::open(path) else {
        return true;
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return true;
    };
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_err() {
        // Empty or shorter than the ar magic.
        return true;
    }
    if magic == AR_THIN_MAGIC {
        // Thin archives reference member data externally; the size-field walk
        // below does not apply.
        return false;
    }
    if magic != AR_MAGIC {
        return true;
    }
    !ar_members_reach_eof(&mut file, len)
}

/// Walk the ar member headers: 60 bytes each with an ASCII decimal size field
/// at offset 48..58, members padded to 2-byte alignment. A file truncated
/// after its magic (writer killed mid-member) fails the walk even though the
/// magic itself is intact.
fn ar_members_reach_eof(file: &mut fs::File, len: u64) -> bool {
    use std::io::{Seek, SeekFrom};

    let mut header = [0u8; 60];
    let mut pos: u64 = 8;
    while pos < len {
        if len - pos < 60
            || file.seek(SeekFrom::Start(pos)).is_err()
            || file.read_exact(&mut header).is_err()
        {
            return false;
        }
        let Some(size) = std::str::from_utf8(&header[48..58])
            .ok()
            .and_then(|field| field.trim().parse::<u64>().ok())
        else {
            return false;
        };
        pos += 60 + size + (size & 1);
    }
    // Some writers omit the final odd-size padding byte, hence `len + 1`.
    pos == len || pos == len + 1
}

/// Resolve the cargo target directory: `CARGO_TARGET_DIR` when set, else
/// `cargo metadata`'s `target_directory` (the only source that sees
/// `.cargo/config.toml` overrides), else `<project_root>/target`.
pub fn resolve_target_dir(project_root: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        // `join` keeps an absolute `dir` as-is and anchors a relative one.
        return project_root.join(dir);
    }
    cargo_metadata_target_dir(project_root).unwrap_or_else(|| project_root.join("target"))
}

fn cargo_metadata_target_dir(project_root: &Path) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    Some(PathBuf::from(metadata.get("target_directory")?.as_str()?))
}

fn stop_child(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    // Give dx a chance to tear down the application process it spawned. A
    // direct `Child::kill` is SIGKILL on Unix and can orphan the live server.
    #[cfg(unix)]
    {
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()?;
        if !status.success() && child.try_wait()?.is_none() {
            child.kill()?;
        }
    }

    #[cfg(not(unix))]
    child.kill()?;

    for _ in 0..30 {
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }

    child.kill()?;
    let _ = child.wait()?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ColdSnapshot(Vec<ColdEntry>);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ColdEntry {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: Option<u64>,
}

impl ColdSnapshot {
    fn capture(root: &Path) -> std::io::Result<Self> {
        let mut entries = Vec::new();
        for relative in COLD_ROOTS {
            let path = root.join(relative);
            capture_path(root, &path, &mut entries)?;
        }
        entries.sort();
        entries.dedup();
        Ok(Self(entries))
    }
}

fn capture_path(root: &Path, path: &Path, entries: &mut Vec<ColdEntry>) -> std::io::Result<()> {
    let relative = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    match fs::metadata(path) {
        Ok(metadata) => {
            entries.push(ColdEntry {
                path: relative,
                modified: metadata.modified().ok(),
                len: metadata.is_file().then_some(metadata.len()),
            });

            if metadata.is_dir() {
                let mut children = fs::read_dir(path)?.collect::<Result<Vec<_>, _>>()?;
                children.sort_by_key(|entry| entry.path());
                for child in children {
                    capture_path(root, &child.path(), entries)?;
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            entries.push(ColdEntry {
                path: relative,
                modified: None,
                len: None,
            });
        }
        Err(error) => return Err(error),
    }
    Ok(())
}

fn ensure_dx_installed() -> Result<(), Box<dyn std::error::Error>> {
    match Command::new("dx").arg("--version").output() {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            println!("{} dioxus-cli found: {}", "ok".green(), version.trim());
            Ok(())
        }
        _ => {
            eprintln!(
                "{} dioxus-cli (dx) is not installed. Install it with:",
                "!".yellow()
            );
            eprintln!("  cargo install dioxus-cli");
            eprintln!();
            eprintln!("Or via the install script:");
            eprintln!("  curl -sSL https://dioxus.dev/install.sh | bash");
            Err("dioxus-cli not found".into())
        }
    }
}

fn ensure_dioxus_config() -> Result<(), Box<dyn std::error::Error>> {
    let config_path = "Dioxus.toml";
    if !Path::new(config_path).exists() {
        let project_name = fs::read_to_string("Cargo.toml")
            .ok()
            .and_then(|content| {
                content
                    .parse::<toml_edit::DocumentMut>()
                    .ok()
                    .and_then(|doc| {
                        doc.get("package")
                            .and_then(|p| p.get("name"))
                            .and_then(|n| n.as_str().map(String::from))
                    })
            })
            .unwrap_or_else(|| "r2e-app".to_string());

        println!(
            "{} Creating minimal Dioxus.toml for hot-reload...",
            "->".blue()
        );
        fs::write(
            config_path,
            format!(
                r#"[application]
name = "{project_name}"

[application.tools]
"#,
            ),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn features_always_include_dev_reload_once() {
        assert_eq!(
            merged_features(vec!["openapi".into(), "dev-reload".into()]),
            "dev-reload,openapi"
        );
    }

    #[test]
    fn snapshot_detects_cold_file_changes_and_creation() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("Cargo.toml"), "[package]\n").unwrap();

        let before = ColdSnapshot::capture(temp.path()).unwrap();
        fs::write(temp.path().join("src/env.rs"), "pub struct AppEnv;\n").unwrap();
        let after_create = ColdSnapshot::capture(temp.path()).unwrap();
        assert_ne!(before, after_create);

        fs::write(
            temp.path().join("src/env.rs"),
            "pub struct AppEnv { pub value: usize }\n",
        )
        .unwrap();
        let after_edit = ColdSnapshot::capture(temp.path()).unwrap();
        assert_ne!(after_create, after_edit);
    }
}

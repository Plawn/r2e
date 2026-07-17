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

    let target_dir = resolve_target_dir(&project_root);

    loop {
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
const AR_MAGICS: [&[u8]; 2] = [b"!<arch>\n", b"!<thin>\n"];

/// Remove empty/truncated `libdeps-*.a` fat-binary archives from the dx
/// profile directories under `target_dir`, returning the purged paths.
///
/// dx writes these archives in place and its cache accepts any existing file
/// as a hit, so a process killed mid-write (e.g. by the cold-boundary restart
/// above) leaves a zero-byte archive that fails every subsequent link with
/// `ld: file is empty` until removed by hand.
pub fn purge_corrupt_fat_archives(target_dir: &Path) -> Vec<PathBuf> {
    // Archives live at `<target>/<triple>/<profile>/libdeps-<hash>.a`;
    // depth 2 also covers layouts without the triple component.
    let mut purged = Vec::new();
    scan_for_corrupt_archives(target_dir, 2, &mut purged);
    purged
}

fn scan_for_corrupt_archives(dir: &Path, depth: usize, purged: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if depth > 0 {
                scan_for_corrupt_archives(&path, depth - 1, purged);
            }
        } else if is_fat_archive_name(&path)
            && is_corrupt_archive(&path)
            && fs::remove_file(&path).is_ok()
        {
            purged.push(path);
        }
    }
}

fn is_fat_archive_name(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "a")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("libdeps-"))
}

fn is_corrupt_archive(path: &Path) -> bool {
    let mut magic = [0u8; 8];
    match fs::File::open(path).and_then(|mut file| file.read_exact(&mut magic)) {
        Ok(()) => !AR_MAGICS.contains(&magic.as_slice()),
        // Empty or shorter than the ar header.
        Err(_) => true,
    }
}

/// Resolve the cargo target directory: `CARGO_TARGET_DIR` when set, else
/// `cargo metadata`'s `target_directory` (the only source that sees
/// `.cargo/config.toml` overrides), else `<project_root>/target`.
pub fn resolve_target_dir(project_root: &Path) -> PathBuf {
    if let Some(dir) = std::env::var_os("CARGO_TARGET_DIR") {
        let dir = PathBuf::from(dir);
        return if dir.is_absolute() {
            dir
        } else {
            project_root.join(dir)
        };
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
    let json = String::from_utf8(output.stdout).ok()?;
    let key = "\"target_directory\":\"";
    let start = json.find(key)? + key.len();
    // Manual JSON string decode (no serde_json dependency): only `\"` and
    // `\\` escapes matter — cargo emits non-ASCII path bytes unescaped.
    let mut value = String::new();
    let mut chars = json[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(PathBuf::from(value)),
            '\\' => value.push(chars.next()?),
            _ => value.push(c),
        }
    }
    None
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

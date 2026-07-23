//! Loading from files: `load_from` / `load_profiled_from`.

use r2e_core::config::{ConfigError, R2eConfig};

// =========================================================================
// Custom base config file: load_from / load_profiled_from (task #446)
// =========================================================================

fn write_file(dir: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    path
}

#[test]
fn load_from_reads_custom_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "app:\n  name: patina\n  port: 9000\n",
    );

    let config = R2eConfig::load_from(&file).unwrap();
    assert_eq!(config.get::<String>("app.name").unwrap(), "patina");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 9000);
}

#[test]
fn load_from_missing_file_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = R2eConfig::load_from(dir.path().join("nope.yaml")).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg) if msg.contains("nope.yaml")),
        "expected Load error naming the file, got: {err}"
    );
}

#[test]
fn load_profiled_from_overlays_derived_profile_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "app:\n  name: patina\n  port: 9000\n",
    );
    // The overlay file name derives from the base name, not `application-*`.
    write_file(dir.path(), "patina-test.yaml", "app:\n  port: 1234\n");

    let config = R2eConfig::load_profiled_from(&file, Some("test")).unwrap();
    assert_eq!(config.get::<String>("app.name").unwrap(), "patina");
    assert_eq!(config.get::<i64>("app.port").unwrap(), 1234);
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}

#[test]
fn load_profiled_from_reads_profile_from_base_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "r2e:\n  profile: staging\napp:\n  port: 9000\n",
    );
    write_file(dir.path(), "patina-staging.yaml", "app:\n  port: 4321\n");

    // Skip when R2E_PROFILE is set in the environment: it wins over the
    // r2e.profile key and would overlay a different (absent) sibling.
    if std::env::var("R2E_PROFILE").is_err() {
        let config = R2eConfig::load_profiled_from(&file, None).unwrap();
        assert_eq!(config.get::<i64>("app.port").unwrap(), 4321);
    }
}

#[test]
fn load_profiled_from_tolerates_missing_profile_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(dir.path(), "patina.yaml", "app:\n  port: 9000\n");

    let config = R2eConfig::load_profiled_from(&file, Some("test")).unwrap();
    assert_eq!(config.get::<i64>("app.port").unwrap(), 9000);
    assert_eq!(config.get::<String>("r2e.profile").unwrap(), "test");
}

#[test]
fn load_from_resolves_secret_placeholders() {
    let dir = tempfile::tempdir().unwrap();
    let file = write_file(
        dir.path(),
        "patina.yaml",
        "app:\n  secret: \"${R2E_TEST_UNSET_446:fallback}\"\n",
    );

    let config = R2eConfig::load_from(&file).unwrap();
    assert_eq!(config.get::<String>("app.secret").unwrap(), "fallback");
}

#[test]
fn load_from_directory_path_errors_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let err = R2eConfig::load_from(dir.path()).unwrap_err();
    assert!(
        matches!(&err, ConfigError::Load(msg) if msg.contains("not a regular file")),
        "expected clear not-a-file error, got: {err}"
    );
}

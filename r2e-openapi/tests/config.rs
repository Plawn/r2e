use r2e_openapi::OpenApiConfig;

// ── Phase 5: OpenApiConfig ──────────────────────────────────────────────────

#[test]
fn config_new() {
    let config = OpenApiConfig::new("My API", "1.0.0");
    assert_eq!(config.title, "My API");
    assert_eq!(config.version, "1.0.0");
    assert!(config.description.is_none());
    assert!(!config.docs_ui);
}

#[test]
fn config_with_description() {
    let config = OpenApiConfig::new("My API", "1.0.0")
        .with_description("A great API");
    assert_eq!(config.description.as_deref(), Some("A great API"));
}

#[test]
fn config_with_docs_ui_true() {
    let config = OpenApiConfig::new("My API", "1.0.0")
        .with_docs_ui(true);
    assert!(config.docs_ui);
}

#[test]
fn config_with_docs_ui_false() {
    let config = OpenApiConfig::new("My API", "1.0.0")
        .with_docs_ui(true)
        .with_docs_ui(false);
    assert!(!config.docs_ui);
}

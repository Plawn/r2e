use r2e_core::config::{ConfigProperties, R2eConfig};
use r2e_core::runtime::tracing_config::{JsonLogConfig, LogFormat, SpanEvents, TracingConfig};
use tracing_subscriber::fmt::format::FmtSpan;

#[test]
fn default_has_expected_values() {
    let cfg = TracingConfig::default();
    assert_eq!(cfg.filter, "info");
    assert!(cfg.format.is_none());
    assert!(cfg.target.is_none());
    assert!(cfg.thread_ids.is_none());
    assert!(cfg.thread_names.is_none());
    assert!(cfg.file.is_none());
    assert!(cfg.line_number.is_none());
    assert!(cfg.level.is_none());
    assert!(cfg.ansi.is_none());
    assert!(cfg.span_events.is_none());
}

#[test]
fn effective_defaults() {
    let cfg = TracingConfig::default();
    assert_eq!(cfg.effective_format(), LogFormat::Pretty);
    assert_eq!(cfg.effective_span_events(), FmtSpan::CLOSE);
}

#[test]
fn from_config_parses_yaml() {
    let yaml = r#"
tracing:
  filter: "debug,hyper=warn"
  format: json
  target: false
  thread-ids: true
  thread-names: true
  file: true
  line-number: true
  level: false
  ansi: false
  span-events: full
"#;
    let r2e_config = R2eConfig::from_yaml_str(yaml).unwrap();
    let cfg = TracingConfig::from_config(&r2e_config, Some("tracing")).unwrap();

    assert_eq!(cfg.filter, "debug,hyper=warn");
    assert_eq!(cfg.format, Some(LogFormat::Json));
    assert_eq!(cfg.target, Some(false));
    assert_eq!(cfg.thread_ids, Some(true));
    assert_eq!(cfg.thread_names, Some(true));
    assert_eq!(cfg.file, Some(true));
    assert_eq!(cfg.line_number, Some(true));
    assert_eq!(cfg.level, Some(false));
    assert_eq!(cfg.ansi, Some(false));
    assert_eq!(cfg.span_events, Some(SpanEvents::Full));
}

#[test]
fn from_config_partial_yaml() {
    let yaml = r#"
tracing:
  format: pretty
  ansi: false
"#;
    let r2e_config = R2eConfig::from_yaml_str(yaml).unwrap();
    let cfg = TracingConfig::from_config(&r2e_config, Some("tracing")).unwrap();

    assert_eq!(cfg.format, Some(LogFormat::Pretty));
    assert_eq!(cfg.ansi, Some(false));
    // Unset fields use default
    assert_eq!(cfg.filter, "info");
    assert!(cfg.target.is_none());
}

#[test]
fn log_format_deserializes() {
    let r2e = R2eConfig::from_yaml_str("fmt: json").unwrap();
    let f: LogFormat = r2e.get("fmt").unwrap();
    assert_eq!(f, LogFormat::Json);

    let r2e = R2eConfig::from_yaml_str("fmt: pretty").unwrap();
    let f: LogFormat = r2e.get("fmt").unwrap();
    assert_eq!(f, LogFormat::Pretty);
}

#[test]
fn span_events_deserializes() {
    for (input, expected) in [
        ("none", SpanEvents::None),
        ("new", SpanEvents::New),
        ("close", SpanEvents::Close),
        ("active", SpanEvents::Active),
        ("full", SpanEvents::Full),
    ] {
        let r2e = R2eConfig::from_yaml_str(&format!("ev: {input}")).unwrap();
        let ev: SpanEvents = r2e.get("ev").unwrap();
        assert_eq!(ev, expected, "failed for input: {input}");
    }
}

#[test]
fn span_events_to_fmt_span() {
    assert_eq!(SpanEvents::None.to_fmt_span(), FmtSpan::NONE);
    assert_eq!(SpanEvents::New.to_fmt_span(), FmtSpan::NEW);
    assert_eq!(SpanEvents::Close.to_fmt_span(), FmtSpan::CLOSE);
    assert_eq!(SpanEvents::Active.to_fmt_span(), FmtSpan::ACTIVE);
    assert_eq!(SpanEvents::Full.to_fmt_span(), FmtSpan::FULL);
}

#[test]
fn builder_methods() {
    let cfg = TracingConfig::default()
        .with_format(LogFormat::Json)
        .with_filter("trace")
        .with_target(false)
        .with_thread_ids(true)
        .with_ansi(false)
        .with_span_events(SpanEvents::Full);

    assert_eq!(cfg.format, Some(LogFormat::Json));
    assert_eq!(cfg.filter, "trace");
    assert_eq!(cfg.target, Some(false));
    assert_eq!(cfg.thread_ids, Some(true));
    assert_eq!(cfg.ansi, Some(false));
    assert_eq!(cfg.span_events, Some(SpanEvents::Full));
}

#[test]
fn default_matches_an_empty_section() {
    let cfg = TracingConfig::default();
    assert!(cfg.enabled);
    assert!(cfg.console);
    assert_eq!(cfg.json, JsonLogConfig::default());
    let bound =
        TracingConfig::from_config(&R2eConfig::from_yaml_str("{}").unwrap(), Some("tracing"))
            .unwrap();
    assert_eq!(bound, cfg);
}

#[test]
fn enabled_and_console_bind() {
    let yaml = "tracing:\n  enabled: false\n  console: false\n";
    let r2e_config = R2eConfig::from_yaml_str(yaml).unwrap();
    let cfg = TracingConfig::from_config(&r2e_config, Some("tracing")).unwrap();
    assert!(!cfg.enabled);
    assert!(!cfg.console);
}

#[test]
fn every_field_reaches_tracing_options() {
    let cfg = TracingConfig::default()
        .with_format(LogFormat::Json)
        .with_filter("debug")
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true)
        .with_level(false)
        .with_ansi(false)
        .with_span_events(SpanEvents::Active)
        .with_console(false)
        .with_json({
            let mut json = JsonLogConfig::default();
            json.flatten = true;
            json.message_key = "_msg".into();
            json.current_span = false;
            json.span_list = false;
            json
        });
    let opts = r2e_core::TracingOptions::from(&cfg);
    assert_eq!(
        opts,
        r2e_core::TracingOptions::new("debug")
            .with_format(LogFormat::Json)
            .with_target(false)
            .with_thread_ids(true)
            .with_thread_names(true)
            .with_file(true)
            .with_line_number(true)
            .with_level(false)
            .with_ansi(false)
            .with_span_events(SpanEvents::Active)
            .with_console(false)
            .with_json(
                r2e_core::JsonOptions::default()
                    .with_flatten(true)
                    .with_message_key("_msg")
                    .with_current_span(false)
                    .with_span_list(false)
            )
    );
    assert_eq!(
        r2e_core::TracingOptions::from(&TracingConfig::default()),
        r2e_core::TracingOptions::default()
    );
}

#[test]
fn json_section_binds_as_a_nested_field() {
    let yaml = r#"
tracing:
  format: json
  json:
    flatten: true
    message-key: _msg
    span-list: false
"#;
    let r2e_config = R2eConfig::from_yaml_str(yaml).unwrap();
    let cfg = TracingConfig::from_config(&r2e_config, Some("tracing")).unwrap();
    assert!(cfg.json.flatten);
    assert_eq!(cfg.json.message_key, "_msg");
    assert!(cfg.json.current_span, "unset key keeps its default");
    assert!(!cfg.json.span_list);

    let opts = r2e_core::TracingOptions::from(&cfg);
    assert_eq!(opts.format, Some(LogFormat::Json));
    assert!(opts.json.flatten);
    assert_eq!(opts.json.message_key, "_msg");
    assert!(opts.json.current_span);
    assert!(!opts.json.span_list);
}

#[test]
fn json_section_defaults_to_nested() {
    let r2e_config = R2eConfig::from_yaml_str("tracing:\n  format: json\n").unwrap();
    let cfg = TracingConfig::from_config(&r2e_config, Some("tracing")).unwrap();
    assert_eq!(cfg.json, JsonLogConfig::default());
    assert_eq!(
        r2e_core::TracingOptions::from(&cfg).json,
        r2e_core::JsonOptions::default()
    );
}

use r2e_tracing::tracing_subscriber::fmt::format::FmtSpan;

// The value types live in `r2e-tracing`; the config binding (the
// `FromConfigValue` impls below and `TracingConfig`) stays here because the
// trait and the `ConfigProperties` derive are core's.
pub use r2e_tracing::{JsonOptions, LogFormat, SpanEvents, TracingOptions};

impl crate::config::FromConfigValue for LogFormat {
    fn from_config_value(
        value: &crate::config::ConfigValue,
        key: &str,
    ) -> Result<Self, crate::config::ConfigError> {
        crate::config::deserialize_value::<Self>(value, key)
    }
}

impl crate::config::FromConfigValue for SpanEvents {
    fn from_config_value(
        value: &crate::config::ConfigValue,
        key: &str,
    ) -> Result<Self, crate::config::ConfigError> {
        crate::config::deserialize_value::<Self>(value, key)
    }
}

/// The YAML `tracing:` section: the process-global subscriber
/// `build_state()` installs.
///
/// The subscriber is installed **once**, by
/// [`AppBuilder::build_state`](crate::AppBuilder::build_state), from this
/// section of the loaded configuration (profile overlay and `R2E_*` env
/// overlay included) plus the layers contributed with
/// [`AppBuilder::tracing_layer`](crate::AppBuilder::tracing_layer) and
/// [`PluginSetupContext::tracing_layer`](crate::plugin::PluginSetupContext::tracing_layer).
///
/// `enabled` and `console` are decisions the builder takes; the rest converts
/// into the [`TracingOptions`] `r2e-tracing` builds the stack from
/// (`From<&TracingConfig>`). The `Option` knobs mean "use the subscriber
/// default" when unset.
///
/// Non-exhaustive: build one with [`Default`] and the `with_*` setters.
///
/// # YAML example
///
/// ```yaml
/// tracing:
///   enabled: true          # false: no subscriber at all, boot logs dropped
///   console: true          # false: only the contributed layers write output
///   filter: "info,tower_http=debug,my_app=trace"
///   format: json
///   ansi: false
///   thread-ids: true
///   span-events: full
///   json:
///     flatten: true
///     message-key: _msg
/// ```
#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
#[config(derive_default)]
#[non_exhaustive]
pub struct TracingConfig {
    /// Install R2E's subscriber at all. `false` installs nothing and drops the
    /// logs buffered during boot (the application owns the subscriber).
    #[config(default = true)]
    pub enabled: bool,

    /// Add R2E's console layer (text, JSON or flat JSON on stdout). `false`
    /// leaves the output to the contributed layers — a custom console added
    /// with `.tracing_layer(..)`.
    #[config(default = true)]
    pub console: bool,

    /// `EnvFilter` directive string. `RUST_LOG` env var takes priority.
    #[config(default = "info")]
    pub filter: String,

    /// Log format: `pretty` (default) or `json`.
    pub format: Option<LogFormat>,

    /// Print the target (module path) in each log line.
    pub target: Option<bool>,

    /// Print thread IDs.
    #[config(key = "thread-ids")]
    pub thread_ids: Option<bool>,

    /// Print thread names.
    #[config(key = "thread-names")]
    pub thread_names: Option<bool>,

    /// Print file name where the log originated.
    pub file: Option<bool>,

    /// Print line number where the log originated.
    #[config(key = "line-number")]
    pub line_number: Option<bool>,

    /// Print the log level.
    pub level: Option<bool>,

    /// Enable ANSI color codes in output.
    pub ansi: Option<bool>,

    /// Which span lifecycle events to record.
    #[config(key = "span-events")]
    pub span_events: Option<SpanEvents>,

    /// Shape of the JSON lines (`tracing.json.*`); only read under
    /// `format: json`.
    #[config(section, default)]
    pub json: JsonLogConfig,
}

impl TracingConfig {
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn with_console(mut self, console: bool) -> Self {
        self.console = console;
        self
    }

    pub fn with_json(mut self, json: JsonLogConfig) -> Self {
        self.json = json;
        self
    }

    pub fn with_format(mut self, format: LogFormat) -> Self {
        self.format = Some(format);
        self
    }

    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = filter.into();
        self
    }

    pub fn with_target(mut self, target: bool) -> Self {
        self.target = Some(target);
        self
    }

    pub fn with_thread_ids(mut self, thread_ids: bool) -> Self {
        self.thread_ids = Some(thread_ids);
        self
    }

    pub fn with_thread_names(mut self, thread_names: bool) -> Self {
        self.thread_names = Some(thread_names);
        self
    }

    pub fn with_file(mut self, file: bool) -> Self {
        self.file = Some(file);
        self
    }

    pub fn with_line_number(mut self, line_number: bool) -> Self {
        self.line_number = Some(line_number);
        self
    }

    pub fn with_level(mut self, level: bool) -> Self {
        self.level = Some(level);
        self
    }

    pub fn with_ansi(mut self, ansi: bool) -> Self {
        self.ansi = Some(ansi);
        self
    }

    pub fn with_span_events(mut self, span_events: SpanEvents) -> Self {
        self.span_events = Some(span_events);
        self
    }

    /// Resolve the effective format, defaulting to `Pretty`.
    pub fn effective_format(&self) -> LogFormat {
        self.format.clone().unwrap_or_default()
    }

    /// Resolve the effective `FmtSpan`, defaulting to `CLOSE`.
    pub fn effective_span_events(&self) -> FmtSpan {
        self.span_events
            .as_ref()
            .map(SpanEvents::to_fmt_span)
            .unwrap_or(FmtSpan::CLOSE)
    }
}

/// Shape of the JSON log lines, bound to the YAML `tracing.json` section.
///
/// The YAML-bound twin of [`JsonOptions`]. Only read under `format: json`.
/// With `flatten: true` every event is one flat object — event fields at the
/// top level, the message under `message-key`, the current span under
/// `span` and the span stack (root first) under `spans`; the default keeps
/// `tracing-subscriber`'s nested JSON (`"fields": {...}`) unchanged.
///
/// The [`json`](TracingConfig::json) field of [`TracingConfig`].
///
/// ```yaml
/// tracing:
///   format: json
///   json:
///     flatten: true
///     message-key: _msg     # VictoriaLogs' message field
///     current-span: true
///     span-list: false
/// ```
#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
#[config(derive_default)]
#[non_exhaustive]
pub struct JsonLogConfig {
    /// Write flat lines instead of the nested `fields` shape.
    #[config(default = false)]
    pub flatten: bool,

    /// Key of the event message in a flat line (the target when the event
    /// has no message).
    #[config(default = "message", key = "message-key")]
    pub message_key: String,

    /// Add the current span (`"span": {"name": .., <fields>}`).
    #[config(default = true, key = "current-span")]
    pub current_span: bool,

    /// Add the span stack, root first (`"spans": [..]`).
    #[config(default = true, key = "span-list")]
    pub span_list: bool,
}

impl JsonLogConfig {
    /// The `tracing.json` prefix this section is read under.
    pub const PREFIX: &'static str = "tracing.json";
}

impl From<&JsonLogConfig> for JsonOptions {
    fn from(c: &JsonLogConfig) -> Self {
        JsonOptions::default()
            .with_flatten(c.flatten)
            .with_message_key(c.message_key.clone())
            .with_current_span(c.current_span)
            .with_span_list(c.span_list)
    }
}

/// Every knob of the section except `enabled` (a decision `build_state()`
/// takes before building anything).
impl From<&TracingConfig> for TracingOptions {
    fn from(c: &TracingConfig) -> Self {
        let mut o = TracingOptions::new(c.filter.clone());
        o.format = c.format.clone();
        o.target = c.target;
        o.thread_ids = c.thread_ids;
        o.thread_names = c.thread_names;
        o.file = c.file;
        o.line_number = c.line_number;
        o.level = c.level;
        o.ansi = c.ansi;
        o.span_events = c.span_events.clone();
        o.json = (&c.json).into();
        o.console = c.console;
        o
    }
}

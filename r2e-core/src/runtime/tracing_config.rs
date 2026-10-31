use r2e_tracing::tracing_subscriber::fmt::format::FmtSpan;

// The value types live in `r2e-tracing`; the config binding (the
// `FromConfigValue` impls below and `TracingConfig`) stays here because the
// trait and the `ConfigProperties` derive are core's.
pub use r2e_tracing::{LogFormat, SpanEvents, TracingOptions};

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

/// Configuration for the `tracing-subscriber` fmt layer, bound to the YAML
/// `tracing:` section.
///
/// The YAML-bound twin of [`TracingOptions`] (`r2e-tracing`); the two convert
/// both ways and the install goes through [`r2e_tracing::try_install`].
///
/// All fields except `filter` are `Option` — `None` means "use the subscriber
/// default". This lets users override only the knobs they care about.
///
/// # YAML example
///
/// ```yaml
/// tracing:
///   filter: "info,tower_http=debug,my_app=trace"
///   format: json
///   ansi: false
///   thread-ids: true
///   span-events: full
/// ```
#[derive(r2e_macros::ConfigProperties, Clone, Debug, PartialEq)]
pub struct TracingConfig {
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
}

impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            filter: "info".to_string(),
            format: None,
            target: None,
            thread_ids: None,
            thread_names: None,
            file: None,
            line_number: None,
            level: None,
            ansi: None,
            span_events: None,
        }
    }
}

impl TracingConfig {
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
        o
    }
}

impl From<&TracingOptions> for TracingConfig {
    fn from(o: &TracingOptions) -> Self {
        Self {
            filter: o.filter.clone(),
            format: o.format.clone(),
            target: o.target,
            thread_ids: o.thread_ids,
            thread_names: o.thread_names,
            file: o.file,
            line_number: o.line_number,
            level: o.level,
            ansi: o.ansi,
            span_events: o.span_events.clone(),
        }
    }
}

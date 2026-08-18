use tracing_subscriber::fmt::format::FmtSpan;

/// Log output format.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

impl crate::config::FromConfigValue for LogFormat {
    fn from_config_value(
        value: &crate::config::ConfigValue,
        key: &str,
    ) -> Result<Self, crate::config::ConfigError> {
        crate::config::deserialize_value::<Self>(value, key)
    }
}

/// Controls which span lifecycle events are recorded.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SpanEvents {
    None,
    New,
    #[default]
    Close,
    Active,
    Full,
}

impl SpanEvents {
    /// Convert to the `tracing_subscriber` `FmtSpan` bitflag.
    pub fn to_fmt_span(&self) -> FmtSpan {
        match self {
            SpanEvents::None => FmtSpan::NONE,
            SpanEvents::New => FmtSpan::NEW,
            SpanEvents::Close => FmtSpan::CLOSE,
            SpanEvents::Active => FmtSpan::ACTIVE,
            SpanEvents::Full => FmtSpan::FULL,
        }
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

/// Configuration for the `tracing-subscriber` fmt layer.
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
#[derive(r2e_macros::ConfigProperties, Clone, Debug)]
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

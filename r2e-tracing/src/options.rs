use tracing_subscriber::fmt::format::FmtSpan;

/// Log output format.
#[derive(serde::Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
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

/// The knobs of the subscriber [`try_install`](crate::try_install) builds.
///
/// All fields except `filter` are `Option` — `None` means "use the subscriber
/// default". `filter` is an `EnvFilter` directive string; the `RUST_LOG`
/// environment variable takes priority over it. [`Default`] gives the filter
/// `"info"` and every other knob unset.
///
/// `r2e-core`'s `TracingConfig` is the YAML-bound twin of this struct
/// (`tracing:` section) and converts both ways.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq)]
pub struct TracingOptions {
    /// `EnvFilter` directive string. `RUST_LOG` env var takes priority.
    pub filter: String,
    /// Log format: `pretty` (default) or `json`.
    pub format: Option<LogFormat>,
    /// Print the target (module path) in each log line.
    pub target: Option<bool>,
    /// Print thread IDs.
    pub thread_ids: Option<bool>,
    /// Print thread names.
    pub thread_names: Option<bool>,
    /// Print file name where the log originated.
    pub file: Option<bool>,
    /// Print line number where the log originated.
    pub line_number: Option<bool>,
    /// Print the log level.
    pub level: Option<bool>,
    /// Enable ANSI color codes in output.
    pub ansi: Option<bool>,
    /// Which span lifecycle events to record.
    pub span_events: Option<SpanEvents>,
}

impl Default for TracingOptions {
    fn default() -> Self {
        Self::new("info")
    }
}

impl TracingOptions {
    /// Options with this `EnvFilter` directive and every other knob unset.
    pub fn new(filter: impl Into<String>) -> Self {
        Self {
            filter: filter.into(),
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

//! Helpers shared across the test targets (not a target: no `main.rs`).
//! Include with `#[path = "../support/mod.rs"] mod support;`.
#![allow(dead_code)]

/// One event seen by [`Capture`]: level, target and fields rendered to
/// strings (`message` included).
#[derive(Clone, Debug)]
pub struct Seen {
    pub level: tracing::Level,
    pub target: String,
    pub fields: Vec<(String, String)>,
}

impl Seen {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn message(&self) -> &str {
        self.field("message").unwrap_or_default()
    }
}

/// Records every event that reaches it.
#[derive(Clone, Default)]
pub struct Capture(std::sync::Arc<std::sync::Mutex<Vec<Seen>>>);

impl Capture {
    pub fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }

    pub fn boxed(&self) -> r2e_tracing::BoxedLayer {
        Box::new(self.clone())
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct V(Vec<(String, String)>);
        impl tracing::field::Visit for V {
            fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
                self.0.push((f.name().to_owned(), v.to_owned()));
            }
            fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                self.0.push((f.name().to_owned(), format!("{v:?}")));
            }
        }
        let mut v = V(Vec::new());
        event.record(&mut v);
        let meta = event.metadata();
        self.0.lock().unwrap().push(Seen {
            level: *meta.level(),
            target: meta.target().to_owned(),
            fields: v.0,
        });
    }
}

//! `LogFields` values in JSON lines: control characters are escaped, so a
//! field value cannot break (or inject into) the line.

use tracing::span::{Attributes, Id};
use tracing::Subscriber;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

use r2e_tracing::{LogFields, LogFormat};
use serde_json::Value;

use super::{plain_text, raw};

/// The value under test: quote, backslash, the named escapes, the control
/// range bounds (0x01, 0x1f), DEL and the first non-control char (space).
const TRICKY: &str = "a b\"\\\n\r\t\u{1}\u{1f}\u{7f}é";

/// What `push_json_escaped` must write for [`TRICKY`], byte for byte.
const ESCAPED: &str = r#""a b\"\\\n\r\t\u0001\u001f"#;

struct Tricky;

impl<S> Layer<S> for Tricky
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, _: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = LogFields::new();
        fields.insert("tricky", TRICKY);
        assert!(!fields.is_empty());
        ctx.span(id).unwrap().extensions_mut().insert(fields);
    }
}

#[test]
fn control_chars_in_log_fields_are_unicode_escaped() {
    let opts = plain_text().with_format(LogFormat::Json);
    let out = raw(&opts, vec![Box::new(Tricky)], || {
        let _s = tracing::info_span!("s").entered();
        tracing::info!(target: "fields_test", "hello");
    });
    assert_eq!(out.matches('\n').count(), 1, "one line: {out:?}");
    let expected = format!("\"tricky\":{ESCAPED}\u{7f}é\"");
    assert!(out.contains(&expected), "{out:?} lacks {expected:?}");

    let line: Value = serde_json::from_str(out.trim_end()).expect("valid JSON");
    assert_eq!(line["tricky"], TRICKY);
}

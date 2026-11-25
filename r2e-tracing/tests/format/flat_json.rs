//! Flat JSON: native JSON types for every `Visit` method, and the span keys
//! when only the span list is on.

use r2e_tracing::JsonOptions;

use super::{flat, one};

#[test]
fn unsigned_float_and_error_fields_keep_their_json_type() {
    let err = std::io::Error::other("boom");
    let line = one(&flat(JsonOptions::default()), || {
        tracing::info!(
            target: "flat_test",
            big = u64::MAX,
            ratio = 1.5_f64,
            err = &err as &(dyn std::error::Error + 'static),
            "typed"
        )
    });
    assert_eq!(line["big"], u64::MAX, "{line}");
    assert_eq!(line["ratio"], 1.5, "{line}");
    // `Display`, not `Debug` (`Custom { kind: Other, .. }`).
    assert_eq!(line["err"], "boom", "{line}");
}

#[test]
fn span_list_without_current_span() {
    let line = one(
        &flat(JsonOptions::default().with_current_span(false)),
        || {
            let _outer = tracing::info_span!("run").entered();
            tracing::info!(target: "flat_test", "inside");
        },
    );
    assert!(line.get("span").is_none(), "{line}");
    assert_eq!(line["spans"][0]["name"], "run", "{line}");
}

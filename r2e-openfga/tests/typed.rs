//! Typed marker runtime behavior (`FgaObject` / `FgaRel` / subjects).
//!
//! The `model!` macro itself is exercised end-to-end in `example-openfga`
//! and `r2e-compile-tests` (it cannot run inside this crate: the generated
//! code paths resolve the crate name externally).

use r2e_openfga::typed::{FgaObject, FgaRel, FgaSubject, FgaType, FgaWildcard};
use r2e_openfga::FgaCheck;

struct DocumentTy;
impl FgaType for DocumentTy {
    const NAME: &'static str = "document";
}

struct ViewerMarker;
const VIEWER: FgaRel<DocumentTy, ViewerMarker> = FgaRel::new("viewer");

#[test]
fn object_formats_type_id() {
    let obj = FgaObject::<DocumentTy>::new("readme");
    assert_eq!(obj.as_str(), "document:readme");
    assert_eq!(obj.id(), "readme");
    assert_eq!(obj.to_string(), "document:readme");
}

#[test]
fn object_rejects_metacharacter_injection() {
    // ':' = type injection, '#' = userset reference, '*' = wildcard.
    for id in ["secret:admin", "readme#viewer", "*"] {
        let err = FgaObject::<DocumentTy>::try_new(id).unwrap_err();
        assert!(err.to_string().contains("must not contain"), "{id}: {err}");
    }
}

#[test]
#[should_panic(expected = "must not contain ':'")]
fn object_new_panics_on_colon() {
    FgaObject::<DocumentTy>::new("secret:admin");
}

#[test]
fn rel_carries_relation_and_object_type() {
    assert_eq!(VIEWER.name(), "viewer");
    assert_eq!(VIEWER.object_type(), "document");
}

#[test]
fn userset_subject_renders_object_hash_relation() {
    let set = VIEWER.of(FgaObject::new("readme"));
    assert_eq!(set.as_str(), "document:readme#viewer");
    assert_eq!(set.subject_str(), "document:readme#viewer");
}

#[test]
fn wildcard_subject_renders_type_star() {
    let wc = FgaWildcard::<DocumentTy>::new();
    assert_eq!(wc.to_string(), "document:*");
    assert_eq!(wc.subject_str(), "document:*");
}

#[test]
fn has_builds_the_same_check_as_the_stringly_form() {
    let typed = FgaCheck::has(VIEWER).from_query("doc_id");
    let stringly = FgaCheck::relation("viewer")
        .on("document")
        .from_query("doc_id");
    assert_eq!(typed.relation, stringly.relation);
    assert_eq!(typed.object_type, stringly.object_type);
}

// ── Wildcard subjects on the request path ───────────────────────────────────
//
// `FgaWildcard` is a ZST, so `subject_str` has to hand out a `'static` string.
// `FgaClient::{check,grant,revoke}` call it, and those run per request: a
// wildcard subject must therefore never allocate — let alone leak — per call.
// `model!` emits `FgaType::WILDCARD` as a literal, which is what the tests
// below stand in for; the interning fallback covers hand-written impls.

/// What `model!` generates: the wire form is a literal, so `subject_str`
/// returns *that* literal — no allocation, no lock, no leak, ever.
struct TeamTy;
impl FgaType for TeamTy {
    const NAME: &'static str = "team";
    const WILDCARD: Option<&'static str> = Some("team:*");
}

/// Same, but with a `WILDCARD` deliberately unlike `"<NAME>:*"`: whatever
/// `subject_str` returns proves which branch ran. A crate-boundary
/// `ptr::eq` against the literal cannot: an equal `&'static str` const is
/// re-materialised per use site.
struct ProbeTy;
impl FgaType for ProbeTy {
    const NAME: &'static str = "probe";
    const WILDCARD: Option<&'static str> = Some("from-the-const:*");
}

#[test]
fn generated_wildcard_returns_the_compile_time_literal() {
    assert_eq!(FgaWildcard::<TeamTy>::new().subject_str(), "team:*");
    assert_eq!(
        FgaWildcard::<ProbeTy>::new().subject_str(),
        "from-the-const:*",
        "a wildcard subject must come from `FgaType::WILDCARD` when it is set — \
         building (and leaking) `\"{{NAME}}:*\"` instead would return \
         \"probe:*\". See docs/claude/hot-path-clone-audit.md",
    );
}

#[test]
fn wildcard_subject_is_stable_across_calls() {
    // Both shapes: the literal (generated) and the interned fallback
    // (hand-written `DocumentTy`, which leaves `WILDCARD` at its default).
    for _ in 0..3 {
        assert!(std::ptr::eq(
            FgaWildcard::<TeamTy>::new().subject_str().as_ptr(),
            FgaWildcard::<TeamTy>::new().subject_str().as_ptr(),
        ));
        assert!(
            std::ptr::eq(
                FgaWildcard::<DocumentTy>::new().subject_str().as_ptr(),
                FgaWildcard::<DocumentTy>::new().subject_str().as_ptr(),
            ),
            "the fallback must intern once per type, not build (and leak) one \
             string per call",
        );
    }
    assert!(DocumentTy::WILDCARD.is_none(), "fallback path under test");
}

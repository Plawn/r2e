//! Semantic validation on top of the syntax-only parser.

use r2e_openfga_model::{parse, validate};

const HEADER: &str = "model\n  schema 1.1\n";

fn errors_of(dsl: &str) -> Vec<String> {
    validate(&parse(dsl).unwrap())
        .unwrap_err()
        .into_iter()
        .map(|e| e.to_string())
        .collect()
}

#[test]
fn valid_model_passes() {
    let dsl = format!(
        "{HEADER}type user\n\ntype team\n  relations\n    define member: [user]\n\ntype doc\n  relations\n    define parent: [doc]\n    define viewer: [user, user:*, team#member] or viewer from parent\n"
    );
    validate(&parse(&dsl).unwrap()).unwrap();
}

#[test]
fn unknown_type_in_restrictions() {
    let errs = errors_of(&format!(
        "{HEADER}type doc\n  relations\n    define viewer: [user]\n"
    ));
    assert!(errs[0].contains("unknown type `user`"), "{errs:?}");
}

#[test]
fn unknown_userset_relation() {
    let errs = errors_of(&format!(
        "{HEADER}type user\ntype team\ntype doc\n  relations\n    define viewer: [team#member]\n"
    ));
    assert!(
        errs[0].contains("relation `member` does not exist on type `team`"),
        "{errs:?}"
    );
}

#[test]
fn unknown_computed_userset() {
    let errs = errors_of(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [user] or editor\n"
    ));
    assert!(
        errs[0].contains("relation `editor` does not exist on type `doc`"),
        "{errs:?}"
    );
}

#[test]
fn unknown_tupleset_relation() {
    let errs = errors_of(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: viewer from parent\n"
    ));
    assert!(
        errs[0].contains("tupleset relation `parent` does not exist"),
        "{errs:?}"
    );
}

#[test]
fn tupleset_subject_types_must_define_computed_relation() {
    let errs = errors_of(&format!(
        "{HEADER}type user\ntype folder\n  relations\n    define owner: [user]\ntype doc\n  relations\n    define parent: [folder]\n    define viewer: reader from parent\n"
    ));
    assert!(
        errs[0].contains("relation `reader` does not exist on any subject type of `parent`"),
        "{errs:?}"
    );
}

#[test]
fn unknown_condition() {
    let errs = errors_of(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [user with expired]\n"
    ));
    assert!(errs[0].contains("unknown condition `expired`"), "{errs:?}");
}

#[test]
fn all_errors_are_collected() {
    let errs = errors_of(&format!(
        "{HEADER}type doc\n  relations\n    define viewer: [user] or editor\n"
    ));
    assert_eq!(errs.len(), 2, "{errs:?}");
}

//! DSL→JSON round-trips against the vendored openfga/language transformer
//! corpus, plus targeted syntax-error cases.

use r2e_openfga_model::{parse, AuthorizationModel};

/// Every corpus case: parse the DSL, compare with the official JSON
/// (value equality — object key order is irrelevant), and check the typed
/// model deserializes back from that JSON to the same value.
#[test]
fn corpus_round_trips() {
    let corpus = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus");
    let mut cases = 0;
    let mut failures = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(&corpus)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();

    for case in entries {
        let name = case.file_name().unwrap().to_string_lossy().to_string();
        let dsl = std::fs::read_to_string(case.join("authorization-model.fga")).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(case.join("authorization-model.json")).unwrap(),
        )
        .unwrap();

        cases += 1;
        let model = match parse(&dsl) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{name}: parse error: {e}"));
                continue;
            }
        };

        let produced = model.to_json();
        if produced != expected {
            failures.push(format!(
                "{name}: JSON mismatch\n--- produced\n{}\n--- expected\n{}",
                serde_json::to_string_pretty(&produced).unwrap(),
                serde_json::to_string_pretty(&expected).unwrap()
            ));
            continue;
        }

        // JSON → typed model → JSON must also be stable (needed by the
        // boot-time structural compare in the OpenFga plugin).
        let reparsed: AuthorizationModel = serde_json::from_value(expected.clone()).unwrap();
        if reparsed.to_json() != expected {
            failures.push(format!("{name}: JSON deserialize/serialize not stable"));
        }
    }

    assert!(cases >= 25, "corpus went missing: only {cases} cases found");
    assert!(
        failures.is_empty(),
        "{} corpus case(s) failed:\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

fn parse_err(dsl: &str) -> String {
    parse(dsl).unwrap_err().to_string()
}

const HEADER: &str = "model\n  schema 1.1\n";

#[test]
fn rejects_missing_model_header() {
    let e = parse_err("type user\n");
    assert!(e.contains("expected `model` header"), "{e}");
}

#[test]
fn rejects_unsupported_schema_version() {
    let e = parse_err("model\n  schema 1.2\ntype user\n");
    assert!(e.contains("unsupported schema version `1.2`"), "{e}");
}

#[test]
fn rejects_modular_models() {
    let e = parse_err("module core\n\nmodel\n  schema 1.1\n");
    assert!(e.contains("expected `model` header"), "{e}");
    let e = parse_err(&format!("{HEADER}extend type user\n"));
    assert!(e.contains("not supported"), "{e}");
}

#[test]
fn rejects_mixed_operators_without_parens() {
    let dsl = format!(
        "{HEADER}type user\ntype doc\n  relations\n    define a: [user]\n    define b: [user]\n    define c: a or b and a\n"
    );
    let e = parse_err(&dsl);
    assert!(e.contains("cannot mix `or` with `and`"), "{e}");

    let dsl = format!(
        "{HEADER}type user\ntype doc\n  relations\n    define a: [user]\n    define b: [user]\n    define c: a but not b or a\n"
    );
    let e = parse_err(&dsl);
    assert!(e.contains("cannot mix `but not` with `or`"), "{e}");
}

#[test]
fn rejects_second_direct_restriction_block() {
    let dsl = format!("{HEADER}type user\ntype doc\n  relations\n    define a: [user] or [user]\n");
    let e = parse_err(&dsl);
    assert!(e.contains("only one `[...]`"), "{e}");
}

#[test]
fn rejects_duplicates() {
    let e = parse_err(&format!("{HEADER}type user\ntype user\n"));
    assert!(e.contains("duplicate type `user`"), "{e}");

    let e = parse_err(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define a: [user]\n    define a: [user]\n"
    ));
    assert!(e.contains("duplicate relation `a`"), "{e}");
}

#[test]
fn reports_line_numbers() {
    let err = parse(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define a: or\n"
    ))
    .unwrap_err();
    assert_eq!(err.line, 6);
}

#[test]
fn comments_are_stripped_but_usersets_are_not() {
    let dsl = format!(
        "# top comment\n{HEADER}type user\n\ntype team\n  relations\n    define member: [user]\n\ntype doc\n  relations\n    # viewers\n    define viewer: [user, team#member] # inline\n"
    );
    let model = parse(&dsl).unwrap();
    let doc = model.type_definition("doc").unwrap();
    let refs = doc.directly_related_user_types("viewer");
    assert_eq!(refs.len(), 2);
    assert_eq!(refs[1].relation.as_deref(), Some("member"));
}

#[test]
fn condition_body_handles_braces_and_strings() {
    let dsl = format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [user with c1]\n\ncondition c1(m: map<string>) {{\n  m[\"k}}\"] == \"v{{\"\n}}\n"
    );
    let model = parse(&dsl).unwrap();
    assert_eq!(model.conditions["c1"].expression, "m[\"k}\"] == \"v{\"");
}

#[test]
fn condition_body_is_verbatim_hash_in_cel_string_is_not_a_comment() {
    let dsl = format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [user with c1]\n\ncondition c1(region: string) {{\n  region == \"EU # west\"\n}} # trailing comment is fine\n"
    );
    let model = parse(&dsl).unwrap();
    assert_eq!(model.conditions["c1"].expression, "region == \"EU # west\"");
}

#[test]
fn schema_keyword_requires_a_separated_version() {
    let e = parse_err("model\n  schema\ntype user\n");
    assert_eq!(e, "line 2: expected `schema <version>`, found `schema`");
    let e = parse_err("model\n  schema1.1\ntype user\n");
    assert_eq!(e, "line 2: expected `schema <version>`, found `schema1.1`");
}

#[test]
fn identifiers_must_start_with_a_letter_or_underscore() {
    let e = parse_err(&format!("{HEADER}type 9lives\n"));
    assert_eq!(e, "line 3: invalid type name `9lives`");

    // `_` may lead, `-` and digits may follow.
    let model = parse(&format!("{HEADER}type _user-2\n")).unwrap();
    assert_eq!(model.type_definitions[0].type_name, "_user-2");
}

#[test]
fn relation_tokens_reject_a_leading_digit_and_accept_a_leading_underscore() {
    let e = parse_err(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [1user]\n"
    ));
    assert_eq!(e, "line 6: unexpected character `1`");

    let model = parse(&format!(
        "{HEADER}type _user\ntype doc\n  relations\n    define _viewer: [_user]\n"
    ))
    .unwrap();
    let doc = model.type_definition("doc").unwrap();
    assert_eq!(
        doc.directly_related_user_types("_viewer")[0].type_name,
        "_user"
    );
}

#[test]
fn tupleset_after_from_cannot_be_a_keyword() {
    let e = parse_err(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: owner from or\n"
    ));
    assert_eq!(e, "line 6: expected relation name after `from`, found `or`");
}

#[test]
fn comment_needs_preceding_whitespace_not_following() {
    // `#comment` (no space after `#`) is still a comment once preceded by
    // whitespace; a `#` ending the line is one too.
    let model = parse(&format!(
        "{HEADER}type user\ntype doc\n  relations\n    define viewer: [user] #comment\n    define editor: [user] #\n"
    ))
    .unwrap();
    let doc = model.type_definition("doc").unwrap();
    assert_eq!(doc.directly_related_user_types("viewer").len(), 1);
    assert_eq!(doc.directly_related_user_types("editor").len(), 1);
}

#[test]
fn bool_condition_parameter_type() {
    let model = parse(&format!(
        "{HEADER}type user\n\ncondition c(flag: bool) {{ flag }}\n"
    ))
    .unwrap();
    let param = &model.conditions["c"].parameters["flag"];
    assert_eq!(param.type_name, "TYPE_NAME_BOOL");
    assert!(param.generic_types.is_empty());
}

#[test]
fn condition_body_tracks_nested_braces() {
    let model = parse(&format!(
        "{HEADER}type user\n\ncondition c(m: map<string>) {{ m == {{\"k\": {{\"v\": 1}}}} }}\n"
    ))
    .unwrap();
    assert_eq!(model.conditions["c"].expression, "m == {\"k\": {\"v\": 1}}");
}

#[test]
fn condition_body_skips_escaped_quotes_in_strings() {
    let model = parse(&format!(
        "{HEADER}type user\n\ncondition c(s: string) {{ s == \"a\\\"}}\" }}\n"
    ))
    .unwrap();
    assert_eq!(model.conditions["c"].expression, "s == \"a\\\"}\"");
}

#[test]
fn unterminated_string_literal_ends_with_its_line() {
    // String state does not carry across lines: the `}` on the next line
    // closes the body (CEL itself rejects the literal later, server-side).
    let model = parse(&format!(
        "{HEADER}type user\n\ncondition c(s: string) {{\n  s == \"abc\n}}\n"
    ))
    .unwrap();
    assert_eq!(model.conditions["c"].expression, "s == \"abc");
}

#[test]
fn unterminated_condition_body_reports_the_last_line() {
    let err = parse(&format!(
        "{HEADER}type user\n\ncondition c(s: string) {{\n  s == \"a\"\n\n\n"
    ))
    .unwrap_err();
    assert_eq!(err.line, 8);
    assert_eq!(err.message, "unterminated condition body for `c`");
}

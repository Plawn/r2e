//! Tests for `r2e_openfga::model_convert` — AST→prost conversion of the
//! `model!`-generated schema-1.1 JSON, plus structural comparison with a live
//! store model.

use std::collections::HashMap;

use r2e_openfga::model_convert::{compile_model, diff_summary, models_equal};
use r2e_openfga::model_parser;

/// A rich model exercising every construct: direct types, wildcard, userset
/// reference (`team#member`), conditioned reference, `or`, `and`, `but not`,
/// `X from Y`, and a condition block.
const RICH_DSL: &str = r#"model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define parent: [document]
    define owner: [user, user:*]
    define blocked: [user]
    define editor: [user, team#member with cond]
    define viewer: [user] or editor
    define admin: [user] and owner
    define can_read: viewer but not blocked
    define inherited: viewer from parent

condition cond(x: string) {
  x == "a"
}
"#;

/// Parse + validate the DSL and produce the schema-1.1 JSON string (what
/// `model!` bakes into `authz::MODEL`).
fn model_json(dsl: &str) -> String {
    let model = model_parser::parse(dsl).expect("DSL parses");
    model_parser::validate(&model).expect("DSL validates");
    model.to_json().to_string()
}

fn find_type<'a>(
    types: &'a [openfga_rs::TypeDefinition],
    name: &str,
) -> &'a openfga_rs::TypeDefinition {
    types
        .iter()
        .find(|t| t.r#type == name)
        .unwrap_or_else(|| panic!("type '{name}' present"))
}

#[test]
fn converts_every_construct_to_prost() {
    use openfga_rs::userset::Userset as Oneof;

    let compiled = compile_model(&model_json(RICH_DSL)).expect("compiles");

    assert_eq!(compiled.schema_version, "1.1");

    let doc = find_type(&compiled.type_definitions, "document");

    // `parent: [document]` → `this` (direct).
    let parent = &doc.relations["parent"];
    assert!(matches!(parent.userset, Some(Oneof::This(_))));

    // `viewer: [user] or editor` → union of `this` and computed `editor`.
    let viewer = &doc.relations["viewer"];
    match viewer.userset.as_ref().expect("viewer userset") {
        Oneof::Union(usersets) => {
            assert_eq!(usersets.child.len(), 2);
            assert!(matches!(usersets.child[0].userset, Some(Oneof::This(_))));
            match usersets.child[1].userset.as_ref().unwrap() {
                Oneof::ComputedUserset(obj) => {
                    // Empty `object`, relation carries the name.
                    assert_eq!(obj.object, "");
                    assert_eq!(obj.relation, "editor");
                }
                other => panic!("expected computed_userset, got {other:?}"),
            }
        }
        other => panic!("expected union, got {other:?}"),
    }

    // `admin: [user] and owner` → intersection.
    let admin = &doc.relations["admin"];
    match admin.userset.as_ref().unwrap() {
        Oneof::Intersection(usersets) => assert_eq!(usersets.child.len(), 2),
        other => panic!("expected intersection, got {other:?}"),
    }

    // `can_read: viewer but not blocked` → difference (boxed).
    let can_read = &doc.relations["can_read"];
    match can_read.userset.as_ref().unwrap() {
        Oneof::Difference(diff) => {
            match diff.base.as_ref().expect("base").userset.as_ref().unwrap() {
                Oneof::ComputedUserset(o) => assert_eq!(o.relation, "viewer"),
                other => panic!("expected computed base, got {other:?}"),
            }
            match diff
                .subtract
                .as_ref()
                .expect("subtract")
                .userset
                .as_ref()
                .unwrap()
            {
                Oneof::ComputedUserset(o) => assert_eq!(o.relation, "blocked"),
                other => panic!("expected computed subtract, got {other:?}"),
            }
        }
        other => panic!("expected difference, got {other:?}"),
    }

    // `inherited: viewer from parent` → tuple_to_userset, both ObjectRelation
    // with empty `object`.
    let inherited = &doc.relations["inherited"];
    match inherited.userset.as_ref().unwrap() {
        Oneof::TupleToUserset(ttu) => {
            let tupleset = ttu.tupleset.as_ref().expect("tupleset");
            assert_eq!(tupleset.object, "");
            assert_eq!(tupleset.relation, "parent");
            let cu = ttu.computed_userset.as_ref().expect("computed_userset");
            assert_eq!(cu.object, "");
            assert_eq!(cu.relation, "viewer");
        }
        other => panic!("expected tuple_to_userset, got {other:?}"),
    }

    // Metadata: `owner: [user, user:*]` — the wildcard reference.
    let md = doc.metadata.as_ref().expect("document has metadata");
    let owner_refs = &md.relations["owner"].directly_related_user_types;
    assert_eq!(owner_refs.len(), 2);
    // `user` — plain direct, no relation/wildcard.
    let user_ref = owner_refs.iter().find(|r| r.r#type == "user").unwrap();
    assert!(user_ref.relation_or_wildcard.is_none());
    assert_eq!(user_ref.condition, "");
    // `user:*` — wildcard.
    let wildcard_ref = owner_refs
        .iter()
        .find(|r| r.relation_or_wildcard.is_some())
        .expect("wildcard reference present");
    assert!(matches!(
        wildcard_ref.relation_or_wildcard,
        Some(openfga_rs::relation_reference::RelationOrWildcard::Wildcard(_))
    ));
    assert_eq!(wildcard_ref.r#type, "user");

    // `editor: [user, team#member with cond]` — userset ref + condition name.
    let editor_refs = &md.relations["editor"].directly_related_user_types;
    let team_ref = editor_refs
        .iter()
        .find(|r| r.r#type == "team")
        .expect("team#member reference present");
    match team_ref.relation_or_wildcard.as_ref().unwrap() {
        openfga_rs::relation_reference::RelationOrWildcard::Relation(rel) => {
            assert_eq!(rel, "member");
        }
        other => panic!("expected Relation, got {other:?}"),
    }
    assert_eq!(team_ref.condition, "cond");

    // Condition: parameter type maps to the prost i32 enum.
    let cond = &compiled.conditions["cond"];
    assert_eq!(cond.name, "cond");
    assert_eq!(cond.expression, "x == \"a\"");
    assert!(cond.metadata.is_none());
    let param = &cond.parameters["x"];
    assert_eq!(
        param.type_name,
        openfga_rs::condition_param_type_ref::TypeName::String as i32
    );
    assert!(param.generic_types.is_empty());
}

#[test]
fn round_trip_equality_ignores_server_noise() {
    let compiled = compile_model(&model_json(RICH_DSL)).expect("compiles");

    // Build a "live" model from the SAME compiled parts, then inject noise:
    // an id, module/source_info annotations, and empty-vs-absent metadata.
    let mut live = openfga_rs::AuthorizationModel {
        id: "01G5JAVJ41T49E9TT3SKVS7X1J".to_owned(),
        schema_version: compiled.schema_version.clone(),
        type_definitions: compiled.type_definitions.clone(),
        conditions: compiled.conditions.clone(),
    };

    for td in &mut live.type_definitions {
        if let Some(md) = td.metadata.as_mut() {
            md.module = "core".to_owned();
            md.source_info = Some(openfga_rs::SourceInfo {
                file: "model.fga".to_owned(),
            });
            for rm in md.relations.values_mut() {
                rm.module = "core".to_owned();
                rm.source_info = Some(openfga_rs::SourceInfo {
                    file: "model.fga".to_owned(),
                });
            }
        }
    }

    // Store adds condition metadata noise.
    for cond in live.conditions.values_mut() {
        cond.metadata = Some(openfga_rs::ConditionMetadata {
            module: "core".to_owned(),
            source_info: Some(openfga_rs::SourceInfo {
                file: "model.fga".to_owned(),
            }),
        });
    }

    // Compiled `user` type has metadata: None; give the live side a
    // Some(Metadata { relations: {} }) — must be treated as absent.
    let live_user = live
        .type_definitions
        .iter_mut()
        .find(|t| t.r#type == "user")
        .unwrap();
    assert!(live_user.metadata.is_none());
    live_user.metadata = Some(openfga_rs::Metadata {
        relations: HashMap::new(),
        module: String::new(),
        source_info: None,
    });

    // And an empty RelationMetadata entry on the live side (must be dropped).
    let live_team = live
        .type_definitions
        .iter_mut()
        .find(|t| t.r#type == "team")
        .unwrap();
    live_team
        .metadata
        .as_mut()
        .unwrap()
        .relations
        .insert(
            "ghost".to_owned(),
            openfga_rs::RelationMetadata {
                directly_related_user_types: Vec::new(),
                module: "core".to_owned(),
                source_info: None,
            },
        );

    assert!(
        models_equal(&compiled, &live),
        "noise-only differences must compare equal: {}",
        diff_summary(&compiled, &live)
    );
    assert_eq!(
        diff_summary(&compiled, &live),
        "models are structurally equal"
    );
}

#[test]
fn inequality_userset_change_names_type() {
    use openfga_rs::userset::Userset as Oneof;

    let compiled = compile_model(&model_json(RICH_DSL)).expect("compiles");
    let mut live = openfga_rs::AuthorizationModel {
        id: "01G5JAVJ41T49E9TT3SKVS7X1J".to_owned(),
        schema_version: compiled.schema_version.clone(),
        type_definitions: compiled.type_definitions.clone(),
        conditions: compiled.conditions.clone(),
    };

    // Mutate the `viewer` relation on the `document` type on the live side.
    let doc = live
        .type_definitions
        .iter_mut()
        .find(|t| t.r#type == "document")
        .unwrap();
    doc.relations.insert(
        "viewer".to_owned(),
        openfga_rs::Userset {
            userset: Some(Oneof::This(openfga_rs::DirectUserset {})),
        },
    );

    assert!(!models_equal(&compiled, &live));
    let summary = diff_summary(&compiled, &live);
    assert!(
        summary.contains("types differing") && summary.contains("document"),
        "summary should name the differing type: {summary}"
    );
}

#[test]
fn inequality_schema_version_mentioned() {
    let compiled = compile_model(&model_json(RICH_DSL)).expect("compiles");
    let live = openfga_rs::AuthorizationModel {
        id: String::new(),
        schema_version: "1.2".to_owned(),
        type_definitions: compiled.type_definitions.clone(),
        conditions: compiled.conditions.clone(),
    };

    assert!(!models_equal(&compiled, &live));
    let summary = diff_summary(&compiled, &live);
    assert!(
        summary.contains("schema_version differs")
            && summary.contains("1.1")
            && summary.contains("1.2"),
        "summary should mention schema versions: {summary}"
    );
}

#[test]
fn inequality_type_present_only_on_one_side() {
    let compiled = compile_model(&model_json(RICH_DSL)).expect("compiles");

    // Live side drops the `team` type entirely.
    let mut live = openfga_rs::AuthorizationModel {
        id: String::new(),
        schema_version: compiled.schema_version.clone(),
        type_definitions: compiled.type_definitions.clone(),
        conditions: compiled.conditions.clone(),
    };
    live.type_definitions.retain(|t| t.r#type != "team");

    assert!(!models_equal(&compiled, &live));
    let summary = diff_summary(&compiled, &live);
    assert!(
        summary.contains("types only in compiled") && summary.contains("team"),
        "summary should name the missing type: {summary}"
    );
}

#[test]
fn compile_model_rejects_invalid_json() {
    let err = compile_model("{ not valid json ]").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("authorization model JSON is invalid"),
        "unexpected error: {msg}"
    );
}

#[test]
fn compile_model_rejects_unknown_condition_param_type() {
    // Hand-crafted JSON with a bogus condition parameter type name.
    let json = r#"{
        "schema_version": "1.1",
        "type_definitions": [{ "type": "user", "relations": {}, "metadata": null }],
        "conditions": {
            "cond": {
                "name": "cond",
                "expression": "x == 1",
                "parameters": {
                    "x": { "type_name": "TYPE_NAME_BOGUS", "generic_types": [] }
                }
            }
        }
    }"#;

    let err = compile_model(json).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unknown condition parameter type name") && msg.contains("TYPE_NAME_BOGUS"),
        "unexpected error: {msg}"
    );
}

/// Reordered but semantically equivalent models compare equal: `union` /
/// `intersection` children and `directly_related_user_types` are
/// order-independent in OpenFGA, so a store authored out of band (or a
/// reordered `.fga`) must not read as a mismatch in verify mode.
#[test]
fn equality_ignores_order_of_unions_and_direct_types() {
    let a = r#"model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define owner: [user, team#member, user:*]
    define editor: [user]
    define viewer: [user] or editor or owner
    define admin: [user] and owner and editor
"#;
    // Same model, `or`/`and` operands and `[...]` entries reordered.
    let b = r#"model
  schema 1.1

type user

type team
  relations
    define member: [user]

type document
  relations
    define owner: [user:*, user, team#member]
    define editor: [user]
    define viewer: [user] or owner or editor
    define admin: [user] and editor and owner
"#;
    let compiled_a = compile_model(&model_json(a)).expect("a compiles");
    let compiled_b = compile_model(&model_json(b)).expect("b compiles");

    let live_b = openfga_rs::AuthorizationModel {
        id: "01G5JAVJ41T49E9TT3SKVS7X1J".to_owned(),
        schema_version: compiled_b.schema_version.clone(),
        type_definitions: compiled_b.type_definitions.clone(),
        conditions: compiled_b.conditions.clone(),
    };

    assert!(
        models_equal(&compiled_a, &live_b),
        "reordered equivalent models must compare equal: {}",
        diff_summary(&compiled_a, &live_b)
    );
}

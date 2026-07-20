//! Conversion of the `model!`-generated schema-1.1 JSON into the prost
//! request types of `openfga_rs`, plus structural comparison with a live
//! store model.
//!
//! The `model!` macro emits `authz::MODEL`: a `&'static str` holding the
//! authorization model as schema-1.1 JSON (the serde shape of
//! [`r2e_openfga_model::AuthorizationModel`]). The `OpenFga` plugin needs the
//! same model as `openfga_rs` wire types — both to write it over gRPC
//! (`WriteAuthorizationModel`) and to compare it structurally against a live
//! model fetched from the store (`ReadAuthorizationModels`).
//!
//! The `openfga_rs` prost types derive serde, but their oneof enums serialize
//! with Rust variant-name external tagging, which does **not** match the
//! OpenFGA JSON. So the JSON is first deserialized back into the
//! `r2e_openfga_model` AST, then converted AST → prost by hand. Never try to
//! serde-deserialize the OpenFGA JSON directly into the prost types.
//!
//! Comparison ignores server-side noise — the model `id`, `module` /
//! `source_info` annotations, and empty-vs-absent metadata containers — and
//! the ordering of semantically order-independent lists (`union` /
//! `intersection` children, `directly_related_user_types`), so a reordered
//! but equivalent model never reads as different. Everything
//! evaluation-relevant is compared: there is no false-equal risk.

use std::collections::HashMap;

use r2e_openfga_model as ast;

use crate::error::OpenFgaError;

/// The compiled-in authorization model, converted to `openfga_rs` wire types.
///
/// The fields mirror `WriteAuthorizationModelRequest` (minus `store_id`), so a
/// write request is a direct field-for-field construction.
#[derive(Debug, Clone)]
pub struct CompiledModel {
    /// Schema version, e.g. `"1.1"`.
    pub schema_version: String,
    /// Type definitions in source order.
    pub type_definitions: Vec<openfga_rs::TypeDefinition>,
    /// Conditions keyed by name (same map type as
    /// `WriteAuthorizationModelRequest.conditions`).
    pub conditions: HashMap<String, openfga_rs::Condition>,
}

/// Parse the schema-1.1 JSON emitted by `model!` and convert it to wire types.
///
/// Fails with [`OpenFgaError::InvalidConfig`] if the JSON does not deserialize
/// into an [`AuthorizationModel`](ast::AuthorizationModel) or references an
/// unknown condition parameter type name.
pub fn compile_model(model_json: &str) -> Result<CompiledModel, OpenFgaError> {
    let model: ast::AuthorizationModel = serde_json::from_str(model_json).map_err(|e| {
        OpenFgaError::InvalidConfig(format!("authorization model JSON is invalid: {e}"))
    })?;

    let type_definitions = model
        .type_definitions
        .iter()
        .map(convert_type_def)
        .collect();

    let mut conditions = HashMap::with_capacity(model.conditions.len());
    for (name, cond) in &model.conditions {
        conditions.insert(name.clone(), convert_condition(cond)?);
    }

    Ok(CompiledModel {
        schema_version: model.schema_version,
        type_definitions,
        conditions,
    })
}

// ---------------------------------------------------------------------------
// AST → prost conversion
// ---------------------------------------------------------------------------

fn convert_type_def(td: &ast::TypeDefinition) -> openfga_rs::TypeDefinition {
    let relations = td
        .relations
        .iter()
        .map(|(name, us)| (name.clone(), convert_userset(us)))
        .collect();

    openfga_rs::TypeDefinition {
        r#type: td.type_name.clone(),
        relations,
        metadata: td.metadata.as_ref().map(convert_metadata),
    }
}

fn convert_userset(us: &ast::Userset) -> openfga_rs::Userset {
    use openfga_rs::userset::Userset as Oneof;

    let inner = match us {
        ast::Userset::This {} => Oneof::This(openfga_rs::DirectUserset {}),
        ast::Userset::ComputedUserset { relation } => {
            Oneof::ComputedUserset(openfga_rs::ObjectRelation {
                object: String::new(),
                relation: relation.clone(),
            })
        }
        ast::Userset::TupleToUserset {
            tupleset,
            computed_userset,
        } => Oneof::TupleToUserset(openfga_rs::TupleToUserset {
            tupleset: Some(object_relation(&tupleset.relation)),
            computed_userset: Some(object_relation(&computed_userset.relation)),
        }),
        ast::Userset::Union { child } => Oneof::Union(openfga_rs::Usersets {
            child: child.iter().map(convert_userset).collect(),
        }),
        ast::Userset::Intersection { child } => Oneof::Intersection(openfga_rs::Usersets {
            child: child.iter().map(convert_userset).collect(),
        }),
        ast::Userset::Difference { base, subtract } => {
            Oneof::Difference(Box::new(openfga_rs::Difference {
                base: Some(Box::new(convert_userset(base))),
                subtract: Some(Box::new(convert_userset(subtract))),
            }))
        }
    };

    openfga_rs::Userset {
        userset: Some(inner),
    }
}

fn object_relation(relation: &str) -> openfga_rs::ObjectRelation {
    openfga_rs::ObjectRelation {
        object: String::new(),
        relation: relation.to_owned(),
    }
}

fn convert_metadata(md: &ast::Metadata) -> openfga_rs::Metadata {
    let relations = md
        .relations
        .iter()
        .map(|(name, rm)| (name.clone(), convert_relation_metadata(rm)))
        .collect();

    openfga_rs::Metadata {
        relations,
        module: String::new(),
        source_info: None,
    }
}

fn convert_relation_metadata(rm: &ast::RelationMetadata) -> openfga_rs::RelationMetadata {
    openfga_rs::RelationMetadata {
        directly_related_user_types: rm
            .directly_related_user_types
            .iter()
            .map(convert_relation_reference)
            .collect(),
        module: String::new(),
        source_info: None,
    }
}

fn convert_relation_reference(rr: &ast::RelationReference) -> openfga_rs::RelationReference {
    use openfga_rs::relation_reference::RelationOrWildcard;

    // `relation` and `wildcard` are mutually exclusive in the DSL; prefer
    // `relation` if both were somehow present.
    let relation_or_wildcard = if let Some(rel) = &rr.relation {
        Some(RelationOrWildcard::Relation(rel.clone()))
    } else if rr.wildcard.is_some() {
        Some(RelationOrWildcard::Wildcard(openfga_rs::Wildcard {}))
    } else {
        None
    };

    openfga_rs::RelationReference {
        r#type: rr.type_name.clone(),
        condition: rr.condition.clone().unwrap_or_default(),
        relation_or_wildcard,
    }
}

fn convert_condition(cond: &ast::Condition) -> Result<openfga_rs::Condition, OpenFgaError> {
    let mut parameters = HashMap::with_capacity(cond.parameters.len());
    for (name, param) in &cond.parameters {
        parameters.insert(name.clone(), convert_param_type(param)?);
    }

    Ok(openfga_rs::Condition {
        name: cond.name.clone(),
        expression: cond.expression.clone(),
        parameters,
        metadata: None,
    })
}

fn convert_param_type(
    param: &ast::ConditionParamType,
) -> Result<openfga_rs::ConditionParamTypeRef, OpenFgaError> {
    use openfga_rs::condition_param_type_ref::TypeName;

    let type_name = TypeName::from_str_name(&param.type_name).ok_or_else(|| {
        OpenFgaError::InvalidConfig(format!(
            "authorization model JSON is invalid: unknown condition parameter type name '{}'",
            param.type_name
        ))
    })?;

    let mut generic_types = Vec::with_capacity(param.generic_types.len());
    for gt in &param.generic_types {
        generic_types.push(convert_param_type(gt)?);
    }

    Ok(openfga_rs::ConditionParamTypeRef {
        type_name: type_name as i32,
        generic_types,
    })
}

// ---------------------------------------------------------------------------
// Structural comparison
// ---------------------------------------------------------------------------

/// A canonicalized view of a model: server-side noise removed so two models
/// that differ only in `id` / `module` / `source_info` / empty-vs-absent
/// metadata compare equal.
struct Canonical {
    schema_version: String,
    /// Types sorted by name.
    types: Vec<openfga_rs::TypeDefinition>,
    conditions: HashMap<String, openfga_rs::Condition>,
}

fn canonical_compiled(compiled: &CompiledModel) -> Canonical {
    Canonical {
        schema_version: compiled.schema_version.clone(),
        types: normalize_types(&compiled.type_definitions),
        conditions: normalize_conditions(&compiled.conditions),
    }
}

fn canonical_live(live: &openfga_rs::AuthorizationModel) -> Canonical {
    Canonical {
        schema_version: live.schema_version.clone(),
        types: normalize_types(&live.type_definitions),
        conditions: normalize_conditions(&live.conditions),
    }
}

fn normalize_types(types: &[openfga_rs::TypeDefinition]) -> Vec<openfga_rs::TypeDefinition> {
    let mut out = types.to_vec();
    for td in &mut out {
        normalize_type_def(td);
    }
    out.sort_by(|a, b| a.r#type.cmp(&b.r#type));
    out
}

fn normalize_type_def(td: &mut openfga_rs::TypeDefinition) {
    for us in td.relations.values_mut() {
        normalize_userset(us);
    }
    if let Some(md) = td.metadata.as_mut() {
        md.module.clear();
        md.source_info = None;
        // Clear module/source_info on each relation metadata, then drop
        // entries that carry no direct type restrictions (fully empty noise).
        md.relations.retain(|_, rm| {
            rm.module.clear();
            rm.source_info = None;
            // Direct type restrictions are order-independent in OpenFGA
            // (`[user, group]` ≡ `[group, user]`).
            rm.directly_related_user_types
                .sort_by_key(|rr| format!("{rr:?}"));
            !rm.directly_related_user_types.is_empty()
        });
        // An all-empty metadata container is equivalent to absent metadata.
        if md.relations.is_empty() {
            td.metadata = None;
        }
    }
}

/// Sort the children of `union` / `intersection` nodes (recursively): OpenFGA
/// evaluates them set-wise, so `a or b` ≡ `b or a`. `difference` and
/// `tupleToUserset` keep their fields — those are semantically ordered. The
/// sort key is the derived `Debug` rendering of the already-normalized child:
/// deterministic and total, which is all canonicalization needs.
fn normalize_userset(us: &mut openfga_rs::Userset) {
    use openfga_rs::userset::Userset as Oneof;

    match us.userset.as_mut() {
        Some(Oneof::Union(children)) | Some(Oneof::Intersection(children)) => {
            for child in &mut children.child {
                normalize_userset(child);
            }
            children.child.sort_by_key(|c| format!("{c:?}"));
        }
        Some(Oneof::Difference(diff)) => {
            if let Some(base) = diff.base.as_mut() {
                normalize_userset(base);
            }
            if let Some(subtract) = diff.subtract.as_mut() {
                normalize_userset(subtract);
            }
        }
        _ => {}
    }
}

fn normalize_conditions(
    conditions: &HashMap<String, openfga_rs::Condition>,
) -> HashMap<String, openfga_rs::Condition> {
    conditions
        .iter()
        .map(|(name, cond)| {
            let mut c = cond.clone();
            c.metadata = None;
            (name.clone(), c)
        })
        .collect()
}

/// Structural equality between the compiled model and a live store model,
/// ignoring the model `id`, `module` / `source_info` annotations,
/// empty-vs-absent metadata containers, and the ordering of
/// order-independent lists (`union`/`intersection` children,
/// `directly_related_user_types`).
pub fn models_equal(compiled: &CompiledModel, live: &openfga_rs::AuthorizationModel) -> bool {
    let c = canonical_compiled(compiled);
    let l = canonical_live(live);
    c.schema_version == l.schema_version && c.types == l.types && c.conditions == l.conditions
}

/// Short human-readable summary of what differs (for the boot-failure error).
///
/// Reports schema version mismatch, type names present on only one side, type
/// names whose definitions differ, and condition names that differ. Computed
/// over the same canonicalized forms as [`models_equal`], so ignored noise
/// never shows up as a difference. Returns `"models are structurally equal"`
/// when nothing differs.
pub fn diff_summary(compiled: &CompiledModel, live: &openfga_rs::AuthorizationModel) -> String {
    let c = canonical_compiled(compiled);
    let l = canonical_live(live);

    let mut segments: Vec<String> = Vec::new();

    if c.schema_version != l.schema_version {
        segments.push(format!(
            "schema_version differs (compiled {}, store {})",
            c.schema_version, l.schema_version
        ));
    }

    let compiled_types: HashMap<&str, &openfga_rs::TypeDefinition> =
        c.types.iter().map(|t| (t.r#type.as_str(), t)).collect();
    let live_types: HashMap<&str, &openfga_rs::TypeDefinition> =
        l.types.iter().map(|t| (t.r#type.as_str(), t)).collect();

    let only_compiled = names_only_in(&compiled_types, &live_types);
    if !only_compiled.is_empty() {
        segments.push(format!("types only in compiled: {}", fmt_list(&only_compiled)));
    }
    let only_live = names_only_in(&live_types, &compiled_types);
    if !only_live.is_empty() {
        segments.push(format!("types only in store: {}", fmt_list(&only_live)));
    }

    let mut differing: Vec<String> = compiled_types
        .iter()
        .filter_map(|(name, td)| match live_types.get(name) {
            Some(other) if other != td => Some((*name).to_owned()),
            _ => None,
        })
        .collect();
    differing.sort();
    if !differing.is_empty() {
        segments.push(format!("types differing: {}", fmt_list(&differing)));
    }

    let cond_diffs = condition_diffs(&c.conditions, &l.conditions);
    if !cond_diffs.is_empty() {
        segments.push(format!("conditions differing: {}", fmt_list(&cond_diffs)));
    }

    if segments.is_empty() {
        "models are structurally equal".to_owned()
    } else {
        segments.join("; ")
    }
}

fn names_only_in(
    a: &HashMap<&str, &openfga_rs::TypeDefinition>,
    b: &HashMap<&str, &openfga_rs::TypeDefinition>,
) -> Vec<String> {
    let mut names: Vec<String> = a
        .keys()
        .filter(|k| !b.contains_key(**k))
        .map(|k| (*k).to_owned())
        .collect();
    names.sort();
    names
}

fn condition_diffs(
    compiled: &HashMap<String, openfga_rs::Condition>,
    live: &HashMap<String, openfga_rs::Condition>,
) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for (name, cond) in compiled {
        match live.get(name) {
            Some(other) if other == cond => {}
            _ => {
                names.insert(name.clone());
            }
        }
    }
    for name in live.keys() {
        if !compiled.contains_key(name) {
            names.insert(name.clone());
        }
    }
    names.into_iter().collect()
}

fn fmt_list(names: &[String]) -> String {
    format!("[{}]", names.join(", "))
}

//! Data model for OpenFGA authorization models (schema 1.1).
//!
//! The serde shapes match the JSON produced by the official
//! `openfga/language` transformer byte-for-byte (modulo object key order):
//! empty types serialize as `"relations": {}, "metadata": null`, relations
//! without direct types get `"directly_related_user_types": []`, and optional
//! reference fields (`relation`, `wildcard`, `condition`) are omitted when
//! absent.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A complete OpenFGA authorization model (schema 1.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthorizationModel {
    pub schema_version: String,
    /// Type definitions in source order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub type_definitions: Vec<TypeDefinition>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub conditions: BTreeMap<String, Condition>,
}

impl AuthorizationModel {
    /// Serialize to the schema 1.1 JSON accepted by the OpenFGA
    /// `WriteAuthorizationModel` API.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("authorization model serializes to JSON")
    }

    /// Look up a type definition by name.
    pub fn type_definition(&self, name: &str) -> Option<&TypeDefinition> {
        self.type_definitions.iter().find(|t| t.type_name == name)
    }
}

/// A single `type` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeDefinition {
    #[serde(rename = "type")]
    pub type_name: String,
    /// Relation rewrite rules. Always serialized (`{}` when empty).
    #[serde(default)]
    pub relations: BTreeMap<String, Userset>,
    /// Relation metadata (direct type restrictions). `null` for types
    /// without relations.
    #[serde(default)]
    pub metadata: Option<Metadata>,
}

impl TypeDefinition {
    /// The direct type restrictions declared for `relation`, if any.
    pub fn directly_related_user_types(&self, relation: &str) -> &[RelationReference] {
        self.metadata
            .as_ref()
            .and_then(|m| m.relations.get(relation))
            .map(|r| r.directly_related_user_types.as_slice())
            .unwrap_or(&[])
    }
}

/// A relation rewrite rule (the value of an entry in `relations`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Userset {
    /// Direct relationship tuples (`[user, ...]` in the DSL).
    #[serde(rename = "this")]
    This {},
    /// Reference to another relation on the same type.
    #[serde(rename = "computedUserset")]
    ComputedUserset { relation: String },
    /// `X from Y` — follow the `tupleset` relation, then evaluate
    /// `computedUserset` on the referenced objects.
    #[serde(rename = "tupleToUserset")]
    TupleToUserset {
        tupleset: ObjectRelation,
        #[serde(rename = "computedUserset")]
        computed_userset: ObjectRelation,
    },
    /// `a or b or ...`
    #[serde(rename = "union")]
    Union { child: Vec<Userset> },
    /// `a and b and ...`
    #[serde(rename = "intersection")]
    Intersection { child: Vec<Userset> },
    /// `base but not subtract`
    #[serde(rename = "difference")]
    Difference {
        base: Box<Userset>,
        subtract: Box<Userset>,
    },
}

/// A relation name reference inside a `tupleToUserset` node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectRelation {
    pub relation: String,
}

/// Per-type metadata: direct type restrictions keyed by relation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub relations: BTreeMap<String, RelationMetadata>,
}

/// Metadata for one relation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationMetadata {
    /// The subject types assignable via direct tuples, in source order.
    /// Empty for relations without a `[...]` block.
    #[serde(default)]
    pub directly_related_user_types: Vec<RelationReference>,
}

/// One entry of a `[...]` direct type restriction:
/// `user`, `user:*`, `team#member`, optionally `with <condition>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationReference {
    #[serde(rename = "type")]
    pub type_name: String,
    /// `team#member` — subjects are members of the userset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<String>,
    /// `user:*` — the public wildcard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wildcard: Option<Wildcard>,
    /// `with <condition>` — the tuple must carry this condition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
}

impl RelationReference {
    /// Plain direct type (`user`).
    pub fn direct(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            relation: None,
            wildcard: None,
            condition: None,
        }
    }
}

/// The `{}` payload of a wildcard reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Wildcard {}

/// A `condition <name>(<params>) { <cel> }` block. The CEL expression is
/// carried verbatim (parser-tolerant passthrough — no typed CEL API).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub name: String,
    pub expression: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub parameters: BTreeMap<String, ConditionParamType>,
}

/// The type of a condition parameter (`string`, `list<string>`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConditionParamType {
    /// Proto enum name, e.g. `TYPE_NAME_STRING`.
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub generic_types: Vec<ConditionParamType>,
}

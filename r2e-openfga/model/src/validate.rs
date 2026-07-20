//! Semantic validation of a parsed [`AuthorizationModel`].
//!
//! The parser (like the official transformer) is syntax-only; these checks
//! catch the referential mistakes that would otherwise surface as opaque
//! store errors — or worse, as a permanently-403 relation:
//!
//! - direct type restrictions referencing unknown types, relations, or
//!   conditions;
//! - computed usersets referencing relations that do not exist on the type;
//! - tuple-to-userset (`X from Y`) where `Y` is not a relation of the type,
//!   or where no direct subject type of `Y` defines `X` (lenient: one match
//!   suffices, mirroring the official semantic validator).

use crate::model::{AuthorizationModel, TypeDefinition, Userset};

/// A semantic error, locatable as `type[.relation]: message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub type_name: String,
    pub relation: Option<String>,
    pub message: String,
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.relation {
            Some(rel) => write!(f, "type `{}`, relation `{}`: {}", self.type_name, rel, self.message),
            None => write!(f, "type `{}`: {}", self.type_name, self.message),
        }
    }
}

/// Check referential integrity across the whole model. Returns every error
/// found (not just the first).
pub fn validate(model: &AuthorizationModel) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    for type_def in &model.type_definitions {
        for (relation, rewrite) in &type_def.relations {
            let mut push = |message: String| {
                errors.push(ValidationError {
                    type_name: type_def.type_name.clone(),
                    relation: Some(relation.clone()),
                    message,
                });
            };

            for reference in type_def.directly_related_user_types(relation) {
                match model.type_definition(&reference.type_name) {
                    None => push(format!("unknown type `{}` in direct type restrictions", reference.type_name)),
                    Some(target) => {
                        if let Some(rel) = &reference.relation {
                            if !target.relations.contains_key(rel) {
                                push(format!(
                                    "relation `{}` does not exist on type `{}` (in `{}#{}`)",
                                    rel, reference.type_name, reference.type_name, rel
                                ));
                            }
                        }
                    }
                }
                if let Some(cond) = &reference.condition {
                    if !model.conditions.contains_key(cond) {
                        push(format!("unknown condition `{}` in `with {}`", cond, cond));
                    }
                }
            }

            validate_rewrite(model, type_def, rewrite, &mut push);
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_rewrite(
    model: &AuthorizationModel,
    type_def: &TypeDefinition,
    rewrite: &Userset,
    push: &mut impl FnMut(String),
) {
    match rewrite {
        Userset::This {} => {}
        Userset::ComputedUserset { relation } => {
            if !type_def.relations.contains_key(relation) {
                push(format!(
                    "relation `{}` does not exist on type `{}`",
                    relation, type_def.type_name
                ));
            }
        }
        Userset::TupleToUserset {
            tupleset,
            computed_userset,
        } => {
            if !type_def.relations.contains_key(&tupleset.relation) {
                push(format!(
                    "tupleset relation `{}` does not exist on type `{}` (in `{} from {}`)",
                    tupleset.relation,
                    type_def.type_name,
                    computed_userset.relation,
                    tupleset.relation
                ));
                return;
            }
            // Lenient cross-type check: the computed relation must exist on
            // at least one plain direct subject type of the tupleset.
            let subject_types = type_def.directly_related_user_types(&tupleset.relation);
            let plain: Vec<_> = subject_types
                .iter()
                .filter(|r| r.relation.is_none() && r.wildcard.is_none())
                .collect();
            if !plain.is_empty()
                && !plain.iter().any(|r| {
                    model
                        .type_definition(&r.type_name)
                        .is_some_and(|t| t.relations.contains_key(&computed_userset.relation))
                })
            {
                push(format!(
                    "relation `{}` does not exist on any subject type of `{}` (in `{} from {}`)",
                    computed_userset.relation,
                    tupleset.relation,
                    computed_userset.relation,
                    tupleset.relation
                ));
            }
        }
        Userset::Union { child } | Userset::Intersection { child } => {
            for c in child {
                validate_rewrite(model, type_def, c, push);
            }
        }
        Userset::Difference { base, subtract } => {
            validate_rewrite(model, type_def, base, push);
            validate_rewrite(model, type_def, subtract, push);
        }
    }
}

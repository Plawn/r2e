//! Rebuild a nested [`ConfigValue`] tree from the flat key map.
//!
//! The config map is **flat**: YAML hierarchies are stored as dot-separated
//! leaf keys (`app.database.url`), and a YAML sequence is stored twice — the
//! whole `ConfigValue::List` under its parent key **and** each element under
//! `key.0`, `key.1`, … The flat keys are the source of truth: the `R2E_` env
//! overlay and `override_config_value` only ever write flat keys, so a parent
//! `List` is a convenience copy that may be stale. This module is the one
//! place that turns the flat map back into a tree, honouring that invariant.

use std::collections::{BTreeMap, HashMap};

use super::value::ConfigValue;

/// A node of the key trie: the value stored at the exact key (if any) and the
/// child segments below it.
#[derive(Default)]
struct Node<'a> {
    value: Option<&'a ConfigValue>,
    children: BTreeMap<&'a str, Node<'a>>,
}

impl<'a> Node<'a> {
    fn insert(&mut self, path: &'a str, value: &'a ConfigValue) {
        let mut node = self;
        for segment in path.split('.') {
            node = node.children.entry(segment).or_default();
        }
        node.value = Some(value);
    }
}

/// Rebuild the subtree under `prefix` (the whole config for `None`).
///
/// Returns `None` when nothing lives at or under the prefix. See
/// [`R2eConfig::tree`](super::R2eConfig::tree) for the full semantics.
pub(crate) fn build(
    values: &HashMap<String, ConfigValue>,
    prefix: Option<&str>,
) -> Option<ConfigValue> {
    let mut root = Node::default();
    match prefix {
        None => {
            for (key, value) in values {
                root.insert(key, value);
            }
        }
        Some(prefix) => {
            root.value = values.get(prefix);
            let dotted = format!("{prefix}.");
            for (key, value) in values {
                if let Some(rest) = key.strip_prefix(&dotted) {
                    root.insert(rest, value);
                }
            }
        }
    }
    materialize(&root)
}

fn materialize(node: &Node<'_>) -> Option<ConfigValue> {
    if node.children.is_empty() {
        return node.value.cloned();
    }

    let indices: Option<Vec<usize>> = node
        .children
        .keys()
        .map(|segment| parse_index(segment))
        .collect::<Option<Vec<usize>>>()
        .map(|mut indices| {
            indices.sort_unstable();
            indices
        });

    match (node.value, indices) {
        // YAML sequence: the parent `List` seeds length and element values,
        // the indexed flat keys override element by element.
        (Some(ConfigValue::List(seed)), Some(indices)) => {
            let len = indices.last().map_or(0, |i| i + 1).max(seed.len());
            Some(ConfigValue::List(build_list(node, seed, len)))
        }
        // Env-only list: every child is a decimal index and they form `0..n`.
        (_, Some(indices)) if is_dense(&indices) => {
            Some(ConfigValue::List(build_list(node, &[], indices.len())))
        }
        // Map — children win over a scalar (or stale list) at the exact key.
        _ => Some(ConfigValue::Map(
            node.children
                .iter()
                .filter_map(|(segment, child)| {
                    materialize(child).map(|v| ((*segment).to_string(), v))
                })
                .collect(),
        )),
    }
}

fn build_list(node: &Node<'_>, seed: &[ConfigValue], len: usize) -> Vec<ConfigValue> {
    (0..len)
        .map(|i| {
            let overlay = node
                .children
                .get(i.to_string().as_str())
                .and_then(materialize);
            match (seed.get(i), overlay) {
                (Some(base), Some(overlay)) => merge(base.clone(), overlay),
                (Some(base), None) => base.clone(),
                (None, Some(overlay)) => overlay,
                (None, None) => ConfigValue::Null,
            }
        })
        .collect()
}

/// Deep-merge `overlay` over `base`: maps merge key by key (keys only in
/// `base` survive — e.g. empty sub-maps that flatten to no key), lists merge
/// element by element, anything else is replaced by `overlay`.
fn merge(base: ConfigValue, overlay: ConfigValue) -> ConfigValue {
    match (base, overlay) {
        (ConfigValue::Map(mut base), ConfigValue::Map(overlay)) => {
            for (key, value) in overlay {
                let merged = match base.remove(&key) {
                    Some(existing) => merge(existing, value),
                    None => value,
                };
                base.insert(key, merged);
            }
            ConfigValue::Map(base)
        }
        (ConfigValue::List(base), ConfigValue::List(overlay)) => {
            let len = base.len().max(overlay.len());
            let mut base = base.into_iter();
            let mut overlay = overlay.into_iter();
            ConfigValue::List(
                (0..len)
                    .map(|_| match (base.next(), overlay.next()) {
                        (Some(b), Some(o)) => merge(b, o),
                        (Some(b), None) => b,
                        (None, Some(o)) => o,
                        (None, None) => ConfigValue::Null,
                    })
                    .collect(),
            )
        }
        (_, overlay) => overlay,
    }
}

/// A decimal list index: ASCII digits only, no sign, no leading zero.
fn parse_index(segment: &str) -> Option<usize> {
    let canonical = !segment.is_empty()
        && segment.bytes().all(|b| b.is_ascii_digit())
        && (segment == "0" || !segment.starts_with('0'));
    if canonical {
        segment.parse().ok()
    } else {
        None
    }
}

/// Sorted indices forming exactly `0..n`.
fn is_dense(indices: &[usize]) -> bool {
    indices.iter().enumerate().all(|(i, &idx)| i == idx)
}

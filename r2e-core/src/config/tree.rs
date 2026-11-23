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

use super::loader::{KeyPaths, Segment};
use super::value::ConfigValue;

/// A node of the key trie: the value stored at the exact key (if any) and the
/// child segments below it.
#[derive(Default)]
struct Node<'a> {
    value: Option<&'a ConfigValue>,
    children: BTreeMap<&'a str, Node<'a>>,
    /// A YAML mapping key that reads as a list index (`"0"`) lives here: the
    /// node is a map whatever its children look like.
    keyed: bool,
}

impl<'a> Node<'a> {
    /// Insert a key by the YAML path that produced it.
    fn insert_path(&mut self, path: &'a [Segment], value: &'a ConfigValue) {
        let mut node = self;
        for segment in path {
            if segment.key && is_index(&segment.text) {
                node.keyed = true;
            }
            node = node.children.entry(segment.text.as_str()).or_default();
        }
        node.value = Some(value);
    }

    /// Insert a key known only by its dotted form (plain YAML key, `R2E_`
    /// overlay, provider, `set`). Segments split on `.`, except that an
    /// existing child whose own name contains dots (a YAML map key such as
    /// `gpt-4.1`) is followed when the rest of the key starts with it — the
    /// longest such child wins.
    fn insert_dotted(&mut self, mut rest: &'a str, value: &'a ConfigValue) {
        let mut node = self;
        loop {
            let dotted_child = node
                .children
                .keys()
                .copied()
                .filter(|name| {
                    name.contains('.')
                        && rest
                            .strip_prefix(name)
                            .is_some_and(|tail| tail.is_empty() || tail.starts_with('.'))
                })
                .max_by_key(|name| name.len());
            let segment = match dotted_child {
                Some(name) => &rest[..name.len()],
                None => rest.split('.').next().unwrap_or(rest),
            };
            node = node.children.entry(segment).or_default();
            match rest[segment.len()..].strip_prefix('.') {
                Some(tail) => rest = tail,
                None => break,
            }
        }
        node.value = Some(value);
    }
}

/// The part of a recorded path below `prefix`, or `None` when the prefix does
/// not end on a segment boundary of that path (`a.gpt-4` against the key
/// `a."gpt-4.1"`): the key is then not under the prefix at all.
fn below<'a>(path: &'a [Segment], prefix: &str) -> Option<&'a [Segment]> {
    let mut consumed = 0;
    for (i, segment) in path.iter().enumerate() {
        consumed += segment.text.len();
        if consumed == prefix.len() {
            return Some(&path[i + 1..]);
        }
        if consumed > prefix.len() {
            return None;
        }
        consumed += 1; // the joining dot
    }
    None
}

/// Rebuild the subtree under `prefix` (the whole config for `None`).
///
/// Returns `None` when nothing lives at or under the prefix. See
/// [`R2eConfig::tree`](super::R2eConfig::tree) for the full semantics.
pub(crate) fn build(
    values: &HashMap<String, ConfigValue>,
    paths: &KeyPaths,
    prefix: Option<&str>,
) -> Option<ConfigValue> {
    let mut root = Node::default();
    let dotted = prefix.map(|prefix| format!("{prefix}."));
    if let Some(prefix) = prefix {
        root.value = values.get(prefix);
    }
    let under = |key: &str| match &dotted {
        Some(dotted) => key.starts_with(dotted.as_str()),
        None => true,
    };

    // Recorded YAML paths first: they create the dotted children the plain
    // keys below are matched against.
    for (key, path) in paths {
        let Some(value) = values.get(key).filter(|_| under(key)) else {
            continue;
        };
        let path = match prefix {
            Some(prefix) => below(path, prefix),
            None => Some(path.as_slice()),
        };
        if let Some(path) = path.filter(|path| !path.is_empty()) {
            root.insert_path(path, value);
        }
    }
    for (key, value) in values {
        if paths.contains_key(key) {
            continue;
        }
        let rest = match &dotted {
            Some(dotted) => key.strip_prefix(dotted.as_str()),
            None => Some(key.as_str()),
        };
        if let Some(rest) = rest {
            root.insert_dotted(rest, value);
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

    let indices = if node.keyed { None } else { indices };

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

pub(crate) fn is_index(segment: &str) -> bool {
    parse_index(segment).is_some()
}

/// Sorted indices forming exactly `0..n`.
fn is_dense(indices: &[usize]) -> bool {
    indices.iter().enumerate().all(|(i, &idx)| i == idx)
}

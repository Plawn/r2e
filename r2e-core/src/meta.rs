use serde::Serialize;
use serde_json::Value;
use std::any::{Any, TypeId};
use std::collections::HashMap;

/// A generic, type-erased metadata registry.
///
/// Plugins register typed consumers via
/// [`AppBuilder::with_meta_consumer`](crate::builder::AppBuilder::with_meta_consumer),
/// and controllers push metadata into the registry via
/// [`Controller::register_meta`](crate::controller::Controller::register_meta).
///
/// Internally stores `Vec<M>` per type, keyed by `TypeId`.
#[derive(Default)]
pub struct MetaRegistry {
    inner: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl MetaRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Push a single metadata item into the registry.
    pub fn push<M: Any + Send + Sync>(&mut self, item: M) {
        self.entry::<M>().push(item);
    }

    /// Extend the registry with multiple metadata items.
    pub fn extend<M: Any + Send + Sync>(&mut self, items: impl IntoIterator<Item = M>) {
        self.entry::<M>().extend(items);
    }

    /// Take all metadata of a given type, leaving the slot empty.
    pub fn take<M: Any + Send + Sync>(&mut self) -> Vec<M> {
        self.inner
            .remove(&TypeId::of::<M>())
            .and_then(|boxed| boxed.downcast::<Vec<M>>().ok())
            .map(|v| *v)
            .unwrap_or_default()
    }

    /// Get a shared reference to all metadata of a given type.
    pub fn get<M: Any + Send + Sync>(&self) -> Option<&[M]> {
        self.inner
            .get(&TypeId::of::<M>())
            .and_then(|boxed| boxed.downcast_ref::<Vec<M>>())
            .map(|v| v.as_slice())
    }

    /// Get a shared reference to all metadata of a given type, or an empty slice.
    pub fn get_or_empty<M: Any + Send + Sync>(&self) -> &[M] {
        self.get::<M>().unwrap_or(&[])
    }

    /// Get or create the `Vec<M>` entry for a given type.
    fn entry<M: Any + Send + Sync>(&mut self) -> &mut Vec<M> {
        self.inner
            .entry(TypeId::of::<M>())
            .or_insert_with(|| Box::new(Vec::<M>::new()))
            .downcast_mut::<Vec<M>>()
            .expect("MetaRegistry: type mismatch (should be impossible)")
    }
}

// ── Metadata types (moved from openapi.rs) ──────────────────────────────────

/// Metadata about a single route, collected at compile time.
#[derive(Debug, Clone, Serialize)]
pub struct RouteInfo {
    pub path: String,
    pub method: String,
    pub operation_id: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub request_body_type: Option<String>,
    pub request_body_schema: Option<Value>,
    pub request_body_required: bool,
    pub response_type: Option<String>,
    pub response_schema: Option<Value>,
    pub response_status: u16,
    pub params: Vec<ParamInfo>,
    pub roles: Vec<String>,
    pub tag: Option<String>,
    pub deprecated: bool,
    pub has_auth: bool,
}

/// Metadata about a route parameter.
#[derive(Debug, Clone, Serialize)]
pub struct ParamInfo {
    pub name: String,
    pub location: ParamLocation,
    pub param_type: String,
    pub required: bool,
}

/// Where a parameter is located in the HTTP request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ParamLocation {
    Path,
    Query,
    Header,
}

//! Typed OpenFGA client — the idiomatic write path.
//!
//! [`FgaClient`] is a clonable bean façade over [`OpenFgaRegistry`] that
//! carries the schema-first guarantees of the `model!`-generated markers into
//! tuple management:
//!
//! - [`grant`](FgaClient::grant) / [`revoke`](FgaClient::revoke) compile only
//!   if the model's `directly_related_user_types` allows that subject type on
//!   that relation ([`DirectlyAssignable`]), and invalidate the decision
//!   cache for the touched object (write-through invalidation).
//! - [`check`](FgaClient::check) is the typed handler-level check (cached via
//!   the registry) for objects known only after e.g. a DB lookup.
//!
//! There is deliberately no `list_objects`: OpenFGA's `ListObjects` response
//! carries no truncation signal (server-side `OPENFGA_LIST_OBJECTS_MAX_RESULTS`
//! / deadline silently return a partial list), so a typed wrapper would look
//! exhaustive without being it. For list filtering, paginate your own objects
//! and `check` them, or drop to the raw client knowingly.
//!
//! ```ignore
//! r2e_openfga::model!(pub mod authz = "fga/model.fga");
//!
//! let alice = authz::user::id("alice");
//! let doc = authz::document::id("readme");
//!
//! fga.grant(&alice, authz::document::viewer, &doc).await?;
//! assert!(fga.check(&alice, authz::document::viewer, &doc).await?);
//! fga.revoke(&alice, authz::document::viewer, &doc).await?;
//!
//! // Userset / wildcard subjects, when the model allows them:
//! fga.grant(&authz::team::member.of(authz::team::id("eng")), authz::document::viewer, &doc).await?;
//! fga.grant(&authz::user::wildcard(), authz::document::viewer, &doc).await?;
//! ```
//!
//! Injection guards hold by construction: every subject/object reaching the
//! wire was built through [`FgaObject::try_new`]/[`new`](FgaObject::new)
//! (which reject `:`, `#`, `*` in ids) or rendered from model-declared
//! names. For batch or conditional writes, drop down to
//! [`GrpcBackend::client()`](crate::backend::GrpcBackend::client).

use crate::error::OpenFgaError;
use crate::registry::OpenFgaRegistry;
use crate::typed::{DirectlyAssignable, FgaObject, FgaRel, FgaSubject, FgaType};

/// Typed OpenFGA client bean. Cheap to clone (shares the registry's backend
/// and cache); provide it from the registry:
///
/// ```ignore
/// #[producer]
/// async fn fga_client(registry: OpenFgaRegistry) -> FgaClient {
///     FgaClient::new(registry)
/// }
/// ```
#[derive(Clone)]
pub struct FgaClient {
    registry: OpenFgaRegistry,
}

impl FgaClient {
    /// Wrap a registry. Writes go to the registry's backend and invalidate
    /// its decision cache, so guards and handler checks see them immediately.
    pub fn new(registry: OpenFgaRegistry) -> Self {
        Self { registry }
    }

    /// Write the tuple `(subject, rel, object)`.
    ///
    /// Compiles only if the model lists the subject's type in the relation's
    /// `directly_related_user_types` — granting a disallowed subject type is
    /// a compile error, not a server rejection. On success the decision
    /// cache is invalidated for `object`.
    ///
    /// OpenFGA `Write` semantics apply: granting an already-existing tuple
    /// is a server error ([`OpenFgaError::ServerError`]), not a no-op.
    ///
    /// Only **direct** decisions on `object` are invalidated; a grant that
    /// affects other objects transitively (e.g. granting `team#member` used
    /// by many documents) leaves their cached decisions until TTL expiry —
    /// call [`OpenFgaRegistry::clear_cache`] after such structural changes.
    pub async fn grant<S, T, R>(
        &self,
        subject: &S,
        rel: FgaRel<T, R>,
        object: &FgaObject<T>,
    ) -> Result<(), OpenFgaError>
    where
        S: FgaSubject,
        T: FgaType,
        R: DirectlyAssignable<S::Marker>,
    {
        self.registry
            .backend()
            .write_tuple(subject.subject_str(), rel.name(), object.as_str())
            .await?;
        self.registry.invalidate_object(object.as_str());
        Ok(())
    }

    /// Delete the tuple `(subject, rel, object)`. Same compile-time subject
    /// bound and write-through cache invalidation as [`grant`](Self::grant).
    ///
    /// OpenFGA `Write` semantics apply: revoking a tuple that does not exist
    /// is a server error, not a no-op.
    pub async fn revoke<S, T, R>(
        &self,
        subject: &S,
        rel: FgaRel<T, R>,
        object: &FgaObject<T>,
    ) -> Result<(), OpenFgaError>
    where
        S: FgaSubject,
        T: FgaType,
        R: DirectlyAssignable<S::Marker>,
    {
        self.registry
            .backend()
            .delete_tuple(subject.subject_str(), rel.name(), object.as_str())
            .await?;
        self.registry.invalidate_object(object.as_str());
        Ok(())
    }

    /// Check whether `subject` has `rel` to `object`, through the registry
    /// (cached when the registry has caching enabled).
    ///
    /// Unlike the writes, this has no [`DirectlyAssignable`] bound: checks
    /// legitimately target **computed** relations (e.g. a `viewer` implied
    /// by `editor`) that no tuple can be written for.
    pub async fn check<S, T, R>(
        &self,
        subject: &S,
        rel: FgaRel<T, R>,
        object: &FgaObject<T>,
    ) -> Result<bool, OpenFgaError>
    where
        S: FgaSubject,
        T: FgaType,
    {
        self.registry
            .check(subject.subject_str(), rel.name(), object.as_str())
            .await
    }
}

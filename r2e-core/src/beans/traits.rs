use super::{BeanContext, BeanRegistry, BootError};
use crate::config::ConfigKeyKind;
use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;

// ── Traits ──────────────────────────────────────────────────────────────────

/// Marker trait for types that can be auto-constructed from a [`BeanContext`].
///
/// Implement this trait (or use `#[derive(Bean)]` / `#[bean]`) to declare
/// a type as a bean that the [`BeanRegistry`] can resolve automatically.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as a Bean",
    label = "this type is not a bean",
    note = "add `#[derive(Bean)]` to your type or implement the `Bean` trait manually"
)]
pub trait Bean: Clone + Send + Sync + 'static {
    /// Type-level list of dependency types required to construct this bean.
    ///
    /// Generated automatically by `#[bean]` and `#[derive(Bean)]`.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// How this bean's constructor can fail.
    ///
    /// Use [`Infallible`](std::convert::Infallible) for a constructor that
    /// cannot fail — the overwhelmingly common case, and what `#[bean]`
    /// generates for a plain `fn new(..) -> Self`. A constructor declared as
    /// `fn new(..) -> Result<Self, E>` gets `type Error = E`; the **bean type
    /// stays `Self`**, the error never contaminates what consumers inject.
    ///
    /// The first failing bean aborts `build_state()` with
    /// [`BeanError::BeanBuild`](super::BeanError::BeanBuild) naming this type;
    /// beans already built in that cycle are dropped as the stack unwinds.
    type Error: Into<BootError>;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    ///
    /// `Option<T>` fields are **hard** dependencies on `Option<T>` (the
    /// whole type, not `T`). A producer must register an `Option<T>` value
    /// in the context for this bean to resolve. See the module docs for
    /// the conditional-bean pattern using `#[producer] -> Option<T>`.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys referenced by this bean as
    /// `(key, type_name, kind)` triples.
    ///
    /// Used by [`BeanRegistry::resolve`] to validate config presence and, under
    /// `dev-reload`, to fingerprint the config values a bean depends on. The
    /// [`ConfigKeyKind`] decides both:
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required`, [`Optional`](ConfigKeyKind::Optional) and
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted (editing any
    /// of them rebuilds the bean under `r2e dev`); and
    /// [`Live`](ConfigKeyKind::Live) keys — `#[live_config]` — are neither:
    /// they are pushed into the bean's `LiveConfig` handle instead. A `Section`
    /// entry's key is a dotted **prefix** (`#[config_section(prefix = "…")]`),
    /// so the whole subtree under it is fingerprinted. The default
    /// implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
        Vec::new()
    }

    /// When `true`, construction is deferred until first injection.
    /// Set by `#[bean(lazy)]`.
    ///
    /// Lazy beans are **not** constructed during `build_state()`. Instead,
    /// a lazy slot is placed in the context and the bean is built on the
    /// first `get::<Self>()` call (construct-on-first-injection, like
    /// Quarkus CDI).
    ///
    /// **Runtime note:** lazy resolution needs a Tokio multi-thread runtime.
    /// Enable the `lazy-fallback-runtime` feature to allow a fallback runtime
    /// when none is available (or when running on a current-thread runtime).
    ///
    /// Consumers use `Self` directly — no wrapper type needed.
    /// Register with `.register::<T>()`
    /// as usual; the builder auto-detects the `LAZY` flag.
    const LAZY: bool = false;

    /// A version stamp derived from the constructor's source tokens.
    ///
    /// The `#[bean]` and `#[derive(Bean)]` macros hash the constructor body /
    /// struct fields at compile time, so a code change automatically bumps this
    /// value. Used by the dev-reload granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your constructor.
    /// If you implement `Bean` manually and want hot-reload to pick up changes,
    /// override this constant and bump it whenever you modify the `build` logic:
    ///
    /// ```ignore
    /// impl Bean for MyService {
    ///     const BUILD_VERSION: u64 = 2; // bump when build() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Construct the bean from a fully resolved context.
    ///
    /// An infallible constructor returns `Ok(..)` with
    /// `type Error = std::convert::Infallible`.
    fn build(ctx: &BeanContext) -> Result<Self, Self::Error>;

    /// Called after registration to allow post-processing (e.g., registering
    /// post-construct hooks). The default is a no-op.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Trait for beans that require async initialization (e.g. DB pools, HTTP clients).
///
/// Use `#[bean]` on an `impl` block with an `async fn new(...)` constructor,
/// or implement this trait manually. Register with `.register::<T>()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as an AsyncBean",
    label = "this type is not an async bean",
    note = "add `#[bean]` to your impl block with an `async fn` constructor, or implement `AsyncBean` manually"
)]
pub trait AsyncBean: Clone + Send + Sync + 'static {
    /// Type-level list of dependency types required to construct this bean.
    ///
    /// Generated automatically by `#[bean]` on async constructors.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// How this bean's async constructor can fail. See [`Bean::Error`] —
    /// the rules are identical (`async fn new(..) -> Result<Self, E>` gives
    /// `type Error = E`, and the bean is still registered as `Self`).
    type Error: Into<BootError>;

    /// When `true`, construction is deferred until first injection.
    /// Set by `#[bean(lazy)]`. See [`Bean::LAZY`] for details.
    const LAZY: bool = false;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to construct this bean.
    ///
    /// `Option<T>` fields are **hard** dependencies on `Option<T>` (the
    /// whole type, not `T`). A producer must register an `Option<T>` value
    /// in the context for this bean to resolve.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys referenced by this bean as
    /// `(key, type_name, kind)` triples. Only
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required` + [`Optional`](ConfigKeyKind::Optional) +
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted under
    /// `dev-reload` (a `Section` key is a dotted **prefix**, and covers the
    /// whole subtree under it), [`Live`](ConfigKeyKind::Live) keys are not. The
    /// default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
        Vec::new()
    }

    /// A version stamp derived from the constructor's source tokens.
    ///
    /// The `#[bean]` macro hashes the async constructor body at compile time,
    /// so a code change automatically bumps this value. Used by the dev-reload
    /// granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your constructor.
    /// Override this constant and bump it when you modify `build` logic:
    ///
    /// ```ignore
    /// impl AsyncBean for MyPool {
    ///     const BUILD_VERSION: u64 = 3; // bump when build() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Construct the bean asynchronously from a fully resolved context.
    ///
    /// **The returned future is deliberately not required to be `Send`.** The
    /// bean graph is built and awaited in place on the boot thread, so nothing
    /// moves a constructor future across threads. A `+ Send` bound here would
    /// be checked for *all* lifetimes and break perfectly ordinary bodies —
    /// notably sqlx's `&mut *tx` executors, which fail with
    /// `error: lifetime bound not satisfied` / `implementation of Executor is
    /// not general enough` (rust-lang/rust#100013). See the "async
    /// constructors are not `Send`-bound" note in `llm.txt`.
    fn build(ctx: &BeanContext) -> impl Future<Output = Result<Self, Self::Error>> + '_;

    /// Called after registration to allow post-processing (e.g., registering
    /// post-construct hooks). The default is a no-op.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Trait for producer functions that create types you don't own
/// (e.g. `SqlitePool`, third-party clients).
///
/// Use the `#[producer]` attribute macro on a free function to generate
/// this implementation automatically. Register with `.register::<P>()`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not registered as a Producer",
    label = "this type is not a producer",
    note = "add `#[producer]` to a free function that returns the desired type"
)]
pub trait Producer: Send + 'static {
    /// The type this producer creates.
    type Output: Clone + Send + Sync + 'static;

    /// Type-level list of dependency types required to produce the output.
    ///
    /// Generated automatically by `#[producer]`.
    /// For manual impls without dependencies, use `type Deps = TNil;`.
    type Deps;

    /// How this producer can fail.
    ///
    /// A `#[producer]` function declared as `-> Result<T, E>` produces the
    /// bean `T` with `type Error = E`: the **`Output`, not the `Result`, is
    /// what gets registered**, so consumers inject `T` and never see the
    /// error type. An infallible producer uses
    /// [`Infallible`](std::convert::Infallible).
    ///
    /// (Conditional availability is a different axis and still travels in the
    /// `Output`: `-> Option<T>` registers `Option<T>`, and
    /// `-> Result<Option<T>, E>` registers `Option<T>` with a failure channel.)
    type Error: Into<BootError>;

    /// Returns the [`TypeId`]s and type names of all dependencies needed
    /// to produce the output.
    ///
    /// `Option<T>` parameters are **hard** dependencies on `Option<T>`.
    fn dependencies() -> Vec<(TypeId, &'static str)>;

    /// Returns the config keys referenced by this producer as
    /// `(key, type_name, kind)` triples. Only
    /// [`Required`](ConfigKeyKind::Required) keys are presence-validated;
    /// `Required` + [`Optional`](ConfigKeyKind::Optional) +
    /// [`Section`](ConfigKeyKind::Section) keys are fingerprinted under
    /// `dev-reload` (a `Section` key is a dotted **prefix**, and covers the
    /// whole subtree under it), [`Live`](ConfigKeyKind::Live) keys are not. The
    /// default implementation returns an empty list.
    fn config_keys() -> Vec<(&'static str, &'static str, ConfigKeyKind)> {
        Vec::new()
    }

    /// A version stamp derived from the producer function's source tokens.
    ///
    /// The `#[producer]` macro hashes the function body at compile time,
    /// so a code change automatically bumps this value. Used by the dev-reload
    /// granular bean cache to detect code changes.
    ///
    /// **Manual implementations:** The default value is `0`, which means the
    /// dev-reload system will **not** detect code changes in your producer.
    /// Override this constant and bump it when you modify `produce` logic:
    ///
    /// ```ignore
    /// impl Producer for MyProducer {
    ///     const BUILD_VERSION: u64 = 1; // bump when produce() changes
    ///     // ...
    /// }
    /// ```
    const BUILD_VERSION: u64 = 0;

    /// Produce the output from a fully resolved context.
    ///
    /// To express conditional availability (a bean that may or may not be
    /// present depending on config), declare `type Output = Option<T>` and
    /// return `Some(...)` / `None`. The whole `Option<T>` is registered as
    /// a bean — consumers inject `Option<T>` as a hard dependency.
    ///
    /// **The returned future is deliberately not required to be `Send`** —
    /// same reasoning as [`AsyncBean::build`].
    fn produce(ctx: &BeanContext) -> impl Future<Output = Result<Self::Output, Self::Error>> + '_;

    /// Called after registration to allow post-processing.
    fn after_register(_registry: &mut BeanRegistry) {}
}

/// Lifecycle hook called after all beans have been constructed.
///
/// Implement this trait (typically via `#[post_construct]` on a `#[bean]`
/// method) to run initialization logic that requires the fully assembled bean.
/// Per-bean fingerprint entries — `(type id, type name, fingerprint)` — used
/// by the dev-reload graph cache to log which beans changed.
#[cfg(feature = "dev-reload")]
pub type BeanFingerprints = Vec<(TypeId, &'static str, u64)>;

/// `Clone` is **not** a supertrait: a bean's post-construct hook is registered
/// via [`BeanRegistry::register_post_construct`] /
/// [`register_provided_post_construct`](BeanRegistry::register_provided_post_construct)
/// (which pull the bean by value from the graph, so they bound `Clone` there),
/// while a controller core — which is not `Clone` — impls this trait too and is
/// run from its own `Arc` at startup.
pub trait PostConstruct: Send + Sync + 'static {
    fn post_construct(&self) -> crate::runtime::lifecycle::LifecycleFuture<'_>;
}

/// Disposal hook, the symmetric counterpart of [`PostConstruct`].
///
/// Implement this trait to run cleanup logic (close pools, flush buffers,
/// cancel background work) during the server's graceful-shutdown sequence.
/// A `PreDestroy` hook is invoked against the bean **as it lives in the
/// resolved graph** (override included), and runs as part of the async
/// shutdown phase — see
/// [`AppBuilder::provide_with_pre_destroy`](crate::AppBuilder::provide_with_pre_destroy)
/// and [`PluginSetupContext::run_pre_destroy`](crate::PluginSetupContext::run_pre_destroy)
/// for the invocation order.
pub trait PreDestroy: Clone + Send + Sync + 'static {
    fn pre_destroy(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// One `#[on_start]` hook, already bound to the bean (or controller core) it
/// belongs to: calling it produces the future that runs the hook body.
///
/// Boxed and owned (rather than borrowing `&self`) so the builder can collect
/// every hook of every bean into one list, sort that list by declared order,
/// and only then start awaiting them.
pub type OnStartHook = Box<
    dyn FnOnce() -> Pin<
            Box<dyn Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>> + Send>,
        > + Send,
>;

/// Startup observer: the bean-level counterpart of the builder's
/// [`AppBuilder::on_start`](crate::AppBuilder::on_start) closure.
///
/// Implemented automatically by `#[bean]` for every impl carrying at least one
/// `#[on_start]` method. Unlike [`PostConstruct`] — which runs *inside*
/// `build_state()`, while the graph is still being assembled — an `#[on_start]`
/// hook runs at server startup, once the whole graph **and** all controller
/// cores exist: it is the first place a bean may safely observe the fully
/// assembled application.
///
/// Hooks are collected across all beans and controllers, sorted by their
/// declared `order` (ascending, ties in registration order), and awaited in
/// sequence. An `Err` aborts boot exactly like an `Err` from a builder
/// `on_start` closure.
///
/// A hook is invoked against the bean **as it lives in the resolved graph**
/// (override included) — and pinning the bean itself with `override_bean`
/// skips its registration entirely, so its hooks never run.
pub trait OnStart: Clone + Send + Sync + 'static {
    /// This bean's `#[on_start]` hooks as `(order, hook)` pairs, in
    /// declaration order.
    fn on_start_hooks(&self) -> Vec<(i32, OnStartHook)>;
}

/// Unified registration entry point for beans, async beans, and producers.
///
/// Implemented automatically by `#[bean]`, `#[derive(Bean)]`, and `#[producer]`
/// as an inherent per-type impl (never a blanket impl, to avoid overlap). It
/// lets [`AppBuilder::register`](crate::AppBuilder::register) register any of
/// the three registration kinds through a single method:
///
/// - `#[bean]` (sync) / `#[derive(Bean)]` → `Provided = Self`,
///   `Deps = <Self as Bean>::Deps`.
/// - `#[bean]` (async) → `Provided = Self`, `Deps = <Self as AsyncBean>::Deps`.
/// - `#[producer]` → `Provided = <Self as Producer>::Output`,
///   `Deps = <Self as Producer>::Deps`.
pub trait Registrable {
    /// The type made available in the [`BeanContext`] once registered.
    ///
    /// For beans this is `Self`; for producers it is the producer's `Output`.
    /// Tracked in the builder's compile-time provision list.
    type Provided: Clone + Send + Sync + 'static;

    /// The type-level list of dependency types required to construct the value.
    type Deps;

    /// Register this type into the given [`BeanRegistry`].
    fn register_into(registry: &mut BeanRegistry);
}

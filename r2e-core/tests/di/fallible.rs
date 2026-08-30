//! Fallible bean construction: `Bean`/`AsyncBean`/`Producer` carry an
//! associated `Error`, so an app whose boot can legitimately fail (a pool that
//! will not connect, a lock already held, a secret that is missing) reports it
//! instead of panicking or calling `process::exit` from library code.
//!
//! Covers the graph-side acceptance criteria: the error type never
//! contaminates the bean type consumers inject, the first failure aborts
//! `build_state()` naming the faulty bean, and beans already built in that
//! cycle are dropped on the failure path.

use std::any::{type_name, TypeId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use r2e_core::beans::{AsyncBean, Bean, BeanContext, Producer, Registrable};
use r2e_core::prelude::*;
use r2e_core::type_list::TNil;

// ── A boot error with a source chain, like a real driver error ─────────────

#[derive(Debug)]
struct ConnectFailed(&'static str);

impl std::fmt::Display for ConnectFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "could not connect to {}", self.0)
    }
}

impl std::error::Error for ConnectFailed {}

// ── 1. A producer may fail without the error contaminating the bean ────────

/// The bean consumers inject. Note the absence of any `Result` here: the
/// producer below declares `-> Result<DbPool, ConnectFailed>` and the graph
/// still registers `DbPool`.
#[derive(Clone)]
struct DbPool(&'static str);

#[producer]
async fn connect_pool() -> Result<DbPool, ConnectFailed> {
    Ok(DbPool("sqlite::memory:"))
}

/// A plain consumer: it injects `DbPool`, not `Result<DbPool, _>`. If the
/// error leaked into the registered type this would not compile.
#[derive(Clone)]
struct Repo {
    url: &'static str,
}

#[bean]
impl Repo {
    fn new(pool: DbPool) -> Self {
        Self { url: pool.0 }
    }
}

#[r2e_core::test]
async fn producer_error_type_does_not_contaminate_the_bean_type() {
    let state = AppBuilder::new()
        .register::<ConnectPool>()
        .register::<Repo>()
        .try_build_state()
        .await
        .expect("graph resolves");

    let ctx = state.bean_context();
    assert_eq!(ctx.get::<DbPool>().0, "sqlite::memory:");
    assert_eq!(ctx.get::<Repo>().url, "sqlite::memory:");
}

// ── 2. The first failure aborts the build, naming the bean ─────────────────

struct FailingPool;

impl Producer for FailingPool {
    type Output = DbPool;
    type Deps = TNil;
    type Error = ConnectFailed;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![]
    }

    async fn produce(_ctx: &BeanContext) -> Result<Self::Output, Self::Error> {
        Err(ConnectFailed("postgres://nowhere"))
    }
}

impl Registrable for FailingPool {
    type Provided = DbPool;
    type Deps = TNil;

    fn register_into(registry: &mut r2e_core::beans::BeanRegistry) {
        registry.register_producer::<Self>();
    }
}

#[r2e_core::test]
async fn a_failing_producer_aborts_the_build_naming_the_bean() {
    let err = AppBuilder::new()
        .register::<FailingPool>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("the pool refuses to connect");

    let rendered = err.to_string();
    assert!(
        rendered.contains(type_name::<DbPool>()),
        "the error must name the bean that failed: {rendered}"
    );
    assert!(
        rendered.contains("could not connect to postgres://nowhere"),
        "the error must carry the cause: {rendered}"
    );
    // The cause stays reachable as a `source()`, so `launch!` can print the
    // whole chain rather than one flattened string.
    let source = std::error::Error::source(&err).expect("source chain preserved");
    assert!(source.to_string().contains("postgres://nowhere"));
}

// ── 3. Beans already built are dropped on the failure path ─────────────────

/// Increments the shared counter when the last clone goes away, so the
/// assertion is about the value being released, not about clone traffic.
struct DropProbe(Arc<AtomicUsize>);

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
struct EarlyBean(#[allow(dead_code)] Arc<DropProbe>);

/// The counter the probe reports to. A plain provided value, so the test keeps
/// its own handle without keeping `EarlyBean` alive.
#[derive(Clone)]
struct DropCounter(Arc<AtomicUsize>);

impl Bean for EarlyBean {
    type Deps = TNil;
    type Error = std::convert::Infallible;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<DropCounter>(), type_name::<DropCounter>())]
    }

    fn build(ctx: &BeanContext) -> Result<Self, Self::Error> {
        Ok(EarlyBean(Arc::new(DropProbe(ctx.get::<DropCounter>().0))))
    }
}

impl Registrable for EarlyBean {
    type Provided = EarlyBean;
    type Deps = TNil;

    fn register_into(registry: &mut r2e_core::beans::BeanRegistry) {
        registry.register::<Self>();
    }
}

/// Fails, and depends on `EarlyBean` so the topological order guarantees the
/// early bean is already in the context when this one blows up.
#[derive(Clone)]
struct LateBean;

impl AsyncBean for LateBean {
    type Deps = TNil;
    type Error = ConnectFailed;

    fn dependencies() -> Vec<(TypeId, &'static str)> {
        vec![(TypeId::of::<EarlyBean>(), type_name::<EarlyBean>())]
    }

    async fn build(ctx: &BeanContext) -> Result<Self, Self::Error> {
        let _early = ctx.get::<EarlyBean>();
        Err(ConnectFailed("late-bean"))
    }
}

impl Registrable for LateBean {
    type Provided = LateBean;
    type Deps = TNil;

    fn register_into(registry: &mut r2e_core::beans::BeanRegistry) {
        registry.register_async::<Self>();
    }
}

#[r2e_core::test]
async fn already_built_beans_are_dropped_on_the_failure_path() {
    let counter = Arc::new(AtomicUsize::new(0));

    let err = AppBuilder::new()
        .provide(DropCounter(counter.clone()))
        .register::<EarlyBean>()
        .register::<LateBean>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("the late bean fails");

    assert!(err.to_string().contains(type_name::<LateBean>()));
    // `process::exit` would have skipped this entirely — the whole point of
    // routing the failure through a `Result` and letting the stack unwind.
    assert_eq!(
        counter.load(Ordering::SeqCst),
        1,
        "the bean built before the failure must be dropped"
    );
}

// ── 4. `#[bean]` accepts a fallible constructor ────────────────────────────

#[derive(Clone)]
struct InstanceGuard;

#[bean]
impl InstanceGuard {
    fn new() -> Result<Self, ConnectFailed> {
        Err(ConnectFailed("advisory lock already held"))
    }
}

#[r2e_core::test]
async fn a_fallible_bean_constructor_aborts_the_build() {
    let err = AppBuilder::new()
        .register::<InstanceGuard>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("the lock is held");

    let rendered = err.to_string();
    assert!(
        rendered.contains(type_name::<InstanceGuard>()),
        "{rendered}"
    );
    assert!(
        rendered.contains("advisory lock already held"),
        "{rendered}"
    );
}

#[derive(Clone)]
struct AsyncGuard;

#[bean]
impl AsyncGuard {
    async fn new() -> Result<Self, ConnectFailed> {
        Err(ConnectFailed("async advisory lock"))
    }
}

#[r2e_core::test]
async fn a_fallible_async_bean_constructor_aborts_the_build() {
    let err = AppBuilder::new()
        .register::<AsyncGuard>()
        .try_build_state()
        .await
        .map(|_| ())
        .expect_err("the lock is held");

    assert!(err.to_string().contains(type_name::<AsyncGuard>()));
}

// ── 5. What counts as a fallible return, exactly ───────────────────────────
//
// The macros look at ONE thing: is the declared return type a two-argument
// `Result<_, _>` (any path spelling)? Everything else is the bean type,
// verbatim — no nesting is unwrapped and no alias is resolved (a macro sees
// tokens, not types). These are the corner cases the rule is easy to get
// wrong on, and the shapes `docs/claude/beans-di.md` documents.

/// Fully-qualified `std::result::Result` — recognised like bare `Result`,
/// since only the LAST path segment is matched.
#[derive(Clone, Debug, PartialEq)]
struct QualifiedPool(&'static str);

#[producer]
async fn make_qualified_pool() -> std::result::Result<QualifiedPool, ConnectFailed> {
    Ok(QualifiedPool("qualified"))
}

/// `Result<Option<T>, E>`: the `Result` is unwrapped (once), the `Option` is
/// not — `Option<T>` is a first-class bean type in its own right.
#[derive(Clone, Debug, PartialEq)]
struct Feature(&'static str);

#[producer]
async fn make_optional_feature() -> Result<Option<Feature>, ConnectFailed> {
    Ok(Some(Feature("enabled")))
}

/// `Option<Result<T, E>>`: NOT a fallible producer. The outer type is an
/// `Option`, so the whole `Option<Result<..>>` is the bean type and the
/// producer is infallible.
#[derive(Clone, Debug, PartialEq)]
struct Wrapped(&'static str);

#[derive(Clone, Debug, PartialEq)]
struct NotAnError;

#[producer]
async fn make_wrapped() -> Option<Result<Wrapped, NotAnError>> {
    Some(Ok(Wrapped("inner")))
}

/// A single-argument alias is not recognised either: `Fallible<T>` IS
/// `Result<T, E>` to the compiler, but the macro sees a path whose last
/// segment is `Fallible`, so the alias itself becomes the bean type. Spell
/// the error out instead. (`#[bean]` diverges here — it rejects such a
/// constructor outright, since the return is neither `Self` nor a literal
/// `Result<Self, E>`; that rejection is covered in r2e-compile-tests.)
type Fallible<T> = std::result::Result<T, NotAnError>;

#[derive(Clone, Debug, PartialEq)]
struct Aliased(&'static str);

#[producer]
async fn make_aliased() -> Fallible<Aliased> {
    Ok(Aliased("alias"))
}

#[r2e_core::test]
async fn the_macros_unwrap_exactly_one_literal_two_argument_result() {
    let state = AppBuilder::new()
        .register::<MakeQualifiedPool>()
        .register::<MakeOptionalFeature>()
        .register::<MakeWrapped>()
        .register::<MakeAliased>()
        .try_build_state()
        .await
        .expect("graph resolves");
    let ctx = state.bean_context();

    // Qualified `Result`: unwrapped, and `E` landed on `Producer::Error`.
    assert_eq!(ctx.get::<QualifiedPool>(), QualifiedPool("qualified"));
    assert_eq!(
        TypeId::of::<<MakeQualifiedPool as Producer>::Error>(),
        TypeId::of::<ConnectFailed>(),
    );

    // `Result<Option<T>, E>` registers `Option<T>`, not `T`.
    assert_eq!(ctx.get::<Option<Feature>>(), Some(Feature("enabled")));
    assert_eq!(
        TypeId::of::<<MakeOptionalFeature as Producer>::Error>(),
        TypeId::of::<ConnectFailed>(),
    );

    // `Option<Result<T, E>>` is one opaque bean type, produced infallibly.
    assert_eq!(
        ctx.get::<Option<Result<Wrapped, NotAnError>>>(),
        Some(Ok(Wrapped("inner"))),
    );
    assert_eq!(
        TypeId::of::<<MakeWrapped as Producer>::Error>(),
        TypeId::of::<std::convert::Infallible>(),
    );

    // The alias is the bean type, and the producer is infallible.
    assert_eq!(ctx.get::<Fallible<Aliased>>(), Ok(Aliased("alias")));
    assert_eq!(
        TypeId::of::<<MakeAliased as Producer>::Error>(),
        TypeId::of::<std::convert::Infallible>(),
    );
}

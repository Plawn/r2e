pub mod beans;
pub mod builder;
pub mod builtins;
pub mod config;
pub mod controller;
pub mod decorators;
pub mod di;
pub mod error;
pub mod http;
pub mod plugin;
pub mod prelude;
/// Async-runtime facade — re-export of the [`r2e_rt`] crate.
///
/// `r2e-rt` sits at the bottom of the workspace graph (below `r2e-http`), so it
/// is the crate that owns the `tokio` dependency. `r2e_core::rt::…` keeps
/// resolving to exactly what it always did.
pub use r2e_rt as rt;
pub mod runtime;
pub mod state;
pub mod type_list;
pub mod types;
pub mod web;

// Used by macro-generated code (schema construction) so user crates don't
// need a direct serde_json dependency. `Value` / `json!` only — typed
// (de)serialization goes through [`json`].
#[doc(hidden)]
pub use serde_json;

/// The JSON codec façade (`to_vec` / `from_slice` / `JsonError`, …).
///
/// The one place that (de)serializes typed values; the backend is a Cargo
/// feature (`json-sonic`). See `plans/json-codec-containment.md`.
pub use r2e_http::json;

pub use beans::{
    AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, BootError, OnStart, OnStartHook,
    PostConstruct, PreDestroy, Producer,
};
pub use builder::{
    boot_error_report, exit_on_boot_error, launch, launch_with, App, AppBuilder, BootableApp,
    LaunchOptions, PreparedApp,
    RegisterController, RegisterControllers, RegisterModule, RegisterModules, RunningApp,
    ServeContext,
    SpawnService, TaskRegistryHandle,
};
pub use builtins::request_id::{RequestId, RequestIdPlugin};
pub use builtins::secure_headers::SecureHeaders;
pub use builtins::{AdvancedHealth, ConfiguredTracing};
pub use config::{
    deserialize_value, register_section, registered_sections, validate_declared_keys,
    validate_declared_sections, validate_keys, validate_section, ConfigError, ConfigProperties,
    ConfigProvider, ConfigProviderContext, ConfigUpdateSink, ConfigValidationDetail,
    ConfigValidationError, ConfigValue, ConfigWatchContext, DefaultSecretResolver, FromConfigValue,
    LiveConfig, LiveConfigReceiver, LiveConfigRegistry, LiveConfigSnapshot, LoadableConfig,
    MissingKeyError, PluginConfig, PropertyMeta, R2eConfig, RegisteredSection, SecretResolver,
    SectionValidator,
};
pub use controller::{ContextConstruct, Controller, EndpointDeps};
pub use decorators::claims::{Audience, ClientAccess, RealmAccess, StandardClaims};
pub use decorators::decorator::{
    decorator_config_errors, BeanDecoFill, Decorate, DecoratorSpec, HasDecoSlot, SelfBuilt,
    SharedDecoSlot,
};
pub use decorators::guards::{
    default_method, no_extensions, parse_forwarded_ip, ClientIp, Guard, GuardContext, GuardError,
    Identity, NoIdentity, PathParam, PathParams, PreAuthGuard, PreAuthGuardContext,
};
pub use decorators::interceptors::{Cacheable, Interceptor, InterceptorContext};
pub use di::event_subscriber::EventSubscriber;
pub use di::late::Late;
pub use di::lazy::Lazy;
pub use di::meta::MetaRegistry;
pub use di::module::FeatureModule;
pub use di::scheduled_source::ScheduledSource;
pub use error::{HttpError, HttpErrorExt};
pub use plugin::{
    DeferredAction, DeferredContext, GraphHandle, Plugin, PluginBuildContext, PluginBuildError,
    PluginInstall, PluginSetupContext, RoutesContext,
};
pub use runtime::harness::WorkerHarness;
pub use runtime::ingress::{reuseport_supported, reuseport_tcp, reuseport_udp, AffinityError};
pub use runtime::layers::{default_cors, default_trace, init_tracing, init_tracing_with_config};
pub use runtime::lifecycle::{LifecycleController, StopHandle};
pub use runtime::mailbox::{Mailbox, MailboxError, Mailboxes};
pub use runtime::service::ServiceComponent;
pub use runtime::tracing_config::{LogFormat, SpanEvents, TracingConfig};
pub use runtime::worker::{
    PerWorkerServiceFactory, WorkerContext, WorkerInfo, WorkerRole, WorkerService,
};
pub use runtime::worker_local::{WorkerLocal, WorkerLocalGuard};
pub use runtime::worker_set::{WorkerHealth, WorkerSet, WorkerSlot, WorkerSnapshot, WorkerState};
pub use state::R2eState;
pub use type_list::{
    AllSatisfied, BeanAccess, BeanLookup, BeanState, BuildHList, Contains, ControllerTuple, HCons,
    HNil, HasBean, Here, PluginDeps, TAppend, TCons, TNil, There,
};
pub use web::extract::{
    assert_unambiguous_extractor, BeanExtract, FromRequestPartsVia, OptionalFromRequestPartsVia,
    PeerAddr, Via, ViaAxum, ViaBean, ViaOpt,
};
pub use web::managed::{
    record_managed_finalize_error, ManagedContext, ManagedDeps, ManagedErr, ManagedGuard,
    ManagedOutcome, ManagedOutcomeKind, ManagedResource,
};
pub use web::pagination::{Page, Pageable};
pub use web::request_head::RequestHead;

// Dev-reload helpers
#[cfg(feature = "dev-reload")]
pub use runtime::dev::{
    commit_dev_cycle, has_staged_dev_cycle, invalidate_state_cache, rollback_dev_cycle,
};

// Entry-point macros
pub use r2e_macros::main;
pub use r2e_macros::test;
pub use r2e_macros::test_suite;

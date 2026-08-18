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
pub mod rt;
pub mod runtime;
pub mod state;
pub mod type_list;
pub mod types;
pub mod web;

// Used by macro-generated code (schema construction) so user crates don't
// need a direct serde_json dependency.
#[doc(hidden)]
pub use serde_json;

pub use beans::{
    AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, PostConstruct, PreDestroy, Producer,
};
pub use builder::{
    launch, App, AppBuilder, BootableApp, PreparedApp, RegisterController, RegisterControllers,
    RegisterModule, ServeContext, SpawnService, TaskRegistryHandle,
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
    PluginSetupContext, PreStatePlugin, RawPreStatePlugin,
};
pub use runtime::layers::{default_cors, default_trace, init_tracing, init_tracing_with_config};
pub use runtime::lifecycle::{LifecycleController, StopHandle};
pub use runtime::service::ServiceComponent;
pub use runtime::tracing_config::{LogFormat, SpanEvents, TracingConfig};
pub use state::R2eState;
pub use type_list::{
    AllSatisfied, BeanAccess, BeanLookup, BuildHList, Contains, ControllerTuple, HCons, HNil,
    HasBean, Here, PluginDeps, TAppend, TCons, TNil, There,
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
pub use runtime::dev::invalidate_state_cache;

// Entry-point macros
pub use r2e_macros::main;
pub use r2e_macros::test;
pub use r2e_macros::test_suite;

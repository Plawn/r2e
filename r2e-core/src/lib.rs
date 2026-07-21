pub mod beans;
pub mod builder;
pub mod config;
pub mod controller;
pub mod decorator;
pub mod dev;
pub mod error;
pub mod event_subscriber;
pub mod extract;
pub mod guards;
pub mod health;
pub mod http;
pub mod interceptors;
pub mod late;
pub mod layers;
pub mod lazy;
pub mod lifecycle;
pub mod managed;
pub mod meta;
pub mod module;
#[cfg(feature = "multipart")]
pub mod multipart;
pub mod pagination;
pub mod params;
pub mod plugin;
pub mod plugins;
pub mod prelude;
pub mod request_id;
pub mod rt;
pub mod scheduled_source;
pub mod secure_headers;
pub mod service;
pub mod sharded;
pub mod sse;
pub mod state;
pub mod tracing_config;
pub mod type_list;
pub mod types;
pub mod validation;
#[cfg(feature = "ws")]
pub mod ws;

// Used by macro-generated code (schema construction) so user crates don't
// need a direct serde_json dependency.
#[doc(hidden)]
pub use serde_json;

pub use beans::{
    AsyncBean, Bean, BeanContext, BeanError, BeanRegistry, PostConstruct, PreDestroy, Producer,
};
pub use builder::{
    launch, App, AppBuilder, BootableApp, PreparedApp, RegisterController, RegisterControllers,
    RegisterModule, ServeContext, TaskRegistryHandle,
};
pub use config::{
    deserialize_value, register_section, registered_sections, validate_keys, validate_section,
    ConfigError, ConfigProperties, ConfigValidationDetail, ConfigValidationError, ConfigValue,
    DefaultSecretResolver, FromConfigValue, LoadableConfig, MissingKeyError, PluginConfig,
    PropertyMeta, R2eConfig, RegisteredSection, SecretResolver,
};
pub use controller::{ContextConstruct, Controller, EndpointDeps};
pub use decorator::{
    BeanDecoFill, Decorate, DecoratorSpec, HasDecoSlot, SelfBuilt, SharedDecoSlot,
};
pub use error::{HttpError, HttpErrorExt};
pub use event_subscriber::EventSubscriber;
pub use extract::{
    assert_unambiguous_extractor, BeanExtract, FromRequestPartsVia, OptionalFromRequestPartsVia,
    Via, ViaAxum, ViaBean, ViaOpt,
};
pub use guards::{
    Guard, GuardContext, GuardError, Identity, NoIdentity, PathParam, PathParams, PreAuthGuard,
    PreAuthGuardContext,
};
pub use interceptors::{Cacheable, Interceptor, InterceptorContext};
pub use late::Late;
pub use layers::{default_cors, default_trace, init_tracing, init_tracing_with_config};
pub use lazy::Lazy;
pub use lifecycle::{LifecycleController, StopHandle};
pub use managed::{
    record_managed_finalize_error, ManagedContext, ManagedErr, ManagedGuard, ManagedOutcome,
    ManagedOutcomeKind, ManagedResource,
};
pub use meta::MetaRegistry;
pub use module::FeatureModule;
pub use pagination::{Page, Pageable};
pub use plugin::{
    DeferredAction, DeferredContext, Plugin, PluginInstallContext, PreStatePlugin,
    RawPreStatePlugin,
};
pub use plugins::{AdvancedHealth, ConfiguredTracing};
pub use request_id::{RequestId, RequestIdPlugin};
pub use scheduled_source::ScheduledSource;
pub use secure_headers::SecureHeaders;
pub use service::ServiceComponent;
pub use state::R2eState;
pub use tracing_config::{LogFormat, SpanEvents, TracingConfig};
pub use type_list::{
    AllSatisfied, BeanAccess, BeanLookup, BuildHList, Contains, ControllerTuple, HCons, HNil,
    HasBean, Here, PluginDeps, TAppend, TCons, TNil, There,
};

// Dev-reload helpers
#[cfg(feature = "dev-reload")]
pub use dev::invalidate_state_cache;

// Entry-point macros
pub use r2e_macros::main;
pub use r2e_macros::test;

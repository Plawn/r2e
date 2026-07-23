//! The `OpenFga` plugin — store lifecycle owned by the framework.
//!
//! `.plugin(OpenFga::model(authz::MODEL))` replaces the hand-rolled backend /
//! registry / client producers with one line, and closes the model half of the
//! schema-first chain: compile time already checks code ↔ checked-in `.fga`
//! (via [`model!`](crate::model)); this plugin checks checked-in `.fga` ↔ live
//! store at boot.
//!
//! # What it does
//!
//! During `build_state()` (before the app serves; a failure aborts startup):
//!
//! 1. Connects to the OpenFGA gRPC endpoint (`openfga.endpoint`).
//! 2. Resolves the store: by explicit `openfga.store_id`, or by name
//!    (`openfga.store`) via `ListStores`. In apply mode a missing store is
//!    **created**; in verify mode it is a startup error.
//! 3. Applies or verifies the authorization model:
//!    - **apply mode** (`openfga.apply_model: true`, the default — dev/test):
//!      structurally compares the compiled-in model with the store's latest;
//!      writes a new model version only when they differ (FGA models are
//!      append-only — an identical re-apply is skipped).
//!    - **verify mode** (`openfga.apply_model: false` — prod): fetches the
//!      live model (latest, or `openfga.model_id` when set) and fails startup
//!      on a structural mismatch, instead of serving mystery 403s.
//! 4. Pins the resolved `model_id` on the backend, so every check runs against
//!    one model version for the lifetime of the deploy.
//!
//! # Provided beans
//!
//! - [`OpenFgaRegistry`] — cached checks; the bean `FgaCheck` guards resolve.
//! - [`FgaClient`] — the typed grant/revoke/check client.
//! - [`OpenFgaHandle`] — the resolved `store_id`/`model_id` and the raw
//!   [`GrpcBackend`] escape hatch.
//!
//! # Example
//!
//! ```ignore
//! r2e_openfga::model!(pub mod authz = "fga/model.fga");
//!
//! b.load_config::<()>()
//!     .plugin(OpenFga::model(authz::MODEL))
//!     .build_state()
//!     .await
//! ```
//!
//! ```yaml
//! openfga:
//!   endpoint: "http://localhost:8081"   # gRPC endpoint
//!   store: "documents"                  # store name (looked up / created)
//!   # store_id: "01H..."                # or an explicit id (wins over `store`)
//!   # apply_model: false                # prod: verify instead of apply
//!   # model_id: "01H..."                # verify mode: pin + verify this version
//!   # enabled: false                    # full off-switch (checks fail closed)
//! ```
//!
//! The plugin must be installed **after** `load_config()` / `with_config()`.

use std::sync::Arc;

use r2e_core::Late;

use openfga_rs::open_fga_service_client::OpenFgaServiceClient;
use openfga_rs::tonic::transport::Channel;
use openfga_rs::{
    AuthorizationModel, CreateStoreRequest, GetStoreRequest, ListStoresRequest,
    ReadAuthorizationModelRequest, ReadAuthorizationModelsRequest, Store,
    WriteAuthorizationModelRequest,
};
use r2e_core::prelude::ConfigProperties;
use r2e_core::{PluginInstallContext, PostConstruct, PreStatePlugin};

use crate::backend::{connect_client, request_with_token, GrpcBackend, OpenFgaBackend};
use crate::client::FgaClient;
use crate::error::OpenFgaError;
use crate::model_convert::{compile_model, diff_summary, models_equal, CompiledModel};
use crate::registry::OpenFgaRegistry;

/// Boot error type: startup diagnostics, always fatal — plain messages.
type BootError = Box<dyn std::error::Error + Send + Sync>;

fn boot_err(msg: String) -> BootError {
    msg.into()
}

/// Configuration for the [`OpenFga`] plugin, under the `openfga` prefix.
///
/// All fields are optional at the type level so file config can be partial;
/// [`OpenFga`] validates the combination at install time (`endpoint` plus one
/// of `store` / `store_id` are required).
#[derive(ConfigProperties, Clone, Debug, Default)]
pub struct OpenFgaPluginConfig {
    /// `false` disables the plugin: no connection, no store/model resolution,
    /// no config validation — the beans still exist and every check fails
    /// closed with [`OpenFgaError::NotReady`]. Default: true.
    pub enabled: Option<bool>,
    /// OpenFGA **gRPC** endpoint, e.g. `http://localhost:8081`. Required.
    pub endpoint: Option<String>,
    /// Store name. Looked up via `ListStores`; created at boot in apply mode
    /// when missing. Ignored when `store_id` is set.
    pub store: Option<String>,
    /// Explicit store id. Skips the name lookup; the store must exist.
    pub store_id: Option<String>,
    /// Pin and verify this exact model version. Only valid in verify mode
    /// (`apply_model: false`); apply mode always resolves the version itself.
    pub model_id: Option<String>,
    /// `true` (default): ensure store + apply the model when it differs from
    /// the store's latest. `false`: verify the live model matches the
    /// compiled-in one and fail startup otherwise.
    pub apply_model: Option<bool>,
    /// Bearer token added to every request.
    pub api_token: Option<String>,
    /// Connection timeout in seconds. Default: 10.
    pub connect_timeout_secs: Option<u64>,
    /// Request timeout in seconds. Default: 5.
    pub request_timeout_secs: Option<u64>,
    /// Whether to enable decision caching. Default: true.
    pub cache_enabled: Option<bool>,
    /// Decision cache TTL in seconds. Default: 60.
    pub cache_ttl_secs: Option<u64>,
}

/// How the boot sequence finds the store.
#[derive(Debug, Clone)]
enum StoreSelector {
    Id(String),
    Name(String),
}

/// Validated install-time settings (config + the compiled-in model).
#[derive(Debug, Clone)]
struct BootSettings {
    endpoint: String,
    store: StoreSelector,
    apply_model: bool,
    /// Verify mode only: pin + verify this exact model version.
    pinned_model_id: Option<String>,
    api_token: Option<String>,
    connect_timeout_secs: u64,
    request_timeout_secs: u64,
}

/// The OpenFGA pre-state plugin. See the [module docs](self).
pub struct OpenFga {
    model_json: &'static str,
}

impl OpenFga {
    /// Manage the store against this compiled-in model — pass the
    /// [`model!`](crate::model)-generated `authz::MODEL`.
    pub fn model(model_json: &'static str) -> Self {
        Self { model_json }
    }
}

impl PreStatePlugin for OpenFga {
    type Provided = (OpenFgaRegistry, FgaClient, OpenFgaHandle);
    type Deps = ();
    type Config = OpenFgaPluginConfig;
    const CONFIG_PREFIX: Option<&'static str> = Some("openfga");

    fn install(&mut self, ctx: &mut PluginInstallContext<'_>) -> Self::Provided {
        let config = ctx.config().unwrap_or_else(|| {
            panic!(
                "the OpenFga plugin reads `openfga.*` at install time — call \
                 `load_config()`/`with_config()` before `.plugin(OpenFga::model(...))`"
            )
        });
        let cfg = <OpenFgaPluginConfig as r2e_core::PluginConfig>::plugin_load(config, "openfga")
            .unwrap_or_else(|e| panic!("invalid `openfga.*` configuration: {e}"));

        let enabled = cfg.enabled.unwrap_or(true);

        let slot: Late<GrpcBackend> = Late::new();
        let lazy = LazyBackend { slot: slot.clone() };
        let registry = if cfg.cache_enabled.unwrap_or(true) {
            OpenFgaRegistry::with_cache(lazy, cfg.cache_ttl_secs.unwrap_or(60))
        } else {
            OpenFgaRegistry::new(lazy)
        };
        let client = FgaClient::new(registry.clone());

        // Disabled: no connection, no store/model resolution — and no config
        // validation, so `enabled: false` alone is a complete off-switch. The
        // backend slot stays empty and every check fails closed (`NotReady`).
        if !enabled {
            tracing::warn!(
                "OpenFga plugin disabled via `openfga.enabled = false`; \
                 no store/model resolution — FGA checks will fail closed"
            );
            let handle = OpenFgaHandle {
                inner: Arc::new(HandleInner { boot: None, slot }),
            };
            return (registry, client, handle);
        }

        let settings = validate_config(cfg);

        // Fail on an unparsable model at install (deterministic), not at boot.
        if let Err(e) = compile_model(self.model_json) {
            panic!("OpenFga::model(...) received an invalid authorization model: {e}");
        }

        let handle = OpenFgaHandle {
            inner: Arc::new(HandleInner {
                boot: Some((settings, self.model_json)),
                slot,
            }),
        };

        // The boot sequence (connect + store/model resolution) runs as this
        // bean's post-construct, inside `build_state()`.
        ctx.run_post_construct::<OpenFgaHandle>();

        (registry, client, handle)
    }
}

/// Turn the raw config section into validated [`BootSettings`], panicking at
/// install (= startup) with an actionable message on an invalid combination.
fn validate_config(cfg: OpenFgaPluginConfig) -> BootSettings {
    let endpoint = match cfg.endpoint {
        Some(e) if !e.is_empty() => e,
        _ => panic!(
            "the OpenFga plugin requires `openfga.endpoint` (the OpenFGA gRPC endpoint, \
             e.g. \"http://localhost:8081\")"
        ),
    };
    let store = match (cfg.store_id, cfg.store) {
        (Some(id), _) if !id.is_empty() => StoreSelector::Id(id),
        (_, Some(name)) if !name.is_empty() => StoreSelector::Name(name),
        _ => panic!(
            "the OpenFga plugin requires `openfga.store` (a store name to look up or \
             create) or `openfga.store_id` (an existing store id)"
        ),
    };
    let apply_model = cfg.apply_model.unwrap_or(true);
    if apply_model && cfg.model_id.is_some() {
        panic!(
            "`openfga.model_id` is only meaningful in verify mode — set \
             `openfga.apply_model: false` to pin a model version, or drop `model_id` \
             and let apply mode resolve it"
        );
    }
    BootSettings {
        endpoint,
        store,
        apply_model,
        pinned_model_id: cfg.model_id,
        api_token: cfg.api_token,
        connect_timeout_secs: cfg.connect_timeout_secs.unwrap_or(10),
        request_timeout_secs: cfg.request_timeout_secs.unwrap_or(5),
    }
}

// ── OpenFgaHandle ──────────────────────────────────────────────────────

/// Handle to the plugin-managed OpenFGA connection: the resolved
/// `store_id` / pinned `model_id`, and the raw [`GrpcBackend`] escape hatch.
///
/// Provided as a bean by [`OpenFga`]; fully resolved once `build_state()`
/// returns (its post-construct runs the boot sequence).
#[derive(Clone)]
pub struct OpenFgaHandle {
    inner: Arc<HandleInner>,
}

struct HandleInner {
    /// `None` when the plugin is disabled (`openfga.enabled: false`) — the
    /// post-construct is not registered then, and the slot stays empty.
    boot: Option<(BootSettings, &'static str)>,
    slot: Late<GrpcBackend>,
}

impl OpenFgaHandle {
    /// The connected backend. Panics before the boot sequence has run —
    /// only reachable when the plugin was disabled (`openfga.enabled: false`)
    /// or the handle escaped `build_state()`.
    pub fn backend(&self) -> GrpcBackend {
        self.try_backend()
            .unwrap_or_else(|| panic!("OpenFgaHandle used before the OpenFga boot sequence ran"))
    }

    /// The connected backend, `None` before the boot sequence has run.
    pub fn try_backend(&self) -> Option<GrpcBackend> {
        self.inner.slot.get().cloned()
    }

    /// The resolved store id.
    pub fn store_id(&self) -> String {
        self.backend().store_id().to_string()
    }

    /// The pinned authorization model id all checks run against.
    pub fn model_id(&self) -> String {
        self.backend()
            .model_id()
            .expect("OpenFga boot always pins a model id")
            .to_string()
    }
}

impl PostConstruct for OpenFgaHandle {
    fn post_construct(&self) -> r2e_core::lifecycle::LifecycleFuture<'_> {
        Box::pin(async move {
            let Some((settings, model_json)) = &self.inner.boot else {
                return Ok(());
            };
            if self.inner.slot.get().is_some() {
                return Ok(());
            }
            let backend = boot(settings, model_json).await?;
            let _ = self.inner.slot.fill(backend);
            Ok(())
        })
    }
}

// ── Boot sequence ──────────────────────────────────────────────────────

/// Connect, resolve the store, apply/verify the model, pin the model id.
async fn boot(settings: &BootSettings, model_json: &str) -> Result<GrpcBackend, BootError> {
    let compiled = compile_model(model_json).map_err(|e| boot_err(e.to_string()))?;

    let client = connect_client(
        &settings.endpoint,
        settings.connect_timeout_secs,
        settings.request_timeout_secs,
    )
    .await
    .map_err(|e| {
        boot_err(format!(
            "OpenFga: cannot connect to `{}`: {e} — is the OpenFGA gRPC endpoint reachable?",
            settings.endpoint
        ))
    })?;

    let token = settings.api_token.as_deref();
    let store_id = resolve_store(&client, token, settings).await?;
    let model_id = resolve_model(&client, token, settings, &store_id, &compiled).await?;

    tracing::info!(
        store_id = %store_id,
        model_id = %model_id,
        mode = if settings.apply_model { "apply" } else { "verify" },
        "OpenFGA store ready; model pinned"
    );

    Ok(GrpcBackend::from_parts(
        client,
        store_id,
        Some(model_id),
        settings.api_token.clone(),
    ))
}

async fn resolve_store(
    client: &OpenFgaServiceClient<Channel>,
    token: Option<&str>,
    settings: &BootSettings,
) -> Result<String, BootError> {
    match &settings.store {
        StoreSelector::Id(id) => {
            client
                .clone()
                .get_store(request_with_token(
                    token,
                    GetStoreRequest {
                        store_id: id.clone(),
                    },
                )?)
                .await
                .map_err(|status| {
                    boot_err(format!(
                        "OpenFga: store `{id}` (openfga.store_id) not usable: {}",
                        status.message()
                    ))
                })?;
            Ok(id.clone())
        }
        StoreSelector::Name(name) => {
            let matches = stores_by_name(client, token, name).await?;
            match matches.len() {
                1 => Ok(matches.into_iter().next().expect("len checked").id),
                0 if settings.apply_model => {
                    let resp = client
                        .clone()
                        .create_store(request_with_token(
                            token,
                            CreateStoreRequest { name: name.clone() },
                        )?)
                        .await
                        .map_err(|status| {
                            boot_err(format!(
                                "OpenFga: failed to create store `{name}`: {}",
                                status.message()
                            ))
                        })?;
                    let id = resp.into_inner().id;
                    tracing::info!(store = %name, store_id = %id, "OpenFGA store created");
                    Ok(id)
                }
                0 => Err(boot_err(format!(
                    "OpenFga: store `{name}` does not exist and verify mode \
                     (`openfga.apply_model: false`) never creates stores — create it out of \
                     band or fix `openfga.store`"
                ))),
                n if settings.apply_model => {
                    // Store names are not unique in OpenFGA. Dev/test keeps
                    // booting deterministically on the oldest; prod (verify
                    // mode, below) refuses to guess. "Oldest" = smallest id:
                    // OpenFGA store ids are ULIDs, lexicographically
                    // time-ordered.
                    let oldest = matches
                        .into_iter()
                        .min_by(|a, b| a.id.cmp(&b.id))
                        .expect("n > 1");
                    tracing::warn!(
                        store = %name,
                        matches = n,
                        store_id = %oldest.id,
                        "multiple OpenFGA stores share this name; using the oldest — set \
                         `openfga.store_id` to disambiguate"
                    );
                    Ok(oldest.id)
                }
                n => Err(boot_err(format!(
                    "OpenFga: {n} stores are named `{name}` — set `openfga.store_id` to \
                     disambiguate"
                ))),
            }
        }
    }
}

/// All stores whose name is exactly `name` (paginating `ListStores`).
async fn stores_by_name(
    client: &OpenFgaServiceClient<Channel>,
    token: Option<&str>,
    name: &str,
) -> Result<Vec<Store>, BootError> {
    let mut matches = Vec::new();
    let mut continuation_token = String::new();
    loop {
        let resp = client
            .clone()
            .list_stores(request_with_token(
                token,
                ListStoresRequest {
                    page_size: Some(100),
                    continuation_token: continuation_token.clone(),
                },
            )?)
            .await
            .map_err(|status| {
                boot_err(format!("OpenFga: ListStores failed: {}", status.message()))
            })?
            .into_inner();
        matches.extend(resp.stores.into_iter().filter(|s| s.name == name));
        continuation_token = resp.continuation_token;
        if continuation_token.is_empty() {
            return Ok(matches);
        }
    }
}

/// Apply or verify the model; returns the model id to pin.
async fn resolve_model(
    client: &OpenFgaServiceClient<Channel>,
    token: Option<&str>,
    settings: &BootSettings,
    store_id: &str,
    compiled: &CompiledModel,
) -> Result<String, BootError> {
    if let Some(pinned) = &settings.pinned_model_id {
        // Verify mode with an explicit version: fetch exactly that model.
        let live = client
            .clone()
            .read_authorization_model(request_with_token(
                token,
                ReadAuthorizationModelRequest {
                    store_id: store_id.to_string(),
                    id: pinned.clone(),
                },
            )?)
            .await
            .map_err(|status| {
                boot_err(format!(
                    "OpenFga: model `{pinned}` (openfga.model_id) not readable from store \
                     `{store_id}`: {}",
                    status.message()
                ))
            })?
            .into_inner()
            .authorization_model
            .ok_or_else(|| {
                boot_err(format!(
                    "OpenFga: store `{store_id}` has no model `{pinned}`"
                ))
            })?;
        return verify(compiled, &live, store_id);
    }

    let latest = latest_model(client, token, store_id).await?;

    if !settings.apply_model {
        let live = latest.ok_or_else(|| {
            boot_err(format!(
                "OpenFga: store `{store_id}` has no authorization model and verify mode \
                 (`openfga.apply_model: false`) never writes one — apply it out of band \
                 (e.g. a dev boot with apply mode, or `fga model write`)"
            ))
        })?;
        return verify(compiled, &live, store_id);
    }

    // Apply mode: models are append-only, so an identical latest is reused
    // instead of writing a redundant new version.
    if let Some(live) = &latest {
        if models_equal(compiled, live) {
            tracing::debug!(model_id = %live.id, "OpenFGA model unchanged; reusing latest");
            return Ok(live.id.clone());
        }
    }

    let resp = client
        .clone()
        .write_authorization_model(request_with_token(
            token,
            WriteAuthorizationModelRequest {
                store_id: store_id.to_string(),
                type_definitions: compiled.type_definitions.clone(),
                schema_version: compiled.schema_version.clone(),
                conditions: compiled.conditions.clone(),
            },
        )?)
        .await
        .map_err(|status| {
            boot_err(format!(
                "OpenFga: WriteAuthorizationModel failed on store `{store_id}`: {}",
                status.message()
            ))
        })?;
    let id = resp.into_inner().authorization_model_id;
    tracing::info!(model_id = %id, "OpenFGA authorization model applied");
    Ok(id)
}

/// The store's newest model, if any (`ReadAuthorizationModels` returns
/// models in descending creation order).
async fn latest_model(
    client: &OpenFgaServiceClient<Channel>,
    token: Option<&str>,
    store_id: &str,
) -> Result<Option<AuthorizationModel>, BootError> {
    let resp = client
        .clone()
        .read_authorization_models(request_with_token(
            token,
            ReadAuthorizationModelsRequest {
                store_id: store_id.to_string(),
                page_size: Some(1),
                continuation_token: String::new(),
            },
        )?)
        .await
        .map_err(|status| {
            boot_err(format!(
                "OpenFga: ReadAuthorizationModels failed on store `{store_id}`: {}",
                status.message()
            ))
        })?;
    Ok(resp.into_inner().authorization_models.into_iter().next())
}

fn verify(
    compiled: &CompiledModel,
    live: &AuthorizationModel,
    store_id: &str,
) -> Result<String, BootError> {
    if models_equal(compiled, live) {
        Ok(live.id.clone())
    } else {
        Err(boot_err(format!(
            "OpenFga: the compiled-in authorization model does not match store `{store_id}` \
             (model `{}`): {} — deploy the model first (a dev boot in apply mode or \
             `fga model write`), or fix the checked-in `.fga`",
            live.id,
            diff_summary(compiled, live),
        )))
    }
}

// ── LazyBackend ────────────────────────────────────────────────────────

/// Backend registered at install time, wired to the real [`GrpcBackend`] by
/// the boot sequence. Every operation before that returns
/// [`OpenFgaError::NotReady`] (unreachable in a normal boot: post-construct
/// runs inside `build_state()`, before the app serves).
#[derive(Clone)]
struct LazyBackend {
    slot: Late<GrpcBackend>,
}

impl OpenFgaBackend for LazyBackend {
    fn check(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<bool, OpenFgaError>> + Send + '_>>
    {
        match self.slot.get() {
            Some(backend) => backend.check(user, relation, object),
            None => Box::pin(async { Err(OpenFgaError::NotReady) }),
        }
    }

    fn write_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), OpenFgaError>> + Send + '_>>
    {
        match self.slot.get() {
            Some(backend) => backend.write_tuple(user, relation, object),
            None => Box::pin(async { Err(OpenFgaError::NotReady) }),
        }
    }

    fn delete_tuple(
        &self,
        user: &str,
        relation: &str,
        object: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), OpenFgaError>> + Send + '_>>
    {
        match self.slot.get() {
            Some(backend) => backend.delete_tuple(user, relation, object),
            None => Box::pin(async { Err(OpenFgaError::NotReady) }),
        }
    }
}

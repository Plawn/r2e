# Plan d'implémentation — Observabilité Complète

## Contexte

L'observabilité actuelle dans R2E :
- `Prometheus` plugin : métriques HTTP basiques (requests_total, duration, in_flight) ✅
- `Tracing` plugin : logs structurés via `tracing` + `tower-http` ✅
- `Health` / `AdvancedHealth` : liveness/readiness avec `HealthIndicator` trait ✅
- `RequestIdPlugin` : propagation X-Request-Id ✅

**Ce qui manque** :
- Tracing distribué OpenTelemetry (export Jaeger/OTLP)
- Corrélation automatique trace_id ↔ logs ↔ métriques
- Health checks enrichis avec dépendances et détails
- Métriques custom déclaratives (`#[counted]`, `#[timed]`)
- Dashboard de métriques applicatives intégré
- Readiness probe intelligent (attendre que les dépendances soient prêtes)

## Architecture cible

```
r2e-observability/                    ← NOUVEAU CRATE
  ├── Cargo.toml
  └── src/
      ├── lib.rs                      ← Re-exports + ObservabilityPlugin
      ├── tracing_otel.rs             ← OpenTelemetry tracing setup
      ├── metrics_otel.rs             ← OpenTelemetry metrics (bridge Prometheus)
      ├── propagation.rs              ← Context propagation (W3C TraceContext, B3)
      ├── middleware.rs               ← Tower layer pour inject trace context
      └── config.rs                   ← ObservabilityConfig

r2e-core/src/health.rs               ← ENRICHIR (health checks avancés)
r2e-prometheus/                       ← ENRICHIR (bridge OpenTelemetry)
r2e-macros/                           ← ENRICHIR (#[counted], #[timed])
```

---

## Étape 1 — Créer le crate `r2e-observability`

### Cargo.toml

```toml
[package]
name = "r2e-observability"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
keywords = ["opentelemetry", "tracing", "observability", "distributed-tracing"]
categories = ["web-programming"]
description = "OpenTelemetry observability plugin for R2E — distributed tracing, metrics, and context propagation"

[features]
default = ["otlp"]
otlp = ["opentelemetry-otlp"]
jaeger = []  # Jaeger via OTLP (le protocole natif Jaeger est deprecated)

[dependencies]
r2e-core = { path = "../r2e-core", version = "0.1.0" }
opentelemetry = "0.28"
opentelemetry_sdk = { version = "0.28", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.28", optional = true }
opentelemetry-semantic-conventions = "0.28"
tracing-opentelemetry = "0.28"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "registry"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["trace"] }
http = "1"
pin-project-lite = "0.2"
```

### Actions
1. Créer le dossier `r2e-observability/` dans le workspace
2. Ajouter `"r2e-observability"` dans `Cargo.toml` workspace members
3. Ajouter `observability = ["dep:r2e-observability"]` dans `r2e/Cargo.toml` features
4. Ajouter à la feature `full` dans `r2e/Cargo.toml`

### Validation
```bash
cargo check -p r2e-observability
```

---

## Étape 2 — Configuration de l'observabilité

**Fichier** : `r2e-observability/src/config.rs`

### Objectif
Configuration centralisée pour toute la stack d'observabilité, pilotable
depuis `application.yaml` ou par env vars.

### Struct de configuration
```rust
/// Configuration for the observability stack.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Service name reported to the tracing backend.
    pub service_name: String,

    /// Service version (used in resource attributes).
    pub service_version: Option<String>,

    /// OTLP exporter endpoint (default: "http://localhost:4317").
    pub otlp_endpoint: String,

    /// Protocol: "grpc" (default) or "http".
    pub otlp_protocol: OtlpProtocol,

    /// Whether to enable tracing export.
    pub tracing_enabled: bool,

    /// Whether to enable metrics export via OTLP.
    pub metrics_enabled: bool,

    /// Sampling ratio (0.0 to 1.0, default 1.0 = all traces).
    pub sampling_ratio: f64,

    /// Propagation format: "w3c" (default), "b3", "jaeger".
    pub propagation_format: PropagationFormat,

    /// Additional resource attributes (key=value).
    pub resource_attributes: Vec<(String, String)>,

    /// Headers to forward as span attributes (e.g. ["x-tenant-id"]).
    pub capture_headers: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub enum OtlpProtocol {
    #[default]
    Grpc,
    Http,
}

#[derive(Debug, Clone, Default)]
pub enum PropagationFormat {
    #[default]
    W3c,
    B3,
    Jaeger,
}
```

### Builder pattern
```rust
impl ObservabilityConfig {
    pub fn new(service_name: &str) -> Self {
        Self {
            service_name: service_name.to_string(),
            service_version: None,
            otlp_endpoint: "http://localhost:4317".to_string(),
            otlp_protocol: OtlpProtocol::Grpc,
            tracing_enabled: true,
            metrics_enabled: true,
            sampling_ratio: 1.0,
            propagation_format: PropagationFormat::W3c,
            resource_attributes: Vec::new(),
            capture_headers: Vec::new(),
        }
    }

    pub fn with_service_version(mut self, version: &str) -> Self { ... }
    pub fn with_endpoint(mut self, endpoint: &str) -> Self { ... }
    pub fn with_sampling_ratio(mut self, ratio: f64) -> Self { ... }
    pub fn with_propagation(mut self, format: PropagationFormat) -> Self { ... }
    pub fn with_resource_attribute(mut self, key: &str, value: &str) -> Self { ... }
    pub fn capture_header(mut self, header: &str) -> Self { ... }
    pub fn disable_tracing(mut self) -> Self { ... }
    pub fn disable_metrics(mut self) -> Self { ... }

    /// Load from R2eConfig with prefix "observability".
    /// Reads keys like:
    ///   observability.service-name
    ///   observability.otlp-endpoint
    ///   observability.sampling-ratio
    ///   observability.propagation
    ///   observability.tracing.enabled
    ///   observability.metrics.enabled
    pub fn from_r2e_config(config: &r2e_core::config::R2eConfig, service_name: &str) -> Self {
        let mut cfg = Self::new(service_name);
        if let Ok(endpoint) = config.get::<String>("observability.otlp-endpoint") {
            cfg.otlp_endpoint = endpoint;
        }
        if let Ok(ratio) = config.get::<f64>("observability.sampling-ratio") {
            cfg.sampling_ratio = ratio;
        }
        if let Ok(enabled) = config.get::<bool>("observability.tracing.enabled") {
            cfg.tracing_enabled = enabled;
        }
        if let Ok(enabled) = config.get::<bool>("observability.metrics.enabled") {
            cfg.metrics_enabled = enabled;
        }
        // etc.
        cfg
    }
}
```

### Validation
```bash
cargo check -p r2e-observability
```

---

## Étape 3 — Setup OpenTelemetry Tracing

**Fichier** : `r2e-observability/src/tracing_otel.rs`

### Objectif
Configurer la pipeline OpenTelemetry pour exporter les traces vers un collecteur OTLP
(Jaeger, Grafana Tempo, Datadog, etc.), tout en gardant la compatibilité avec
le système `tracing` existant.

### Implémentation
```rust
use opentelemetry::trace::TracerProvider;
use opentelemetry_sdk::{
    trace::{self as sdktrace, Sampler},
    Resource,
};
use opentelemetry_semantic_conventions::resource as semconv;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Registry};

use crate::config::ObservabilityConfig;

/// Initialize the full tracing stack: console logs + OpenTelemetry export.
///
/// This replaces `r2e_core::init_tracing()` when observability is enabled.
/// Returns a guard that flushes traces on drop.
pub fn init_tracing(config: &ObservabilityConfig) -> OtelGuard {
    // Build resource attributes
    let mut resource_kv = vec![
        opentelemetry::KeyValue::new(
            semconv::SERVICE_NAME,
            config.service_name.clone(),
        ),
    ];
    if let Some(ref version) = config.service_version {
        resource_kv.push(opentelemetry::KeyValue::new(
            semconv::SERVICE_VERSION,
            version.clone(),
        ));
    }
    for (k, v) in &config.resource_attributes {
        resource_kv.push(opentelemetry::KeyValue::new(k.clone(), v.clone()));
    }
    let resource = Resource::builder()
        .with_attributes(resource_kv)
        .build();

    // Build the OTLP exporter
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()  // gRPC
        .with_endpoint(&config.otlp_endpoint)
        .build()
        .expect("Failed to build OTLP span exporter");

    // Build the tracer provider
    let sampler = if config.sampling_ratio >= 1.0 {
        Sampler::AlwaysOn
    } else if config.sampling_ratio <= 0.0 {
        Sampler::AlwaysOff
    } else {
        Sampler::TraceIdRatioBased(config.sampling_ratio)
    };

    let provider = sdktrace::TracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_sampler(sampler)
        .with_resource(resource)
        .build();

    let tracer = provider.tracer("r2e");

    // Build the tracing-subscriber stack
    let otel_layer = OpenTelemetryLayer::new(tracer);
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_file(false);

    Registry::default()
        .with(env_filter)
        .with(fmt_layer)
        .with(otel_layer)
        .init();

    OtelGuard { provider }
}

/// Guard that ensures traces are flushed when the application shuts down.
pub struct OtelGuard {
    provider: sdktrace::TracerProvider,
}

impl Drop for OtelGuard {
    fn drop(&mut self) {
        if let Err(e) = self.provider.shutdown() {
            eprintln!("Failed to shut down OpenTelemetry tracer: {e}");
        }
    }
}
```

### Validation
```bash
cargo check -p r2e-observability
```

---

## Étape 4 — Middleware de propagation de contexte

**Fichier** : `r2e-observability/src/propagation.rs`

### Objectif
Extraire le contexte de trace des headers entrants (W3C `traceparent`, B3, etc.)
et l'injecter dans les headers sortants. Cela permet la corrélation des traces
entre microservices.

### Implémentation
```rust
use opentelemetry::propagation::{TextMapCompositePropagator, TextMapPropagator};
use opentelemetry_sdk::propagation::TraceContextPropagator;

use crate::config::{ObservabilityConfig, PropagationFormat};

/// Install the global propagator based on config.
pub fn install_propagator(config: &ObservabilityConfig) {
    let propagator: Box<dyn TextMapPropagator + Send + Sync> = match config.propagation_format {
        PropagationFormat::W3c => Box::new(TraceContextPropagator::new()),
        PropagationFormat::B3 => {
            // B3 propagation
            Box::new(opentelemetry_sdk::propagation::BaggagePropagator::new())
            // Note: pour un vrai B3, il faut opentelemetry-zipkin.
            // Ici on simplifie. Si B3 complet est nécessaire, ajouter la dep.
        }
        PropagationFormat::Jaeger => {
            // Jaeger propagation headers (uber-trace-id)
            // Via OTLP, on utilise W3C de toute façon
            Box::new(TraceContextPropagator::new())
        }
    };
    opentelemetry::global::set_text_map_propagator(
        TextMapCompositePropagator::new(vec![propagator])
    );
}
```

**Fichier** : `r2e-observability/src/middleware.rs`

### Tower middleware pour extraire/injecter le trace context
```rust
use std::task::{Context, Poll};
use tower::{Layer, Service};
use http::Request;
use opentelemetry::propagation::Extractor;

/// Header extractor for OpenTelemetry propagation.
struct HeaderExtractor<'a>(&'a http::HeaderMap);

impl<'a> Extractor for HeaderExtractor<'a> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|v| v.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|k| k.as_str()).collect()
    }
}

/// Layer that extracts trace context from incoming HTTP headers
/// and creates a span for each request.
#[derive(Clone)]
pub struct OtelTraceLayer {
    capture_headers: Vec<String>,
}

impl OtelTraceLayer {
    pub fn new(capture_headers: Vec<String>) -> Self {
        Self { capture_headers }
    }
}

impl<S> Layer<S> for OtelTraceLayer {
    type Service = OtelTraceService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OtelTraceService {
            inner,
            capture_headers: self.capture_headers.clone(),
        }
    }
}

#[derive(Clone)]
pub struct OtelTraceService<S> {
    inner: S,
    capture_headers: Vec<String>,
}

// L'implémentation du Service doit :
// 1. Extraire le context parent depuis les headers HTTP (traceparent, etc.)
// 2. Créer un span avec le method, path, et headers capturés
// 3. Attacher le trace_id et span_id dans les extensions de la request
// 4. Propager le context pour les appels sortants
// 5. Enregistrer le status code dans le span à la fin
//
// Utiliser pin-project-lite pour le Future.
// Voir l'implémentation de tower-http TracingLayer comme référence.
```

### Attention
L'implémentation complète du `Service` avec le `Future` piné est non-triviale.
Se baser sur le pattern de `r2e-prometheus/src/layer.rs` qui fait exactement la même chose
(wrapper un service, mesurer le temps, enregistrer des données).

Structure du Future :
```rust
pin_project! {
    pub struct OtelResponseFuture<F> {
        #[pin]
        inner: F,
        span: tracing::Span,
        start: std::time::Instant,
    }
}

impl<F, B> Future for OtelResponseFuture<F>
where
    F: Future<Output = Result<http::Response<B>, Infallible>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let _enter = this.span.enter();
        match this.inner.poll(cx) {
            Poll::Ready(result) => {
                if let Ok(ref response) = result {
                    this.span.record("http.status_code", response.status().as_u16());
                }
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}
```

### Validation
```bash
cargo check -p r2e-observability
```

---

## Étape 5 — Plugin d'observabilité (assemblage)

**Fichier** : `r2e-observability/src/lib.rs`

### Objectif
Fournir un plugin unique `Observability` qui installe toute la stack en un seul appel.

### API utilisateur cible
```rust
use r2e::r2e_observability::{Observability, ObservabilityConfig};

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(Observability::new(
        ObservabilityConfig::new("my-service")
            .with_service_version("1.0.0")
            .with_endpoint("http://otel-collector:4317")
            .with_sampling_ratio(0.5)
            .capture_header("x-tenant-id"),
    ))
    // ...
```

### Implémentation du plugin
```rust
pub mod config;
pub mod tracing_otel;
pub mod propagation;
pub mod middleware;

pub use config::{ObservabilityConfig, OtlpProtocol, PropagationFormat};

use r2e_core::Plugin;

/// Observability plugin — installs OpenTelemetry tracing, metrics, and propagation.
pub struct Observability {
    config: ObservabilityConfig,
}

impl Observability {
    pub fn new(config: ObservabilityConfig) -> Self {
        Self { config }
    }

    /// Create from R2eConfig (reads `observability.*` keys).
    pub fn from_config(r2e_config: &r2e_core::config::R2eConfig, service_name: &str) -> Self {
        Self {
            config: ObservabilityConfig::from_r2e_config(r2e_config, service_name),
        }
    }
}

impl Plugin for Observability {
    fn install<T: Clone + Send + Sync + 'static>(self, app: r2e_core::AppBuilder<T>) -> r2e_core::AppBuilder<T> {
        // 1. Install propagator
        propagation::install_propagator(&self.config);

        // 2. Initialize tracing (replaces r2e::init_tracing)
        // Note: Le guard doit rester vivant toute la durée de l'app.
        // On le stocke dans les plugin_data du builder.
        let guard = if self.config.tracing_enabled {
            Some(tracing_otel::init_tracing(&self.config))
        } else {
            None
        };

        // 3. Install the trace context extraction middleware
        let capture_headers = self.config.capture_headers.clone();
        let app = app.with_layer_fn(move |router| {
            router.layer(middleware::OtelTraceLayer::new(capture_headers))
        });

        // 4. Store the guard so it lives for the app lifetime
        // Use on_stop to flush traces gracefully
        let app = if let Some(guard) = guard {
            let guard = std::sync::Arc::new(std::sync::Mutex::new(Some(guard)));
            let guard_clone = guard.clone();
            app.on_stop(move || async move {
                // Drop the guard to trigger flush
                let _ = guard_clone.lock().unwrap().take();
                tracing::info!("OpenTelemetry traces flushed");
            })
        } else {
            app
        };

        app
    }
}
```

### Validation
```bash
cargo check -p r2e-observability
```

---

## Étape 6 — Enrichir les Health Checks

**Fichier** : `r2e-core/src/health.rs`

### Objectif
Améliorer le système existant (déjà bien conçu) avec :
- Détails par check (latence, dernière vérification)
- Catégorisation liveness vs readiness
- Cache des résultats (ne pas re-vérifier à chaque requête)
- Health checks built-in pour les dépendances courantes

### Enrichir HealthResponse
```rust
#[derive(Debug, Clone, Serialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthCheckStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,        // NEW: temps de vérification
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>, // NEW: détails arbitraires
}

#[derive(Debug, Clone, Serialize)]
pub struct HealthResponse {
    pub status: HealthCheckStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<HealthCheck>,
    pub uptime_seconds: u64,             // NEW: temps depuis le démarrage
}
```

### Enrichir le trait HealthIndicator
```rust
pub trait HealthIndicator: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn check(&self) -> impl std::future::Future<Output = HealthStatus> + Send;

    /// Whether this check affects readiness (default: true).
    /// Liveness-only checks don't block readiness.
    fn affects_readiness(&self) -> bool { true }
}
```

### Ajouter un cache de résultats
```rust
pub struct CachedHealthState {
    checks: Vec<Box<dyn HealthIndicatorErased>>,
    cache: tokio::sync::RwLock<Option<(HealthResponse, std::time::Instant)>>,
    cache_ttl: std::time::Duration,
    start_time: std::time::Instant,
}

impl CachedHealthState {
    pub fn new(
        checks: Vec<Box<dyn HealthIndicatorErased>>,
        cache_ttl: std::time::Duration,
    ) -> Self {
        Self {
            checks,
            cache: tokio::sync::RwLock::new(None),
            cache_ttl,
            start_time: std::time::Instant::now(),
        }
    }

    pub async fn aggregate(&self) -> HealthResponse {
        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some((ref response, ref timestamp)) = *cache {
                if timestamp.elapsed() < self.cache_ttl {
                    return response.clone();
                }
            }
        }

        // Recompute
        let mut checks = Vec::with_capacity(self.checks.len());
        let mut all_up = true;

        for indicator in &self.checks {
            let start = std::time::Instant::now();
            let status = indicator.check().await;
            let duration_ms = start.elapsed().as_millis() as u64;

            let (check_status, reason) = match &status {
                HealthStatus::Up => (HealthCheckStatus::Up, None),
                HealthStatus::Down(r) => {
                    all_up = false;
                    (HealthCheckStatus::Down, Some(r.clone()))
                }
            };
            checks.push(HealthCheck {
                name: indicator.name().to_string(),
                status: check_status,
                reason,
                duration_ms: Some(duration_ms),
                details: None,
            });
        }

        let response = HealthResponse {
            status: if all_up { HealthCheckStatus::Up } else { HealthCheckStatus::Down },
            checks,
            uptime_seconds: self.start_time.elapsed().as_secs(),
        };

        // Update cache
        let mut cache = self.cache.write().await;
        *cache = Some((response.clone(), std::time::Instant::now()));

        response
    }
}
```

### Health Checks Built-in

**Fichier** : `r2e-core/src/health_checks.rs` (nouveau)

Fournir des implémentations prêtes à l'emploi :

```rust
/// Health check for SQLx database pools.
pub struct DatabaseHealth<DB: sqlx::Database> {
    name: String,
    pool: sqlx::Pool<DB>,
}

impl<DB: sqlx::Database> DatabaseHealth<DB> {
    pub fn new(pool: sqlx::Pool<DB>) -> Self {
        Self { name: "database".into(), pool }
    }

    pub fn with_name(pool: sqlx::Pool<DB>, name: &str) -> Self {
        Self { name: name.into(), pool }
    }
}

impl<DB: sqlx::Database> HealthIndicator for DatabaseHealth<DB> {
    fn name(&self) -> &str { &self.name }

    async fn check(&self) -> HealthStatus {
        match self.pool.acquire().await {
            Ok(_conn) => HealthStatus::Up,
            Err(e) => HealthStatus::Down(format!("Cannot acquire connection: {}", e)),
        }
    }
}

/// Health check for HTTP services (hits a URL and checks for 2xx).
pub struct HttpHealth {
    name: String,
    url: String,
    timeout: std::time::Duration,
}

impl HttpHealth {
    pub fn new(name: &str, url: &str) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            timeout: std::time::Duration::from_secs(5),
        }
    }

    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl HealthIndicator for HttpHealth {
    fn name(&self) -> &str { &self.name }

    async fn check(&self) -> HealthStatus {
        let client = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .unwrap();

        match client.get(&self.url).send().await {
            Ok(resp) if resp.status().is_success() => HealthStatus::Up,
            Ok(resp) => HealthStatus::Down(format!("HTTP {}", resp.status())),
            Err(e) => HealthStatus::Down(e.to_string()),
        }
    }
}

/// Disk space health check.
pub struct DiskSpaceHealth {
    path: String,
    threshold_mb: u64,
}

impl HealthIndicator for DiskSpaceHealth {
    fn name(&self) -> &str { "disk" }

    async fn check(&self) -> HealthStatus {
        // Utiliser std::fs ou nix pour vérifier l'espace disque
        // fs2::available_space ou equivalent
        HealthStatus::Up // Placeholder — implémenter avec fs2 ou sys-info
    }
}
```

### Enrichir HealthBuilder
```rust
impl HealthBuilder {
    pub fn new() -> Self { ... }
    pub fn check<H: HealthIndicator>(mut self, indicator: H) -> Self { ... }

    /// Set cache TTL for health check results (default: 5s).
    pub fn cache_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Convenience: add a database pool health check.
    pub fn with_db_pool<DB: sqlx::Database>(self, pool: sqlx::Pool<DB>) -> Self {
        self.check(DatabaseHealth::new(pool))
    }

    /// Convenience: add an HTTP dependency health check.
    pub fn with_http_dependency(self, name: &str, url: &str) -> Self {
        self.check(HttpHealth::new(name, url))
    }

    pub fn build(self) -> crate::plugins::AdvancedHealth { ... }
}
```

### Validation
```bash
cargo test -p r2e-core -- health
```

---

## Étape 7 — Métriques custom déclaratives

**Fichier** : `r2e-macros/src/codegen/handlers.rs` (enrichir)

### Objectif
Permettre d'annoter des méthodes de controller pour auto-enregistrer des métriques.

### API cible
```rust
#[routes]
impl UserController {
    #[get("/")]
    #[counted]                          // Incrémente user_controller_list_total
    #[timed]                            // Enregistre user_controller_list_duration_seconds
    async fn list(&self) -> Json<Vec<User>> { ... }

    #[post("/")]
    #[counted(name = "user_creations")]  // Métrique custom
    async fn create(&self, ...) -> Json<User> { ... }
}
```

### Implémentation dans les macros

Ces annotations sont des variantes simplifiées des intercepteurs existants (`Timed`, `Logged`).
Plutôt que de créer de nouvelles macros complexes, les implémenter comme
des **sucre syntaxique** pour les intercepteurs existants :

```rust
// Dans r2e-macros, quand on parse #[counted]:
// → Générer l'équivalent de #[intercept(Counted::new("controller_method"))]

// Dans r2e-macros, quand on parse #[timed]:
// → Générer l'équivalent de #[intercept(Timed::metric("controller_method"))]
```

### Nouveaux intercepteurs dans r2e-utils

**Fichier** : `r2e-utils/src/counted.rs` (nouveau)
```rust
use r2e_core::interceptors::{Interceptor, InterceptorContext};

/// Interceptor that increments a Prometheus counter on each invocation.
pub struct Counted {
    metric_name: String,
}

impl Counted {
    pub fn new(name: &str) -> Self {
        Self { metric_name: name.to_string() }
    }

    /// Auto-generate metric name from controller + method.
    pub fn auto(controller: &str, method: &str) -> Self {
        Self {
            metric_name: format!("{}_{}_{}", controller, method, "total")
                .to_lowercase()
                .replace('-', "_"),
        }
    }
}

impl<R: Send> Interceptor<R> for Counted {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {
        let metric_name = self.metric_name.clone();
        async move {
            // Enregistrer la métrique via le registre prometheus global
            // On utilise le registre de r2e-prometheus s'il est disponible
            let result = next().await;
            // Incrémenter le compteur
            tracing::debug!(metric = %metric_name, "counted");
            result
        }
    }
}
```

### Note importante
L'intégration avec Prometheus nécessite que le crate `r2e-prometheus` soit présent.
Utiliser le feature flag `prometheus` pour conditionner le code d'enregistrement.
Si prometheus n'est pas activé, les annotations `#[counted]` et `#[timed]`
loguent simplement via `tracing`.

### Reconnaissance dans les macros
**Fichier** : `r2e-macros/src/extract/route.rs`

Lors du parsing d'une méthode de route, vérifier si elle porte `#[counted]` ou `#[timed]`.
Si oui, les transformer en `#[intercept(Counted::auto(...))]` / `#[intercept(Timed::metric(...))]`
dans le code de parsing, avant la phase de codegen.

### Validation
```bash
cargo test -p r2e-utils
cargo test -p r2e-macros
cargo check --workspace
```

---

## Étape 8 — Corrélation trace_id dans les logs

**Fichier** : `r2e-observability/src/tracing_otel.rs` (enrichir)

### Objectif
Quand OpenTelemetry est actif, chaque ligne de log doit inclure le `trace_id`
et le `span_id` courants. Cela permet de retrouver dans les logs toutes les
lignes liées à une requête spécifique.

### Implémentation
Le layer `tracing-opentelemetry` le fait déjà automatiquement pour les spans.
Il faut s'assurer que le format des logs inclut les champs OpenTelemetry.

```rust
// Dans init_tracing, modifier le fmt_layer :
let fmt_layer = tracing_subscriber::fmt::layer()
    .with_target(true)
    .with_thread_ids(false)
    .with_file(false)
    .json(); // Format JSON pour la corrélation — optionnel

// Alternative : format texte avec les champs otel
let fmt_layer = tracing_subscriber::fmt::layer()
    .with_target(true)
    .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE);
```

Le `tracing-opentelemetry` layer attache automatiquement les contextes de span.
Pour que le `trace_id` apparaisse dans les logs texte, il faut un layer custom :

```rust
/// Custom layer that injects trace_id into log events.
pub struct TraceIdLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for TraceIdLayer {
    fn on_event(&self, event: &tracing::Event<'_>, ctx: tracing_subscriber::layer::Context<'_, S>) {
        // Extraire le trace_id du span courant via opentelemetry
        // L'injecter comme champ du log
        // Cela dépend de la version exacte de tracing-opentelemetry
    }
}
```

### Note
La corrélation est automatique si l'utilisateur utilise le format JSON pour les logs.
Pour le format texte, documenter comment activer les spans dans les logs.

### Validation
```bash
cargo run -p example-app
# Vérifier que les logs incluent trace_id quand le plugin est activé
```

---

## Étape 9 — Intégration dans l'example-app

**Fichier** : `example-app/src/main.rs`

### Actions
1. Ajouter `r2e-observability` comme dépendance de l'example-app
2. Remplacer `r2e::init_tracing()` par le plugin `Observability`
3. Ajouter des health checks avancés

```rust
// example-app/src/main.rs

use r2e::r2e_observability::{Observability, ObservabilityConfig};
use r2e_core::health::HealthBuilder;

#[tokio::main]
async fn main() {
    // Ne plus appeler r2e::init_tracing() — le plugin le fait

    let otel_config = ObservabilityConfig::new("r2e-example")
        .with_service_version("0.1.0")
        .with_endpoint("http://localhost:4317")
        .capture_header("x-request-id");

    // Health checks avancés
    let health = HealthBuilder::new()
        .with_db_pool(pool.clone())
        .cache_ttl(std::time::Duration::from_secs(10))
        .build();

    AppBuilder::new()
        .plugin(Scheduler)
        // ... (existing setup)
        .build_state::<Services, _, _>()
        .await
        .with(Observability::new(otel_config))
        .with(health)  // Remplace le simple Health
        // ...
}
```

### application.yaml
```yaml
observability:
  otlp-endpoint: "http://localhost:4317"
  sampling-ratio: 1.0
  tracing:
    enabled: true
  metrics:
    enabled: true
```

### Validation
```bash
cargo run -p example-app
# Vérifier GET /health (nouveau format avec détails)
# Vérifier GET /health/ready
# Si un collecteur OTLP tourne, vérifier les traces dans Jaeger/Grafana
```

---

## Ordre d'implémentation

| # | Étape | Fichiers | Dépendance |
|---|-------|----------|------------|
| 1 | Créer le crate r2e-observability | Cargo.toml, workspace | Aucune |
| 2 | ObservabilityConfig | config.rs | Étape 1 |
| 3 | OpenTelemetry tracing setup | tracing_otel.rs | Étape 2 |
| 4 | Middleware propagation | propagation.rs, middleware.rs | Étape 3 |
| 5 | Plugin assemblage | lib.rs | Étape 4 |
| 6 | Health checks enrichis | r2e-core/src/health.rs | Indépendant |
| 7 | Métriques déclaratives | r2e-macros + r2e-utils | Indépendant |
| 8 | Corrélation trace_id/logs | tracing_otel.rs | Étape 3 |
| 9 | Intégration example-app | example-app/ | Étape 5+6 |

## Nouvelles dépendances Cargo

### r2e-observability
- `opentelemetry = "0.28"`
- `opentelemetry_sdk = { version = "0.28", features = ["rt-tokio"] }`
- `opentelemetry-otlp = "0.28"` (optional, feature-gated)
- `opentelemetry-semantic-conventions = "0.28"`
- `tracing-opentelemetry = "0.28"`

### r2e-core (enrichissement health)
- `reqwest` (optional, pour HttpHealth) — feature-gated
- `serde_json` (déjà présent)

## ⚠️ Notes de compatibilité OpenTelemetry
Les versions d'OpenTelemetry Rust bougent vite. Vérifier les versions
compatibles entre `opentelemetry`, `opentelemetry_sdk`, `opentelemetry-otlp`,
et `tracing-opentelemetry` au moment de l'implémentation. Elles DOIVENT
partager la même version mineure pour être compatibles.

Commande pour vérifier les dernières versions :
```bash
cargo search opentelemetry
cargo search tracing-opentelemetry
```

## Critères de succès
- [ ] `cargo check --workspace` passe
- [ ] `cargo test --workspace` passe
- [ ] L'example-app démarre sans erreur avec le plugin Observability
- [ ] Les traces apparaissent dans un collecteur OTLP (tester avec Jaeger all-in-one)
- [ ] GET /health retourne le format enrichi avec uptime et durées
- [ ] GET /health/ready vérifie la DB
- [ ] Les logs contiennent le trace_id quand OpenTelemetry est actif
- [ ] `#[counted]` et `#[timed]` compilent et fonctionnent
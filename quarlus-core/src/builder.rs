use crate::controller::Controller;
use crate::layers;
use crate::lifecycle::{ShutdownHook, StartupHook};
use tower_http::cors::CorsLayer;
use tracing::info;

/// Builder for assembling a Quarlus application.
///
/// Collects state, controller routes, and Tower layers, then produces an
/// `axum::Router` (or starts serving directly) with everything wired together.
pub struct AppBuilder<T: Clone + Send + Sync + 'static> {
    state: Option<T>,
    routes: Vec<axum::Router<T>>,
    cors: Option<CorsLayer>,
    tracing: bool,
    health: bool,
    error_handling: bool,
    dev_reload: bool,
    config: Option<crate::config::QuarlusConfig>,
    startup_hooks: Vec<StartupHook<T>>,
    shutdown_hooks: Vec<ShutdownHook>,
}

impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    /// Create a new, empty builder.
    pub fn new() -> Self {
        Self {
            state: None,
            routes: Vec::new(),
            cors: None,
            tracing: false,
            health: false,
            error_handling: false,
            dev_reload: false,
            config: None,
            startup_hooks: Vec::new(),
            shutdown_hooks: Vec::new(),
        }
    }

    /// Set the application state that will be shared across all handlers.
    pub fn with_state(mut self, state: T) -> Self {
        self.state = Some(state);
        self
    }

    /// Enable the default (permissive) CORS layer.
    ///
    /// Allows any origin, any method, and any header.
    /// For a stricter configuration use [`with_cors_config`](Self::with_cors_config).
    pub fn with_cors(mut self) -> Self {
        self.cors = Some(layers::default_cors());
        self
    }

    /// Enable CORS with a custom `CorsLayer` configuration.
    pub fn with_cors_config(mut self, cors: CorsLayer) -> Self {
        self.cors = Some(cors);
        self
    }

    /// Enable the Tower tracing layer for HTTP request/response logging.
    pub fn with_tracing(mut self) -> Self {
        self.tracing = true;
        self
    }

    /// Enable a built-in `/health` endpoint that returns `"OK"` with status 200.
    pub fn with_health(mut self) -> Self {
        self.health = true;
        self
    }

    /// Enable structured error handling: catch panics and return JSON 500.
    pub fn with_error_handling(mut self) -> Self {
        self.error_handling = true;
        self
    }

    /// Enable dev-mode endpoints (`/__quarlus_dev/status` and `/__quarlus_dev/ping`).
    ///
    /// These endpoints let tooling (e.g., `quarlus dev`) and browser scripts
    /// detect server restarts for live-reload workflows.
    pub fn with_dev_reload(mut self) -> Self {
        self.dev_reload = true;
        self
    }

    /// Store a `QuarlusConfig` in the builder.
    ///
    /// The config is stored as an Axum extension and can be extracted via
    /// `FromRef` if the user state implements it. This is a convenience
    /// method — you can also embed `QuarlusConfig` directly in your state.
    pub fn with_config(mut self, config: crate::config::QuarlusConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Register a startup hook that runs before the server starts listening.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_start(|state| Box::pin(async move {
    ///         sqlx::query("SELECT 1").execute(&state.pool).await?;
    ///         Ok(())
    ///     }))
    /// ```
    pub fn on_start<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce(T) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'static,
    {
        self.startup_hooks
            .push(Box::new(move |state| Box::pin(hook(state))));
        self
    }

    /// Register a shutdown hook that runs after the server stops.
    ///
    /// # Example
    ///
    /// ```ignore
    /// AppBuilder::new()
    ///     .on_stop(|| Box::pin(async { tracing::info!("Bye"); }))
    /// ```
    pub fn on_stop<F, Fut>(mut self, hook: F) -> Self
    where
        F: FnOnce() -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        self.shutdown_hooks
            .push(Box::new(move || Box::pin(hook())));
        self
    }

    /// Register a raw `axum::Router` fragment to be merged into the application.
    pub fn register_routes(mut self, router: axum::Router<T>) -> Self {
        self.routes.push(router);
        self
    }

    /// Register a [`Controller`] whose routes will be merged into the application.
    pub fn register_controller<C: Controller<T>>(mut self) -> Self {
        self.routes.push(C::routes());
        self
    }

    /// Assemble the final `axum::Router` from all registered routes and layers.
    ///
    /// # Panics
    ///
    /// Panics if [`with_state`](Self::with_state) was never called.
    pub fn build(self) -> axum::Router {
        self.build_inner().0
    }

    fn build_inner(self) -> (axum::Router, Vec<StartupHook<T>>, Vec<ShutdownHook>, T) {
        let state = self
            .state
            .expect("AppBuilder: state must be set before build");

        let mut router = axum::Router::new();

        // Merge all controller / manual routes.
        for r in self.routes {
            router = router.merge(r);
        }

        // Optional built-in health endpoint (added before with_state so it
        // shares the same state type, though the handler ignores state).
        if self.health {
            router = router.route("/health", axum::routing::get(health_handler));
        }

        // Dev-mode reload endpoints.
        if self.dev_reload {
            router = router.merge(crate::dev::dev_routes());
        }

        // Apply the application state.
        let mut app = router.with_state(state.clone());

        // Apply layers. Order matters: the last layer added is the first to
        // process an incoming request, so we add tracing first (outermost)
        // then CORS (inner).
        if let Some(cors) = self.cors {
            app = app.layer(cors);
        }

        if self.error_handling {
            app = app.layer(layers::catch_panic_layer());
        }

        if self.tracing {
            app = app.layer(layers::default_trace());
        }

        (app, self.startup_hooks, self.shutdown_hooks, state)
    }

    /// Build the application and start serving on the given address.
    ///
    /// Runs startup hooks before listening, and shutdown hooks after
    /// graceful shutdown completes.
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let (app, startup_hooks, shutdown_hooks, state) = self.build_inner();

        // Run startup hooks
        for hook in startup_hooks {
            hook(state.clone()).await.map_err(|e| -> Box<dyn std::error::Error> { e })?;
        }

        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(%addr, "Quarlus server listening");
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;

        // Run shutdown hooks
        for hook in shutdown_hooks {
            hook().await;
        }

        info!("Quarlus server stopped");
        Ok(())
    }
}

/// Built-in health-check handler.
async fn health_handler() -> &'static str {
    "OK"
}

/// Wait for a shutdown signal (Ctrl-C or SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to listen for Ctrl-C");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to listen for SIGTERM")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received, starting graceful shutdown");
}

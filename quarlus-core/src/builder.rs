use crate::controller::Controller;
use crate::layers;
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

        // Apply the application state.
        let mut app = router.with_state(state);

        // Apply layers. Order matters: the last layer added is the first to
        // process an incoming request, so we add tracing first (outermost)
        // then CORS (inner).
        if let Some(cors) = self.cors {
            app = app.layer(cors);
        }

        if self.tracing {
            app = app.layer(layers::default_trace());
        }

        app
    }

    /// Build the application and start serving on the given address.
    ///
    /// Prints the listening address to stdout before entering the accept loop.
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let app = self.build();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        info!(%addr, "Quarlus server listening");
        axum::serve(listener, app).await?;
        Ok(())
    }
}

/// Built-in health-check handler.
async fn health_handler() -> &'static str {
    "OK"
}

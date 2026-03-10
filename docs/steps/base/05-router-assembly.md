# Step 5 — Router Assembly and Wiring

## Goal

Complete the `AppBuilder` so that it automatically assembles controller routes, configures Tower layers (CORS, tracing, security), and produces a functional Axum server.

## Files to Modify/Create

```
r2e-core/src/
  builder.rs          # Enhance AppBuilder
  layers.rs           # Tower layer configuration
  lib.rs              # Re-export layers
```

## 1. Enhanced AppBuilder (`builder.rs`)

### Final API

```rust
let app = AppBuilder::new()
    .with_state(services)
    .with_security(security_config)        // Configure JWT/OIDC
    .with_cors(cors_config)                // Configure CORS
    .with_tracing()                        // Enable Tower tracing
    .register_controller::<UserResource>() // Register a controller
    .register_controller::<HealthResource>()
    .build();
```

### `register_controller`

```rust
pub fn register_controller<C: Controller<T>>(mut self) -> Self {
    self.routes.push(C::routes());
    self
}
```

Uses the `Controller<T>` trait implemented by the macros.

### `build()` — Final Assembly

```rust
pub fn build(self) -> axum::Router {
    let state = AppState::new(self.state.expect("state must be set"));

    let mut router = axum::Router::new();

    // Merge all controller routes
    for r in self.routes {
        router = router.merge(r);
    }

    // Apply the state
    let router = router.with_state(state);

    // Apply layers (order matters: last added = first executed)
    let router = self.apply_layers(router);

    router
}
```

## 2. Tower Layers (`layers.rs`)

### CORS

```rust
use tower_http::cors::{CorsLayer, Any};

pub fn default_cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}
```

Also allow custom configuration via `CorsConfig`.

### Tracing

```rust
use tower_http::trace::TraceLayer;

pub fn default_trace() -> TraceLayer<...> {
    TraceLayer::new_for_http()
}
```

### Security Layer

Optional — if `with_security()` is called, inject the `JwtValidator` into request extensions or into the state.

## 3. Health Check Route

Provide an optional built-in controller:

```rust
// In r2e-core
pub async fn health_handler() -> &'static str {
    "OK"
}

// Registered by default if enabled
router = router.route("/health", axum::routing::get(health_handler));
```

## 4. Serve Helper

```rust
impl<T: Clone + Send + Sync + 'static> AppBuilder<T> {
    pub async fn serve(self, addr: &str) -> Result<(), Box<dyn std::error::Error>> {
        let app = self.build();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        println!("Server listening on {}", addr);
        axum::serve(listener, app).await?;
        Ok(())
    }
}
```

## 5. `#[application]` Macro (optional)

If implemented, `#[application]` could:

1. Generate a `main()` that builds the AppBuilder
2. Scan the controllers declared in the same crate
3. Call `.serve()` automatically

For v0.1, this macro is **optional**. The user can write the `main()` manually.

## Validation Criteria

```rust
#[tokio::main]
async fn main() {
    AppBuilder::new()
        .with_state(services)
        .register_controller::<HelloController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}

// curl http://localhost:3000/hello → 200 OK
```

## Dependencies Between Steps

- Requires: step 1 (base AppBuilder), step 3 (Controller trait implemented)
- Blocks: step 6 (example-app)

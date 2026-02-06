# Etape 5 — Assemblage du router et wiring

## Objectif

Completer le `AppBuilder` pour qu'il assemble automatiquement les routes des controllers, configure les layers Tower (CORS, tracing, securite), et produise un serveur Axum fonctionnel.

## Fichiers a modifier/creer

```
r2e-core/src/
  builder.rs          # Enrichir AppBuilder
  layers.rs           # Configuration des layers Tower
  lib.rs              # Re-export layers
```

## 1. AppBuilder enrichi (`builder.rs`)

### API finale

```rust
let app = AppBuilder::new()
    .with_state(services)
    .with_security(security_config)        // Configure JWT/OIDC
    .with_cors(cors_config)                // Configure CORS
    .with_tracing()                        // Active le tracing Tower
    .register_controller::<UserResource>() // Enregistre un controller
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

Utilise le trait `Controller<T>` implemente par les macros.

### `build()` — assemblage final

```rust
pub fn build(self) -> axum::Router {
    let state = AppState::new(self.state.expect("state must be set"));

    let mut router = axum::Router::new();

    // Merge toutes les routes des controllers
    for r in self.routes {
        router = router.merge(r);
    }

    // Appliquer le state
    let router = router.with_state(state);

    // Appliquer les layers (ordre important : dernier ajoute = premier execute)
    let router = self.apply_layers(router);

    router
}
```

## 2. Layers Tower (`layers.rs`)

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

Permettre aussi une configuration custom via `CorsConfig`.

### Tracing

```rust
use tower_http::trace::TraceLayer;

pub fn default_trace() -> TraceLayer<...> {
    TraceLayer::new_for_http()
}
```

### Security Layer

Optionnel — si `with_security()` est appele, injecter le `JwtValidator` dans les extensions de requete ou dans le state.

## 3. Route de healthcheck

Fournir un controller built-in optionnel :

```rust
// Dans r2e-core
pub async fn health_handler() -> &'static str {
    "OK"
}

// Enregistre par defaut si active
router = router.route("/health", axum::routing::get(health_handler));
```

## 4. Serve helper

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

## 5. Macro `#[application]` (optionnelle)

Si implementee, `#[application]` pourrait :

1. Generer un `main()` qui construit l'AppBuilder
2. Scanner les controllers declares dans le meme crate
3. Appeler `.serve()` automatiquement

Pour la v0.1, cette macro est **optionnelle**. L'utilisateur peut ecrire le `main()` manuellement.

## Critere de validation

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

## Dependances entre etapes

- Requiert : etape 1 (AppBuilder base), etape 3 (trait Controller implemente)
- Bloque : etape 6 (example-app)

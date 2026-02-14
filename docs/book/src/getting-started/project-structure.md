# Project Structure

A typical R2E project follows this layout:

```
my-app/
├── Cargo.toml
├── application.yaml              # Base configuration
├── application-dev.yaml          # Dev profile overrides
├── application-prod.yaml         # Prod profile overrides
├── migrations/                   # SQL migrations (if using data features)
│   └── 20250101000001_init.sql
├── src/
│   ├── main.rs                   # Application entry point
│   ├── state.rs                  # AppState definition
│   ├── error.rs                  # Custom error type (optional)
│   ├── models/
│   │   ├── mod.rs
│   │   └── user.rs               # Entity definitions, request/response types
│   ├── services/
│   │   ├── mod.rs
│   │   └── user_service.rs       # Business logic
│   ├── controllers/
│   │   ├── mod.rs
│   │   └── user_controller.rs    # HTTP handlers
│   └── middleware/                # Custom interceptors (optional)
│       ├── mod.rs
│       └── audit_log.rs
└── tests/
    └── user_test.rs              # Integration tests
```

## Key files

### `main.rs` — Application entry point

The main function assembles the application using `AppBuilder`:

```rust
#[tokio::main]
async fn main() {
    r2e::init_tracing();

    let config = R2eConfig::load("dev").unwrap_or_else(|_| R2eConfig::empty());

    AppBuilder::new()
        .with_bean::<UserService>()
        .build_state::<AppState, _>()
        .await
        .with_config(config)
        .with(Health)
        .with(Cors::permissive())
        .with(Tracing)
        .with(ErrorHandling)
        .register_controller::<UserController>()
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

### `state.rs` — Application state

The state struct holds all app-scoped dependencies. `BeanState` derives `FromRef` for each field:

```rust
#[derive(Clone, BeanState)]
pub struct AppState {
    pub user_service: UserService,
    pub pool: SqlitePool,
    pub event_bus: EventBus,
    pub config: R2eConfig,
}
```

### Controllers

Controllers are structs with `#[derive(Controller)]` and an impl block with `#[routes]`. Each method becomes an Axum handler:

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[inject] user_service: UserService,
}

#[routes]
impl UserController {
    #[get("/")]
    async fn list(&self) -> Json<Vec<User>> { ... }
}
```

### Services

Services contain business logic and are injected into controllers via `#[inject]`. Use `#[bean]` on the impl block to register with the DI system:

```rust
#[derive(Clone)]
pub struct UserService { ... }

#[bean]
impl UserService {
    pub fn new(pool: SqlitePool) -> Self { ... }
}
```

### Models

Model files define data types: entities (database-mapped), request/response DTOs, and events:

```rust
#[derive(Clone, Serialize, Deserialize)]
pub struct User { pub id: i64, pub name: String }

#[derive(Deserialize, Validate)]
pub struct CreateUserRequest {
    #[validate(length(min = 1))]
    pub name: String,
}
```

### Configuration files

YAML files with profile-based overrides and env var overlay:

```yaml
# application.yaml — base config
app:
  name: "my-app"
database:
  url: "sqlite:data.db"

# application-dev.yaml — dev overrides
database:
  url: "sqlite::memory:"
```

## Convention over configuration

R2E follows a few conventions:
- Controllers live in `src/controllers/` and are registered explicitly via `register_controller::<T>()`
- Services live in `src/services/` and are registered as beans via `with_bean::<T>()`
- Configuration is loaded from `application.yaml` (with optional profile overrides)
- The state struct uses `#[derive(BeanState)]` for automatic `FromRef` implementations

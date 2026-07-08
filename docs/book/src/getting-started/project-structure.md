# Project Structure

A typical R2E project follows this layout:

```
my-app/
├── Cargo.toml
├── application.yaml              # Configuration
├── migrations/                   # SQL migrations (if using data features)
│   └── 20250101000001_init.sql
├── src/
│   ├── main.rs                   # Application entry point
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

    AppBuilder::new()
        .load_config::<()>()
        .register::<UserService>()
        .build_state()
        .await
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

`build_state()` takes no type arguments — the state type is inferred from the
registered beans. If your bean graph grows past ~127 registrations, add
`#![recursion_limit = "512"]` at the top of `main.rs` (`r2e doctor` warns as you
approach the threshold).

### Application state — inferred, no struct to write

R2E has **no hand-written state struct**. Application state is the *inferred* set
of beans: each value you `.provide(...)` and each type you `.register::<T>()`
forms the bean graph, and `build_state()` materializes it into a state that
controllers and plugins read from *by type*. Config (`R2eConfig`) and typed
`#[config(section)]` children are registered as beans automatically by
`load_config`. There is no `state.rs` file in an R2E project.

### Controllers

Controllers are structs with `#[controller]` and an impl block with `#[routes]`. Each method becomes an Axum handler:

```rust
#[controller(path = "/users")]
pub struct UserController {
    #[inject] user_service: UserService,   // resolved from the bean graph by type
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
    #[garde(length(min = 1))]
    pub name: String,
}
```

### Configuration files

YAML file with env var overlay:

```yaml
# application.yaml
app:
  name: "my-app"
database:
  url: "sqlite:data.db"
```

## Convention over configuration

R2E follows a few conventions:
- Controllers live in `src/controllers/` and are registered explicitly via `register_controller::<T>()`
- Services live in `src/services/` and are registered as beans via `register::<T>()`
- Configuration is loaded from `application.yaml` (with env var overlay)
- There is no state struct — application state is the inferred bean graph, materialized by `build_state()`

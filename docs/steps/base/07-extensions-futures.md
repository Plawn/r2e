# Step 7 — Future Extensions (outside v0.1 scope)

## Goal

Document planned extensions for subsequent versions. These features are **not blocking** for the initial deliverable.

---

## 1. `#[roles("admin", "manager")]`

### Description

Attribute macro on a controller method that restricts access to users having at least one of the specified roles.

### Planned Implementation

```rust
#[get("/admin/users")]
#[roles("admin")]
async fn admin_list(&self) -> Json<Vec<User>> { ... }
```

The `#[controller]` macro generates an additional guard in the handler:

```rust
if !user.roles.iter().any(|r| ["admin"].contains(&r.as_str())) {
    return Err(HttpError::Forbidden("Insufficient roles".into()));
}
```

### Complexity

Low — adding an additional attribute to parse in the controller macro.

---

## 2. `#[transactional]`

### Description

Wraps method execution in an SQL transaction. Automatic commit on success, rollback on error.

### Prerequisites

- SQLx integration in `r2e-core`
- Connection pool in AppState
- `Transactional` trait for services

### Planned Implementation

```rust
#[post("/users")]
#[transactional]
async fn create(&self, Json(body): Json<CreateUser>) -> Json<User> {
    // Everything runs inside a transaction
    self.user_service.create(&body).await?
}
```

### Complexity

Medium — requires passing a `Transaction` or `&mut PgConnection` to services.

---

## 3. `#[config("app.database.url")]`

### Description

Injection of configuration values at compile-time or runtime from an `application.yaml` file or environment variables.

### Planned Implementation

```rust
#[controller(state = Services)]
impl MyController {
    #[config("app.greeting")]
    greeting: String,

    #[get("/hello")]
    async fn hello(&self) -> String {
        self.greeting.clone()
    }
}
```

### Complexity

Medium — requires a configuration system (serde_yaml, dotenv, etc.) integrated into the AppBuilder.

---

## 4. Automatic OpenAPI Generation

### Description

Generate an OpenAPI 3.x spec from annotated controllers.

### Planned Implementation

- Extract routes, HTTP methods, request/response types
- Generate an `openapi.json` or serve it at `/openapi.json`
- Integrate an API documentation interface at `/docs`

### Complexity

High — requires introspection of Serde types to generate JSON schemas.

---

## 5. Dev Mode / Hot Reload

### Description

Automatic recompilation and restart when source files are modified.

### Planned Implementation

- Use `cargo-watch` or `watchexec` externally
- Or integrate a watcher into the dev binary

### Complexity

Low if external (just documentation), high if integrated.

---

## 6. Declarative Custom Middleware

### Description

Allow declaring Tower middlewares via macros:

```rust
#[middleware]
async fn log_request(req: Request, next: Next) -> Response {
    println!("→ {} {}", req.method(), req.uri());
    let response = next.run(req).await;
    println!("← {}", response.status());
    response
}
```

### Complexity

Medium — wrapper around Tower layers.

---

## Suggested Priority

| Extension | Priority | Effort |
|-----------|----------|--------|
| `#[roles]` | High | Low |
| `#[config]` | High | Medium |
| `#[transactional]` | Medium | Medium |
| OpenAPI | Medium | High |
| Custom middleware | Low | Medium |
| Hot reload | Low | Low (external) |

# Configuration

R2E uses YAML-based configuration with profile support and environment variable overlay.

## Configuration files

Create `application.yaml` in your project root:

```yaml
app:
  name: "my-app"
  greeting: "Hello"
  max-retries: 3

database:
  url: "sqlite:data.db"

server:
  port: 8080
```

### Profile overrides

Create profile-specific files that override the base config:

```yaml
# application-dev.yaml
database:
  url: "sqlite::memory:"

# application-prod.yaml
database:
  url: "${DATABASE_URL}"
server:
  port: 80
```

### Loading configuration

```rust
// Load base + profile overrides
let config = R2eConfig::load("dev").unwrap();

// Profile can be set via R2E_PROFILE env var
let config = R2eConfig::load("dev").unwrap(); // R2E_PROFILE=prod overrides

// Empty config for testing
let config = R2eConfig::empty();
```

Resolution order (later wins):
1. `application.yaml`
2. `application-{profile}.yaml`
3. Environment variables

## Environment variable overlay

Environment variables override any YAML key using the convention: dots become underscores, hyphens become underscores, all uppercase.

| YAML key | Environment variable |
|----------|---------------------|
| `app.name` | `APP_NAME` |
| `database.url` | `DATABASE_URL` |
| `app.max-retries` | `APP_MAX_RETRIES` |
| `server.port` | `SERVER_PORT` |

## Using configuration

### In controllers

```rust
#[derive(Controller)]
#[controller(path = "/users", state = AppState)]
pub struct UserController {
    #[config("app.greeting")] greeting: String,
    #[config("app.max-retries")] max_retries: i64,
    #[config("feature.enabled")] enabled: bool,
    #[config("optional.key")] maybe: Option<String>,
}
```

Supported types: `String`, `i64`, `f64`, `bool`, `Option<T>`.

Missing required keys (non-`Option`) panic at request time with a descriptive message including the env var name.

### In beans

```rust
#[bean]
impl NotificationService {
    pub fn new(
        bus: EventBus,
        #[config("notification.capacity")] capacity: i64,
    ) -> Self {
        Self { bus, capacity: capacity as usize }
    }
}
```

### In producers

```rust
#[producer]
async fn create_pool(#[config("database.url")] url: String) -> SqlitePool {
    SqlitePool::connect(&url).await.unwrap()
}
```

### Programmatic access

```rust
let config = R2eConfig::load("dev").unwrap();

// Typed access
let name: String = config.get("app.name").unwrap();
let retries: i64 = config.get("app.max-retries").unwrap();

// With default
let port: i64 = config.get_or("server.port", 3000);

// Manual set
config.set("app.key", ConfigValue::String("value".into()));
```

## Providing config to the app

Pass the config to the builder so controllers can access it:

```rust
let config = R2eConfig::load("dev").unwrap();

AppBuilder::new()
    .provide(config.clone())
    .build_state::<AppState, _>()
    .await
    .with_config(config)  // makes it available for #[config] fields
    // ...
```

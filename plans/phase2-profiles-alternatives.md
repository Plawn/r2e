# Phase 2 — Profiles & Bean Alternatives

**Depends on:** Phase 1 (`Option<T>` + conditional builder methods)

## Goal

Add environment-based profile selection and default/alternative bean patterns, enabling different wiring for dev/test/prod without `if/else` sprawl in main.

---

## Feature A: Profiles

### User-facing API

```yaml
# application.yaml
r2e:
  profile: dev    # or via env: R2E_PROFILE=prod
```

```rust
// Bean registered only in "dev" profile
#[bean(profile = "dev")]
impl FakeMailer {
    fn new() -> Self { Self }
}

// Bean registered only in "prod" profile
#[bean(profile = "prod")]
impl SmtpMailer {
    async fn new(#[config("smtp.host")] host: String) -> Self { ... }
}

// Builder API
AppBuilder::new()
    .load_config::<AppConfig>()
    .with_bean_for_profile::<FakeMailer>("dev")
    .with_async_bean_for_profile::<SmtpMailer>("prod")
    .build_state::<AppState, _, _>().await
```

### Design

#### Active profile resolution

1. Check env var `R2E_PROFILE`
2. Fall back to config key `r2e.profile`
3. Default: `"default"` (matches beans with no `profile` attribute)

Resolved once at `load_config()` time, stored in `BuilderConfig`.

#### Compile-time safety

Profiled beans behave like conditional beans (Phase 1):
- `with_bean_for_profile::<B>("dev")` does **NOT** add `B` to `P`
- Consumers use `Option<T>` — compiler enforces

#### Guaranteed profile groups (advanced)

For cases where exactly one impl is always present:

```rust
// Exactly one of these will be registered — type Mailer IS in P
AppBuilder::new()
    .with_profiled_group::<dyn MailerTrait>()
    .profile_bean::<FakeMailer>("dev")
    .profile_bean::<SmtpMailer>("prod")
    .end_group()  // compile-time: at least one profile must match
```

**Alternative (simpler):** Use a wrapper enum or trait object. The user registers a single type that delegates:

```rust
#[bean]
impl MailerService {
    fn new(config: AppConfig, #[config("r2e.profile")] profile: String) -> Self {
        match profile.as_str() {
            "prod" => Self::Smtp(SmtpMailer::new(config)),
            _ => Self::Fake(FakeMailer),
        }
    }
}
```

**Recommendation:** Start with the simple `with_bean_for_profile` (same as `with_bean_when` but reads the active profile). Defer guaranteed profile groups to later if needed.

### Implementation

#### 1. Store active profile in `BuilderConfig`

```rust
struct BuilderConfig {
    // ... existing fields ...
    active_profile: String,
}
```

Set during `load_config()`:
```rust
let profile = std::env::var("R2E_PROFILE")
    .ok()
    .or_else(|| config.try_get::<String>("r2e.profile").ok())
    .unwrap_or_else(|| "default".to_string());
self.shared.active_profile = profile;
```

#### 2. Builder methods

```rust
pub fn with_bean_for_profile<B: Bean>(mut self, profile: &str) -> AppBuilder<NoState, P, R> {
    if self.shared.active_profile == profile {
        self.shared.bean_registry.register::<B>();
    }
    self.with_updated_types()
}

pub fn with_async_bean_for_profile<B: AsyncBean>(mut self, profile: &str) -> AppBuilder<NoState, P, R> {
    if self.shared.active_profile == profile {
        self.shared.bean_registry.register_async::<B>();
    }
    self.with_updated_types()
}

pub fn with_producer_for_profile<Pr: Producer>(mut self, profile: &str) -> AppBuilder<NoState, P, R> {
    if self.shared.active_profile == profile {
        self.shared.bean_registry.register_producer::<Pr>();
    }
    self.with_updated_types()
}

/// Returns the active profile name.
pub fn active_profile(&self) -> &str {
    &self.shared.active_profile
}
```

#### 3. Macro support (optional — pure sugar)

`#[bean(profile = "dev")]` → generates a marker in the `Bean` impl:

```rust
impl Bean for FakeMailer {
    const PROFILE: Option<&'static str> = Some("dev");
    // ... rest unchanged
}
```

The builder can then auto-filter by profile during registration. But the simpler approach is the builder method — the macro attribute is sugar for later.

---

## Feature B: Default / Alternative Beans

### User-facing API

```rust
// Default impl — always registered
#[bean(default)]
impl InMemoryCache {
    fn new() -> Self { Self { store: HashMap::new() } }
}

// Alternative — replaces default when condition is met
#[bean(alternative, when = "cache.redis.enabled")]
impl RedisCache {
    async fn new(#[config("cache.redis.url")] url: String) -> Self { ... }
}
```

Both `InMemoryCache` and `RedisCache` implement the same contract (e.g., same field type in the state struct, or a shared trait).

### Design

This builds on `allow_overrides` which already exists in `BeanRegistry`.

**Resolution order:**
1. Register the `default` bean (adds to `P`)
2. Register the `alternative` bean — if its condition is met, it overrides the default (same `TypeId`)
3. If condition is NOT met, the alternative is not registered; default stays

**Compile-time safety:**
- The `default` bean IS in `P` — consumers can use `T` directly (not `Option<T>`)
- The `alternative` replaces it at runtime; same type, so `P` is unchanged

### Implementation

#### 1. Builder methods

```rust
/// Register a default bean that can be overridden by alternatives.
/// Adds to the provision list (guaranteed to be present).
pub fn with_default_bean<B: Bean>(mut self) -> AppBuilder<NoState, TCons<B, P>, ...> {
    self.shared.bean_registry.allow_overrides = true;
    self.shared.bean_registry.register::<B>();
    self.with_updated_types()
}

/// Register an alternative bean that replaces the default when condition is true.
/// Does NOT change the provision list (the default already covers it).
pub fn with_alternative_bean_when<B: Bean>(mut self, condition: bool) -> AppBuilder<NoState, P, R> {
    if condition {
        self.shared.bean_registry.register::<B>();  // overrides due to allow_overrides
    }
    self.with_updated_types()
}
```

**Note:** `allow_overrides` is currently a global flag. For fine-grained control, we may need per-registration override flags instead. Add `allow_override: bool` to `BeanRegistration`.

#### 2. Per-registration override flag

```rust
struct BeanRegistration {
    // ... existing fields ...
    /// When true, this registration can be overridden by a later one of the same TypeId.
    overridable: bool,
}
```

The `register_default()` method sets `overridable: true`. The alternative registration replaces only overridable entries.

---

## Test Plan

### Profiles
1. Active profile from env var `R2E_PROFILE`
2. Active profile from config `r2e.profile`
3. Default profile when neither is set
4. Bean registered only for matching profile
5. Bean NOT registered for non-matching profile → consumers get `None`
6. Multiple profiles: only matching beans constructed

### Alternatives
7. Default bean present when no alternative matches
8. Alternative replaces default when condition is true
9. Multiple alternatives — last matching wins
10. Default + alternative produce same type — consumers use `T` directly

---

## File Change Summary

| File | Change |
|------|--------|
| `r2e-core/src/builder.rs` | `active_profile` field, `with_bean_for_profile`, `with_default_bean`, `with_alternative_bean_when` |
| `r2e-core/src/beans.rs` | `overridable` field on `BeanRegistration`, per-registration override logic |
| `r2e-core/src/config.rs` | `try_get::<String>("r2e.profile")` (may already exist from Phase 1) |
| `r2e-macros/src/bean_attr.rs` | (Optional) Parse `#[bean(profile = "...", default, alternative)]` |
| `r2e-core/tests/` | Profile + alternative test cases |

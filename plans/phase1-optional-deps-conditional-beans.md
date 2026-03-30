# Phase 1 — Optional Dependencies + Conditional Bean Registration

## Goal

Add two foundational DI features that preserve compile-time safety:

1. **`Option<T>` injection** — a bean can declare an optional dependency that resolves to `None` if absent
2. **Conditional builder methods** — register beans conditionally at setup time without polluting the compile-time provision list

These two features compose naturally: conditional beans are NOT in `P` (provision list), so consumers MUST use `Option<T>` — the compiler enforces this.

---

## Part A: `Option<T>` Injection

### User-facing API

```rust
// In #[bean] constructor params:
#[bean]
impl NotificationService {
    fn new(mailer: Mailer, cache: Option<RedisClient>) -> Self {
        Self { mailer, cache }
    }
}

// In #[derive(Bean)] fields:
#[derive(Clone, Bean)]
struct MyService {
    #[inject] mailer: Mailer,
    #[inject] cache: Option<RedisClient>,
}

// In BeanState:
#[derive(Clone, BeanState)]
struct AppState {
    user_service: UserService,
    cache: Option<RedisClient>,  // no compile error if RedisClient not in P
}
```

### Compile-time safety rules

| Dependency type | Added to `Deps` (type list) | Added to `dependencies()` (runtime) | Resolution |
|---|---|---|---|
| `T` | Yes | Yes | `ctx.get::<T>()` — panics if absent |
| `Option<T>` | **No** | **No** | `ctx.try_get::<T>()` — returns `None` |

- `Option<T>` deps are invisible to the compile-time graph. No `Contains<T, _>` bound is generated.
- Hard deps (`T`) remain fully checked at compile time.
- `BeanState` fields of type `Option<T>` do NOT generate a `BuildableFrom` bound for `T`.

### Implementation

#### 1. Add utility function: `unwrap_option_type()`

**File:** `r2e-macros/src/type_utils.rs` (new file)

```rust
use syn::Type;

/// If `ty` is `Option<X>` (or `std::option::Option<X>`), return `Some(X)`.
/// Otherwise, return `None`.
pub fn unwrap_option_type(ty: &Type) -> Option<&Type> {
    let Type::Path(type_path) = ty else { return None };
    let segments = &type_path.path.segments;

    // Match `Option<X>` or `std::option::Option<X>`
    let last = segments.last()?;
    if last.ident != "Option" {
        return None;
    }

    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };

    if args.args.len() != 1 {
        return None;
    }

    match &args.args[0] {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}
```

#### 2. Modify `#[bean]` macro — `bean_attr.rs`

**Location:** `r2e-macros/src/bean_attr.rs`, lines 58-107 (constructor param loop)

Current code for non-config params (line 103-107):
```rust
} else {
    dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), ...) });
    dep_types.push(quote! { #ty });
    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
}
```

Change to:
```rust
} else if let Some(inner_ty) = unwrap_option_type(ty) {
    // Optional dependency — NOT added to Deps, uses try_get
    build_args.push(quote! { let #arg_name: #ty = ctx.try_get::<#inner_ty>(); });
} else {
    dep_type_ids.push(quote! { (std::any::TypeId::of::<#ty>(), ...) });
    dep_types.push(quote! { #ty });
    build_args.push(quote! { let #arg_name: #ty = ctx.get::<#ty>(); });
}
```

Key: no `dep_type_ids.push` and no `dep_types.push` for `Option<T>`.

#### 3. Modify `#[derive(Bean)]` — `bean_derive.rs`

**Location:** `r2e-macros/src/bean_derive.rs`, lines 56-59 (inject fields)

Current:
```rust
if is_inject {
    dep_type_ids.push(...);
    dep_types.push(...);
    field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
}
```

Change to:
```rust
if is_inject {
    if let Some(inner_ty) = unwrap_option_type(field_type) {
        // Optional — not in deps, try_get
        field_inits.push(quote! { #field_name: ctx.try_get::<#inner_ty>() });
    } else {
        dep_type_ids.push(...);
        dep_types.push(...);
        field_inits.push(quote! { #field_name: ctx.get::<#field_type>() });
    }
}
```

#### 4. Modify `#[derive(BeanState)]` — `bean_state_derive.rs`

**4a. `from_context()` field init (lines 89-96):**

Current:
```rust
let field_inits: Vec<TokenStream2> = fields.iter().map(|f| {
    let field_name = f.ident.as_ref().unwrap();
    let field_type = &f.ty;
    quote! { #field_name: ctx.get::<#field_type>() }
}).collect();
```

Change to:
```rust
let field_inits: Vec<TokenStream2> = fields.iter().map(|f| {
    let field_name = f.ident.as_ref().unwrap();
    let field_type = &f.ty;
    if let Some(inner_ty) = unwrap_option_type(field_type) {
        quote! { #field_name: ctx.try_get::<#inner_ty>() }
    } else {
        quote! { #field_name: ctx.get::<#field_type>() }
    }
}).collect();
```

**4b. `BuildableFrom` bounds (lines 104-119):**

Current:
```rust
for field in fields {
    let field_type = &field.ty;
    let type_key = type_to_string(field_type);
    if buildable_seen.insert(type_key) {
        // Always add Contains bound
        buildable_bounds.push(quote! { __P: Contains<#field_type, #idx_ident> });
    }
}
```

Change to:
```rust
for field in fields {
    let field_type = &field.ty;

    // Option<T> fields: no Contains bound needed
    if unwrap_option_type(field_type).is_some() {
        continue;
    }

    let type_key = type_to_string(field_type);
    if buildable_seen.insert(type_key) {
        buildable_bounds.push(quote! { __P: Contains<#field_type, #idx_ident> });
    }
}
```

**4c. `FromRef` impls — no change needed.** `Option<T>` as a field type generates `impl FromRef<State> for Option<T>` which works correctly. Controllers extracting `Option<T>` get `None` when the inner bean isn't provided.

#### 5. Modify `#[producer]` — `producer_attr.rs`

Same pattern as `bean_attr.rs`: detect `Option<T>` params in the producer function and use `try_get`.

---

## Part B: Conditional Builder Methods

### User-facing API

```rust
let app = AppBuilder::new()
    .load_config::<AppConfig>()
    .with_bean::<UserService>()               // always in P — guaranteed
    .with_bean_when::<RedisCache>(use_redis)   // NOT in P — consumers use Option<RedisCache>
    .with_async_bean_when::<SmtpMailer>(       // NOT in P
        config.get::<bool>("smtp.enabled").unwrap_or(false)
    )
    .with_bean_on_config::<MetricsCollector>("metrics.enabled")  // sugar for config bool
    .build_state::<AppState, _, _>()
    .await;
```

### Compile-time safety rules

| Method | Added to `P` | Added to `R` | Runtime |
|---|---|---|---|
| `with_bean::<B>()` | Yes (`TCons<B, P>`) | Yes (`B::Deps`) | Always constructed |
| `with_bean_when::<B>(true)` | **No** (P unchanged) | **No** (R unchanged) | Constructed |
| `with_bean_when::<B>(false)` | **No** | **No** | Skipped |
| `with_bean_on_config::<B>("key")` | **No** | **No** | Constructed if key is truthy |

- Conditional beans bypass the compile-time system entirely. Their own deps are validated at runtime by `BeanRegistry::resolve()` (which already checks for missing deps).
- Consumers of conditional beans MUST use `Option<T>` in their constructors/state. Using `T` directly will fail to compile because `T` is not in `P`.

### Implementation

#### 1. Add methods to `AppBuilder` — `builder.rs`

After the existing `with_bean` method (line ~189):

```rust
/// Conditionally register a bean based on a runtime boolean.
///
/// Does NOT add to the provision list — consumers must use `Option<T>`.
/// The bean's own dependencies are checked at runtime during `build_state()`.
///
/// # Example
///
/// ```ignore
/// let use_redis = std::env::var("USE_REDIS").is_ok();
/// AppBuilder::new()
///     .with_bean_when::<RedisCache>(use_redis)
///     .build_state::<AppState, _, _>().await
/// // AppState must use `cache: Option<RedisCache>` (compiler enforces this)
/// ```
pub fn with_bean_when<B: Bean>(mut self, condition: bool) -> AppBuilder<NoState, P, R> {
    if condition {
        self.shared.bean_registry.register::<B>();
    }
    self.with_updated_types()
}

/// Conditionally register an async bean based on a runtime boolean.
///
/// Does NOT add to the provision list — consumers must use `Option<T>`.
pub fn with_async_bean_when<B: AsyncBean>(mut self, condition: bool) -> AppBuilder<NoState, P, R> {
    if condition {
        self.shared.bean_registry.register_async::<B>();
    }
    self.with_updated_types()
}

/// Conditionally register a producer based on a runtime boolean.
///
/// Does NOT add to the provision list — consumers must use `Option<Pr::Output>`.
pub fn with_producer_when<Pr: Producer>(mut self, condition: bool) -> AppBuilder<NoState, P, R> {
    if condition {
        self.shared.bean_registry.register_producer::<Pr>();
    }
    self.with_updated_types()
}

/// Register a bean only if a config key is truthy (bool `true`, non-empty string, etc.).
///
/// Requires `.load_config()` or `.with_config()` to have been called first.
/// Does NOT add to the provision list — consumers must use `Option<T>`.
///
/// # Panics
///
/// Panics if no config has been loaded.
pub fn with_bean_on_config<B: Bean>(mut self, key: &str) -> AppBuilder<NoState, P, R> {
    let enabled = self.shared.config
        .as_ref()
        .expect("with_bean_on_config requires config — call .load_config() first")
        .try_get::<bool>(key)
        .unwrap_or(false);
    if enabled {
        self.shared.bean_registry.register::<B>();
    }
    self.with_updated_types()
}

/// Register an async bean only if a config key is truthy.
pub fn with_async_bean_on_config<B: AsyncBean>(mut self, key: &str) -> AppBuilder<NoState, P, R> {
    let enabled = self.shared.config
        .as_ref()
        .expect("with_async_bean_on_config requires config — call .load_config() first")
        .try_get::<bool>(key)
        .unwrap_or(false);
    if enabled {
        self.shared.bean_registry.register_async::<B>();
    }
    self.with_updated_types()
}
```

#### 2. Verify `R2eConfig::try_get` exists

Check that `R2eConfig` has a `try_get::<bool>(key)` method. If not, add one that returns `Option<T>` or `Result<T, _>` — the current `get::<T>(key)` panics on missing keys.

---

## Test Plan

### Unit tests (r2e-macros)

1. **`Option<T>` in `#[bean]` constructor** — bean builds with `None` when dep absent
2. **`Option<T>` in `#[bean]` constructor** — bean builds with `Some(T)` when dep present
3. **`Option<T>` in `#[derive(Bean)]`** — same two cases
4. **Mixed deps** — bean with both required `T` and optional `Option<U>` compiles when only `T` is provided
5. **`Option<T>` in `#[producer]`** — same pattern

### Integration tests (r2e-core)

6. **`BeanState` with `Option<T>` field** — `build_state` succeeds when inner type not provided
7. **`BeanState` with `Option<T>` field** — `build_state` includes `Some(T)` when type is provided
8. **`BuildableFrom` compile check** — verify that `Option<T>` field does NOT require `T` in `P`
9. **`with_bean_when(false)`** — bean is NOT constructed, consumers get `None`
10. **`with_bean_when(true)`** — bean IS constructed, consumers get `Some(T)`
11. **`with_bean_on_config`** — reads config key and conditionally registers
12. **Conditional bean with missing own deps** — `build_state` returns `BeanError::MissingDependency` (runtime check)
13. **Compile-time error** — using `T` directly when only `with_bean_when` was called should fail to compile (trybuild test)

### Example app

14. Update `example-app` to showcase `Option<T>` injection with a conditional feature

---

## File Change Summary

| File | Change |
|------|--------|
| `r2e-macros/src/type_utils.rs` | **New** — `unwrap_option_type()` |
| `r2e-macros/src/lib.rs` | Add `mod type_utils;` |
| `r2e-macros/src/bean_attr.rs` | `Option<T>` detection in constructor params |
| `r2e-macros/src/bean_derive.rs` | `Option<T>` detection in `#[inject]` fields |
| `r2e-macros/src/bean_state_derive.rs` | `Option<T>` in `from_context()` + skip `BuildableFrom` bound |
| `r2e-macros/src/producer_attr.rs` | `Option<T>` detection in producer params |
| `r2e-core/src/builder.rs` | `with_bean_when`, `with_async_bean_when`, `with_producer_when`, `with_bean_on_config`, `with_async_bean_on_config` |
| `r2e-core/src/config.rs` | Possibly add `try_get::<T>()` if missing |
| `r2e-core/tests/` | New test files for optional deps + conditional registration |

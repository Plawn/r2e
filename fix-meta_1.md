# Fix Proposal: Non-Draining Meta Consumers

## Problem
`AppBuilder::with_meta_consumer` currently calls `MetaRegistry::take`, which
drains metadata. If multiple plugins consume the same metadata type, only the
first one sees any items.

## Goals
- Allow multiple consumers of the same metadata type.
- Preserve an opt-in drain mode for consumers that require ownership.
- Keep runtime behavior monomorphic and build-time only.
- Make ordering deterministic (shared readers before drainers).

## Proposed API
1) Make `with_meta_consumer` **shared (non-draining)** and pass a slice:

```rust
pub fn with_meta_consumer<M, F>(self, f: F) -> Self
where
    M: Any + Send + Sync,
    F: FnOnce(&[M]) -> Router<T> + Send + 'static
```

2) Add `with_meta_consumer_drain` for ownership (old behavior):

```rust
pub fn with_meta_consumer_drain<M, F>(self, f: F) -> Self
where
    M: Any + Send + Sync,
    F: FnOnce(Vec<M>) -> Router<T> + Send + 'static
```

## Detailed Changes
### 1) `r2e-core/src/builder.rs`
- Replace the `MetaConsumer` alias with a struct that carries a mode:

```rust
enum MetaConsumeMode { Shared, Drain }

struct MetaConsumer<T> {
    mode: MetaConsumeMode,
    run: Box<dyn FnOnce(&mut MetaRegistry) -> Router<T> + Send>,
}
```

- Implement the shared version:

```rust
pub fn with_meta_consumer<M, F>(mut self, f: F) -> Self
where
    M: Any + Send + Sync,
    F: FnOnce(&[M]) -> Router<T> + Send + 'static,
{
    self.meta_consumers.push(MetaConsumer {
        mode: MetaConsumeMode::Shared,
        run: Box::new(move |registry| {
            let items: &[M] = registry.get::<M>().unwrap_or(&[]);
            f(items)
        }),
    });
    self
}
```

- Add the drain version:

```rust
pub fn with_meta_consumer_drain<M, F>(mut self, f: F) -> Self
where
    M: Any + Send + Sync,
    F: FnOnce(Vec<M>) -> Router<T> + Send + 'static,
{
    self.meta_consumers.push(MetaConsumer {
        mode: MetaConsumeMode::Drain,
        run: Box::new(move |registry| f(registry.take::<M>())),
    });
    self
}
```

- Ensure shared consumers run before drainers in `build_inner()`:

```rust
let mut meta_registry = self.meta_registry;
let (shared, drain): (Vec<_>, Vec<_>) = self
    .meta_consumers
    .into_iter()
    .partition(|c| matches!(c.mode, MetaConsumeMode::Shared));

for consumer in shared {
    router = router.merge((consumer.run)(&mut meta_registry));
}
for consumer in drain {
    router = router.merge((consumer.run)(&mut meta_registry));
}
```

### 2) `r2e-core/src/meta.rs`
- No structural changes required. `get` already returns `Option<&[M]>`.
- Optional: add a helper `get_or_empty::<M>() -> &[M]` to avoid `unwrap_or(&[])`.

### 3) `r2e-openapi`
- Update the plugin to use the shared consumer:

```rust
app.with_meta_consumer::<RouteInfo, _>(|routes| openapi_routes::<T>(config, routes))
```

- Update `openapi_routes` to accept a slice:

```rust
pub fn openapi_routes<T: Clone + Send + Sync + 'static>(
    config: OpenApiConfig,
    routes: &[RouteInfo],
) -> Router<T> { ... }
```

### 4) Docs / Examples
- Update docs to use the shared consumer.
- Add a short note: use `with_meta_consumer_drain` only if you truly need
  ownership; shared consumers are the default.

## Edge Cases / Semantics
- Multiple shared consumers for the same type all see identical metadata.
- Shared + drain: shared sees data, drain consumes after shared is done.
- If no metadata is registered, the consumer receives an empty slice.

## Tests
Add a unit test in `r2e-core`:
- Define `struct M(u8);`
- Register a controller that pushes two `M` items in `register_meta`.
- Register two shared consumers for `M` that record the observed length.
- Assert both see length 2.
- Add a third drain consumer and assert it sees length 2 while a shared
  consumer registered after drain sees 0 (to confirm ordering).

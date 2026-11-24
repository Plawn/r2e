# Live Configuration

A `#[config("key")]` field is a **copy**: the value is read once when the bean
or controller is built and never changes afterwards. Some values must change
while the app runs: feature flags, timeouts, upstream URLs, rotated
credentials. For these, R2E offers **live config**. A `LiveConfig<T>` handle
reads its key from a registry that config providers (Vault, a remote config
service, …) can update at runtime.

## Setup

No Cargo feature is needed. Everything on this page is part of `r2e-core` and
exported by `r2e::prelude`. The app must call `load_config`, which creates the
`LiveConfigRegistry` bean.

## Declaring a live value

`#[live_config("key")]` works wherever `#[config("key")]` does:
`#[derive(Bean)]`, `#[derive(DecoratorBean)]`, `#[derive(BackgroundService)]`,
`#[bean]` constructor parameters, `#[producer]` parameters and `#[controller]`
structs.

```rust
use r2e::prelude::*;

#[derive(Clone, Bean)]
pub struct PricingService {
    #[live_config("pricing.multiplier")]
    multiplier: LiveConfig<f64>,
}

impl PricingService {
    pub fn price(&self, base: f64) -> Result<f64, ConfigError> {
        Ok(base * self.multiplier.get()?)
    }
}

#[controller(path = "/pricing")]
pub struct PricingController {
    #[live_config("pricing.multiplier")]
    multiplier: LiveConfig<f64>,
}
```

The handle is resolved once, like any app-scoped field. What changes is the
value it reads.

A field cannot be `#[live_config]` and `#[config]`, `#[config_section]`,
`#[inject]` or a request-scoped injection at the same time. Combining them is a
compile error.

## Reading and watching

| Method | Returns | Notes |
|---|---|---|
| `get()` | `Result<T, ConfigError>` | Current value. Cheap enough to call on every request: the conversion is cached until the value changes. |
| `key()` | `&str` | The config key |
| `snapshot()` | `LiveConfigSnapshot` | Version, update time, last error |
| `subscribe()` | `LiveConfigReceiver<T>` | Wait for changes |

`get()` returns `NotFound` while the key has no value. A key present in YAML at
boot starts with that value.

To rebuild a resource whenever the value changes (a client, a pool), use the
receiver's `drive` helper in a background service:

```rust
use r2e::rt::CancelToken;

#[derive(BackgroundService, Clone)]
pub struct SearchClientRefresher {
    #[live_config("search.endpoint")]
    endpoint: LiveConfig<String>,
    #[inject]
    client: SearchClientSlot,
}

impl SearchClientRefresher {
    async fn run(&self, shutdown: CancelToken) {
        let client = self.client.clone();
        self.endpoint
            .subscribe()
            .drive(shutdown, move |value| {
                let client = client.clone();
                async move {
                    match value {
                        Ok(url) => client.reconnect(&url).await,
                        Err(e) => tracing::warn!(error = %e, "invalid search.endpoint"),
                    }
                }
            })
            .await;
    }
}
```

`drive` delivers the current value first, then each update, until shutdown.
For manual control, the receiver also has `changed().await`,
`borrow_and_update()` and `snapshot()`.

## Config providers

A `ConfigProvider` loads values at boot and can optionally watch for changes:

```rust
use std::{future::Future, pin::Pin, sync::Arc};

pub struct RemoteConfig { /* client */ }

impl ConfigProvider for RemoteConfig {
    fn load(&self, config: &mut R2eConfig, _ctx: ConfigProviderContext<'_>) -> Result<(), ConfigError> {
        config.set("pricing.multiplier", ConfigValue::from(1.0));
        Ok(())
    }

    fn watch(
        self: Arc<Self>,
        ctx: ConfigWatchContext,
        sink: ConfigUpdateSink,
    ) -> Pin<Box<dyn Future<Output = Result<(), ConfigError>> + Send + 'static>> {
        Box::pin(async move {
            let shutdown = ctx.shutdown_token();
            loop {
                r2e::rt::select! {
                    _ = shutdown.cancelled() => return Ok(()),
                    _ = r2e::rt::sleep(std::time::Duration::from_secs(30)) => {
                        let value = 1.2; // fetch from the remote source
                        sink.set("pricing.multiplier", value);
                    }
                }
            }
        })
    }
}

AppBuilder::new()
    .with_config_provider(RemoteConfig { /* ... */ })   // before load_config
    .load_config::<()>()
```

- `load` runs inside `load_config`, before typed config is built, so provider
  values are visible to `#[config]` fields and typed sections too. `R2E_*`
  environment variables are applied again after the providers, so they keep the
  highest priority.
- `watch` starts with the server and is supervised. Return `Ok(())` when you
  are done for good (for example on shutdown). An `Err` is logged and `watch`
  is called again after an exponential backoff, capped at 30 s.
- `ConfigUpdateSink::set(key, value)` publishes a value. `set_error` records a
  failure on a key, and `registry()` gives the `LiveConfigRegistry`.

The `LiveConfigRegistry` bean also offers `get`, `set`, `snapshot`,
`set_error` and `is_dead_key`, so other code can publish values without a
provider.

## Rules worth knowing

- **Copied values never change.** `#[config]` fields and typed config structs
  stay boot snapshots, even when a provider later updates their key.
- **Live keys are not checked at startup**, because the value may legitimately
  arrive later. A handle for a key with no value, in an app with no provider,
  logs a `WARN` naming the key, which catches typos.
- **Pinned values.** Values set by `override_config_value` or by an `R2E_*`
  environment variable are pinned. Provider updates for those keys are ignored
  (`set` returns `false`).
- **Datasources** read `datasource.url` as a live value already. Do not write
  your own pool-rotation code.
- **Under `r2e dev`**, editing a live key in YAML pushes the new value into
  existing handles without a rebuild. Editing a copied key rebuilds the beans
  that declare it.

## Testing

Pin a value for a test boot:

```rust
#[r2e::test]
async fn doubles_prices() {
    let app = TestApp::boot_with::<MyApp>(|b| b.override_config_value("pricing.multiplier", 2.0)).await;
    app.get("/pricing/10").send().await.assert_ok();
}
```

To simulate a rotation, leave the key unpinned (set it in
`application-test.yaml`) and publish a new value during the test with
`app.bean::<LiveConfigRegistry>().set("pricing.multiplier", 3.0)`.

## See also

- Full reference: [docs/claude/configuration.md](https://github.com/plawn/r2e/blob/master/docs/claude/configuration.md) (section "Config Providers and Live Config")
- [Configuration](./configuration.md)
- [Executor and Background Services](../events-and-scheduling/background-services.md)

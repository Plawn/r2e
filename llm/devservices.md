---
topic: devservices
features: core (dev-dependency `r2e-devservices`)
tokens: ~2300
requires: testing
---

## Dev Services (containers)

### TL;DR

- A test that needs a real backing service starts it as a container here, then
  points the app at it from the boot hook of llm/testing.md.
- Use `DevPostgres::shared().await` / `DevRedis::shared().await` for one
  container shared by every test process of the workspace session; `start()` /
  `start_with_tag(tag)` stay isolated and handle-scoped.
- Wire it in with `b.override_config_value("app.database.url", pg.url())`.
- Custom image or credentials: `shared_with(PostgresImage::new(...))` /
  `PostgresSpec::default().with_user(...)` — both are part of the container
  identity, so each distinct spec gets its own shared container.
- Any other service: `DevService::shared(DevServiceSpec::new(name, || …))`, with
  no feature flag; build the image against the re-exported `testcontainers` /
  `testcontainers_modules` so versions match.
- Keep the spec closure deterministic — it is called again for the identity and
  on every start attempt, and the shared path panics on a container its name
  does not describe.
- `with_port` resolves a port the image already exposes; it does not publish one.
- Add `with_discriminator("...")` for anything the identity cannot read (ulimits,
  copied file contents, post-start seeding); `shared` panics on a spec that sets
  a host-config modifier without one.
- `DevOpenFga` (feature `openfga`): point the app at it, let the `OpenFga` plugin
  create the store and apply the model, and use a unique `openfga.store` per test.
- `DevKeycloak` (feature `keycloak`): default realm `r2e-mcp` (`mcp-public`,
  `test-cli`, users alice/bob); `kc.issuer()` + `password_token(...)` for OAuth tests.
- Ryuk needs a local Docker Unix socket; `R2E_DEVSERVICES_*` tunes socket path,
  grace period and session key, and `R2E_DEVSERVICES_KEEP=1` disables cleanup.

Integration tests get their backing services as containers:

`r2e-devservices` (features `postgres`, `redis`, `openfga`, `keycloak`):
`DevPostgres::shared().await` = one stable container shared by every test
process in the workspace session; wire via
`b.override_config_value("app.database.url", pg.url())` in the boot hook. A
workspace-scoped `testcontainers/ryuk:0.14.0` reaper holds one TCP lease per
test process and removes the containers after the final process exits
(10-second reconnection grace by default). `start()` / `start_with_tag(tag)`
remain isolated and handle-scoped, with Ryuk as the crash/`SIGKILL` fallback.

`DevPostgres` and `DevRedis` also take a full image (repository + tag), and
`DevPostgres` takes credentials. Both are part of the container's identity, so
two specs differing in either get their own shared container, reused across test
binaries:

```rust
use r2e_devservices::{DevPostgres, DevRedis, PostgresImage, PostgresSpec, RedisImage};

# async fn __doc() {
let pg = DevPostgres::shared_with(PostgresImage::new("pgvector/pgvector", "pg18")).await;
// isolated equivalent: DevPostgres::start_with(...)

let app_db = DevPostgres::shared_with(
    PostgresSpec::default()
        .with_user("app")
        .with_password("s3cret")
        .with_database("appdb"),
)
.await; // url → postgres://app:s3cret@localhost:32771/appdb

let valkey = DevRedis::shared_with(RedisImage::new("valkey/valkey", "8-alpine")).await;
# }
```

`PostgresSpec::default()` is `postgres:16-alpine` with `postgres`/`postgres` on
database `postgres` (a `PostgresImage` converts into a spec), and
`RedisImage::default()` is `redis:7-alpine`, so `shared()` / `start()` /
`start_with_tag(tag)` are unchanged. A Postgres image must speak Postgres on
5432 and honour `POSTGRES_USER`/`POSTGRES_PASSWORD`/`POSTGRES_DB`.

**Any other service.** `DevService` is the generic form behind all of these:
it gives any testcontainers `Image` the same labelling, Ryuk reaping and
cross-process sharing, with no feature flag. `testcontainers` and
`testcontainers_modules` are re-exported so the spec builds against matching
versions:

```rust
use r2e_devservices::testcontainers::core::{IntoContainerPort, WaitFor};
use r2e_devservices::testcontainers::{GenericImage, ImageExt};
use r2e_devservices::{DevService, DevServiceSpec};

# async fn __doc() {
let clickhouse = DevService::shared(
    DevServiceSpec::new("clickhouse", || {
        GenericImage::new("clickhouse/clickhouse-server", "24.8-alpine")
            .with_exposed_port(8123.tcp())
            .with_wait_for(WaitFor::message_on_either_std("Ready for connections"))
            .into()
    })
    .with_port(8123),
)
.await;
let url = format!("http://{}", clickhouse.endpoint(8123));
// DevService::start(spec) for an isolated container
# }
```

A ready-made `testcontainers_modules` image (`ClickHouse`, `Kafka`, `Mongo`, …)
works the same way, but each sits behind its own feature: add
`testcontainers-modules = { version = "0.15", features = ["clickhouse"] }` to
your own `[dev-dependencies]` and Cargo unifies it with the re-export.

The closure builds the request on demand (`ContainerRequest` is not `Clone`
and a contended start is retried) and must be deterministic — it is called
again for the identity and on each start attempt, and the shared path panics
rather than start a container its name does not describe. `with_port` resolves
a port the *image*
exposes (`with_exposed_port` / `Image::expose_ports` / `EXPOSE`) — it does not
publish one. Sharing is keyed on the request fields that shape the container
Docker creates (image, env, labels, cmd, mounts, port mappings, device
requests, network, …), each folded the way Docker resolves it — keyed fields
keep the effective value, set-like fields are sorted, ordered fields stay in
order — so two specs that differ in any of them get two containers;
`with_discriminator("...")` appends to that key for the rest: ulimits
(testcontainers keeps them private), the contents of a file copied by path, and
anything applied after start (seeded data, or exec hooks the image runs
itself). A host-config modifier is refused rather than guessed — `shared`
panics on a spec that sets one without a discriminator, since its effect is a
closure the identity cannot read.

`DevOpenFga` (feature `openfga`) runs `openfga/openfga` with the in-memory
datastore. With the `OpenFga` plugin, a test only points the app at the
container — the plugin creates the store and applies the model at boot; seed
tuples through the typed `FgaClient` bean afterwards. Use a unique
`openfga.store` name per test for isolation on the session-shared container:

```rust
use r2e_devservices::DevOpenFga;
use r2e_test::TestApp;

# async fn __doc(alice: UserRef, doc: DocRef) -> Result<(), Box<dyn std::error::Error>> {
let fga = DevOpenFga::shared().await;
let grpc = fga.grpc_endpoint().to_string();
let app = TestApp::boot_with::<MyApp>(move |b| {
    b.override_config_value("openfga.endpoint", grpc)
        .override_config_value("openfga.store", unique_store_name())
}).await;
app.bean::<FgaClient>().grant(&alice, authz::document::viewer, &doc).await?;
# Ok(()) }
```

For non-plugin wiring, the HTTP bootstrap helpers remain:
`create_store(name) -> store_id`, `write_model(store_id, &json) -> model_id`,
`write_tuples(store_id, model_id, &[(user, relation, object)])`
(`http_endpoint()` backs them). See `examples/example-openfga`.

`DevKeycloak` (feature `keycloak`) runs `quay.io/keycloak/keycloak` in
`start-dev --import-realm` mode. The default import (`DEFAULT_REALM_JSON`)
is realm `r2e-mcp`, built for MCP OAuth tests: public client `mcp-public`
(authorization code + PKCE, localhost/claude.ai redirect URIs, default scope
`mcp` whose audience mapper stamps `http://localhost:3000/mcp` into the token
`aud`, optional scopes `mcp:read`/`mcp:write`), direct-grant client
`test-cli`, confidential `mcp-introspect`/`introspect-secret` (RFC 7662),
users `alice`/`alice-password` (roles admin, user) and `bob`/`bob-password`
(user). `start_with(json)` / `shared_with(json)` import a custom realm —
the JSON is part of the shared-container identity.

```rust
use r2e_devservices::DevKeycloak;

# async fn __doc(builder: AppBuilder) {
let kc = DevKeycloak::shared().await;
let app = builder
    .override_config_value("mcp.auth.issuer", kc.issuer())
    .override_config_value("mcp.auth.allow-insecure", true)
    // must match the realm's audience mapper:
    .override_config_value("mcp.auth.resource", "http://localhost:3000/mcp");
let token = kc.password_token("alice", "alice-password", "test-cli", "mcp:read").await;
// also: kc.client_token(id, secret), kc.admin_token(), kc.base_url(), kc.realm()
# }
```

Ryuk requires a local Docker Unix socket. Configuration:
`R2E_DEVSERVICES_DOCKER_SOCKET` overrides its host path,
`R2E_DEVSERVICES_RYUK_RECONNECTION_TIMEOUT` changes the grace period,
`R2E_DEVSERVICES_RYUK_PRIVILEGED=1` enables privileged mode where required,
and `R2E_DEVSERVICES_SESSION` overrides the workspace-derived session key.
`R2E_DEVSERVICES_KEEP=1` intentionally disables Ryuk and fallback cleanup for
post-mortem inspection.

### Do not

- Do not implement shared dev services with a bare process-local static
  container: Rust statics are not dropped at process exit.

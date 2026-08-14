//! Integration tests booting the **real** app — the same `MultiTenantDbApp`
//! `cargo run` starts, provisioning its own throwaway SQLite files per boot.
//!
//! `#[r2e::test(app = ...)]` boots it; `.as_tenant("acme")` sets the
//! `x-tenant-id` header the app's resolver reads. (`.as_tenant_user(sub,
//! tenant, roles)` is the variant that also mints a JWT carrying a `tenant`
//! claim, for apps whose resolver reads the identity instead of a header.)

use r2e_test::TestApp;
use serde_json::{json, Value};

// ── Isolation: separate databases, not a `WHERE tenant_id = ?` ─────────────

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn each_tenant_reads_its_own_database(app: TestApp) {
    let acme = app.get("/notes").as_tenant("acme").send().await;
    acme.assert_ok().assert_json_path("tenant", "acme");
    assert_eq!(
        acme.json_path::<Vec<String>>("notes"),
        vec!["Ship the beta", "Order more anvils"]
    );

    let globex = app.get("/notes").as_tenant("globex").send().await;
    globex.assert_ok().assert_json_path("tenant", "globex");
    assert_eq!(
        globex.json_path::<Vec<String>>("notes"),
        vec!["Acquire a smaller company"]
    );
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn a_write_for_one_tenant_is_invisible_to_the_other(app: TestApp) {
    app.post("/notes")
        .as_tenant("acme")
        .json(&json!({ "body": "Only acme can see this" }))
        .send()
        .await
        .assert_created()
        .assert_json_path_contains("notes", json!(["Only acme can see this"]));

    let globex = app.get("/notes").as_tenant("globex").send().await;
    assert_eq!(
        globex.json_path::<Vec<String>>("notes"),
        vec!["Acquire a smaller company"],
        "globex's database must be untouched"
    );
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn a_failing_handler_rolls_back_on_the_tenants_own_database(app: TestApp) {
    app.post("/notes/rollback-demo")
        .as_tenant("acme")
        .send()
        .await
        .assert_bad_request();

    let acme = app.get("/notes").as_tenant("acme").send().await;
    assert_eq!(
        acme.json_path::<Vec<String>>("notes").len(),
        2,
        "the insert before the failure must not have committed"
    );
}

// ── Resolution failures: the two statuses a client can hit ────────────────

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn a_request_without_a_tenant_is_a_400(app: TestApp) {
    // `tenancy.on-missing: reject` — a multi-tenant route with no tenant is
    // malformed, and failing closed keeps it from serving anyone's data.
    app.get("/notes").send().await.assert_bad_request();
    app.get("/whoami").send().await.assert_bad_request();
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn an_unknown_tenant_is_a_404_and_leaves_no_pool(app: TestApp) {
    app.get("/notes")
        .as_tenant("ghost")
        .send()
        .await
        .assert_not_found();

    let pools = app.get("/admin/pools").send().await;
    assert_eq!(
        pools.json_path::<Vec<String>>("active"),
        Vec::<String>::new(),
        "an unknown tenant must not leave a pool behind"
    );
    pools.assert_json_path("metrics.unknown", 1);
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn a_malformed_tenant_never_reaches_the_directory(app: TestApp) {
    // Uppercase is not a valid `TenantId`: the resolver rejects it (400)
    // before any lookup, so nothing built from a tenant id can be poisoned.
    app.get("/notes")
        .as_tenant("../etc/passwd")
        .send()
        .await
        .assert_bad_request();
    app.get("/notes")
        .as_tenant("ACME")
        .send()
        .await
        .assert_bad_request();
}

// ── Cascade ───────────────────────────────────────────────────────────────

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn the_client_is_built_on_the_same_tenants_pool(app: TestApp) {
    let acme = app.get("/client").as_tenant("acme").send().await;
    acme.assert_ok()
        .assert_json_path("tenant", "acme")
        .assert_json_path("token", "acme-token-7f3")
        // Queried through the pool the cascade resolved: acme's two seeded
        // notes, not globex's one.
        .assert_json_path("notes_visible_through_the_cascaded_pool", 2);

    let globex = app.get("/client").as_tenant("globex").send().await;
    globex
        .assert_ok()
        .assert_json_path("token", "globex-token-22a")
        .assert_json_path("notes_visible_through_the_cascaded_pool", 1);

    // One creation each, in both maps: the client's `ctx.get::<Pool<Sqlite>>()`
    // reused (or created) the pool rather than opening a second one.
    app.get("/admin/pools")
        .send()
        .await
        .assert_json_path("metrics.created", 2);
    app.get("/admin/clients")
        .send()
        .await
        .assert_json_path("metrics.created", 2);
}

// ── Fallback ──────────────────────────────────────────────────────────────

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn tenants_without_custom_branding_get_the_shared_bean(app: TestApp) {
    app.get("/branding")
        .as_tenant("acme")
        .send()
        .await
        .assert_ok()
        .assert_json_path("branding.theme", "acme-dark")
        .assert_json_path("branding.support_email", "support@acme.example");

    // No theme row value → `Ok(None)` → the app-scoped default, not a 404.
    app.get("/branding")
        .as_tenant("globex")
        .send()
        .await
        .assert_ok()
        .assert_json_path("branding.theme", "r2e-default");

    // Same for a tenant that does not exist at all: `/notes` is a 404 for
    // `ghost`, `/branding` is a 200 — fallback is per resource, not per app.
    app.get("/branding")
        .as_tenant("ghost")
        .send()
        .await
        .assert_ok()
        .assert_json_path("branding.theme", "r2e-default");
}

// ── Admin surface ─────────────────────────────────────────────────────────

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn the_admin_routes_need_no_tenant(app: TestApp) {
    let tenants: Vec<Value> = app.get("/admin/tenants").send().await.json();
    let slugs: Vec<&str> = tenants
        .iter()
        .map(|record| record["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, ["acme", "globex"]);
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn evicting_a_tenant_releases_its_resources(app: TestApp) {
    app.get("/client").as_tenant("acme").send().await.assert_ok();
    app.get("/admin/pools")
        .send()
        .await
        .assert_json_path_contains("active", json!(["acme"]));

    app.post("/admin/tenants/acme/evict")
        .send()
        .await
        .assert_ok()
        .assert_json_path("evicted_pool", true)
        .assert_json_path("evicted_client", true);

    let pools = app.get("/admin/pools").send().await;
    assert_eq!(
        pools.json_path::<Vec<String>>("active"),
        Vec::<String>::new()
    );
    // `PoolSource::dispose` closed the pool rather than leaving it to `Drop`.
    pools.assert_json_path("metrics.disposed", 1);

    // And the next request rebuilds it — eviction is a cache operation, not a
    // deprovisioning.
    app.get("/notes")
        .as_tenant("acme")
        .send()
        .await
        .assert_ok()
        .assert_json_path("tenant", "acme");
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn invalidating_a_tenant_drops_it_without_waiting(app: TestApp) {
    app.get("/notes").as_tenant("globex").send().await.assert_ok();

    app.post("/admin/tenants/globex/invalidate")
        .send()
        .await
        .assert_ok()
        .assert_json_path("invalidated_pool", true);

    // Synchronous drop: the map is empty the moment `invalidate` returns, while
    // the old pool closes on a detached task behind it (unlike `evict`, which
    // awaits the disposal).
    let pools = app.get("/admin/pools").send().await;
    assert_eq!(
        pools.json_path::<Vec<String>>("active"),
        Vec::<String>::new()
    );

    // The next request rebuilds from the source — which is the point: a rotated
    // DSN takes effect without a restart.
    app.get("/notes").as_tenant("globex").send().await.assert_ok();
}

#[r2e::test(app = example_multi_tenant_db::MultiTenantDbApp)]
async fn an_invalid_tenant_id_in_an_admin_path_is_a_400(app: TestApp) {
    app.post("/admin/tenants/NOPE/evict")
        .send()
        .await
        .assert_bad_request();
}

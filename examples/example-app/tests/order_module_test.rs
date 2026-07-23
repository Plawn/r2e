//! Integration test for **module-to-module composition**.
//!
//! `OrderModule` imports `UserModule`'s exports via `imports(module(UserModule))`
//! (see `src/app.rs`). `OrderService` injects the `UserService` bean exported by
//! `UserModule` without `OrderModule` re-listing the `UserService` type. This
//! test boots the real app and proves the cross-module dependency is exercised
//! at runtime: placing an order looks the user up through `UserService` and
//! denormalizes their name onto the returned `Order`.

use example_app::models::Order;
use r2e_test::TestApp;

#[r2e::test(app = example_app::ExampleApp)]
async fn list_orders_starts_empty(app: TestApp) {
    let resp = app.get("/orders").send().await;
    resp.assert_ok();
    let orders: Vec<Order> = resp.json();
    assert!(orders.is_empty());
}

#[r2e::test(app = example_app::ExampleApp)]
async fn place_order_reaches_into_user_module(app: TestApp) {
    // Alice is seeded with id 1 in `UserService::new`.
    let resp = app
        .post("/orders")
        .json(&serde_json::json!({ "user_id": 1, "item": "Widget" }))
        .send()
        .await;
    resp.assert_ok();

    let order: Order = resp.json();
    assert_eq!(order.user_id, 1);
    assert_eq!(order.item, "Widget");
    // The denormalized name proves OrderService reached across into the
    // UserService bean imported from UserModule.
    assert_eq!(order.user_name, "Alice");

    // The order is now visible in the list.
    let resp = app.get("/orders").send().await;
    resp.assert_ok();
    let orders: Vec<Order> = resp.json();
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].user_name, "Alice");
}

#[r2e::test(app = example_app::ExampleApp)]
async fn place_order_for_missing_user_is_404(app: TestApp) {
    app.post("/orders")
        .json(&serde_json::json!({ "user_id": 9999, "item": "Ghost" }))
        .send()
        .await
        .assert_not_found();
}

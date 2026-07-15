use example_app::ExampleApp;

#[r2e::main]
async fn main() {
    r2e::launch!(ExampleApp).await.unwrap();
}

use r2e::prelude::*;

#[derive(Params)]
struct RouteParams {
    #[param(path)]
    id: u64,
    #[param(path, name = "articleSlug")]
    slug: String,
    #[query]
    page: Option<u32>,
}

fn assert_params_metadata<T: r2e::web::params::ParamsMetadata>() {}

fn main() {
    assert_params_metadata::<RouteParams>();
    let params = RouteParams {
        id: 42,
        slug: "rust".to_owned(),
        page: Some(1),
    };
    let _ = (params.id, params.slug, params.page);
}

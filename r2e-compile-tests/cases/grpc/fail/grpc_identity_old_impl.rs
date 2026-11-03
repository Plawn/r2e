//! `GrpcIdentity` was removed (#1213): the trait left behind is an empty
//! `#[deprecated]` stub over an unsatisfiable sealed supertrait, so an old
//! impl fails to build — its items are not members of the trait, the
//! supertrait error (E0277) names the migration page, and so does the
//! deprecation note (`docs/migration/grpc-identity.md`).

use r2e::prelude::*;
use r2e::r2e_grpc::tonic::metadata::MetadataMap;
use r2e::r2e_grpc::GrpcIdentity;

#[derive(Clone)]
pub struct ApiKeyUser {
    key: String,
}

impl Identity for ApiKeyUser {
    fn sub(&self) -> &str {
        &self.key
    }
}

impl GrpcIdentity for ApiKeyUser {
    type Spec = ();

    fn spec() -> Self::Spec {}

    async fn extract(_: &(), metadata: &MetadataMap) -> Result<Self, HttpError> {
        let key = metadata.get("x-api-key").unwrap().to_str().unwrap().to_owned();
        Ok(ApiKeyUser { key })
    }
}

fn main() {}

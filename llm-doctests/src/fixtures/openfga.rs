//! Scaffolding for `llm/openfga.md`.

use r2e::prelude::*;

pub use serde::{Deserialize, Serialize};

pub use r2e::r2e_openfga::{
    FgaCheck, FgaClient, GrpcBackend, MockBackend, OpenFga, OpenFgaConfig, OpenFgaRegistry,
};

// The typed module the snippets refer to as `authz` — generated from the
// same `fga/model.fga` the blocks name.
r2e::r2e_openfga::model!(pub mod authz = "fga/model.fga");

/// The document a route returns.
#[derive(Clone, Serialize)]
pub struct Document {
    pub id: String,
}

/// The body of the update route.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct Update {
    pub title: String,
}

/// The service the guard-idiom controller injects.
#[derive(Clone)]
pub struct DocumentService;

#[bean]
impl DocumentService {
    pub fn new() -> Self {
        Self
    }

    pub async fn load(&self, id: &str) -> Result<Document, HttpError> {
        Ok(Document { id: id.to_string() })
    }

    pub async fn update(&self, id: &str, _body: Update) -> Result<Document, HttpError> {
        Ok(Document { id: id.to_string() })
    }
}

/// Marker for [`DocUser`]'s extraction impl.
pub struct ViaDocUser;

/// A bean-free identity, so the `register_controller` lines of the setup
/// blocks compile on their own. A real app injects `AuthenticatedUser`, whose
/// extraction additionally needs the `Arc<JwtClaimsValidator>` bean (see
/// `llm/security.md`).
#[derive(Clone)]
pub struct DocUser {
    pub sub: String,
}

impl Identity for DocUser {
    fn sub(&self) -> &str {
        &self.sub
    }
}

impl<S: Send + Sync> FromRequestPartsVia<S, ViaDocUser> for DocUser {
    type Rejection = HttpError;

    async fn from_request_parts_via(
        _parts: &mut r2e::http::header::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(DocUser { sub: "alice".into() })
    }
}

/// The controller the setup snippets register.
#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject]
    fga: FgaClient,
    #[inject(identity)]
    user: DocUser,
}

#[routes]
impl DocumentController {
    #[get("/{doc_id}")]
    #[guard(FgaCheck::has(authz::document::viewer).from_path(path::doc_id))]
    async fn view(&self, Path(doc_id): Path<String>) -> Json<Document> {
        let _ = (&self.fga, self.user.sub());
        Json(Document { id: doc_id })
    }
}

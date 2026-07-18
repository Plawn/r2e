//! Fine-grained authorization with OpenFGA (`r2e-openfga`).
//!
//! Each route is guarded by an [`FgaCheck`]: the current user must hold a
//! given relation (`viewer` / `editor`) on the `document:{doc_id}` object.
//! The object ID is resolved from the `{doc_id}` path parameter — the
//! `path::doc_id` descriptor is compile-checked against the route path, so a
//! typo or a param the route does not declare fails to build.
//!
//! The app seeds a few relationship tuples into an in-memory backend at
//! startup (see `app.rs`), so this controller is self-demonstrating without a
//! live OpenFGA server.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use r2e::prelude::*;
use r2e::r2e_openfga::FgaCheck;
use serde::{Deserialize, Serialize};

/// A document exposed by the API.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Document {
    pub id: String,
    pub title: String,
    pub body: String,
}

/// Body for updating a document.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct UpdateDocument {
    pub body: String,
}

/// In-memory document store bean. Seeded with two documents.
#[derive(Clone)]
pub struct DocumentService {
    docs: Arc<RwLock<HashMap<String, Document>>>,
}

impl DocumentService {
    /// Create a store pre-populated with the demo documents (`readme`,
    /// `roadmap`) referenced by the seeded FGA tuples.
    pub fn seeded() -> Self {
        let mut docs = HashMap::new();
        for (id, title) in [("readme", "README"), ("roadmap", "Roadmap")] {
            docs.insert(
                id.to_string(),
                Document {
                    id: id.to_string(),
                    title: title.to_string(),
                    body: format!("Initial contents of {title}."),
                },
            );
        }
        Self {
            docs: Arc::new(RwLock::new(docs)),
        }
    }

    fn get(&self, id: &str) -> Option<Document> {
        self.docs.read().unwrap().get(id).cloned()
    }

    fn update(&self, id: &str, body: String) -> Option<Document> {
        let mut docs = self.docs.write().unwrap();
        let doc = docs.get_mut(id)?;
        doc.body = body;
        Some(doc.clone())
    }
}

#[controller(path = "/documents")]
pub struct DocumentController {
    #[inject]
    documents: DocumentService,

    // Every route authenticates; the FGA guard uses `user.sub()` as the
    // `user:<sub>` subject of the relationship check.
    #[inject(identity)]
    user: AuthenticatedUser,
}

#[routes]
impl DocumentController {
    /// Read a document — requires the `viewer` relation on `document:{doc_id}`.
    #[get("/{doc_id}")]
    #[guard(FgaCheck::relation("viewer").on("document").from_path(path::doc_id))]
    async fn get(&self, Path(doc_id): Path<String>) -> Result<Json<Document>, HttpError> {
        tracing::debug!(user = %self.user.sub(), %doc_id, "reading document");
        self.documents
            .get(&doc_id)
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("document not found".into()))
    }

    /// Update a document — requires the stronger `editor` relation.
    #[put("/{doc_id}")]
    #[guard(FgaCheck::relation("editor").on("document").from_path(path::doc_id))]
    async fn update(
        &self,
        Path(doc_id): Path<String>,
        Json(body): Json<UpdateDocument>,
    ) -> Result<Json<Document>, HttpError> {
        self.documents
            .update(&doc_id, body.body)
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("document not found".into()))
    }
}

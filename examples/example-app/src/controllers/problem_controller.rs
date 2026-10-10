//! Error envelopes in practice (0.5 error projection).
//!
//! Two routes return `Result<T, Problem>`: on them, **every** failure — the
//! handler's own `Err`, a malformed JSON body, a missing `content-type`, a bad
//! `{id}`, a missing bearer token, a garde report — is projected once into
//! [`Problem`] and rendered as `application/problem+json`. Nothing is
//! annotated: the return type is the declaration.
//!
//! The third route is declared infallible (`-> Json<..>`). Its failures still
//! exist (a bad `{id}`) and render with the **app-level** envelope installed in
//! `app.rs` (`.error_projection::<AppError>()` → `{"error": ".."}`): a route
//! that must speak `Problem` has to say so in its signature.

use std::sync::Arc;

use garde::Validate;
use r2e::prelude::*;
use r2e::r2e_security::AuthenticatedUser;
use r2e::rt::sync::RwLock;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::Problem;

#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct Ticket {
    pub id: u32,
    pub title: String,
    pub reporter: String,
}

#[derive(Deserialize, Validate, JsonSchema)]
pub struct CreateTicket {
    #[garde(length(min = 3, max = 80))]
    pub title: String,
}

/// In-memory ticket store (a bean, provided in `app.rs`).
#[derive(Clone)]
pub struct TicketService {
    tickets: Arc<RwLock<Vec<Ticket>>>,
}

impl TicketService {
    pub fn seeded() -> Self {
        let tickets = vec![Ticket {
            id: 1,
            title: "Login page times out".to_string(),
            reporter: "alice".to_string(),
        }];
        Self {
            tickets: Arc::new(RwLock::new(tickets)),
        }
    }

    pub async fn get(&self, id: u32) -> Option<Ticket> {
        self.tickets.read().await.iter().find(|t| t.id == id).cloned()
    }

    pub async fn create(&self, title: String, reporter: String) -> Ticket {
        let mut tickets = self.tickets.write().await;
        let ticket = Ticket {
            id: tickets.len() as u32 + 1,
            title,
            reporter,
        };
        tickets.push(ticket.clone());
        ticket
    }
}

#[controller(path = "/problems")]
pub struct ProblemController {
    #[inject]
    tickets: TicketService,
}

#[routes]
impl ProblemController {
    /// Open a ticket. Identity is extracted **before** the body is read, so a
    /// missing token answers 401 as a `Problem` without touching the payload;
    /// then 415 (no `content-type`), 400 (malformed JSON), 422 (`title` too
    /// short — `Problem::status_of(Validation)` remaps garde's 400) follow,
    /// all in the same shape.
    #[post("/")]
    async fn create(
        &self,
        #[inject(identity)] user: AuthenticatedUser,
        Json(body): Json<CreateTicket>,
    ) -> Result<Json<Ticket>, Problem> {
        let ticket = self.tickets.create(body.title, user.sub().to_string()).await;
        Ok(Json(ticket))
    }

    /// A non-numeric `{id}` is a `Problem` (400, code `InvalidPath`); an
    /// unknown one is the handler's own `Problem` (404).
    #[get("/{id}")]
    async fn get(&self, Path(id): Path<u32>) -> Result<Json<Ticket>, Problem> {
        self.tickets
            .get(id)
            .await
            .map(Json)
            .ok_or_else(|| Problem::not_found(format!("No ticket #{id}")))
    }

    /// Declared infallible: a non-numeric `{id}` renders with the app-level
    /// envelope (`AppError` → `{"error": ".."}`), not as a `Problem`.
    #[get("/legacy/{id}")]
    async fn legacy(&self, Path(id): Path<u32>) -> Json<Option<Ticket>> {
        Json(self.tickets.get(id).await)
    }
}

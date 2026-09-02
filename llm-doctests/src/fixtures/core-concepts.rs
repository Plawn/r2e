//! Scaffolding for `llm/core-concepts.md`.

use r2e::prelude::*;

pub use serde::{Deserialize, Serialize};

/// The entity the routes return.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct User {
    pub id: u64,
    pub name: String,
}

/// Body of `POST /users`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct CreateUser {
    pub name: String,
}

/// Body of `PUT /users/{id}`.
#[derive(Deserialize, schemars::JsonSchema)]
pub struct UpdateUser {
    pub name: String,
}

/// The bean the controller injects.
#[derive(Clone)]
pub struct UserService;

#[bean]
impl UserService {
    pub fn new() -> Self {
        Self
    }

    pub async fn list(&self) -> Vec<User> {
        Vec::new()
    }

    pub async fn get(&self, id: u64) -> Result<User, HttpError> {
        Ok(User { id, name: "ada".into() })
    }

    pub async fn create(&self, body: CreateUser) -> Result<User, HttpError> {
        Ok(User { id: 1, name: body.name })
    }

    pub async fn update(&self, id: u64, body: UpdateUser) -> Result<User, HttpError> {
        Ok(User { id, name: body.name })
    }

    pub async fn delete(&self, _id: u64) -> Result<(), HttpError> {
        Ok(())
    }
}

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::models::User;

#[derive(Clone)]
pub struct UserService {
    users: Arc<RwLock<Vec<User>>>,
}

impl UserService {
    pub fn new() -> Self {
        let users = vec![
            User { id: 1, name: "Alice".into(), email: "alice@example.com".into() },
            User { id: 2, name: "Bob".into(), email: "bob@example.com".into() },
        ];
        Self { users: Arc::new(RwLock::new(users)) }
    }

    pub async fn list(&self) -> Vec<User> {
        self.users.read().await.clone()
    }

    pub async fn get_by_id(&self, id: u64) -> Option<User> {
        self.users.read().await.iter().find(|u| u.id == id).cloned()
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let mut users = self.users.write().await;
        let id = users.len() as u64 + 1;
        let user = User { id, name, email };
        users.push(user.clone());
        user
    }
}

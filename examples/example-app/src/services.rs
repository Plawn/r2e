use std::sync::Arc;
use tokio::sync::RwLock;

use dashmap::DashMap;
use r2e::prelude::*;
use r2e::sse::SseBroadcaster;
use r2e::ws::WsRooms;

use crate::models::{User, UserCreatedEvent};

#[derive(Clone)]
pub struct UserService {
    users: Arc<RwLock<Vec<User>>>,
    event_bus: LocalEventBus,
}

#[bean]
impl UserService {
    pub fn new(event_bus: LocalEventBus) -> Self {
        let users = vec![
            User { id: 1, name: "Alice".into(), email: "alice@example.com".into() },
            User { id: 2, name: "Bob".into(), email: "bob@example.com".into() },
        ];
        Self {
            users: Arc::new(RwLock::new(users)),
            event_bus,
        }
    }

    pub async fn list(&self) -> Vec<User> {
        self.users.read().await.clone()
    }

    pub async fn get_by_id(&self, id: u64) -> Option<User> {
        self.users.read().await.iter().find(|u| u.id == id).cloned()
    }

    pub async fn create(&self, name: String, email: String) -> User {
        let user = {
            let mut users = self.users.write().await;
            let id = users.len() as u64 + 1;
            let user = User { id, name, email };
            users.push(user.clone());
            user
        };
        self.event_bus
            .emit(UserCreatedEvent {
                user_id: user.id,
                name: user.name.clone(),
                email: user.email.clone(),
            })
            .await;
        user
    }

    pub async fn count(&self) -> usize {
        self.users.read().await.len()
    }
}

#[derive(Clone)]
pub struct NotificationService {
    ws_rooms: WsRooms,
    sse_users: Arc<DashMap<String, SseBroadcaster>>,
    capacity: usize,
}

impl NotificationService {
    pub fn new(capacity: usize) -> Self {
        Self {
            ws_rooms: WsRooms::new(capacity),
            sse_users: Arc::new(DashMap::new()),
            capacity,
        }
    }

    pub fn ws_room(&self, user_id: &str) -> r2e::ws::WsBroadcaster {
        self.ws_rooms.room(user_id)
    }

    pub fn sse_broadcaster(&self, user_id: &str) -> SseBroadcaster {
        self.sse_users
            .entry(user_id.to_string())
            .or_insert_with(|| SseBroadcaster::new(self.capacity))
            .clone()
    }

    pub fn notify(&self, user_id: &str, message: &str) {
        self.ws_rooms.room(user_id).send_text(message);

        if let Some(broadcaster) = self.sse_users.get(user_id) {
            let _ = broadcaster.value().send_event("notification", message);
        }
    }
}

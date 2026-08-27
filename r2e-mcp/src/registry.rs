//! Registry that accumulates MCP services during registration.

use std::sync::{Arc, Mutex};

use crate::route::ToolRoute;

/// One registered MCP service: its name and built tool routes.
pub struct RegisteredMcpService {
    /// The service name ([`McpService::service_name`](crate::McpService::service_name)).
    pub name: &'static str,
    /// The service's tool routes, built once from the bean graph at
    /// registration.
    pub tools: Vec<ToolRoute>,
}

/// Registry that accumulates MCP services during registration.
///
/// The MCP peer of `r2e-grpc`'s `GrpcServiceRegistry` (and, like the
/// scheduler's `TaskRegistryHandle`, a coordination datum): deposited into
/// plugin data by the [`McpServer`](crate::McpServer) plugin's `setup()` —
/// **ungated**, so `register_mcp_service` keeps working when `mcp.enabled =
/// false` — populated by `register_mcp_service`, and drained once by the
/// plugin's `wrap_router` closure when the router is assembled.
#[derive(Clone, Default)]
pub struct McpServiceRegistry {
    inner: Arc<Mutex<Vec<RegisteredMcpService>>>,
}

impl McpServiceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a service's built tool routes.
    pub fn add_service(&self, name: &'static str, tools: Vec<ToolRoute>) {
        self.inner
            .lock()
            .unwrap()
            .push(RegisteredMcpService { name, tools });
    }

    /// Drain the registry — or `None` when no service was registered. The
    /// registry is empty afterwards.
    pub fn take(&self) -> Option<Vec<RegisteredMcpService>> {
        let mut guard = self.inner.lock().unwrap();
        if guard.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut *guard))
    }

    /// The names of the currently registered services (without draining).
    pub fn service_names(&self) -> Vec<&'static str> {
        self.inner.lock().unwrap().iter().map(|s| s.name).collect()
    }
}

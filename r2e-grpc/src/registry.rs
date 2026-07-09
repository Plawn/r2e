use std::sync::{Arc, Mutex};

use tonic::service::Routes;

/// Registry that accumulates gRPC services during registration.
///
/// Analogous to [`r2e_core::builder::TaskRegistryHandle`] for the scheduler:
/// stored in `plugin_data` by the `GrpcServer` plugin, populated by
/// `register_grpc_service`, and drained once by the plugin's serve-time
/// wiring (the `on_serve` hook for the separate-port transport, the router
/// layer for the multiplexed transport).
///
/// Services fold into a single [`tonic::service::Routes`] collection so any
/// number of services can share one gRPC endpoint.
#[derive(Clone)]
pub struct GrpcServiceRegistry {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    routes: Routes,
    names: Vec<&'static str>,
}

impl GrpcServiceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    /// Fold a service into the accumulated route collection.
    ///
    /// `add` receives the current [`Routes`] and returns it with the service
    /// added (see [`GrpcService::add_to_routes`](crate::GrpcService::add_to_routes)).
    pub fn add_service<F>(&self, name: &'static str, add: F)
    where
        F: FnOnce(Routes) -> Routes,
    {
        // `add` (service construction from the bean graph) runs under the
        // lock — a re-entrant `register_grpc_service` from inside it would
        // deadlock on this non-reentrant Mutex. Not reachable today:
        // registration is a linear builder chain.
        let mut guard = self.inner.lock().unwrap();
        let routes = std::mem::take(&mut guard.routes);
        guard.routes = add(routes);
        guard.names.push(name);
    }

    /// Drain the registry: the accumulated [`Routes`] plus the registered
    /// service names, or `None` when no service was registered. The registry
    /// is empty afterwards.
    pub fn take(&self) -> Option<(Routes, Vec<&'static str>)> {
        let mut guard = self.inner.lock().unwrap();
        if guard.names.is_empty() {
            return None;
        }
        let inner = std::mem::take(&mut *guard);
        Some((inner.routes, inner.names))
    }

    /// The names of the currently registered services (without draining).
    pub fn service_names(&self) -> Vec<&'static str> {
        self.inner.lock().unwrap().names.clone()
    }
}

impl Default for GrpcServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

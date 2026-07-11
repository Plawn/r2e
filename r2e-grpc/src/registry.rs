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
    descriptors: Vec<&'static [u8]>,
}

/// The accumulated services drained from a [`GrpcServiceRegistry`] at serve
/// time (see [`GrpcServiceRegistry::take`]).
pub struct RegisteredServices {
    /// The folded route collection hosting every registered service.
    pub routes: Routes,
    /// The registered service names, in registration order.
    pub names: Vec<&'static str>,
    /// Encoded `FileDescriptorSet`s collected from services that declared one
    /// (`#[grpc_routes(..., descriptor = ...)]`), deduplicated. Fed to the
    /// reflection service when `GrpcServer::with_reflection()` is enabled.
    pub descriptors: Vec<&'static [u8]>,
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
    /// `descriptor` is the service's encoded file descriptor set, if it
    /// declares one; identical sets (services generated from the same proto
    /// compilation) are stored once.
    pub fn add_service<F>(&self, name: &'static str, descriptor: Option<&'static [u8]>, add: F)
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
        if let Some(descriptor) = descriptor {
            push_unique(&mut guard.descriptors, descriptor);
        }
    }

    /// Drain the registry: the accumulated [`Routes`], the registered service
    /// names, and the collected descriptor sets — or `None` when no service
    /// was registered. The registry is empty afterwards.
    pub fn take(&self) -> Option<RegisteredServices> {
        let mut guard = self.inner.lock().unwrap();
        if guard.names.is_empty() {
            return None;
        }
        let inner = std::mem::take(&mut *guard);
        Some(RegisteredServices {
            routes: inner.routes,
            names: inner.names,
            descriptors: inner.descriptors,
        })
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

/// Store `descriptor` unless an identical set is already present — the single
/// dedup used everywhere descriptor sets accumulate (registry collection and
/// the plugin's reflection extras).
pub(crate) fn push_unique(descriptors: &mut Vec<&'static [u8]>, descriptor: &'static [u8]) {
    if !descriptors.contains(&descriptor) {
        descriptors.push(descriptor);
    }
}

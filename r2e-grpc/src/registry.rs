use std::sync::{Arc, Mutex};

use tonic::service::Routes;

/// Registry that accumulates gRPC services during registration.
///
/// Analogous to [`r2e_core::builder::TaskRegistryHandle`] for the scheduler:
/// stored in `plugin_data` by the `GrpcServer` plugin, populated by
/// `register_grpc_service`, and drained once by the plugin's serve-time
/// wiring (the `on_serve_each_cycle` hook for the separate-port transport, the router
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

/// A service with this name is already registered — the error half of
/// [`GrpcServiceRegistry::add_service`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateService {
    /// The gRPC service's full name (e.g. `greeter.Greeter`).
    pub name: &'static str,
}

impl std::fmt::Display for DuplicateService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "gRPC service '{}' is already registered", self.name)
    }
}

impl std::error::Error for DuplicateService {}

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
    ///
    /// A service name may be registered **once**. A second registration of the
    /// same name — the same service listed by two feature modules, or by a
    /// module and an app-level `register_grpc_service` — returns
    /// [`DuplicateService`] and changes nothing: `add` is not called (the
    /// service is not even built), so tonic never receives two overlapping
    /// route sets for one name and the reported service list stays truthful.
    pub fn add_service<F>(
        &self,
        name: &'static str,
        descriptor: Option<&'static [u8]>,
        add: F,
    ) -> Result<(), DuplicateService>
    where
        F: FnOnce(Routes) -> Routes,
    {
        // `add` (service construction from the bean graph) runs under the
        // lock — a re-entrant `register_grpc_service` from inside it would
        // deadlock on this non-reentrant Mutex. Not reachable today:
        // registration is a linear builder chain.
        let mut guard = self.inner.lock().unwrap();
        if guard.names.contains(&name) {
            return Err(DuplicateService { name });
        }
        let routes = std::mem::take(&mut guard.routes);
        guard.routes = add(routes);
        guard.names.push(name);
        if let Some(descriptor) = descriptor {
            push_unique(&mut guard.descriptors, descriptor);
        }
        Ok(())
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

    /// A copy of what [`take`](Self::take) would return, leaving the registry
    /// as it is — `None` when no service is registered.
    pub(crate) fn snapshot(&self) -> Option<RegisteredServices> {
        let guard = self.inner.lock().unwrap();
        if guard.names.is_empty() {
            return None;
        }
        Some(RegisteredServices {
            routes: guard.routes.clone(),
            names: guard.names.clone(),
            descriptors: guard.descriptors.clone(),
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

/// The services of one app, as the plugin serves them: the registry plus the
/// reflection setting that decorates it on the way out.
///
/// Both transports drain through [`drain`](Self::drain), which keeps a copy of
/// the routes it hands to the transport. That copy — or, for an app that was
/// never served, a snapshot of the still-full registry — is what an in-process
/// test client calls (`r2e-test`'s `grpc` feature), so a test reaches exactly
/// the service set a socket would.
#[derive(Clone)]
pub(crate) struct ServedServices {
    registry: GrpcServiceRegistry,
    drained: Arc<Mutex<Option<Drained>>>,
    #[cfg(feature = "reflection")]
    reflection: Option<Vec<&'static [u8]>>,
}

#[derive(Clone)]
struct Drained {
    routes: Routes,
    names: Vec<&'static str>,
}

impl ServedServices {
    pub(crate) fn new(
        registry: GrpcServiceRegistry,
        #[cfg(feature = "reflection")] reflection: Option<Vec<&'static [u8]>>,
    ) -> Self {
        Self {
            registry,
            drained: Arc::default(),
            #[cfg(feature = "reflection")]
            reflection,
        }
    }

    fn decorate(&self, services: RegisteredServices) -> RegisteredServices {
        #[cfg(feature = "reflection")]
        let services = crate::server::apply_reflection(services, &self.reflection);
        services
    }

    /// Drain the registry for a transport, reflection services included.
    /// `None` when no service was registered.
    pub(crate) fn drain(&self) -> Option<RegisteredServices> {
        let services = self.decorate(self.registry.take()?);
        *self.drained.lock().unwrap() = Some(Drained {
            routes: services.routes.clone(),
            names: services.names.clone(),
        });
        Some(services)
    }

    /// The routes a transport serves (or would serve) and their service
    /// names, without consuming anything.
    pub(crate) fn current(&self) -> Option<(Routes, Vec<&'static str>)> {
        if let Some(drained) = self.drained.lock().unwrap().clone() {
            return Some((drained.routes, drained.names));
        }
        let services = self.decorate(self.registry.snapshot()?);
        Some((services.routes, services.names))
    }
}

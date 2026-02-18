use std::any::Any;
use std::sync::{Arc, Mutex};

/// Registry that collects gRPC service factories during controller registration.
///
/// Analogous to [`r2e_core::builder::TaskRegistryHandle`] for the scheduler.
/// Stored in `plugin_data` by the `GrpcServer` plugin and populated by
/// `register_grpc_service`.
#[derive(Clone)]
pub struct GrpcServiceRegistry {
    inner: Arc<Mutex<Vec<Box<dyn Any + Send>>>>,
}

impl GrpcServiceRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Add a type-erased service factory to the registry.
    pub fn add(&self, factory: Box<dyn Any + Send>) {
        self.inner.lock().unwrap().push(factory);
    }

    /// Take all collected factories, leaving the registry empty.
    pub fn take_all(&self) -> Vec<Box<dyn Any + Send>> {
        std::mem::take(&mut *self.inner.lock().unwrap())
    }
}

impl Default for GrpcServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

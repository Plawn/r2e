use bytes::Bytes;
use std::future::Future;

/// Context passed to each interceptor, including a reference to the
/// application state.
///
/// The state reference allows interceptors to access DI-resolved services,
/// database pools, or any other component in the application state.
pub struct InterceptorContext<'a, S> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub state: &'a S,
}

// Manual Clone/Copy impls are not possible because S may not be Copy.
// InterceptorContext is consumed by value in the around() call.

/// Generic interceptor trait with an `around` pattern.
///
/// Each interceptor wraps the next computation. Interceptors are composed
/// by nesting: the outermost interceptor calls `next()` which runs the
/// next interceptor, and so on.
///
/// Type parameter `R` is the return type of the wrapped computation.
/// Type parameter `S` is the application state type, available via
/// [`InterceptorContext::state`].
///
/// Interceptors that don't need the state use a generic `S: Send + Sync`:
///
/// ```ignore
/// impl<R: Send, S: Send + Sync> Interceptor<R, S> for Logged { ... }
/// ```
///
/// Interceptors that need state access constrain `S` to their concrete type:
///
/// ```ignore
/// impl<R: Send> Interceptor<R, AppState> for AuditInterceptor { ... }
/// ```
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not implement `Interceptor<{R}, {S}>`",
    label = "this type cannot be used as an interceptor",
    note = "implement `Interceptor<R, S>` for your type and apply it with `#[intercept(YourInterceptor)]`"
)]
pub trait Interceptor<R, S> {
    fn around<F, Fut>(
        &self,
        ctx: InterceptorContext<'_, S>,
        next: F,
    ) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}

/// Trait for types that can be cached by the `Cache` interceptor.
///
/// Provides serialization to/from raw bytes for storage in a [`CacheStore`](r2e_cache::CacheStore).
/// Implement this trait for custom response types to enable caching with
/// `#[intercept(Cache::ttl(...))]`.
///
/// # Built-in implementations
///
/// - **`Json<T>`** where `T: Serialize + DeserializeOwned` — serializes the inner value as JSON.
/// - **`Result<T, E>`** where `T: Cacheable` — only `Ok` values are cached; errors are never
///   stored and always pass through. This means `Result<Json<User>, HttpError>` works
///   out of the box.
///
/// # Deriving
///
/// For types that implement `Serialize` and `DeserializeOwned`, use `#[derive(Cacheable)]`
/// to auto-generate a JSON-based implementation:
///
/// ```ignore
/// #[derive(Serialize, Deserialize, Cacheable)]
/// pub struct UserList {
///     pub users: Vec<User>,
///     pub total: usize,
/// }
/// ```
///
/// # Manual implementation
///
/// Implement `Cacheable` manually when you need a custom serialization format
/// (e.g. bincode, MessagePack) or selective caching logic:
///
/// ```ignore
/// use bytes::Bytes;
/// use r2e_core::Cacheable;
///
/// struct CachedReport {
///     data: Vec<u8>,
///     generated_at: Instant,
/// }
///
/// impl Cacheable for CachedReport {
///     fn to_cache(&self) -> Option<Bytes> {
///         // Only cache if the report has data
///         if self.data.is_empty() {
///             return None;
///         }
///         Some(Bytes::from(self.data.clone()))
///     }
///
///     fn from_cache(bytes: &[u8]) -> Option<Self> {
///         Some(CachedReport {
///             data: bytes.to_vec(),
///             generated_at: Instant::now(),
///         })
///     }
/// }
/// ```
///
/// # Returning `None` from `to_cache`
///
/// Return `None` to skip caching for a particular value. This is useful for
/// conditional caching (e.g. empty results, partial data). The `Result<T, E>`
/// implementation uses this to avoid caching error responses.
pub trait Cacheable: Sized + Send {
    /// Serialize this value into bytes for cache storage.
    ///
    /// Return `None` to skip caching (e.g. for empty or error values).
    /// The returned [`Bytes`] is stored directly in the [`CacheStore`](r2e_cache::CacheStore)
    /// with O(1) clone cost.
    fn to_cache(&self) -> Option<Bytes>;

    /// Reconstruct a value from previously cached bytes.
    ///
    /// Return `None` if the bytes cannot be deserialized (e.g. schema changed).
    /// The cache entry will be evicted and the handler will be called normally.
    fn from_cache(bytes: &[u8]) -> Option<Self>;
}

impl<T> Cacheable for axum::Json<T>
where
    T: serde::Serialize + serde::de::DeserializeOwned + Send,
{
    fn to_cache(&self) -> Option<Bytes> {
        serde_json::to_vec(&self.0).ok().map(Bytes::from)
    }

    fn from_cache(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes).ok().map(axum::Json)
    }
}

impl<T, E> Cacheable for Result<T, E>
where
    T: Cacheable,
    E: Send,
{
    fn to_cache(&self) -> Option<Bytes> {
        match self {
            Ok(val) => val.to_cache(),
            Err(_) => None, // never cache errors
        }
    }

    fn from_cache(bytes: &[u8]) -> Option<Self> {
        T::from_cache(bytes).map(Ok)
    }
}

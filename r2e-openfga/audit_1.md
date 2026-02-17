Critical                                                                                                                                                                                                      
                                                                                                                                                                                                            
1. RwLock around the tonic client serializes all requests                                                                                                                                                     
                                                                                                                                                                                                            
GrpcBackend wraps the client in Arc<RwLock<...>> and takes a write lock on every operation. Tonic clients are designed to be cheaply cloned (they share the underlying Channel). The fix is to remove the lock
entirely and clone the client per call:                                                                                                                                                                      
                                                                                                                                                                                                            
// Instead of Arc<RwLock<Client>>
client: OpenFgaServiceClient<Channel>,

// In each method:
let mut client = self.client.clone(); // cheap Arc clone
client.check(request).await?;

2. api_token is configured but never sent

OpenFgaConfig::with_api_token() stores the token, but GrpcBackend::connect() never adds an Authorization interceptor to the tonic channel. Deployments requiring auth (Okta FGA, etc.) will silently connect
without authentication.

3. Object ID injection in FgaGuard

If a path/query/header param contains a colon (e.g. ?doc_id=secret:admin), the guard bypasses the declared object_type and checks against an arbitrary type. The guard declares .on("document") but an
attacker can craft a check against any type. Fix: reject colons in resolved IDs (except for Fixed variant).

Important

4. request_timeout field is stored but never applied — annotated #[allow(dead_code)].

5. No background cache eviction — evict_expired() exists but is never called. Expired entries stay in the DashMap forever (memory leak under load).

6. No cache size limit — unbounded DashMap growth if many unique (user, relation, object) combinations are checked.

7. CacheKey allocated twice per cache miss — same 3 string allocations done once for lookup, then again for insertion.

8. Transitive relationship cache staleness — writing a tuple on org:acme doesn't invalidate cached checks on document:1 even if the model derives permissions transitively. Should be documented prominently.

9. OpenFgaConfig::default() produces invalid config — empty store_id that fails validate(). Misleading Default impl.

10. Missing Deserialize on OpenFgaConfig — can't load from application.yaml via the R2E config system, unlike every other R2E config struct.

Design / Consistency

11. Pin<Box<dyn Future>> vs RPITIT — the rest of R2E uses RPITIT for traits like Guard. OpenFgaBackend uses boxed futures because it needs object safety (Arc<dyn OpenFgaBackend>). This is correct but could
be replaced with enum dispatch to avoid the heap allocation.

12. No Bean/Producer integration — users must manually construct and .provide() the registry. A #[producer] that reads from R2eConfig would match the standard R2E DI pattern.

13. FgaGuard fields are pub — should be private since construction goes through the builder.

14. Missing #[non_exhaustive] on OpenFgaError and ObjectResolver — adding variants later will be a breaking change.

15. Denied variant is never constructed — documented as "not exposed as error" but is public. Dead code.

16. MockBackend doesn't model transitive relationships — should be documented so users understand it only does direct tuple lookup.

Missing Features

17. batch_write / batch_check — OpenFGA supports batch operations; the current API forces N sequential calls.

18. Expand API — critical for debugging why a check returned true/false.

19. ReadTuples API — "who can view document:1?" (the inverse of list_objects).

20. No retry / circuit breaker — if the gRPC connection drops, no application-level recovery.
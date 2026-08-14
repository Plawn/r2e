# W14 tenancy — external audit findings

Status (2026-08-14): findings 1/4/9 are fixed with the private resolve-once
`TenantMemo` cell, the `Tenancy` router layer, and request-head snapshot ordering
after handler parameters. Findings 2/5/6 are fixed under the narrowed contract:
ready-only removal, latched drain and at-most-once disposal, with no leases and a
soft `max_active` cap. Finding 7 is fixed by validating
`TenantId::from_static`. Finding 8 is fixed by the documentation batch. Finding
3 is accepted as a documented limitation: concurrent-root cycles end at
`create-timeout` with a 504 (or hang when the timeout is disabled).

The original report follows verbatim and is intentionally unchanged.

---

# W14 per-tenant bean routing audit

## 1. Verdict

The architecture is coherent at the DI boundary and the ordinary extractor path is well tested, but the implementation is not sound enough to merge as a tenant-isolation feature yet. The single most important fix is to replace the read-only, extractor-dependent `Extensions` memo with a true request-local single-flight tenant cell shared by guards, extractors, and every managed acquisition. As written, a managed-only handler with two resources can resolve two different tenants in one request. `Tenanted<T>` also needs a lifecycle/state-machine pass: eviction can dispose a resource still held by a request, races with creation can skip or duplicate disposal, and cross-root cascade cycles can deadlock until timeout.

## 2. Findings

### CONFIRMED

#### 1. Critical — the “once per request” memo does not cover managed-only or guard-first resolution

**Locations:** `r2e-tenant/src/router.rs:124`, `r2e-tenant/src/router.rs:133`, `r2e-data/backends/sqlx/src/tenant.rs:124`, `r2e-data/backends/sqlx/src/tenant.rs:129`, `r2e-data/backends/diesel/src/tenant.rs:136`, `r2e-macros/src/codegen/handlers.rs:525`

`resolve_parts` is the only path that writes a memo. `RequestHead` is read-only, so `TenantRouter::resolve(&head)` cannot cache its answer. Every generated `#[managed]` acquisition independently receives the same immutable head and the SQLx/Diesel tenant transaction sources independently call the resolver when there was no preceding tenancy extractor.

Concrete failure: a handler has two `#[managed]` tenant resources and no `#[inject(request)] TenantId`/`Tenant<T>` field. An async or stateful resolver returns tenant A on its first call and tenant B on its second (for example, its backing session record changes between awaits). The first transaction opens A's database and the second opens B's database; one handler can now read or write across tenants. Likewise, a custom guard can resolve A through `ctx.head()`, but because it cannot memoize, the managed transaction may subsequently resolve B.

The shipped memo tests do not cover this. `r2e-data/backends/sqlx/tests/tx/tenant.rs:363` and its Diesel twin put a `TenantId` controller field before one transaction, so `resolve_parts` has already written the memo.

**Fix direction:** put a private, shared request-local cell (for example an `Arc<OnceCell<Result<Option<TenantId>, ...>>>` or equivalent framework-owned carrier) in the generated request context before any guard/extractor/resource can run. All three paths must use the same resolve-once operation. Do not use raw `TenantId` as the private marker. Add adversarial two-managed-resource and guard-plus-managed tests whose resolver alternates tenant IDs.

#### 2. High — eviction/drain are not coordinated with creation or active users

**Locations:** `r2e-tenant/src/map.rs:125`, `r2e-tenant/src/map.rs:276`, `r2e-tenant/src/map.rs:358`, `r2e-tenant/src/map.rs:412`, `r2e-tenant/src/map.rs:443`, `r2e-tenant/src/extract.rs:72`, `r2e-data/backends/sqlx/src/tenant.rs:296`

`get` returns a raw clone of `T`; neither `Tenant<T>` nor a managed acquisition retains a slot lease or increments an active-user count. `last_used` records lookup time, not how long the value remains in use. `evict`, idle/LRU sweep, `invalidate`, and `drain` remove a slot and call `dispose` without waiting for request-held clones.

Concrete failure: a request extracts `Tenant<Pool<_>>`, then stalls beyond `idle-ttl` before acquiring a connection. The sweep removes the slot and SQLx `PoolSource::dispose` calls `Pool::close()`. The still-running request owns a clone of that same pool but its later query fails with `PoolClosed`. This contradicts `map.rs:5` (“keeping it alive while it is being used”) and the user guide's claim at `docs/features/24-tenancy.md:261` that in-flight requests finish on the old resource. `Tenant<T>::into_inner` also explicitly permits the clone to escape the request.

There are additional exact race interleavings:

- `evict`/`drain` can remove an empty in-flight slot; `dispose` observes `None`, then creation succeeds into the now-detached slot. The caller gets a value, but the source's `dispose` is never called for the cached copy.
- `drain` snapshots old slots, then removes by tenant ID without `ptr_eq` (`map.rs:413-424`). A concurrent recreate can install a replacement which `drain` removes accidentally and never disposes.
- If `evict` disposes a snapshotted slot while `drain` is between snapshot and disposal, `drain` calls `dispose` on the same old slot again. `TenantSource::dispose` is not required to be idempotent.

**Fix direction:** introduce explicit slot states and leases. Removal should be conditional on slot identity, block/reject new creation during drain, defer disposal until all active leases release, and guarantee exactly-once disposal. If the API intentionally cannot provide this for arbitrary cloneable `T`, narrow and document the contract rather than claiming in-flight safety.

#### 3. High — concurrent roots bypass cascade cycle detection

**Locations:** `r2e-tenant/src/source.rs:128`, `r2e-tenant/src/source.rs:156`, `r2e-tenant/src/map.rs:483`

The cycle chain is local to one recursive call path. It detects sequential A → B → A, and the diamond case is correctly not a cycle. It cannot see a wait-for cycle formed by independent root tasks.

Concrete failure: for one tenant, task 1 starts creating A while task 2 starts creating B. A's source calls `ctx.get::<B>()`; its chain contains only A, so it waits on B's occupied `OnceCell`. B calls `ctx.get::<A>()`; its chain contains only B, so it waits on A. Neither source is re-entered and neither local chain observes the cycle. With the default creation timeout both requests eventually return `TenantError::Timeout` (504), not `Cycle` (500); with timeout disabled they hang indefinitely.

**Fix direction:** track ownership/wait edges across in-flight slots per tenant and detect a cycle before awaiting an occupied cell, or serialize a tenant's cascade through a coordinator that carries a shared dependency graph. Add simultaneous A-root/B-root coverage; the existing test at `r2e-tenant/tests/tenant/cascade.rs:255` is sequential only.

#### 4. High — JWT/extension tenancy fails for parameter-level identities

**Locations:** `r2e-macros/src/codegen/handlers.rs:1467`, `r2e-macros/src/codegen/handlers.rs:1491`, `r2e-macros/src/codegen/handlers.rs:1592`, `r2e-tenant/src/resolver.rs:38`, `r2e-tenant/src/resolver.rs:57`

The generated closure extracts controller request data first, then clones `Extensions` as part of the six-item head prefix, and only then extracts handler parameters, including `#[inject(identity)]` parameters. An identity extractor that inserts a tenant claim therefore runs too late for both cases users need:

- A controller request field `Tenant<T>` is resolved before the parameter identity exists.
- A managed-only route captures an `Extensions` clone before the parameter identity mutates the original request parts, so the later managed resolver cannot see the claim.

Concrete failure: a mostly-public controller protects one route with a parameter-level identity and uses the documented `ExtensionTenantResolver` JWT pattern. A valid JWT contains tenant `acme`, but the route returns the missing-tenant status rather than serving `acme`. Struct-level identity and middleware-populated extensions work, which can mask this in testing. The documentation's unqualified “Extraction order makes this work” is therefore false.

**Fix direction:** ensure all identity extraction which can populate tenancy inputs precedes tenancy resolution and the head snapshot, or make the shared request context observe live memo/extension updates. Add struct-identity and parameter-identity tests separately.

#### 5. Medium — failed/unknown initialization is not single-flight for the waiting wave; panic leaks empty slots

**Locations:** `r2e-tenant/src/map.rs:470`, `r2e-tenant/src/map.rs:483`, `r2e-tenant/src/map.rs:529`, `r2e-tenant/tests/tenant/map.rs:227`

`tokio::sync::OnceCell::get_or_try_init` leaves the cell empty on `Err`. Existing waiters then take turns running the initializer rather than sharing the first error; the test itself acknowledges this at `map.rs:243-244`. They also do not re-check the negative cache inside the initializer loop.

Concrete failure: 100 concurrent first requests for one unknown tenant all pass `negative_hit` before the first result. The first source call returns `Ok(None)` and writes the negative cache, but queued waiters on that same empty cell can each run `source.create` again. Cleanup can remove the mapped slot while an old waiter is retrying it, allowing a new request to create a replacement slot concurrently. The cold 404 wave can therefore hammer the directory despite negative caching, and “one create call” is false for failure/unknown waves.

A panic in `source.create` unwinds past the post-await `remove_if`, leaving the empty slot mapped when there is no waiter to retry it. Repeated hostile tenant IDs that select a panicking source path can accumulate slots.

**Fix direction:** model an in-flight generation whose result, including an error/unknown outcome, is shared with all current waiters, then remove that generation before later callers retry. Use an unwind/cancellation cleanup guard for empty slots and test panic/cancellation explicitly.

#### 6. Medium — the advertised cache/resource caps are not strict under concurrency

**Locations:** `r2e-tenant/src/map.rs:609`, `r2e-tenant/src/map.rs:648`, `r2e-tenant/src/map.rs:683`, `r2e-tenant/tests/tenant/map.rs:275`

The negative-cache `len`/purge/`len`/insert sequence is not atomic. Concurrent unique unknowns can all observe room and insert, exceeding `max_negative` by the number of racing callers.

`max_active` is an asynchronous cleanup target rather than an admission bound. Every concurrent cold tenant may finish creation before detached trimming catches up. In addition, only one trim may be scheduled; completions that see `trimming = true` return without arranging a recheck, so a trim that snapshots too few ready slots can finish after all other completions and leave the map over the cap until the periodic sweep.

Concrete failure: a burst for thousands of cold tenants can open thousands of pools even with `max-active: 100`, exhausting database connections before LRU disposal. A flood of unique unknown IDs can exceed `max-negative`, defeating the claimed memory bound. The current max-active test manually calls `map.sweep()` at `r2e-tenant/tests/tenant/map.rs:285`, so it would pass if automatic enforcement were removed entirely.

**Fix direction:** use atomic admission/accounting (or a semaphore) for live creation, loop/recheck after a trim before clearing the trimming flag, and implement a truly bounded negative structure or serialize its capacity decision.

#### 7. Medium — tenant ID validation is publicly bypassable

**Location:** `r2e-tenant/src/id.rs:60`

`TenantId::parse` correctly enforces `[a-z0-9][a-z0-9._-]{0,62}` and there is deliberately no `Deserialize` implementation. However, safe public `from_static_unchecked` can construct values such as `../secrets`; every custom resolver is free to return that value without parsing. A leaked string can also turn request data into a `'static` input.

Concrete failure: a custom path resolver maps an attacker-controlled branch to `TenantId::from_static_unchecked("../shared")`, and a source uses `tenant.as_str()` in a database filename. The type no longer provides the promised traversal-safe boundary. The shipped example parses HTTP/admin path input, so the example itself does not expose this bypass.

**Fix direction:** make the unchecked constructor private or `unsafe`, or replace it with a compile-time-validating literal macro/const constructor. Keep all safe public constructors validating.

#### 8. Medium — canonical and public documentation contains behaviorally significant mismatches

**Locations:** `llm.txt:2097`, `docs/features/24-tenancy.md:69`, `docs/features/24-tenancy.md:261`, `r2e-tenant/src/plugin.rs:157`, `r2e-tenant/src/plugin.rs:285`, `examples/example-multi-tenant-db/src/controllers.rs:230`, `docs/claude/error-handling.md:187`, `r2e-core/README.md:227`

- `llm.txt` and the user guide say guards, extractors, and managed resources resolve once. Finding 1 disproves that for managed-only, multiple-managed, and guard-first paths.
- The in-flight-resource guarantee at `docs/features/24-tenancy.md:261` is not implemented generically (finding 2).
- `DefaultFallback` rustdoc says missing/tenant-less requests receive the fallback. The fallback is only consulted after `TenantSource::create` returns `Ok(None)`; missing required tenancy fails in `TenantRouter`, while optional/allow yields `None` before the map is called. The user guide and `llm.txt` describe the code correctly; the public rustdoc does not.
- The example controller still says `invalidate` does not dispose. The implementation launches detached disposal.
- The managed-resource examples in `docs/claude/error-handling.md` and `r2e-core/README.md` omit the newly mandatory `ManagedDeps` implementation, so copied custom resources no longer compile. `llm.txt` is accurate here.

**Fix direction:** update all of these in the same fix. In particular, do not paper over findings 1 and 2 by documenting weaker isolation after advertising request consistency and in-flight safety as feature guarantees.

### SUSPECTED (needs a focused repro)

#### 9. Low — raw `TenantId` extension values can be mistaken for framework memos

**Locations:** `r2e-tenant/src/router.rs:124`, `r2e-tenant/src/router.rs:192`

The memo marker is the public `TenantId` type itself, not a private wrapper. Any middleware or earlier extractor that inserts a `TenantId` for another purpose silently bypasses the configured resolver. A request whose header/path says A can therefore be routed as extension B. This is definitely the lookup behavior; what needs confirmation is whether the framework intends every `TenantId` extension to be authoritative by contract.

**Fix direction:** store a private `ResolvedTenant`/memo carrier in extensions and expose controlled accessors, rather than treating every public `TenantId` extension as a completed resolver decision.

## 3. Invariants verified

1. **No new scope / no qualifiers — HOLDS.** The DI graph contains one ordinary `Tenanted<T>` bean per concrete `T`; tenant instances live behind its `DashMap` (`r2e-tenant/src/map.rs:103-128`). No qualifier/name resolution was added. Different backend/resource types remain different `TypeId`s.

2. **`Tenanted<T>` concurrency safety — BROKEN.** DashMap guards are correctly cloned/dropped before awaits (`map.rs:474-485`), sequential successful cold creation is single-flight, failures are removed with `ptr_eq`, and error classification preserves nested `TenantError`. However, findings 2, 5, and 6 break active-use safety, creation/eviction coordination, exactly-once disposal, panic cleanup, failure-wave single-flight, and both bounded-cache claims.

3. **Cascade and cycle detection — BROKEN.** Same-tenant propagation, sequential A → B → A detection, diamond reuse, and nested error classification hold (`source.rs:128-136`, `map.rs:713-725`; tests at `cascade.rs:186`, `:255`, `:277`). Independent root tasks can form an invisible wait-for cycle and produce timeout/hang instead of `TenantError::Cycle` (finding 3).

4. **Fail-closed ordering — HOLDS for supported handler forms.** Generated normal/anonymous/any/fallback routes build the head, construct/check guards, validate, and only then enter interceptor-wrapped or plain managed acquisition (`r2e-macros/src/codegen/handlers.rs:843-920`). Pre-auth guards are outer middleware. SSE and WebSocket handlers do not support `#[managed]`, so acquisition ordering is not applicable there. The SQLx/Diesel rejected-guard tests genuinely prove zero directory lookups/pools for the ordinary guarded route (`sqlx/tests/tx/tenant.rs:348`), though they do not exercise every codegen shape.

5. **Tenant ID validation — BROKEN.** Parsing and built-in resolvers validate the requested grammar; `TenantId` is Serialize-only. Safe public `from_static_unchecked` bypasses the boundary (finding 7).

6. **Memoization contract / A-B consistency — BROKEN.** Controller request data is extracted first, so a handler combining `Tenant<T>`/`TenantId` with one managed transaction reuses the memo, as the backend tests prove. Managed-only multiple resources and guard-first resolution can resolve independently, and parameter-identity extension timing is broken (findings 1 and 4). `Tenant<T>` stores the ID/value pair together, but its raw value clone can outlive map eviction.

7. **Compile-time guarantees — HOLDS for normal plugin wiring.** `controller_deps_fold` appends every distinct managed type's `ManagedDeps::Deps` (`r2e-macros/src/codegen/decorators.rs:620-635`). The trybuild snapshot for `TenantTx` reports both missing `TenantRouter` and missing `Tenanted<Pool<Sqlite>>`; the `Tenant<T>` snapshot fails the real `FromRequestPartsVia`/`HasBean` bound at `register_controller`. These tests are not vacuous and passed. Public `unwired` constructors remain an intentional manual escape hatch that can defer failure to runtime.

8. **Macro blast radius — HOLDS for arity/order; performance changed on guarded routes.** Route closure prefix arithmetic matches `State? + 6`; SSE matches 6; WS matches decorator + 6 = 7 (`handlers.rs:1546`, `:1650`, `:1746`). Fallback/`#[any]` use the common route generator. Routes with neither guards nor managed resources remain on the old simple path and do not extract a head. SSE/WS have no managed parameters. Guarded routes now additionally clone `Extensions` and `Method`, which is a hot-path cost, but no arity or behavioral mismatch was found.

## 4. Perf notes

- A warm `Tenant<T>` request still does a negative-cache DashMap lookup before checking the live slot, then a slot DashMap lookup/`Arc` clone, `OnceCell` load, `last_used` atomic store, hit-counter atomic increment, `TenantId` clone, and `T::clone`. Checking the ready slot before the negative map would remove one shard lock from the overwhelmingly common warm path.
- Header/path resolution allocates a new `Arc<str>` tenant ID per request. Extractor memoization avoids repeats inside the covered request shapes, but managed-only routes resolve/allocate once per managed resource today.
- Contention for one hot tenant concentrates on its DashMap shard plus the same `last_used` and metrics atomics. Many tenants distribute across shards, but `metrics()`/`stats()`/LRU sweep scan the full map and LRU sorts all ready entries.
- Routes with neither guards nor managed resources gain no request-time work. All managed routes now extract/clone State plus six request-head values. Existing guarded, non-tenancy routes newly clone the complete `Extensions` map and `Method`; `Extensions` cost scales with the number of request extensions.
- Failure/unknown waves can serialize repeated directory calls behind one `OnceCell` and then create parallel replacement flights (finding 5). This is a latency and upstream-load amplification risk.
- `max_active` is not an admission bound, so multiplying database `max_connections × max_active` is unsafe as a hard capacity calculation during cold bursts.

## 5. DX friction list

- `TenantedMetrics` and `TenantStats` do not implement `Serialize`; admin endpoints must hand-build JSON.
- `TenantId` deliberately lacks `Deserialize`, which also prevents typed test-response deserialization. A validating `Deserialize` would not itself permit bypassing `parse`, though body-extraction policy would need to remain explicit.
- `Tenant<T>` is only supported as a controller `#[inject(request)]` field, not a handler parameter. A mostly-public controller must carry it on every façade even if one route uses it.
- JWT/extension tenancy works with struct identity or middleware but not parameter-level identity (finding 4). This clashes with the framework's recommended parameter identity pattern for mostly-public controllers.
- `ExtensionTenantResolver` accepts `Fn(&T) -> Option<TenantId>`, so a present malformed claim is indistinguishable from an absent claim. The documented `.ok()` example turns malformed input into the missing-tenant policy; under `on-missing=allow`, an optional route may serve its global view. Add a fallible projection variant.
- SQLx uses `TenantTx<'_, DB>` while Diesel uses `TenantTx<Conn>`; pool customization is `with_options` versus `with_factory`. Backend examples are easy to copy incorrectly.
- SQLx `PoolSource::new` closures receive owned `TenantId`, while `TenantSource::create` itself receives `&TenantId`; custom directory APIs will often pay/handle an avoidable signature mismatch.
- `fallback_to_default` public rustdoc promises missing-tenant fallback, while actual behavior is unknown-source fallback only.
- Per-resource builders expose `max_active`, TTLs, and timeout but not `max_negative`; that knob is global only.
- `invalidate` reports success after spawning disposal, and outside a Tokio runtime silently degrades to drop-without-`dispose` (debug log only). The public contract should state this or offer an awaited form only.
- `Tenant<T>::into_inner`/`into_parts` make it easy to retain a supposedly request-scoped resource beyond eviction, with no lease semantics.
- `max_active(0)` is accepted and creates-then-evicts resources asynchronously rather than being rejected or defined as disabled.
- The new mandatory `ManagedDeps` is a breaking API change (allowed pre-production), but the core README and detailed managed-resource guide still teach the old implementation.

## 6. Test gaps

The following are load-bearing and absent:

- Two managed tenant resources on one managed-only handler, using a counting/alternating resolver; also guard-resolves-then-managed.
- JWT `ExtensionTenantResolver` with both struct-level and parameter-level identity, including a managed-only parameter-identity route.
- Evict, invalidate, idle sweep, and drain racing with slow creation; assert conditional removal and exactly-once disposal.
- Eviction while a request still holds `Tenant<T>` or is between pool lookup and transaction begin; assert the in-flight request completes.
- Concurrent root A and root B with A → B and B → A, both with timeout disabled and enabled; assert `Cycle`, not hang/504.
- Concurrent diamond resolution from independent roots, to ensure the cycle fix does not introduce false positives.
- Concurrent failed and unknown cold waves; assert one source call per wave, all waiters receive the same classification, and a later request retries.
- Panic and cancellation inside `TenantSource::create`; assert no empty slot remains and no resource escapes disposal.
- A concurrent unique-ID flood asserting `negative <= max_negative` at all times.
- Automatic `max_active` enforcement without manually calling `sweep`, plus a concurrent cold burst. The current test at `r2e-tenant/tests/tenant/map.rs:275` only verifies explicit sweep.
- `drain` concurrent with a replacement insertion, and concurrent `evict` + `drain`, using a non-idempotent disposal counter.
- Managed guarded routes with interceptors, `#[anonymous]`, `#[any]`, and fallback/catch-all. Static code inspection supports the ordering, but the sole backend proof is the plain guarded shape.
- Raw `TenantId` extension collision versus configured header/path resolution, followed by an explicit decision on whether it is supported authority or private memo state.
- `fallback_to_default` with a genuinely missing request tenant, so the public rustdoc mismatch cannot persist unnoticed.
- No test asserts `max_negative`; no test asserts that `invalidate` actually calls `dispose`.

Verification performed: `cargo test -p r2e-tenant --test tenant` (88 passed), `cargo test -p r2e-compile-tests --test compile_tests` (2 targets passed), SQLx tenant transaction tests (7 passed), Diesel tenant transaction tests (7 passed), and `git diff --check master...HEAD` (clean). No repository file was modified by this audit.

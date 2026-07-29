# Dev-reload × config semantics

**Status:** **CLOSED — Phases 1, 2 and 3+4 all shipped** (2026-07-29).
B1, B2, B3 and B5 are fixed, B6 is requalified, and **B4 is the only item left
open** (carried to `docs/claude/roadmap.md` § W13). Sections Q1–Q7 keep the
original Phase-0 analysis with the shipped behavior called out inline, so the
reasoning that motivated each fix stays readable.

This document answers Q1–Q7 of the Phase 0 brief: what *exactly* happens to
`R2eConfig`, `LiveConfigRegistry`, typed config beans, late overrides and
config providers when `build_state()` runs a second time inside the Subsecond
hot-patch loop. Every claim carries `file:line` evidence; the behaviors that
can be reproduced in-process are locked by tests in
`r2e-core/tests/runtime/dev_reload_config.rs` (the ones still named
`characterize_*` describe behavior that was *not* changed).

Companion docs: `docs/claude/configuration.md` (config surface),
`docs/claude/beans-di.md` (bean graph), `docs/features/19-sharded-serving.md`
and `docs/features/22-serve-lifecycle.md` (serve/lifecycle).

---

## The model: a config value is either copied or subscribed

Everything below follows from one rule. A config key reaches application code
through exactly one of two modes, and its freshness story is decided by which:

- **Copied** — `#[config("key")]`, `#[config_section(prefix = …)]`,
  `ConfigProperties`, `config.get::<T>(…)`. The value is read once at
  construction and stored by value. Freshness comes from **rebuilding** the
  holder, so these keys are **fingerprinted**: editing one changes the declaring
  bean's per-bean fingerprint and the dev-reload partial rebuild reconstructs it
  (and its dependents). A `#[config_section]` entry declares a dotted **prefix**
  rather than an exact key, and is fingerprinted over the whole subtree under it
  (Phase 3).
- **Subscribed** — `#[live_config("key")]` → `LiveConfig<T>`. The handle binds
  a registry slot at construction and reads through it. Freshness comes from
  **pushing** a new value into the slot, so these keys are deliberately **not
  fingerprinted**: a rebuild would produce an identical handle and change
  nothing. Pushes come from `override_config_value`, `ConfigProvider::watch`,
  and (under dev-reload) the per-cycle re-seed described in Q5.

`ConfigKeyKind::{Required, Optional, Section, Live}` in `config_keys()` is that
rule made machine-readable: `is_required()` drives startup presence validation,
`is_fingerprinted()` (false only for `Live`) drives rebuild invalidation, and
`is_prefix()` (true only for `Section`) switches the hash from
`config_fingerprint(&[key])` to `prefix_fingerprint(prefix)`.
`Optional` is copied-and-absent-is-fine, which is why `required == false`
could not be used as the live discriminator; `Section` is
copied-and-self-validating, for the same reason (`from_config` is its
validator — the field set lives in the typed struct, not in the attribute).

---

## 0. The three cycle shapes

`AppBuilder::try_build_state` (`r2e-core/src/builder/nostate.rs:819-908`) has
three outcomes when `dev::hot_reload_loop_active()`:

| Shape | When | What is reused |
|---|---|---|
| **Cold** | first cycle, or after `invalidate_state_cache()` | nothing |
| **Full-reuse** | graph fingerprint unchanged **and** `!requires_resolution_on_cache_hit()` **and** `Cached<P>` downcasts (`nostate.rs:832-849`) | the whole `(state, ctx)` tuple from the last caching cycle |
| **Partial rebuild** | otherwise (`nostate.rs:855-900`) | beans whose per-bean fingerprint is unchanged; **all provided values except the config-derived ones** |

Because the graph fingerprint is seeded with the **entire** config
(`beans.rs:1301-1311`), *any* config edit — declared by a bean or not — forces
the partial-rebuild shape. Full-reuse therefore only ever happens with a
byte-identical config. This single fact answers most of Q2.

"Config-derived" is the Phase-1 generalization of the old `R2eConfig`-only
exemption: `BeanRegistry` tracks a `config_derived: HashSet<TypeId>` seeded
with `R2eConfig` and `LiveConfigRegistry`, and `load_config` re-provides
everything it derives from the config inside `registry.config_derived_scope(…)`
so typed `ConfigProperties` beans join the set automatically. Members are never
pinned — `load_config` recomputes them from the fresh config on every cycle, so
re-providing them is both safe and cheap.

---

## Q1 — `provided_reuse_clones` semantics

**Which provided beans get pinned?** All of them except the **config-derived**
ones, and except any TypeId in `forced_rebuild`.

```rust
// r2e-core/src/beans.rs (Phase 1)
for (tid, value) in self.provided.iter_mut() {
    if self.config_derived.contains(tid) || forced_rebuild.contains(tid) { continue; }
    if let Some(clone_fn) = self.provided_reuse_clones.get(tid) {
        if let Some(old) = clone_fn(&plan.old_ctx) { *value = old; pinned_provided.insert(*tid); }
    }
}
```

Phase 0 read `if *tid == TypeId::of::<R2eConfig>()` here — a hard-coded single
type. Phase 1 replaced it with membership in `self.config_derived`, populated
declaratively by `config_derived_scope` rather than by a type list, so any
future value `load_config` computes from the config is exempt for free.

`forced_rebuild` (`beans.rs:1360-1390`) is seeded from `deco_fills` targets and
grown along `self.beans`/`self.lazy_beans` dependency edges only — it is a set
of **registered bean** TypeIds and can never contain a provided value's TypeId.
So the exemption clause is dead for provided values: the only escape is being
config-derived.

A clone fn is registered by `provide()`/`pin_provide()`
(`beans.rs:790-812`), so every `.provide()`d value, the `R2eConfig`, the
`LiveConfigRegistry` and every typed `ConfigProperties` struct participate.

**Which instance ends up in the rebuilt context when `load_config` re-provides
a new `R2eConfig` and a `LiveConfigRegistry`?**

- `R2eConfig` → the **new** one (config-derived; the per-patch YAML re-read is
  deliberate).
- `LiveConfigRegistry` → **the same one as last cycle**, but for a different
  reason than in Phase 0. It is no longer pinned by `resolve_reusing`; it is
  config-derived and therefore skipped by the loop. Instead `load_config`
  itself hands back the *carried* instance (Q5) and re-seeds it. Identity is
  now stable **by construction**, not by accident of pinning.
- Typed `ConfigProperties` beans → the **new** ones, rebuilt from the fresh
  config by `load_config` (this is B2's fix).

**Does the answer differ between full-reuse and partial rebuild?** No — the
observable outcome is identical. Full-reuse returns the cached `(state, ctx)`
tuple wholesale (`nostate.rs:843-848`), so the state holds the registry from
the cycle that produced that cache — which is the same carried instance
`load_config` just re-seeded. The two paths differ only in *how* the instance
is retained, never in *which* instance wins.

Locked by
`live_config_registry_keeps_one_identity_and_reseeds_across_cycles`.

---

## Q2 — config freshness after a partial rebuild

**Do rebuilt beans read the new config?** Yes. `resolve_reusing` pins provided
values *before* constructing anything, and `R2eConfig` is the fresh instance,
so every rebuilt factory's `ctx.get::<R2eConfig>()` and every `#[config]`
field resolution sees the edit. Already covered by
`dev_reload.rs` (`state2.get::<ConfDep>().val == "b"`).

**Do controllers?** Yes, on both paths.

- Controller cores are **not** cached: `register_controllers` runs on every
  cycle in both branches (`nostate.rs:843` and `nostate.rs:902`), and
  `ContextConstruct::from_context` re-reads `#[config]` fields from the
  `R2eConfig` in the context each time.
- On the **partial-rebuild** path that context holds the fresh config.
- On the **full-reuse** path the context is cycle-1's, so the config object is
  cycle-1's — but that path is only reachable when the whole-config
  fingerprint is unchanged, i.e. the content is identical. No staleness.

**Is there any path where a controller or bean reads stale config?** Phase 0
found three. Two are fixed; one is intended behavior and stays.

1. ~~**Typed config beans.**~~ **FIXED (B2).** `impl LoadableConfig for
   T: ConfigProperties` still constructs the struct inside `load_config` and
   hands it over with `registry.provide(typed)`, but that now happens inside
   `registry.config_derived_scope(…)`, so the struct's TypeId joins
   `config_derived` and the pinning loop skips it. Every cycle re-provides a
   struct built from the fresh config.
   Locked by `typed_config_bean_tracks_edits_across_cycles`.
2. ~~**Every `#[live_config]` value.**~~ **FIXED (B1).** The registry now has
   one stable identity per process (Q5) and `load_config` re-seeds it on every
   cycle: it swaps the boot snapshot and pushes changed boot values into
   already-materialized slots. A YAML edit to a live key reaches every handle
   without rebuilding anything.
   Locked by `live_key_edit_pushes_without_rebuilding_copied_key_edit_rebuilds`.
3. **Undeclared keys in hand-written beans.** A bean that reads
   `config.get("x")` without listing `"x"` in `config_keys()` keeps its
   per-bean fingerprint stable and is reused with the old value. This is
   documented, intended, and is precisely why the whole-config seed exists
   (it keeps the `R2eConfig` *instance* fresh even so). Not a new finding, and
   not a bug: declaring the key (or using `#[config]`/`#[live_config]`) is the
   fix.

Phase 0 also noted that a `#[live_config]` bean rebuilt on a live-key edge for
nothing (the key was in `config_keys()` and therefore fingerprinted). Phase 2
removed that: `Live` keys are excluded from the per-bean fingerprint, so a
live-key edit reuses every bean and freshness arrives purely by push.

---

## Q3 — what exactly is in the graph fingerprint

`BeanRegistry::compute_fingerprint` (`beans.rs:1294-1332`):

1. **Seed** = `R2eConfig::full_fingerprint()` of the config sitting in
   `self.provided` at fingerprint time (`beans.rs:1296-1311`).
   `full_fingerprint` (`config/mod.rs:450-460`) hashes **every key and value**
   of the resolved map, order-independent (keys sorted).
2. Then each bean's fingerprint, in topological order, is folded in
   (`beans.rs:1324-1330`). Per bean (`beans.rs:1908-1940`):
   `build_version` (constructor token hash) + `is_lazy` +
   `config_fingerprint(declared keys)` + transitive dep fingerprints.
   Since Phase 2, "declared keys" means the **copied** ones only:
   `config_keys()` entries are filtered by `ConfigKeyKind::is_fingerprinted()`,
   which excludes `Live`. A bean whose only config keys are live has a
   fingerprint that never moves on a config edit (the filtered list is empty
   and `config_fingerprint` is skipped entirely, so it does not even fold in
   an empty-hash constant). Since Phase 3 the surviving entries are split by
   `is_prefix()`: exact keys go through `config_fingerprint(&keys)` as before,
   and each `Section` prefix folds in its own `R2eConfig::prefix_fingerprint`
   (prefix + every key under it + values, keys sorted, same hasher shape as
   `full_fingerprint`). Prefixes are sorted and deduped first, so the digest
   does not depend on field declaration order.

The config that gets fingerprinted is the **fully cooked** one, because
`load_config` provides it only after: preloaded/`override_config` or file +
profile overlay → all `ConfigProvider::load` calls → `${...}` placeholder
resolution → `apply_current_env_overlay()` → the `config_overrides` drain
(`nostate.rs:344-395`). So provider-loaded values, env overlay values and
pre-`load_config` overrides are all inside the seed. A **late**
`override_config_value` also lands there: it re-`provide()`s the patched
config (`nostate.rs:565-568`) before `try_build_state` fingerprints it.

Not fingerprinted: the profile *name* (only its effect on values), the
`pinned_keys` set, anything the `LiveConfigRegistry` receives at runtime (the
registry is a provided value, hashed by nothing), and — per bean — its `Live`
config keys.

Note the asymmetry, and that it is deliberate: a live-key edit **still**
changes the whole-config **seed**, so the graph fingerprint still moves and the
partial-rebuild shape still runs. Phase 2 only makes that pass reuse every
bean. Weakening the seed to exclude live keys would reintroduce the stale
`R2eConfig` instance the seed exists to prevent — do not go there.

---

## Q4 — watch-task lifecycle across a hot patch

Per patch, `launch!` re-runs `App::build` and `serve_auto`
(`r2e/src/lib.rs:171-205`), so:

- `load_config` re-runs → each provider's `load` re-runs → **the carried**
  `LiveConfigRegistry` (Phase 0: a new one) and a **new**
  `DeferredAction("ConfigProviderWatch")` capturing it (`nostate.rs:400-430`).
- `AppBuilder::from_pre` runs **all** deferred actions on **every** cycle
  (`builder/typed.rs:104`), so the new action does register a new `on_serve`
  hook against the new registry.
- But `PreparedApp::run_inner` computes
  `let skip_lifecycle = crate::dev::is_lifecycle_initialized();`
  (`builder/prepared.rs:219`) and only drains `self.serve_hooks` inside
  `if !skip_lifecycle` (`prepared.rs:240,268`), marking the flag at
  `prepared.rs:284`. From cycle 2 on, serve hooks never fire.

**Net:** exactly one watch task exists for the whole dev session — cycle 1's,
holding a `ConfigUpdateSink` over cycle 1's registry. **Still true after
Phase 1** (this is B4, out of scope), but now harmless by construction rather
than by luck: cycle 1's registry *is* the carried registry, so the one live
watcher writes to the instance every cycle reads.

**Where does a provider runtime update land after one cycle?** In the carried
registry — the only one there is. Provider pushes remain visible to beans and
controllers across patches, and the Phase-1 re-seed is careful not to trample
them: it only pushes keys whose **boot** value actually changed between
cycles, so a runtime push for an untouched key survives arbitrarily many
reload cycles. Locked by
`runtime_push_survives_an_unrelated_cycle_but_a_real_edit_wins`.

**"Orphan registry" bug — confirmed or refuted?** *Refuted for the provider
path* even in Phase 0. The remaining conclusion is (a) `watch` is **not**
restarted per cycle, so a provider whose `watch` future ends (or whose
subscription is tied to cycle-1 resources) is never revived — B4, still open.
Conclusion (b) from Phase 0 — the orphaned `BuilderConfig.live_config` on
cycles ≥ 2 losing a late `override_config_value` — is **fixed** (B3): with one
stable instance there is nothing to orphan.

Locked by
`characterize_provider_watch_runs_once_but_its_registry_is_the_live_one`
(asserts `load` count 2, `watch` count 1, and that the watch's value is still
readable through cycle 2's state).

---

## Q5 — `LiveConfigRegistry` identity across cycles

**Phase 0 (before):** one instance was constructed per cycle at
`nostate.rs:389` and all but the first were immediately garbage — the pinning
loop replaced the fresh one with cycle 1's. Identity was stable *by accident*,
and the survivor carried a boot snapshot frozen at cycle 1 (B1), while several
holders (`BuilderConfig.live_config`, the per-cycle deferred watch action)
pointed at the doomed instance (B3).

**Phase 1 (now):** `load_config` calls
`live_config_registry_for_cycle(&config, pinned_keys)`
(`r2e-core/src/builder/mod.rs`):

- **Under an active hot-reload loop** — get-or-create from a carrier in
  `crate::dev` (`static LIVE_CONFIG_REGISTRY: OnceLock<Mutex<Option<…>>>`,
  same single-slot shape as `CTX_CACHE`). If one is carried, **re-seed** it
  and return it; otherwise build a fresh one, carry it, and return it.
  `invalidate_state_cache()` drops it, so the documented cold-rebuild escape
  hatch stays cold for live config too.
- **Otherwise** (production, tests without the gate) — build a fresh registry
  per `load_config` and carry nothing. Byte-identical to Phase 0 behavior.
  Locked by `without_the_hot_reload_gate_each_load_config_builds_its_own_registry`.

The re-seed (`LiveConfigRegistry::reseed`, `config/runtime.rs`, gated
`#[cfg(feature = "dev-reload")]`) is deliberately **diff-based**:

1. Swap the `BootSnapshot` (`{ config, at }` behind an `RwLock`), keeping the
   old one.
2. Replace the pinned set wholesale with the new cycle's (env-overlay keys +
   drained overrides) — replace, never accumulate, or a key overridden once
   would stay unwritable forever.
3. For every **already-materialized** slot: skip if the key is pinned; skip if
   the new boot value equals the old boot value (compared by hashing —
   `ConfigValue` has no `PartialEq`); otherwise push the new value, or clear
   the slot if the key disappeared from the config.

Step 3's "skip if unchanged" is what keeps a runtime `ConfigProvider::watch`
push (B4's single surviving task) from being reverted on every unrelated hot
patch. When a live key *is* edited in YAML, the edit wins over a prior runtime
push for that key — deliberate: the file is the boot source of truth.

Unmaterialized slots need no push at all: `slot()` seeds lazily from
`inner.boot`, which now holds the current cycle's config.

Holders of a registry handle, after Phase 1 — all the same instance:

| Holder | Created | Points at |
|---|---|---|
| `bean_registry.provided[TypeId::of::<LiveConfigRegistry>()]` | `load_config`, inside `config_derived_scope` | **the carried instance** (never pinned — it is already the right one) |
| `BuilderConfig.live_config` | `load_config` | **the carried instance** → a late `override_config_value` reaches live readers (B3 fixed) |
| `DeferredAction`/`on_serve` closure → `ConfigUpdateSink` | `load_config` | the carried instance; only cycle 1's hook ever runs (B4) |
| `BeanContext` entry / HList state slot | `resolve`/`resolve_reusing` + `BuildHList` | **the carried instance** |
| `LiveConfig<T>` handles in beans/producers/controller cores | `field_resolver.rs` → `ctx.get::<LiveConfigRegistry>()` | **the carried instance** (a handle binds one slot of one registry permanently — which is exactly why identity must be stable) |
| Handles inside *reused* beans (e.g. a `DbPool` subscription) | cycle 1 | **the carried instance** |

Text diagram of a two-cycle session (partial rebuild):

```
CYCLE 1                                   CYCLE 2 (hot patch)
────────────────────────────────────      ──────────────────────────────────────
load_config                               load_config
  R2eConfig  C1 ──provide──┐                R2eConfig  C2 ──provide──┐
                           │                                         │
  dev carrier: (empty)     │                dev carrier: L ──────────┤
    └─ create L ───────────┤                  └─ reseed(C2, pinned2) │
    └─ carry L             │                     · swap boot C1→C2   │
                           │                     · repin             │
  BuilderConfig.live_config│                     · push slots whose  │
        └──────────────► L │                       boot value moved  │
  DeferredAction(watch)──► L│                                        │
                           │                BuilderConfig.live_config│
resolve()                  │                      └──────────────► L │
  ctx { C1, L, beans… }  ◄─┘                                         │
                                          resolve_reusing(plan)      │
state1 = HList { C1, L, … }                 pin: every provided that │
                                              is NOT config-derived  │
  watch task ──sink──► L  (alive)             ctx { C2, L, beans… } ◄┘
                                                        ▲
                                            state2 = HList { C2, L, … }
                                            controller cores rebuilt from ctx:
                                              #[config]      → C2  (fresh copy)
                                              #[live_config] → L   (same slots,
                                                                    freshly seeded)
```

So: **one registry per process-session, created once**, its boot snapshot
tracking the current cycle's config, and every holder — old or new — pointing
at it.

---

## Q6 — is `ContextConstruct::config_keys()` consumed at runtime?

**No** — still true after Phase 3, and now **by decision** rather than by
oversight: Phase 3 requalified it as introspection-only and its rustdoc says so
(B6, closed). Declared with a default impl in `r2e-core/src/controller.rs`,
emitted by `r2e-macros/src/controller_codegen.rs`, and read by exactly one place
in the workspace: the assertion in `r2e-core/tests/controller/live_config.rs`.
Phase 2 gave it the same `ConfigKeyKind` shape as `Bean::config_keys`
(consistency for whatever ends up consuming it) and dropped the false
"fingerprint" claim from its doc comment; Phase 3 replaced that with an explicit
statement of what it does *not* drive, and why. There is no call site in
`beans.rs`, `nostate.rs`, `dev.rs`, the OpenAPI crate, or the CLI. (The many
`config_keys()` hits in `beans.rs:853-1225` are `Bean::config_keys` /
`Producer::config_keys`, a different trait method, consumed by
`FingerprintReg`.)

**What would have to consume it?** Only a controller-aware fingerprint: today
`compute_fingerprint` walks `self.beans`/`self.lazy_beans` only, and
controller cores are not registrations, so their declared keys cannot reach a
fingerprint without also making cores participate in the graph.

**Is that even needed, given Q2?** No. Controller cores are rebuilt
unconditionally every cycle from the context, and the only context in which a
core can be built with a stale `R2eConfig` is the full-reuse path — which
requires an unchanged whole-config fingerprint. So there is nothing left to
invalidate. The method is pure introspection surface, and that is now its
documented contract — not a gap waiting to be closed.

---

## Q7 — the `crate::dev` carrier

Shape (`r2e-core/src/dev.rs`): process-global `static`s, each a
`OnceLock<Mutex<…>>` or an `AtomicBool`, all gated behind
`hot_reload_loop_active()` so ordinary processes never engage them:

| Static | Line | Type |
|---|---|---|
| `HOT_RELOAD_LOOP` | 47 | `AtomicBool` (opt-in, set by `launch!`) |
| `LISTENER_STORE` | 36 | `OnceLock<Mutex<HashMap<String, TcpListener>>>` |
| `QUIC_ENDPOINT_STORE` | 117 | keyed by address string |
| `STATE_CACHE` | 152 | `OnceLock<Mutex<Option<Box<dyn Any + Send + Sync>>>>` |
| `LIFECYCLE_INITIALIZED` | 155 | `AtomicBool` |
| `CTX_CACHE` | 188 | `OnceLock<Mutex<Option<Arc<BeanContext>>>>` |
| `GRAPH_FINGERPRINT` / `PER_BEAN_FINGERPRINTS` | 256 / 284 | single slot / map |
| `BOOT_TIME` | 295 | `OnceLock<u64>` |

`invalidate_state_cache()` (`dev.rs:228`) resets the cache group.

**Phase 1 added `LIVE_CONFIG_REGISTRY`** to that list —
`OnceLock<Mutex<Option<LiveConfigRegistry>>>` with
`carried_live_config_registry()` / `carry_live_config_registry()` accessors
(both check `hot_reload_loop_active()` internally, so a non-dev process never
touches the mutex), cleared by `invalidate_state_cache()`. It also added
`unmark_hot_reload_loop()` — `#[doc(hidden)]`, dev-reload only, **tests only**
— which is what makes the production path (no gate ⇒ fresh registry per
`load_config`) assertable in-process; callers must hold the serial lock.

The constraints identified in Phase 0, all respected:

- **Single slot, not keyed.** Like `STATE_CACHE`/`CTX_CACHE`, it is one
  registry per *process*, not per app. Two apps built in one process share it.
  Acceptable only because the whole group is gated behind
  `hot_reload_loop_active()`, which `launch!` sets for exactly one app.
- **Parallel tests.** Anything added here must stay behind the same gate, or
  `cargo test` (which runs test fns on many threads in one process) would
  cross-contaminate. `tests/runtime/dev_serial.rs` is the lock; **`HOT_RELOAD_LOOP`
  is process-global and one-way**, so once *any* test in the target marks it,
  *every* other test in the same binary that calls `load_config()` starts
  reusing and re-seeding the carried registry. That bit during implementation:
  `sharded.rs` and `tcp_nodelay.rs` were flaking the dev tests until they took
  the same lock. `dev_serial` is therefore no longer `#[cfg(feature =
  "dev-reload")]` — every `load_config()` caller in the `runtime` target holds
  it.
- **Reset story.** `invalidate_state_cache()` clears the registry, so the
  documented "escape hatch forces a cold rebuild" stays cold for live config.
- **`dev.rs` is not feature-gated as a module**, but every consumer is; the new
  accessors are `#[cfg(feature = "dev-reload")]` in line with `STATE_CACHE`'s
  neighbours.

---

## Confirmed bugs and quirks

| # | Sev | Behavior (as characterized in Phase 0) | Status |
|---|---|---|---|
| **B1** | high | `#[live_config]` values are frozen at cycle-1 boot for the whole dev session: the registry (and its boot snapshot) is pinned, so YAML edits to live keys never reach a slot. | **FIXED (Phase 1)** — the registry is carried in `crate::dev` with one stable identity and `load_config` re-seeds it per cycle (swap boot snapshot, push changed values into materialized unpinned slots). Test `live_config_registry_keeps_one_identity_and_reseeds_across_cycles`. |
| **B2** | high | Typed `ConfigProperties` / `#[config(section)]` beans are pinned from cycle 1 — a section edit is invisible under `r2e dev` even though the raw `R2eConfig` refreshes. | **FIXED (Phase 1)** — the pinning exemption generalized from "is `R2eConfig`" to "is in `config_derived`", and `LoadableConfig` provides typed sections inside `config_derived_scope`. Test `typed_config_bean_tracks_edits_across_cycles`. |
| **B3** | medium | On cycles ≥ 2, a **late** `override_config_value` patches and pins the registry that is about to be discarded — silently lost for `#[live_config]` readers (it still reaches `R2eConfig`). | **FIXED (Phase 1)**, for free — `BuilderConfig.live_config` now references the stable instance, so there is no discarded registry to patch. Test `late_override_config_value_reaches_live_readers_and_stays_pinned`. |
| **B4** | medium | `ConfigProvider::watch` is started exactly once per process; later cycles register a hook that is never drained, and a watch that terminates is never restarted. Its updates *do* stay visible, so this is a restart/robustness gap, not a data-loss one. | **OPEN — the only one left.** Out of scope for Phases 1–3; carried to `docs/claude/roadmap.md` § W13. Still characterized by `characterize_provider_watch_runs_once_but_its_registry_is_the_live_one`. Phase 1 made it *more* benign: the single watcher writes to the one registry everybody reads, and the re-seed's changed-only rule preserves its pushes. |
| **B5** | low | Editing a live key rebuilds every bean declaring it (its `config_keys()` fingerprint changes) for no benefit. | **FIXED (Phase 2)** — `config_keys()` entries carry a `ConfigKeyKind`; `Live` keys are excluded from the per-bean fingerprint, so a live-key edit reuses every bean. Test `live_key_edit_pushes_without_rebuilding_copied_key_edit_rebuilds`. |
| **B6** | low | `ContextConstruct::config_keys()` has no runtime consumer. | **CLOSED / REQUALIFIED (Phase 3)** — kept as declared introspection, not deleted. Its rustdoc now states outright that it takes no part in dev-reload invalidation *and why* (cores are rebuilt from a fresh context every cycle; the whole-config fingerprint guards the only reuse path), and that presence validation runs through `Controller::validate_config`. Do not re-open this as "wire it into the fingerprint" — see Q6. |

Non-bugs worth recording, so a later phase does not "fix" them:

- Exempting config-derived provided values from pinning is deliberate and
  load-bearing. Do **not** narrow it back to `R2eConfig`, and do not widen it
  to "all provided values" (that would defeat reuse entirely).
- The whole-config fingerprint seed is what keeps the full-reuse fast path
  from serving a stale config object, and is why controllers never read stale
  `#[config]` values (`config/mod.rs:443-449`). It stays whole-config even for
  live keys — see Q3.
- The re-seed pushes **only** keys whose boot value changed. Making it push
  unconditionally would revert every runtime `ConfigProvider`/`set` push on
  each hot patch.
- `Optional` (`Option<T>` `#[config]`) is a **copied** kind and stays
  fingerprinted. It is not "live-ish" just because it is not required.

---

## What is not testable in-process

- A **real** Subsecond hot patch (new `build_version` token hashes). All tests
  here simulate a cycle by calling `build_state()` twice with an edited config,
  which is the same trigger the existing `dev_reload.rs` uses and exercises the
  identical code paths — but it cannot cover "the constructor source changed".
- `launch!`'s re-invocation of `App::build` itself (macro + dioxus-devtools);
  only its consequences (`load_config` re-running, deferred actions re-running)
  are reachable, and those are asserted through the provider `load`/`watch`
  counters.
- Cross-process/`r2e dev` file-watching behavior.

---

## What shipped

### Phase 1 — stable registry identity across cycles

Implemented as designed, with both Phase-0 adjustments honored (value-aware
re-seed; changed-boot-values-only so runtime pushes survive). Fixes B1 and B3,
and folds in B2.

| File | Change |
|---|---|
| `r2e-core/src/dev.rs` | `LIVE_CONFIG_REGISTRY` carrier + `carried_/carry_live_config_registry()`; cleared by `invalidate_state_cache()`; `unmark_hot_reload_loop()` (tests only) |
| `r2e-core/src/builder/mod.rs` | `live_config_registry_for_cycle(config, pinned_keys)` — the get-or-create/re-seed decision, with a non-dev-reload arm that always builds fresh |
| `r2e-core/src/builder/nostate.rs` | `load_config` uses it and provides config-derived values inside `config_derived_scope` |
| `r2e-core/src/config/runtime.rs` | `BootSnapshot` behind an `RwLock`; `reseed()` (dev-reload gated), publishing through the shared `publish(key, Option<ConfigValue>)` |
| `r2e-core/src/beans.rs` | `config_derived: HashSet<TypeId>` + `config_derived_scope()`; pinning loop keyed on membership instead of `TypeId::of::<R2eConfig>()` |
| `r2e-core/src/config/mod.rs` | `LoadableConfig for T: ConfigProperties` provides the typed struct inside `config_derived_scope` |

`ConfigValue` has no `PartialEq` (it holds an `f64`), so the re-seed's
equality test goes through its manual `Hash` impl — it compares
`R2eConfig::config_fingerprint(&[key])` on the old and new snapshot.

### Phase 2 — live keys leave the per-bean fingerprint

Fixes B5. The discriminator Phase 0 said was missing is
`ConfigKeyKind::{Required, Optional, Live}` (`r2e-core/src/config/mod.rs`),
carried as the third element of each `config_keys()` entry — `(key, type,
kind)` — rather than as a 4th tuple field or a parallel accessor, so there is
exactly one list to keep consistent.

- `is_required()` → startup presence validation (`Required` only; `Optional`
  and `Live` are never presence-checked).
- `is_fingerprinted()` → per-bean fingerprint inclusion (everything but
  `Live`).

Updated in lockstep: `Bean::config_keys`, `AsyncBean::config_keys`,
`Producer::config_keys`, `ContextConstruct::config_keys`, both validation
filters and `compute_reg_fingerprint` in `beans.rs`, and the macro emitters —
which now share three helpers in `r2e-macros/src/field_resolver.rs`
(`config_keys_ret_ty`, `copied_config_key_entry`, `live_config_key_entry`)
instead of four hand-rolled `quote!`s, so the kind decision is single-sourced
across `bean_derive.rs`, `producer_attr.rs`, `bean_attr.rs` and
`controller_codegen.rs`.

Phase 0's warning holds and is worth repeating in any changelog entry: a
live-key edit still changes the **graph** fingerprint (whole-config seed) and
still runs the partial-rebuild pass. What changed is that the pass now reuses
every bean. "A live-key edit changes nothing" would be wrong; "a live-key edit
rebuilds nothing" is right.

### Known gaps left behind

- ~~**`#[config_section]` fields emit no `config_keys()` entry.**~~ **FIXED
  (Phase 3)** — see below.
- `bg_service_derive.rs` and `decorator_bean_derive.rs` emit no
  `config_keys()` at all: a background service or decorator bean resolves
  `#[config]` / `#[config_section]` / `#[live_config]` fields but declares none
  of them, so none are presence-validated or fingerprinted. Pre-existing, still
  open, carried to `docs/claude/roadmap.md` § W13. The fix is mechanical (reuse
  the three `field_resolver` entry helpers); the open question is where the
  declaration lands for hosts that are not `Bean`s.

---

### Phase 3 + 4 — requalification, the section gap, and the typo guard

**B6 — `ContextConstruct::config_keys()`: requalified, not deleted.** Q2 and Q6
together say there is nothing to invalidate: controller cores are rebuilt from
the context on every cycle in both branches, and the only branch that hands them
a stale config is unreachable with a changed config. Wiring controller keys into
the fingerprint would add graph machinery (cores are not registrations) for zero
behavioral gain, so that option is **dropped for good**. The method stays as the
machine-readable record of a controller's config surface — the natural input for
`r2e routes` / `r2e doctor`-style tooling, and the mirror of `Bean::config_keys`
that "the controller core IS a bean" implies. `r2e-core/src/controller.rs` now
says so explicitly, including the *why*, under a "This list drives nothing at
runtime" heading.

**The `#[config_section]` fingerprint gap: fixed.** A section covers a dotted
prefix, not an exact key, and its field set lives in the typed
`ConfigProperties` struct rather than in the attribute — so nothing in the macro
can enumerate the keys to hash. Phase 3 adds a prefix-aware kind instead:

| File | Change |
|---|---|
| `r2e-core/src/config/mod.rs` | `ConfigKeyKind::Section` (key = prefix) + `is_prefix()`; `R2eConfig::prefix_fingerprint(prefix)` — prefix, then every key equal to it or under `"{prefix}."` with its value, keys sorted, same hasher shape as `full_fingerprint` |
| `r2e-core/src/beans.rs` | `compute_reg_fingerprint` splits the fingerprinted entries into exact keys (unchanged `config_fingerprint`) and prefixes (sorted + deduped, each folded in via `prefix_fingerprint`) |
| `r2e-macros/src/field_resolver.rs` | `section_config_key_entry(krate, prefix, ty)` — the single source of the `Section` entry |
| `r2e-macros/{bean_derive,bean_attr,producer_attr,controller_codegen}.rs` | emit it wherever `#[config_section]` is accepted |

`is_required()` stays **false** for `Section` on purpose: a section validates
itself at construction (`ConfigProperties::from_config` errors and the generated
init panics naming the prefix), so a key-level presence check would be both
redundant and unable to name what it should check.

Because `prefix_fingerprint` hashes key *names* as well as values, adding or
removing a key inside the section moves the digest too — not just editing one.

Tests: `runtime/dev_reload_config.rs::section_key_edit_rebuilds_the_declaring_bean`
(two simulated cycles: a key inside the section rebuilds the holder, a key
outside it does not) and `di/fingerprint.rs`
(`config_section_declares_its_prefix_as_a_section_key`,
`per_bean_fingerprint_tracks_every_key_under_the_section_prefix`,
`prefix_fingerprint_covers_the_subtree_only`).

**Boot-time WARN for dead live keys.** Live keys are never presence-validated
(deliberate: absence at boot is legal), which means a typo'd
`#[live_config("db.ulr")]` fails silently forever. `LiveConfigRegistry::
live_config()` now warns when the key is *dead*:
`LiveConfigRegistry::is_dead_key(key)` — no registered `ConfigProvider` **and**
no value in the slot (neither boot-seeded nor pushed at runtime). The provider
flag is an `AtomicBool` on the registry inner, set by `load_config` when
`config_providers` is non-empty and only ever moving `false → true`; a
hand-built `LiveConfigRegistry::new()` / `Default` therefore warns, which is the
correct default (nobody registered a writer, so nobody will). It stays a
warning, never an error: filling a key in later with `set` is legitimate, and a
hard failure would break every test that builds a registry over an empty config.

`is_dead_key` is `#[doc(hidden)] pub` so the precondition is assertable from the
integration tests (the workspace has no tracing-capture harness, and the
predicate *is* the whole logic). Tests: `config/live_config.rs` —
`a_key_with_a_boot_value_is_not_dead`,
`a_key_absent_at_boot_without_providers_is_dead`,
`a_key_written_at_runtime_is_not_dead`,
`a_registered_config_provider_silences_the_diagnostic` (the last one drives the
`load_config` wiring through `AppBuilder`).

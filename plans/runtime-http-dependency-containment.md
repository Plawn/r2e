# Plan — Containing the `tokio` and `axum` dependency surface

Goal: `tokio` is a direct dependency of exactly one crate (`r2e-rt`), `axum` of
exactly one crate (`r2e-http`), and the amount of R2E code that *names* axum
types is small enough that swapping the runtime or the HTTP layer is a
bounded, reviewable change instead of a workspace rewrite.

Branch: `task/tokio-axum-containment`. Executed as 6 sequential phases (see
§8 Execution log at the bottom — the orchestrator keeps it current).

## 1. Where we actually are

### tokio

Measured on `master` (src only, tests/examples excluded).

| | count |
|---|---|
| crates with `tokio` / `tokio-util` / `tokio-stream` as a **non-dev** dependency | 19 |
| source files naming `tokio::` / `tokio_util::` outside `r2e-core/src/rt.rs` | 58 |
| `tokio_util::sync::CancellationToken` | 29 uses / 28 files |
| `tokio::select!` | 26 uses / 18 files |
| `tokio::sync::*` (mpsc, oneshot, broadcast, Mutex, RwLock, Notify, Semaphore, OnceCell) | ~43 uses |
| `tokio::time` / `net` / `runtime` / `task` / `spawn` | ~53 uses |

`r2e-core/src/rt.rs` (335 lines) already **is** the facade — `JobHandle`,
`JoinError`, `Elapsed`, `spawn`/`spawn_ctl`/`spawn_blocking`, `sleep`,
`timeout`, `interval`, `bind_tcp`, `shutdown_signal` — and `clippy.toml`
already denies raw `tokio::spawn`. Two structural gaps keep it from being the
boundary:

1. **It lives in `r2e-core`, which is not the bottom of the graph.** `r2e-http`
   sits *below* `r2e-core`, so `r2e-http/src/quic.rs` calls `tokio::spawn`
   directly — documented in `rt.rs` as a permanent exception. It does not need
   to be one.
2. **It explicitly excludes the largest category.** `rt.rs` declares
   `tokio::sync` and `CancellationToken` out of scope as "runtime-agnostic in
   practice". That was a reasonable call for a *thread-per-core* migration (the
   original stated purpose). It is the wrong call for a *runtime swap*: those
   two categories are ~72 of the ~150 remaining call sites, and
   `CancellationToken` is in the **public API** — `AppBuilder::shutdown_token()`,
   `RuntimeContext::new`, `SchedulerHandle::{new,channel,token}`,
   `BackendState::register_poller_cancel`, plugin `ServeContext`. Today a user
   app must add `tokio-util` to its own `Cargo.toml` to consume R2E's own
   shutdown API.

Three crates declare a non-dev `tokio` dependency and never name it in `src/`:
**`r2e-utils`** (`features = ["full"]`), **`r2e-openfga`**, **`r2e-grpc`**.
Free deletions (pending a feature-unification check).

### axum

The manifest side is already clean: `axum` appears in exactly one
`Cargo.toml`, `r2e-http`. But `r2e-http` is a **50-line re-export shim** —
`lib.rs` + 8 modules that are pure `pub use axum::…` (the only real code in the
crate is `quic.rs`, 521 lines, which is h3/quinn and not axum). So axum's
*types* are R2E's API and its users' API:

| crate | axum-typed names in `src/` |
|---|---|
| `r2e-macros` (generated code) | ~280 |
| `r2e-core` | ~250 |
| `r2e-cli` (templates) | ~115 |
| everything else | ~150 |
| `examples/` + user apps | ~200 |

12 public functions take or return `axum::Router`. There are 24
`FromRequestParts` impls and 8 `IntoResponse` impls across the workspace.

**Conclusion:** the tokio problem is a *containment* problem (move code behind
an existing facade). The axum problem is an *abstraction* problem (the facade
does not abstract anything yet). They deserve different plans.

---

## 2. Phase 0 — Freeze the boundary before moving anything

Cheap, and it is what makes phases 1–3 stick.

- `scripts/check-dep-boundary.sh`: parse `cargo metadata`, assert that the set
  of crates with a direct `tokio*` dependency equals an explicit allowlist
  file, same for `axum`. Wire into CI.
- `scripts/check-source-boundary.sh`: grep `src/` for `tokio::` / `axum::` /
  `tokio_util::`, compare against a checked-in allowlist of `path:count`.
  The allowlist **only ever shrinks** — each migration PR deletes lines from
  it. This gives every phase below a mechanical definition of done.
- `clippy.toml` `disallowed-types` (`JoinHandle`, `runtime::Handle`,
  `CancellationToken`, `time::Sleep`) would fire on ~30 pre-migration files —
  so that tightening lands at the **end of Phase 2**, not here. Phase 0 only
  captures the baseline allowlists.

## 3. Phase 1 — Extract `r2e-rt` at the bottom of the graph

New crate `r2e-rt`. Dependencies: `tokio`, `tokio-util`, `tokio-stream`, and
nothing else from the workspace. Every R2E crate may depend on it, including
`r2e-http`.

1. Move `r2e-core/src/rt.rs` → `r2e-rt/src/lib.rs` verbatim. `r2e-core` keeps
   `pub use r2e_rt as rt;` so no call site changes and no user-visible break.
2. **Widen the scope** to the two categories `rt.rs` currently excludes:
   - `r2e_rt::sync` — re-export `mpsc`, `oneshot`, `broadcast`, `Mutex`,
     `RwLock`, `Notify`, `Semaphore`, `OnceCell`. Re-export, not wrap: their
     shape is runtime-neutral, but their *identity* is tokio's, and re-exporting
     is what removes the name from 20+ files at zero cost.
   - `r2e_rt::CancelToken` — a **newtype** over `CancellationToken`, not a
     re-export, because it is in the public API and a swap must not force every
     downstream app to change. Surface needed by current call sites:
     `new`, `child_token`, `cancel`, `is_cancelled`, `cancelled()`,
     `drop_guard`. ~40 lines.
   - `pub use tokio::select;` — removes `tokio::select!` from 18 files.
     (`select!` over R2E futures keeps working; it is a macro over `Future`.)
3. Fold `r2e-http/src/quic.rs`'s direct `tokio::spawn` onto `r2e_rt::spawn`,
   and delete the "known facade exception" paragraph from the module docs.
4. Add `r2e_rt::block_on` + a `Builder` shim so `r2e-macros` can emit
   `::r2e_rt::` paths instead of `tokio::runtime::Builder` tokens
   (`util/runtime_args.rs`, `derives/bg_service_derive.rs`, `lib.rs`).

**Breaking**: `AppBuilder::shutdown_token()`, `SchedulerHandle::token()`,
`RuntimeContext::new`, and the plugin `ServeContext` signatures change
`CancellationToken` → `r2e::rt::CancelToken`. Pre-1.0, acceptable; needs a
`CHANGELOG.md` entry, an `llm.txt` update, and a note in
`docs/features/22-serve-lifecycle.md`.

## 4. Phase 2 — Migrate the ~150 call sites, one layer per PR

Bottom-up, each step shrinking the Phase-0 allowlist. Sizes are file counts.

| Step | Scope | Files |
|---|---|---|
| 2a | Delete dead deps: `r2e-utils`, `r2e-openfga`, `r2e-grpc` | 0 (manifest only) |
| 2b | `r2e-security` (Mutex/RwLock), `r2e-oidc` (Semaphore), `r2e-data-sqlx`, `r2e-data-diesel` (CancelToken only) | 4 |

**Step 2b note (from execution)** — `r2e-security` and `r2e-oidc` migrated
fully and dropped their `tokio` dependency. `r2e-data-sqlx` /
`r2e-data-diesel` dropped their (already dead) `tokio` dependency and now
convert to `CancelToken` at the top of `start`, but each keeps one
`tokio_util::sync::CancellationToken` name — and the `tokio-util` dependency —
because `ServiceComponent::start` still takes the raw token. That signature is
r2e-core's (`runtime/service.rs`) *and* `#[derive(BackgroundService)]`'s
(`bg_service_derive.rs` emits `::tokio_util::sync::CancellationToken`, and the
user-written `run()` it forwards to takes the same type), so flipping it is a
**user-visible break** that belongs to steps 2e+2f, not here. Those two files
leave the baseline then.
| 2c | `r2e-events` + 4 backends (iggy/kafka/pulsar/rabbitmq) — mostly `select!` + `sync` + `CancelToken` | 18 |
| 2d | `r2e-scheduler`, `r2e-executor`, `r2e-tenant` | 11 |
| 2e | `r2e-core` internals: `web/{sse,ws,managed}`, `di/lazy`, `builder/prepared`, `runtime/{dev,service,lifecycle}`, `config/runtime`, `builtins/health`, `beans/registry` | 15 |
| 2f | `r2e-macros` emitted paths → `::r2e_rt::`; then tighten `clippy.toml` with `disallowed-types` | 3 |

**Step 2f note (from Phase 1)** — `r2e-rt` carries a non-default `test-util`
feature (`tokio/test-util`), kept off by default because it changes timer
behaviour and feature unification would otherwise hand paused clocks to every
crate in the workspace. `#[r2e::test(start_paused = true)]` needs it, so when
2f moves the macro-emitted paths onto `::r2e_rt::`, **forward the feature**:
`r2e-core` → `test-util = ["r2e-rt/test-util"]`, `r2e` → `test-util =
["r2e-core/test-util"]`, and have `r2e-test` enable it (its harness is where
paused clocks are legitimate). Until then `start_paused` keeps resolving
through the direct `tokio` dependency the emitting crates still have.

**Permanently allowlisted, by design** — document each in `r2e-rt`'s module docs:

- `r2e-rt` itself.
- `r2e-core/src/runtime/sharded.rs` — building `current_thread` runtimes *is*
  the sharding mechanism, not a touchpoint to abstract.
- `r2e-test`, `r2e-devservices` — test harnesses; they legitimately own a
  runtime. Keep them out of the boundary check rather than pretending.

End state: `cargo tree -i tokio --workspace` shows `r2e-rt` as the only direct
dependent outside that allowlist.

## 5. Phase 3 — axum: make `r2e-http` abstract something

Staged by cost/benefit, because unlike tokio this is not mechanical. Split the
current axum surface into three buckets and treat each differently.

### 3a — Re-source the neutral types from `http` (cheap, do it now)

`StatusCode`, `HeaderMap`, `HeaderName`, `HeaderValue`, `Method`, `Uri`,
`Parts`, `Extensions`, `Bytes` are **`http`/`bytes` crate types**, not axum
types — axum merely re-exports them. `r2e-http` already depends on `http` and
`bytes` directly. Change `header.rs`/`lib.rs` to source them from `http`
instead of `axum::http::`.

Cost: ~10 lines in `r2e-http`. Benefit: a large share of "axum names" in the
workspace stops being an axum coupling at all, and the remaining count becomes
an honest measure of the real problem. Do this before measuring 3b.

### 3b — R2E-owned traits over the impl sites (medium)

The 24 `FromRequestParts` impls and 8 `IntoResponse` impls are the places
where R2E code *implements axum's contracts*, which is the coupling a swap
actually pays for. R2E already owns `FromRequestPartsVia` for the marker/E0207
problem — extend the same pattern:

- `r2e_http::FromParts<S, M>` — R2E trait, blanket-bridged to axum's
  `FromRequestParts`. The 24 impls become R2E impls.
- `r2e_http::IntoHttpResponse` — same, for the 8 response impls.

A swap then rewrites two bridge impls instead of 32 scattered ones.
Cost: moderate; touches `r2e-macros` codegen and every extractor.

### 3c — `Router` newtype (expensive — decide explicitly, do not drift into it)

`Router`, `MethodRouter`, `routing::{get,post,…}`, `middleware::from_fn`, and
`Handler` are genuinely axum-shaped and are what the 12 public signatures
expose. Wrapping `Router` in `r2e_http::Routes` with only the ~8 methods R2E
uses (`route`, `merge`, `nest`, `layer`, `with_state`, `fallback`,
`into_make_service`, `into_make_service_with_connect_info`) is feasible, but
it touches macro codegen, every plugin's `configure`, `r2e-static`,
`r2e-grpc`, `r2e-test`, and the `r2e-cli` templates.

**Recommendation: defer, with a written decision.** The honest cost/benefit is
that a real HTTP-layer swap also means replacing tower `Layer`/`Service`,
`hyper`, and the `http-body` plumbing — a `Router` newtype alone does not buy
a swap, it only buys the *option* of one. Revisit if and when a concrete
second backend is on the roadmap.

### 3d — decide what users see

Today `use r2e::prelude::*` hands users `Json`, `Path`, `Query`, `Router` —
axum types by identity, which makes every downstream app part of the coupling.
Pick one and write it down in `llm.txt`:

- **(A)** The public promise is "R2E types"; axum stays reachable through an
  explicit `r2e::http::axum_compat` escape hatch.
- **(B)** The public promise is "axum, ergonomically wrapped" — then 3c is
  pointless and the containment goal stops at the workspace boundary.

(A) is consistent with the stated goal; (B) is cheaper and consistent with how
`r2e-http` is documented today. This choice gates 3c.

## 6. Recommended sequencing

**Now:** Phase 0 → Phase 1 → Phase 2 (2a…2f) → Phase 3a. This is mechanical,
low-risk, and delivers the stated tokio goal in full plus the cheap half of
the axum goal.

**Next, after a decision on 3d:** Phase 3b.

**Only with a concrete second HTTP backend in view:** Phase 3c.

## 7. Definition of done

- `scripts/check-dep-boundary.sh` green with `tokio` allowlist =
  `{r2e-rt, r2e-test, r2e-devservices}` and `axum` allowlist = `{r2e-http}`.
- `scripts/check-source-boundary.sh` allowlist contains only the four
  by-design entries of §4.
- `CLAUDE.md` architecture block, `AGENTS.md`, `llm.txt`,
  `docs/claude/subsystems.md`, and `docs/features/22-serve-lifecycle.md`
  updated for `r2e-rt` and `CancelToken`.
- `CHANGELOG.md` records the `CancellationToken` → `CancelToken` break.

---

## 8. Execution log (orchestrator-maintained)

The 6 execution phases on this branch, in order. One commit (or a small
commit series) per phase; `cargo check --workspace` + targeted tests green
before a phase is marked done.

| # | Phase | Status |
|---|---|---|
| 1 | Phase 0 — boundary scripts + baseline allowlists | done |
| 2 | Phase 1 — extract `r2e-rt` (facade move + sync/CancelToken/select widening) | done |
| 3 | Phase 2a+2b — dead deps + small crates (security/oidc/sqlx/diesel) | done |
| 4 | Phase 2c+2d — events + 4 backends; scheduler/executor/tenant | pending |
| 5 | Phase 2e+2f — r2e-core internals + macro-emitted paths + clippy tightening | pending |
| 6 | Phase 3a — re-source neutral `http`/`bytes` types + docs/llm.txt/CHANGELOG sweep | pending |

Phases 3b/3c stay out of this branch, gated on the §5.3d decision (A vs B).

Notes:
- Use the shared global cargo target dir, never a local `CARGO_TARGET_DIR`, and
  never run two builds concurrently (disk has been tight on this machine).
- Commits: no `Co-Authored-By` trailers.

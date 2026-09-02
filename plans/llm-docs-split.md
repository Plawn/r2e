# Plan — Splitting `llm.txt` into a hub + topic files, delivered version-matched

`llm.txt` is the canonical AI/agent-facing reference (`CLAUDE.md` § "Keeping
`llm.txt` Fresh"). This plan reshapes it so an agent working in a consumer app
loads the ~10–15k tokens relevant to its task instead of the whole file, gets
the version that matches its `Cargo.lock`, and copies examples that are known
to compile.

Branch: `task/llm-docs-split`. Started 2026-09-01.

## 1. Where we actually are

- `llm.txt` is **239 KB / 4 990 lines ≈ 60k tokens**, one file, 35 `##`
  sections. Section sizes are wildly uneven: Dependency Injection 34 KB (beans
  + plugins + modules + lifecycle hooks in one section), Multi-Tenancy 23 KB,
  Testing 23 KB, Guards 19 KB — versus SSE 1 KB, Proxy 0.9 KB.
- Distribution: the `AGENTS.md` scaffolded by `r2e new`
  (`r2e-cli/src/commands/templates/project.rs`, `agents_md`) says *"the full
  AI-facing API reference is `llm.txt` at the root of the R2E repository"*.
  That is **outside the consumer's repo and not version-matched**: an agent
  has to fetch `master` from GitHub, or — more often — reads nothing.
- Two building blocks already exist: `r2e docs <module>` embeds
  `docs/features/*.md` via `include_str!` (version-matched by construction,
  `## TL;DR` extraction), and this repo's own `CLAUDE.md` routes agents with a
  **keyword → file table**, which works well in practice.
- The failure mode `CLAUDE.md` warns about — a stale example makes a consumer
  agent generate non-compiling code — is not checked by anything today.

## 2. Target shape

### 2.1 Hub + spokes (`llms.txt` convention)

- `llm.txt` (the **hub**, ≤ 5k tokens, hand-written): what R2E is and the
  "R2E types, not axum" promise; "Coming from Axum — do X, not Y"; core
  concepts (`App` trait, controller + the four injection scopes,
  `#[anonymous]`); the two quick references (method attributes, builder
  methods); and a **routing table** — *if your task involves… → read
  `llm/<topic>.md` (≈N tokens, features required)*.
- `llm/<topic>.md` (the **spokes**, 2–8k tokens each): one per topic. The
  current oversized sections get split (DI → `di-beans`, `di-plugins`,
  `di-modules`, `lifecycle-hooks`; Testing → `testing`, `devservices`; …).
- `llm-full.txt`: **generated** concatenation of hub + spokes (the
  `llms-full.txt` convention) for tools that ingest a single file and for
  pasting into a chat. Never hand-edited.

Agents reason by task ("add auth", "write a test"), not by crate — the split
and the routing table are by **topic**, deliberately not by crate.

### 2.2 Spoke format

Every spoke starts with a stable header block and follows one skeleton, so an
agent can decide within 20 lines whether to keep reading:

```
---
topic: guards                    # == file stem
features: core, rate-limit       # `r2e` features to enable, or `core`
tokens: ~4500                    # bytes/4, recomputed by the check script
requires: core-concepts, di-beans
---

## Guards
### TL;DR             — mandatory: the imperative rules
…                      — free-form body; `### Do not` where it earns its place
```

`requires:` replaces re-explaining prerequisites in every spoke (a good part
of the DI section's 34 KB is repeated in Guards, Tenancy and Testing).

*As shipped (step 2):* the front matter is exactly the six lines above (no
`since:` — the version is stamped on delivery, see §2.3, and `crates:` became
`features:` because that is what an agent has to write in `Cargo.toml`).
Only `### TL;DR` is mandatory; "Canonical example" / "Details" / "Do not"
stayed optional because forcing four headings onto a 300-token topic like
`sse` produces filler, not signal.

### 2.3 Delivery — local and version-matched

1. `r2e docs --llm [<topic>] [--full] [--export [DIR]]` — same `include_str!`
   mechanism as `r2e-cli/src/commands/docs.rs`, so the text always matches the
   installed CLI. `--export` writes the hub + every topic under `DIR`
   (default `docs/r2e/`), the hub stamped with
   `<!-- R2E vX.Y.Z — exported by … -->`, so an agent can `Read` the files
   instead of consuming stdout.
2. `r2e new` exports the set into `docs/r2e/` and writes
   `.claude/skills/r2e/SKILL.md` (loaded on demand by Claude Code — the exact
   behaviour we want); the generated `AGENTS.md` routes agents to
   `docs/r2e/llm.txt`. `r2e doctor` (check 10) warns when `docs/r2e/llm.txt`
   is missing or its stamp ≠ the CLI version.

*Decided against (step 4):* shipping `llm/**` inside the published `r2e`
crate — the files live outside the package directory, so `include = [...]`
cannot reach them without a symlink or a copy step, and `cargo metadata`
resolution in the CLI would only add a second, weaker path to the same text.
The exported copy in the project is the delivery; upgrading `r2e` means
re-running `r2e docs --llm --export`, which `doctor` reminds you of.
Per-feature routing rows from `r2e add` are unnecessary: the hub already
routes every topic and states the feature each one needs.

Optional, not planned: `r2e docs --mcp` serving `r2e://docs/<topic>` through
`r2e-mcp`. (1)+(2) cover the need.

### 2.4 Freshness — make the "MUST update llm.txt" rule checkable

- `scripts/check-llm-docs.sh` (CI): regenerates `llm-full.txt` and fails on
  drift; checks every spoke has the six-line front matter, a single `##`, a
  `### TL;DR`, existing `requires:` targets, and is routed from the hub (the
  hub doubles as the manifest — first-reference order is concatenation
  order). `--update` recomputes `tokens:` and rewrites `llm-full.txt`.
- **Compile the snippets**: a dev-only `llm-doctests` crate does
  `#[doc = include_str!("../llm/<topic>.md")]` for every spoke, so
  `cargo test --doc -p llm-doctests` compiles every ```` ```rust ```` block.
  Deliberately partial blocks are marked ```` ```rust,ignore ````. This is the
  only mechanism that guarantees an agent copying an example gets code that
  compiles.

## 3. Steps — each one mergeable on its own

| # | Step | Status |
|---|---|---|
| 1 | Mechanical split of `llm.txt` by `##` into `llm/NN-<topic>.md`; `llm.txt` becomes the **generated** concatenation (byte-identical to before); `scripts/check-llm-docs.sh [--update]` + CI workflow; `CLAUDE.md` rule updated to "edit `llm/`, regenerate". No content is rewritten. | done 2026-09-01 |
| 2 | Split the four oversized sections (DI / Tenancy / Testing / Guards); add the header block (§2.2) to every spoke; use `requires:` to remove repeated prerequisites. The check script starts enforcing the header + skeleton. | done 2026-09-01 (§5) |
| 3 | Rewrite the hub (≤ 5k tokens) with the routing table. The generated file becomes `llm-full.txt`; `llm.txt` is the hand-written hub. Numeric prefixes on spokes are dropped (order then comes from the hub's table). **Breaking for links**: anything pointing at `llm.txt` as a full reference must move to `llm-full.txt`; update `README.md` § "For AI agents" and the `agents_md` template. | done 2026-09-01 (§5) |
| 4 | `r2e docs --llm` (+ `--export`), `AGENTS.md` + `.claude/skills/r2e/SKILL.md` generated by `r2e new`, `r2e doctor` version check. Crate `include` and `r2e add` rows dropped — see §2.3. | done 2026-09-01 (§5) |
| 5 | `llm-doctests` crate, `rust,ignore` triage of the existing blocks, wired into `cargo test --workspace`. | done 2026-09-01 (§5) |

## 4. Step 1 — what was done (2026-09-01)

- `llm/00-preamble.md` = the title + intro blockquote; `llm/01-…` to
  `llm/35-…` = the 35 `##` sections, in order, slugged by hand from the
  heading. Files are the section text with trailing blank lines trimmed.
- `llm.txt` = `00-preamble.md` + spokes joined with one blank line. The
  round-trip is byte-identical to the pre-split file (verified with `cmp`),
  which is the proof that the split was purely mechanical.
- `scripts/check-llm-docs.sh` — `--update` regenerates `llm.txt`; without it,
  regenerates into a temp file and fails on any diff. Also fails if a spoke
  does not start with a `## ` heading, or contains a second `## ` outside a
  code fence (one topic per file). `.github/workflows/llm-docs.yml` runs it on
  push/PR.
- `CLAUDE.md` § "Keeping `llm.txt` Fresh" now says: edit the spoke under
  `llm/`, run `scripts/check-llm-docs.sh --update`, commit both.

Deliberately **not** done in step 1: no heading was renamed, no section was
split or merged, no header blocks were added — that is step 2, so that the
mechanical move and the content edits are reviewable separately.

## 5. Steps 2–5 — what was done (2026-09-01)

**Step 2 — content split + front matter.** The four oversized sections were
cut at their `###` boundaries into 41 topics (`di-beans`, `modules`,
`lifecycle-hooks`, `tenancy` / `tenancy-datasources`, `testing` /
`devservices`, `guards` / `interceptors`, …); numeric prefixes were dropped
here rather than in step 3 because the hub's routing table was written in the
same pass. Every spoke got the six-line front matter and a `### TL;DR`
(written per topic by five review passes, which also removed the prerequisite
paragraphs that `requires:` now covers and fixed the stale facts they hit:
`version = "0.1"` in the quick start, a plugin `install` hook that is really
`setup` / `PluginSetupContext`, a truncated sentence in the test-suite section
that predated the split). `additional-plugins` is `features: core` (it
documents `RequestIdPlugin` / `SecureHeaders` / `Health` / `NormalizePath`,
not the cache and rate-limit crates); `testing` / `devservices` state their
dev-dependency in `features:` since the script only requires the line.

**Step 3 — hub.** `llm.txt` is hand-written (~9 KB): intro, "How to use",
ten golden rules, a tiered routing table `| task involves… | read | features |`
that references all 41 spokes. It is also the manifest: `check-llm-docs.sh`
derives the spoke list and the concatenation order from the first reference
of each `llm/<slug>.md` in it, so an unrouted spoke fails CI. `llm-full.txt`
= stamp + hub + every spoke (front matter stripped) — 5 898 lines, generated.
`README.md` § "For AI agents", `CLAUDE.md` § "Keeping `llm.txt` Fresh",
`docs/claude/cli.md` and `llm/dev-experience.md` were repointed.

**Step 4 — delivery.** `r2e-cli/src/commands/llm_docs.rs`: `HUB` +
`TOPICS` (one `include_str!` per spoke, list kept in sync with `llm/` by
`r2e-cli/tests/llm_docs.rs`), `full()`, `export()`, `stamped_version()`;
clap: `r2e docs --llm [<topic>] [--full] [--pretty] [--export [DIR]]`
(`--export` requires `--llm`). `r2e new` exports into `docs/r2e/` and writes
`.claude/skills/r2e/SKILL.md`; the `agents_md` template routes to
`docs/r2e/llm.txt` and says how to refresh. `r2e doctor` check 10 reads the
stamp back. Not done, by decision: crate `include`, `--path`, `r2e add` rows
(§2.3).

**Step 5 — doctests.** `llm-doctests/` (workspace member, edition 2024 so
rustdoc merges the blocks into one binary — a full run is ~10 s after the
first build): `build.rs` copies the spokes into `OUT_DIR`, rewrites
```` ```rust ```` to ```` ```rust,no_run ```` and injects two hidden lines
(`use r2e::prelude::*;` and `use llm_doctests::fixtures::<topic>::*;`);
`src/fixtures/<topic>.rs` holds the placeholder types a topic's snippets name
without defining, so the markdown stays free of `# ` scaffolding (agents read
it raw). Hidden lines inside a spoke are reserved for wrapping a body fragment
in an `async fn`; deliberately partial blocks are ```` ```rust,ignore ````.
CI: `cargo test --workspace` already covers it; the runner gained
`libsqlite3-dev libpq-dev` for the diesel snippets. Triage record: see below.

### 5.1 Doctest triage record

136 ```` ```rust ```` blocks at the start; 9 compiled untouched. Final state
(2026-09-01): **124 blocks compile, 12 are `rust,ignore`** (deliberately
partial shapes), `cargo test --doc -p llm-doctests` = 0 failures. Per topic,
what it took (a "correction" is a snippet that was **wrong** against the
current API — the failures this whole mechanism exists to catch):

| topic | corrections | `rust,ignore` (reason) |
|---|---|---|
| openfga | `try_id(..)?` / `grant(..).await?` in a handler returning `Result<_, HttpError>` — there is no `From<InvalidObjectId>` / `From<OpenFgaError>` for `HttpError`; rewritten with explicit `map_err` (follow-up: add the two `From` impls and put `?` back); two `pub mod authz` in one block; `use crate::authz;` with no origin | — |
| security | `DecodingKey` used, never imported | role-based access fragment (bare attributes, no impl) |
| di-beans | a rustc error message in a bare fence (compiled as Rust) → ```` ```text ```` | — |
| events | — (placeholder bodies / missing structs completed) | — |
| quick-start | — | `src/app.rs`, `src/lib.rs`, `src/main.rs` layout blocks (file modules / `include!("app.rs")` cannot resolve in a doctest) |
| mcp-server | `Params<T>` needs `#[derive(ObjectParams)]` (sealed) — the derive was missing, prose fixed too | per-tool authorization block (stacks `scopes` and `any_scopes` forms on one method) |
| configuration | `.register::<UserController>()` before `build_state()` — controllers go through `register_controller` on the typed builder; `VaultConfigProvider::new(...)` literal ellipsis | — |
| managed-resources | bare `r2e_data_sqlx::` / `r2e_data_diesel::` paths (apps see them as `r2e::r2e_data_*`); `DieselDataSource` unimported; `sqlx::Error?` into `HttpError` — no `From`, and `map_error!` cannot add one downstream (orphan rule) → `map_err(HttpError::internal)` idiom; undefined bindings | — |
| background-work | `#[producer] fn reindexer() -> Reindexer` — the producer marker type collides with the returned type; `BackgroundService` derive without the `run()` it calls | — |
| scheduled-tasks | bare `r2e_scheduler::` path; dangling `.schedule_task(…)` receiver; bean with `SqlitePool` dep registered without a pool; `use r2e::Decorate;` was a comment | — |
| modules | builder chain contradicted the `#[module]` declaration above it (`requires_plugins(Executor)`, `grpc_services`, `imports(LocalEventBus, SqlitePool)` all unsatisfied) | — |
| error-handling | `map_error! { sqlx::Error => Internal }` expands to `impl From<sqlx::Error> for HttpError` — orphan error in every downstream crate; section rewritten around the `for <YourError> { … }` form + `map_err` fallback. **Source fix** in `r2e-core/src/error.rs`: form 2 was unreachable (rule 1's `$err_ty:ty` fatally parsed `for MyError {` as a HRTB), rules reordered with a load-bearing comment | — |
| validation | `id: u64` without a garde rule (needs `#[garde(skip)]`); `#[param(default = 1)]` on `u32` fails (`(1).into()` — no `From<i32>`) → `1u32`; before/after block declared `SearchQuery` twice → split into two fences | — |
| static-files | `.immutable_prefix("assets/")` takes `Into<Option<String>>` (unlike sibling `exclude_prefix`) → `.to_string()`; `EmbeddedFrontend` unimported | — |
| lifecycle-hooks | `self.drain_and_close()` / `ConnectionPool::new` bodies never shown; `#[on_start]` warm-cache called `self.load()` on the cache instead of the injected store | — |
| guards | `ProjectGuard` impl had no `check`; `RateLimit::per_user(..)` shown on controllers without identity (compile error by design, `REQUIRES_IDENTITY`) → identity added | — |
| grpc | tonic `Request/Response/Status` unimported (prelude `Response` shadows); `CorsLayer` ellipsis → real `.allow_methods`; `tower-http` (cors) added to `llm-doctests` | build.rs + `include_protos!()` block (needs a build script's `OUT_DIR`) |
| devservices | `DevOpenFga` / `DevKeycloak` / `TestApp` unimported | — |
| core-concepts | `{ ... }` handler bodies replaced by real one-liners; bare `#[sse]` method outside any impl → full controller | — |
| websockets | — (fixtures only) | — |
| testing | `.bearer(self.user_token.clone())` — `bearer` takes `&str` → `.bearer(&self.user_token)`; `TestJwt` unimported | — |
| tenancy | `PoolDirectory` bean without `Clone`; bare `sqlx::Error?` into `HttpError` (no `From`) → `map_err` idiom | — |
| tenancy-datasources | 2× bare `sqlx::Error?` → `map_err`; `use diesel::PgConnection;` misses `RunQueryDsl` → `use diesel::prelude::*;`; `PerTenant` unimported; `impl App for MultiTenantDbApp` without the struct; 3 route methods floating outside any impl → real controllers | — |
| multipart / data-access / proxy-catch-all | floating route methods wrapped in real controllers; `dispatch` had a literal `...` body → `self.upstream.forward(req).await` | data-access first block — imports `r2e_data::Entity`; **`Repository<T,ID>`/`SqlxRepository`/`QueryBuilder` don't exist anywhere in the workspace** — topic needs a rewrite by the data-story owner |
| interceptors | — | attribute catalogue with no host item; `impl UserController { ... }` placeholder |
| prometheus | — | 3 alternative `.plugin(...)` lines side by side; block requiring `metrics-exporter-prometheus` (deliberate non-dependency) |
| app-builder | — | signature listing (not items) |
| prometheus / plugins / openapi / oidc-server / runtime-facade / observability / sse / dev-experience | wrapping + fixtures only | — |

Harness note: `llm-doctests/build.rs` originally matched only column-0 ```` ```rust ```` fences; fences indented inside a list item (one case: `dev-experience.md`) were skipped. Now matches on `trim()` and re-emits fence + hidden lines with the original indent.

### 5.2 Follow-ups surfaced by the triage (not done here)

- **`llm/data-access.md` first block documents an API that does not exist**:
  `r2e_data::Entity`, `Repository<T, ID>`, `SqlxRepository`, `QueryBuilder`
  appear nowhere in the workspace (only the managed-transaction API under
  `r2e-data/backends/*` is real). Block parked as `rust,ignore`; the topic
  needs a rewrite by whoever owns the data story.
- Add `From<InvalidObjectId>` / `From<OpenFgaError>` for `HttpError` in
  `r2e-openfga`, then restore `?` in `llm/openfga.md` (currently `map_err`).
- Papercut: `#[param(default = <int literal>)]` expands via `.into()`, so any
  non-`i32` integer field needs a suffixed literal (`1u32`).
- Papercut: `EmbeddedFrontendBuilder::immutable_prefix` takes
  `Into<Option<String>>` while every sibling takes `Into<String>` —
  `&str`-hostile inconsistency.
- The three `rust,ignore` quick-start layout blocks (`src/app.rs` /
  `src/main.rs` / controllers) could compile if `llm-doctests/build.rs`
  emitted stub sibling files; not worth it today.
- Source fix landed as part of the triage: `r2e-core/src/error.rs` —
  `map_error!` form 2 (`for MyError { … }`) was unreachable (rule order,
  fatal `:ty` fragment parse); rules reordered, form 2 first.



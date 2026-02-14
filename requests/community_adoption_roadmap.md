# R2E — Roadmap Recommendations for Community Adoption

> What the project needs to go from "promising personal project" to "framework people trust in production."

---

## Priority 1 — Blocking for any adoption

### Publish on crates.io

Right now, using r2e means adding a git dependency. No serious project will do this in production — crates.io publication is table stakes.

**What to do:**
- Publish all workspace crates (`r2e`, `r2e-core`, `r2e-macros`, `r2e-security`, etc.) to crates.io.
- Pick a clear namespace strategy (the `r2e-*` prefix is fine).
- Ensure each crate has a proper `description`, `license`, `repository`, and `readme` in its `Cargo.toml`.

### Semver and tagged releases

Without versioned releases, users have no way to pin a stable version or know what changed.

**What to do:**
- Tag a `v0.1.0` release on GitHub with release notes.
- Maintain a `CHANGELOG.md` (keep it simple — [keep a changelog](https://keepachangelog.com/) format works well).
- Follow semver strictly: breaking changes bump minor (in 0.x), non-breaking bump patch.
- For each release, note any migration steps needed.

### CI pipeline (GitHub Actions)

A visible green badge builds trust instantly.

**What to do:**
- Add a GitHub Actions workflow that runs `cargo check --workspace`, `cargo test --workspace`, and `cargo clippy --workspace` on every PR and push to main.
- Add the badge to the README.
- Bonus: add `cargo fmt --check` to enforce consistent formatting.

---

## Priority 2 — Needed for first real users

### Documentation site

The README is excellent for a quick overview but doesn't scale. Users need searchable, structured docs.

**What to do:**
- Set up [mdBook](https://rust-lang.github.io/mdBook/) or a similar tool. It's simple, Rust-native, and deploys to GitHub Pages for free.
- Structure it roughly like:
  - **Getting Started** — from zero to running app in 5 minutes.
  - **Core Concepts** — Controllers, DI, State, Plugins.
  - **Security** — JWT/OIDC setup, guards, roles.
  - **Data Access** — Entity, Repository, QueryBuilder, transactions.
  - **Events & Scheduling** — EventBus, consumers, cron.
  - **Testing** — TestApp, TestJwt, mocking services.
  - **Advanced** — Custom interceptors, custom guards, custom plugins, escape hatches to raw Axum.
  - **API Reference** — link to auto-generated `cargo doc` output.

### More example apps

The single `example-app` covers the basics. Real-world patterns need more coverage.

**Suggested examples:**
- **`example-postgres`** — Full CRUD with PostgreSQL, migrations (sqlx or refinery), and pagination.
- **`example-multi-tenant`** — Auth with tenant isolation via guards.
- **`example-websocket-chat`** — WsRooms + EventBus for a real-time app.
- **`example-microservice`** — Two services communicating, showing how r2e apps compose.

Each example should have its own README explaining what it demonstrates.

### Escape hatches

See the companion document (`escape-hatches-proposal.md`) for detailed proposals. The most impactful ones to implement first:

1. **Raw Axum router mounting** (`merge_router`) — low effort, high value.
2. **Bean override in tests** — critical for testability.
3. **`#[raw]` extractor passthrough** — enables Axum ecosystem interop.

---

## Priority 3 — Accelerates community growth

### `cargo expand` guide for macros

Proc macros are a trust barrier. Many Rust developers are wary of "magic" they can't inspect.

**What to do:**
- Add a doc page: "What does `#[derive(Controller)]` generate?" with a concrete `cargo expand` output, annotated with comments.
- Same for `#[routes]`, `#[bean]`, `#[scheduled]`, `#[consumer]`.
- This also helps contributors understand the codebase.

### Contributing guide

**What to include:**
- How to set up the dev environment.
- How the workspace is structured (which crate does what).
- How to run tests locally.
- PR conventions (squash? conventional commits?).
- A list of "good first issues" — label them on GitHub.

### Plugin authoring guide

The plugin system (`Health`, `Cors`, `Tracing`, `Scheduler`...) is already there, but there's no documentation on how to write a custom one.

**What to do:**
- Document the `Plugin` trait (or whatever abstraction plugins implement).
- Provide a minimal example: "How to write a plugin that adds a custom header to all responses."
- This opens the door to community-contributed plugins (Sentry, OpenTelemetry, S3, etc.).

### Error messages quality

Framework macros often produce terrible error messages when misused. This is one of the biggest sources of frustration.

**What to do:**
- Invest in `compile_error!` messages in the proc macros for common mistakes:
  - Missing `state` attribute on controller.
  - `#[inject]` on a type that isn't in the state.
  - `#[routes]` on a struct without `#[derive(Controller)]`.
  - `#[config("key")]` with an unsupported type.
- Test these error paths with [trybuild](https://github.com/dtolnay/trybuild).

---

## Priority 4 — Nice to have, longer term

### Security audit transparency

The framework includes JWT validation, JWKS caching, role-based access, and rate limiting. These are security-critical components.

**What to do:**
- Add a `SECURITY.md` with a vulnerability reporting process.
- Document the threat model: what r2e protects against, what it doesn't.
- Long-term: consider a third-party audit once the API stabilizes.

### Benchmarks

Actix Web and Axum both publish benchmarks. r2e is built on Axum, so the overhead of the macro layer is the interesting question.

**What to do:**
- Add a simple benchmark (e.g. [criterion](https://github.com/bheisler/criterion.rs) or [wrk](https://github.com/wg/wrk)) comparing:
  - Raw Axum handler vs. equivalent r2e controller handler.
  - With and without interceptors/guards.
- Publish results in the docs. The goal isn't to "win" but to show the overhead is negligible.

### Migration guide from Axum

Many potential users already have Axum apps. A guide showing "here's your Axum app, here's the same thing in r2e" would lower the adoption barrier significantly.

---

## Summary table

| Item | Effort | Impact | Priority |
|---|---|---|---|
| Publish on crates.io | Small | Critical | P1 |
| Semver + tagged releases | Small | Critical | P1 |
| CI pipeline | Small | High | P1 |
| Documentation site (mdBook) | Medium | High | P2 |
| More example apps | Medium | High | P2 |
| Escape hatches (merge_router, bean override) | Medium | High | P2 |
| `cargo expand` guide | Small | Medium | P3 |
| Contributing guide + good first issues | Small | Medium | P3 |
| Plugin authoring guide | Small | Medium | P3 |
| Error messages quality (trybuild) | Medium | Medium | P3 |
| Security audit transparency | Small | Medium | P4 |
| Benchmarks | Medium | Low | P4 |
| Migration guide from Axum | Medium | Low | P4 |
---
topic: dev-experience
features: dev-reload
tokens: ~1500
requires: quick-start
---

## Dev Experience

### TL;DR

- `r2e::app_main!(MyApp)` is the canonical `main.rs`; a custom entry point must
  end with `r2e::exit_on_boot_error(r2e::launch!(MyApp).await)` to keep the
  one-message, exit-1 contract.
- Build through the fallible forms (`try_build_state()` /
  `try_build_with_consumers()`) for that contract — `build_state()` /
  `build_with_consumers()` panic with the same rendered error instead.
- Relay the feature in YOUR package: `dev-reload = ["r2e/dev-reload"]`, because
  `r2e dev` passes `--features dev-reload` to your package, not to `r2e`.
- Hot reload needs the macro `r2e::launch!` (not `launch::<MyApp>()`): Subsecond
  only patches the tip crate that owns `main.rs`.
- Put pools/buses/validators in `App::setup` — its `Env` survives every patch;
  `App::build` re-runs per patch, so keep `load_config` there to pick up YAML edits.
- Keep `AppEnv` / `setup_env` in `src/env.rs`: `Cargo.toml`, `build.rs`,
  `src/env.rs` and `src/env/**` are cold paths that trigger a full restart.
- Under `dev-reload` a failed `build` is logged and the loop waits for the next
  patch; only an `App::setup` failure propagates out of `launch!`.
- CLI: `r2e new`, `r2e dev`, `r2e generate controller <Name>`, `r2e routes`,
  `r2e doctor`, `r2e docs [<module>]`, `r2e docs --llm [<topic>] [--export]`.
- `cargo expand -p <crate>` to inspect macro output when debugging.

- `r2e::app_main!(MyApp)` is the canonical `main.rs` — see llm/quick-start.md.
  Use a manual `#[r2e::main]` + `launch!` only when the entry point must be
  customized.
- **Boot failures exit non-zero with one message.** `r2e::launch!(A)` /
  `r2e::launch::<A>()` yield `Result<(), BootError>`; `app_main!` ends with
  `r2e::exit_on_boot_error(...)`, which prints `error: <e>` plus one
  `  caused by: <cause>` line per `source()` level to stderr and exits `1` — no
  panic, no backtrace. A custom entry point gets the same contract by ending the
  same way:

  ```rust
  #[r2e::main]
  async fn main() {
      r2e::exit_on_boot_error(r2e::launch!(MyApp).await);
  }
  ```

  `r2e::boot_error_report(&err) -> String` renders that message on its own if a
  binary wants to log it instead. What reaches it: `App::setup`/`App::build`,
  config loading (a missing/malformed file, a failing provider, an unresolved
  `${...}` placeholder, a typed section that does not bind), bean/producer
  construction, plugin build, module/plugin controller config validation, and
  the controller `#[post_construct]`/`#[on_start]` hooks — provided the app
  builds through the fallible forms (`try_build_state()`,
  `try_build_with_consumers()`). The `build_state()` / `build_with_consumers()`
  wrappers deliberately panic with the same rendered error instead; use them
  only where a panic is the intended failure mode.
  **Under `dev-reload` this contract is different by design:** `launch!` runs a
  hot-patch loop, so a failed `build` (or `serve_auto`) is logged and the loop
  waits for the next patch instead of exiting — only an `App::setup` failure
  propagates out of `launch!`. A failed cycle rolls its staged bean graph back,
  so the caches keep the last successful cycle and the next patch re-runs the
  full startup lifecycle.
- Hot reload setup: `r2e dev` passes `--features dev-reload` to **your package**,
  not to the `r2e` crate. Your app's `Cargo.toml` must relay the feature:

  ```toml
  [features]
  dev-reload = ["r2e/dev-reload"]
  ```

  Without this relay the build fails with
  `error: the package 'my-app' does not contain this feature: dev-reload`.
- Hot reload (`dev-reload` feature): `r2e::launch!` runs the Subsecond hot-patch
  loop. It is a **macro** (not `launch::<MyApp>()`) because Subsecond only remaps
  symbols in the *tip crate* that owns `main.rs`; the macro expands its loop —
  including the concrete function Subsecond patches — into your crate. A generic
  dispatcher monomorphised from `r2e-core` would never be patched. Without
  `dev-reload` the macro just calls `r2e::launch::<MyApp>()`.
  `app_main!` solves the tip-crate constraint without duplicating application
  code: it includes canonical `src/app.rs` directly in the binary in every
  build, while `src/lib.rs` includes the exact same source for tests. Thus
  controller, service, and `App::build` edits are patchable in dev without any
  user-written feature `cfg` or knowledge of the library crate identifier.
  `App::setup()` runs **once** and its `Env` survives every
  hot-patch (put pools/buses/validators there); `App::build()` re-runs per
  patch. Keep `load_config` inside `build`: because `build()` re-runs per patch,
  its `load_config` re-reads `application.yaml` from disk each time, so YAML edits
  are picked up on the next hot-patch.
  Keep `AppEnv` and `setup_env` in `src/env.rs`: values allocated before a patch
  must not cross an incompatible Rust layout. `r2e dev` therefore treats
  `Cargo.toml`, `build.rs`, `src/env.rs`, and `src/env/**` as **cold paths** and
  performs a full child-process restart when they change.
- `r2e new <name>` — scaffold a project (`App`-trait layout). `r2e dev` — hot
  reload. `r2e generate controller <Name>` — scaffold pieces. `r2e routes` —
  list routes. `r2e doctor` — diagnose setup. `r2e docs [<module>]` — print
  bundled, version-matched per-module docs (curated `TL;DR` by default; `--full`
  for the whole doc, `--pretty` to render markdown). No arg lists modules;
  accepts a slug (`events`) or a crate name (`r2e-events`). `r2e docs --llm`
  prints this reference for the installed version (hub, `<topic>`, or
  `--full`); `r2e docs --llm --export` writes it under `docs/r2e/` — refresh
  it after every R2E upgrade (`r2e doctor` warns when it is stale).
- `cargo expand -p <crate>` — inspect macro output when debugging.

### Do not

- Do not replace `app_main!` with a library-only `use my_app::MyApp`; that would
  put hot code back behind the unpatchable library boundary.

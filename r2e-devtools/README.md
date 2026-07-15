# r2e-devtools

Subsecond hot-reload integration for R2E.

## Overview

Wraps [Dioxus Subsecond](https://github.com/DioxusLabs/dioxus) to enable hot-patching of Rust code at runtime during development. Only used via the `dev-reload` feature flag.

> **Application code should not call these directly.** Declare your app with the
> `App` trait (`setup`/`build`) and run `r2e::launch::<MyApp>()` — `launch`
> delegates to `serve_with_hotreload_env` internally under `dev-reload`. The
> functions below are the low-level machinery that entry point wraps.

## Usage

```rust
use r2e_devtools::serve_with_hotreload;

serve_with_hotreload(
    || async {
        // Called once at startup — expensive setup (DB, tracing)
        let db = setup_db().await;
        AppEnv { db }
    },
    |env| async move {
        // Called on every hot-patch — load config (re-read from disk per patch),
        // build app and serve
        let config = load_config();
        build_and_serve(config, env).await;
    },
).await;
```

The setup closure runs once; the server closure is re-executed on every code change. The `Env` persists across hot-patches.

## Key functions

| Function | Description |
|----------|-------------|
| `serve_with_hotreload` | Setup closure + server closure |
| `serve_with_hotreload_env` | Pre-built env + server closure |

## Important

The server function must be a non-capturing closure or named function — wrapping in `Arc` breaks Subsecond's jump-table dispatch and falls back to stale code.

## License

Apache-2.0

# Migration: the plugin API (post-state `.with()` → one `Plugin` trait)

R2E is not in production, so this break shipped without a feature flag. What it
keeps is a **compile-time migration path**: every leftover use of the old API
fails to build with an error that names this page (`r2e-compile-tests/cases/
plugins/fail/plugin_old_install_signature.rs`, `plugin_with_removed.rs` pin
those diagnostics).

## What changed

| Old (pre-`820a5a8`) | New |
|---|---|
| Two plugin kinds: pre-state `PluginInstall` and post-state `Plugin { fn install(self, app: AppBuilder<T>) -> AppBuilder<T> }` | **One** `Plugin` trait: `type Provided`, `type Deps`, `type Config`, `type Controllers`, `async fn build(self, deps, config, ctx) -> Result<Provided, PluginBuildError>` |
| `AppBuilder::with(plugin)` after `build_state()` | `AppBuilder::plugin(plugin)` **before** `build_state()` — always |
| `should_be_last()` ordering hint | `ctx.wrap_router(..)` (Finalize stage; outermost layer) |
| Router mutation inside `install` | `ctx.add_layer(..)` (Graph), `ctx.after_routes(\|routes\| ..)` (Routes), `ctx.wrap_router(..)` (Finalize) |
| Reading beans from `app.state()` | `type Deps = (A, B)` — resolved and passed to `build`; `Late<T>` for beans that exist only after the graph is complete |
| Ad-hoc config reads | `type Config = MyConfig; const CONFIG_PREFIX` — parsed and passed as `Option<Config>` |
| Registering controllers from a plugin | `type Controllers = (C1, C2)`; their deps are compile-checked |

## Step by step

1. Replace `fn install(self, app)` with the new trait shape:

   ```rust
   impl Plugin for Metrics {
       type Provided = (MetricsRegistry,);   // beans you add to the graph
       type Deps = (HealthRegistry,);         // beans you need (compile-checked)
       type Config = MetricsConfig;           // `metrics.*` section, or ()
       type Controllers = (MetricsController,);

       async fn build(self, (health,): Self::Deps, cfg: Option<MetricsConfig>,
                      ctx: &mut PluginBuildContext)
           -> Result<Self::Provided, PluginBuildError>
       {
           let reg = MetricsRegistry::new(cfg.unwrap_or_default());
           ctx.add_layer(move |r| r.layer(track_requests(reg.clone())));
           Ok((reg,))
       }
   }
   ```

2. Move the `.with(Metrics)` call above `build_state()` and rename it to
   `.plugin(Metrics)`. Order between plugins no longer matters for bean
   resolution (the graph is solved); router effects are staged
   Graph → Routes → Finalize regardless of call order.

3. Delete any `impl PluginInstall` — it is generated from `Plugin`.

4. Run `cargo check`. A leftover `.with(..)` fails with `E0277` whose message
   names this page; a legacy `impl Plugin { fn install(self, app) }` fails
   with `E0407` (`install` is not a member of trait `Plugin`) + `E0046`
   listing exactly the items to add (`Provided`, `Deps`, `Config`,
   `Controllers`, `build`).

## Compatibility period

- `AppBuilder::with` still **exists** as a `#[deprecated]` + `#[doc(hidden)]`
  shim whose bound (`PostStatePluginRemoved`) nothing implements: the call
  compiles to an `E0277` with the migration text instead of a bare "no method
  named `with`". It will be deleted one minor version after the first tagged
  release that carries this page (tracked in `CHANGELOG.md`).
- `Plugin`'s `on_unimplemented` diagnostic (a type passed to `.plugin(..)`
  without any `impl Plugin`) names the old `install(self, app)` method and
  this page.
- Future breaks to `Plugin` follow the same rule: the old shape must keep
  failing with a diagnostic that names a `docs/migration/*.md` page, pinned by
  a trybuild case in `r2e-compile-tests/cases/plugins/fail/`.

Reference: `docs/claude/plugins.md` (authoritative plugin guide).

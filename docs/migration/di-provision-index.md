# Generic DI builder bounds

Ordinary `.provide()`, `.register::<T>()`, and `.build_state().await` chains
infer the provision list, requirement list, and lookup witnesses. These steps
apply to helpers that write those types or bounds explicitly.

The requirement list `R` is now a list of **groups**: each registration pushes
its whole dependency list as one element (`TCons<B::Deps, R>`) instead of
concatenating it into a flat list.

1. For a helper registering a bean, async bean, or producer `B`, delete the
   `R: TAppend<B::Deps>` bound: registration needs no bound on `R` any more.
   Prefer the public result alias
   `r2e::builder::Registered<B::Provided, B::Deps, P, R, Mods>` for
   `B: Registrable`. If spelling out the builder type, the new requirement
   list is `TCons<B::Deps, R>`. The same applies to `with_default_bean`,
   `with_default_async_bean`, and `with_default_producer`. For
   `with_bean_factory` the new requirement list is
   `TCons<TCons<R2eConfig, TNil>, R>`.
2. For a helper calling `.plugin(pl)` on a generic builder, or a hand-written
   `PluginInstall`, replace `R: TAppend<Pl::Required>` with
   `Pl::Required: PushRequired<R>` (from `r2e::type_list`). Prefer the result
   alias `r2e::builder::WithPluginInstalled<Pl, P, R, Mods>`. An empty
   `Required` leaves `R` unchanged; a non-empty one yields
   `TCons<Pl::Required, R>`. For `register_module::<M>()`, delete the
   `ModulePluginsR<M, P, R, Mods>: TAppend<M::Imports>` bound; the result
   alias `ModuleRegistered<M, P, R, Mods>` is unchanged.
3. For a helper calling `build_state` or `try_build_state`, import
   `r2e::type_list::{AllGroupsSatisfied, ProvisionIndex}`. Use
   `P: BuildHList + ProvisionIndex` and
   `R: AllGroupsSatisfied<P::Index, W>` in its bounds. Keep the existing module
   bounds and return type. A custom provision-list type implementing
   `BuildHList` must also implement `ProvisionIndex`; it can use
   `type Index = Self` if its existing `Contains` implementations already
   describe membership. For a custom tail that will receive further registrations,
   use a provision list ending in `TNil` so every partial block can be indexed.
4. Let Rust infer the lookup witness at concrete call sites. Controller and
   endpoint registration still check dependencies against the materialized
   state. Runtime factories, construction order, and hooks use the bean registry.

# Generic DI builder bounds

Ordinary `.provide()`, `.register::<T>()`, and `.build_state().await` chains
infer the provision list, requirement list, and lookup witnesses. These steps
apply to helpers that write those types or bounds explicitly.

1. For a helper registering a bean, async bean, or producer `B`, change
   `R: TAppend<B::Deps>` to `B::Deps: TAppend<R>`. Prefer the public result
   alias `r2e::builder::Registered<B::Provided, B::Deps, P, R, Mods>` for
   `B: Registrable`. If spelling out the builder type, the new requirement
   list is `<B::Deps as TAppend<R>>::Output`. The same order applies to
   `with_default_bean`, `with_default_async_bean`, and `with_default_producer`.
   For `with_bean_factory` the new requirement list is `TCons<R2eConfig, R>`.
2. For a helper calling `build_state` or `try_build_state`, import
   `r2e::type_list::ProvisionIndex`. Use `P: BuildHList + ProvisionIndex` and
   `R: AllSatisfied<P::Index, W>` in its bounds. Keep the existing module
   bounds and return type. A custom provision-list type implementing
   `BuildHList` must also implement `ProvisionIndex`; it can use
   `type Index = Self` if its existing `Contains` implementations already
   describe membership. For a custom tail that will receive further registrations,
   use a provision list ending in `TNil` so every partial block can be indexed.
3. Let Rust infer the lookup witness at concrete call sites. Controller and
   endpoint registration still check dependencies against the materialized
   state. Runtime factories, construction order, and hooks use the bean registry.

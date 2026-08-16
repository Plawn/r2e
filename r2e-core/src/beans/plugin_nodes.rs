use super::{reuse_clone_none, BeanError, BeanRegistration, BeanRegistry};
use std::any::{type_name, Any, TypeId};

impl BeanRegistry {
    /// Register a [`PreStatePlugin`](crate::PreStatePlugin) as bean-graph
    /// nodes: one **group node** running the plugin's `build` (yielding the
    /// whole `Provided` tuple as a hidden `PluginOut<Pl>` bean), plus one
    /// **projection node** per `Provided` element cloning its slot out of the
    /// group. Called by the blanket `RawPreStatePlugin` impl at `.plugin()`
    /// time; the caller has already handled the all-pinned skip.
    ///
    /// Projections register **strict** (`overridable: false`): colliding with
    /// an app `.provide()`/`.register()` of the same type — or installing the
    /// same plugin twice — is a `DuplicateBean` error at `build_state()`. A
    /// type pinned via [`pin_provide`](Self::pin_provide) *before* install
    /// keeps its override (the projection is skipped); the group still runs.
    ///
    /// All plugin nodes are volatile: rebuilt every dev-reload cycle, and
    /// forcing resolution on a same-fingerprint cache hit.
    pub(crate) fn register_plugin_group<Pl: crate::PreStatePlugin>(
        &mut self,
        plugin: Pl,
        effects: crate::plugin::EffectsSlot,
    ) {
        use crate::plugin::{plugin_action_name, PluginOut};
        use crate::type_list::{PluginDeps, PluginProvisions};

        let name = plugin_action_name::<Pl>();
        let graph_handle = self.graph_handle.clone();
        let base_version = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            type_name::<Pl>().hash(&mut hasher);
            hasher.finish() ^ Pl::BUILD_VERSION
        };

        // Group node: deps = the plugin's declared `Deps` (real topo edges).
        // `R2eConfig` needs no edge — `load_config` provides it as a value,
        // available to every factory before construction starts.
        self.beans.push(BeanRegistration {
            type_id: TypeId::of::<PluginOut<Pl>>(),
            type_name: type_name::<PluginOut<Pl>>(),
            dependencies: <Pl::Deps as PluginDeps>::dependencies(),
            config_keys: vec![],
            build_version: base_version,
            factory: Box::new(move |ctx| {
                Box::pin(async move {
                    let config = ctx.try_get::<crate::config::R2eConfig>();
                    let enabled =
                        crate::plugin::plugin_config_enabled(config.as_ref(), Pl::CONFIG_PREFIX);
                    let typed = crate::plugin::load_plugin_config_from::<Pl>(config.as_ref(), name);
                    let deps = <Pl::Deps as PluginDeps>::resolve_from_context(&ctx);
                    let mut bctx =
                        crate::plugin::PluginBuildContext::new(enabled, graph_handle, config);
                    // Fully qualified so a plugin's own inherent `build` method
                    // (e.g. a builder-style `fn build(self)`) can't shadow it.
                    let provided = crate::plugin::PreStatePlugin::build(plugin, deps, typed, &mut bctx)
                        .await
                        .map_err(
                        |source| BeanError::PluginBuild {
                            plugin: name,
                            source,
                        },
                    )?;
                    // The `enabled` decision travels WITH the effects: it was
                    // taken here, from the graph's `R2eConfig`, and the
                    // install-order action must not recompute it from the
                    // builder's own config (they can disagree — a pinned
                    // `R2eConfig` bean).
                    effects.fill(enabled, bctx.into_effects());
                    let boxed: Box<dyn Any + Send + Sync> = Box::new(PluginOut::<Pl>(provided));
                    Ok((ctx, boxed))
                })
            }),
            post_construct: None,
            overridable: false,
            reuse_clone: reuse_clone_none,
            volatile: true,
        });

        // Projection nodes: one per `Provided` element, cloning slot `i` out
        // of the group tuple. Skipped for pinned types (override wins).
        for (i, (tid, tname)) in <Pl::Provided as PluginProvisions>::element_ids()
            .into_iter()
            .enumerate()
        {
            if self.pinned.contains(&tid) {
                continue;
            }
            self.beans.push(BeanRegistration {
                type_id: tid,
                type_name: tname,
                dependencies: vec![(
                    TypeId::of::<PluginOut<Pl>>(),
                    type_name::<PluginOut<Pl>>(),
                )],
                config_keys: vec![],
                build_version: base_version.wrapping_add(1 + i as u64),
                factory: Box::new(move |ctx| {
                    Box::pin(async move {
                        let out = ctx.get::<PluginOut<Pl>>();
                        Ok((ctx, out.0.clone_element(i)))
                    })
                }),
                post_construct: None,
                overridable: false,
                reuse_clone: reuse_clone_none,
                volatile: true,
            });
        }
    }
}

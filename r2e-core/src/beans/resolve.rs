use super::{
    BeanContext, BeanError, BeanRegistration, BeanRegistry, Factory, PostConstructFn, ReusePlan,
};
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

impl BeanRegistry {
    /// Resolve the dependency graph and build all beans.
    ///
    /// Uses Kahn's algorithm for topological sorting. Returns a
    /// [`BeanContext`] with all instances, or a [`BeanError`] if the graph
    /// is invalid (cycles, missing deps, or duplicates).
    pub async fn resolve(self) -> Result<BeanContext, BeanError> {
        self.resolve_reusing(None).await
    }

    /// Resolve the graph, optionally reusing unchanged instances from a
    /// previous dev-reload cycle. [`resolve`](Self::resolve) is the `None`
    /// case; `build_state()` passes `Some` when a hot-patch changed the
    /// graph fingerprint, so only changed beans (and their transitive
    /// dependents) are reconstructed and every other instance — with its
    /// in-memory state — carries over.
    #[doc(hidden)]
    pub async fn resolve_reusing(
        mut self,
        reuse: Option<ReusePlan>,
    ) -> Result<BeanContext, BeanError> {
        // ── Dev-reload partial rebuild: harvest reusable material ───────
        // Beans targeted by a decorator fill are excluded from reuse: their
        // `DecoSlot` is a `OnceLock` already set on the old instance, so a
        // refill against the new graph would silently no-op and leave stale
        // interceptor sets. They rebuild (fresh slot, fresh fill) instead.
        let deco_targets: HashSet<TypeId> = self.deco_fills.iter().map(|(t, _)| *t).collect();
        // A decorator target cannot keep its old OnceLock-backed slot. Any
        // bean that captured that target (directly or transitively) must also
        // rebuild, otherwise the new context would expose a fresh target while
        // a reused dependent still held a clone of the previous instance.
        let mut forced_rebuild = deco_targets.clone();
        // Same argument for volatile registrations (the plugin group node and
        // its per-provision projections): their factories re-run every cycle by
        // design, so the new context exposes a FRESH plugin bean while any
        // dependent reused from cycle N-1 still holds a clone of the previous
        // one — split-brain, e.g. a service holding a `Tenanted<T>` whose
        // `GraphHandle` points at the dropped cycle-N-1 graph (`NoSource` at
        // request time). Seeding them here lets the closure loop below carry
        // the rebuild to every transitive dependent. Cost: those dependents lose
        // their in-memory state on each hot patch, which is the same trade the
        // deco-target rule already makes, and dev-only.
        forced_rebuild.extend(
            self.beans
                .iter()
                .filter(|reg| reg.volatile)
                .map(|reg| reg.type_id),
        );
        // The injectable app shutdown signal is **cycle-scoped**. Every hot
        // patch builds a fresh `AppBuilder`, hence a fresh shutdown root, and
        // the cycle it replaces has already cancelled its own. Carrying the
        // provided value over would hand cycle N a token that reads
        // `is_cancelled() == true` from its first request on — every `#[sse]`
        // stream would close immediately and every task waiting on it would
        // exit at once. Seeding it here both keeps it out of the
        // provided-pinning loop below and rebuilds every bean that captured a
        // clone of it, so no dependent is left holding the dead token.
        forced_rebuild.insert(TypeId::of::<crate::rt::ShutdownToken>());
        loop {
            let mut grew = false;
            for (type_id, dependencies) in self
                .beans
                .iter()
                .map(|reg| (reg.type_id, &reg.dependencies))
                .chain(
                    self.lazy_beans
                        .iter()
                        .map(|reg| (reg.type_id, &reg.dependencies)),
                )
            {
                if !forced_rebuild.contains(&type_id)
                    && dependencies
                        .iter()
                        .any(|(dep_id, _)| forced_rebuild.contains(dep_id))
                {
                    forced_rebuild.insert(type_id);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let mut reused_instances: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();
        let mut pinned_provided: HashSet<TypeId> = HashSet::new();
        if let Some(plan) = &reuse {
            for reg in &self.beans {
                // Volatile registrations (plugin nodes) are never carried
                // over: their factories re-run every cycle by design.
                if plan.unchanged.contains(&reg.type_id)
                    && !forced_rebuild.contains(&reg.type_id)
                    && !reg.volatile
                {
                    if let Some(inst) = (reg.reuse_clone)(&plan.old_ctx) {
                        reused_instances.insert(reg.type_id, inst);
                    }
                }
            }
            // Pin provided values from the previous cycle so reused and
            // rebuilt beans keep sharing one instance (no split-brain).
            // **Config-derived** values stay fresh: `R2eConfig`, the
            // `LiveConfigRegistry` and every typed `ConfigProperties` /
            // `#[config(section)]` bean are recomputed from the freshly loaded
            // config by this cycle's `load_config`, so the per-patch YAML
            // re-read is deliberate — config edits must apply on the next
            // patch, whatever shape they are read in.
            for (tid, value) in self.provided.iter_mut() {
                if self.config_derived.contains(tid) || forced_rebuild.contains(tid) {
                    continue;
                }
                if let Some(clone_fn) = self.provided_reuse_clones.get(tid) {
                    if let Some(old) = clone_fn(&plan.old_ctx) {
                        *value = old;
                        pinned_provided.insert(*tid);
                    }
                }
            }
            tracing::debug!(
                reused_beans = reused_instances.len(),
                pinned_provided = pinned_provided.len(),
                "dev-reload: partial rebuild — carrying unchanged instances over"
            );
        }

        let mut entries: HashMap<TypeId, Box<dyn Any + Send + Sync>> = HashMap::new();

        // Move provided instances into the resolved set.
        for (tid, value) in self.provided {
            entries.insert(tid, value);
        }

        // Lift the lifecycle hooks out before the bean fields are consumed.
        let provided_post_constructs = std::mem::take(&mut self.provided_post_constructs);
        let disposer_builders = std::mem::take(&mut self.disposers);
        let deco_fills = std::mem::take(&mut self.deco_fills);

        // Resolve default/alternative beans: remove overridable registrations
        // that have been superseded by a later registration of the same TypeId.
        Self::resolve_alternatives(&mut self.beans);
        Self::resolve_alternatives(&mut self.lazy_beans);

        let bean_count = self.beans.len();
        let lazy_type_ids: HashSet<TypeId> = self.lazy_beans.iter().map(|lr| lr.type_id).collect();

        // Check for duplicates before any construction.
        Self::check_for_duplicates(&self.beans, &entries, &self.plugin_owners)?;
        Self::check_for_lazy_duplicates(&self.lazy_beans, &entries, &self.beans)?;

        // ── Config validation, aggregated across every declaring host ────
        //
        // Beans declare their keys through `BeanRegistration::config_keys`;
        // background services registered through `#[producer(start)]` declare
        // theirs through `ServiceComponent::config_keys` / `config_sections`
        // (they are constructed from the graph later, at serve time, where a
        // missing key would otherwise be a fail-late panic).
        //
        // Both go into ONE report: an app missing a bean key *and* a service
        // key must not have to fix them one boot at a time. This also runs
        // unconditionally — a pinned-only app (`bean_count == 0`) can still
        // register a `#[producer(start)]` service.
        Self::validate_all_config(&self.beans, &self.service_config_keys, &entries)?;

        // Factory-bean post-construct hooks, in topological order. Populated
        // inside the construction branch, run after the decorator fills below.
        let mut factory_pc_fns: Vec<PostConstructFn> = Vec::new();

        let mut ctx = if bean_count == 0 {
            BeanContext::new(entries)
        } else {
            // Build dependency graph
            let id_to_idx = Self::build_type_index(&self.beans);

            // Include lazy beans in the known-types set for dependency validation
            Self::check_missing_dependencies(&self.beans, &entries, &id_to_idx, &lazy_type_ids)?;

            // Topological sort (shared generic; builds its own type index).
            let sorted_order = Self::topological_sort(&self.beans)?;

            // Extract post-construct fns before consuming beans. Reused
            // instances skip theirs: the hook already ran on that same
            // instance in the cycle that built it.
            factory_pc_fns = sorted_order
                .iter()
                .filter_map(|&idx| {
                    if reused_instances.contains_key(&self.beans[idx].type_id) {
                        None
                    } else {
                        self.beans[idx].post_construct.take()
                    }
                })
                .collect();

            // Construct beans in order (async)
            Self::construct_beans_in_order(self.beans, sorted_order, entries, reused_instances)
                .await?
        };

        // Fill bean decorator slots from the fully-resolved graph, BEFORE any
        // post-construct hook — so `#[post_construct]` and any direct call see
        // a decorated bean. The slot's Arc is shared with every clone handed
        // out during construction, so the fill is observed everywhere. Runs
        // unconditionally (a pinned-only app has `bean_count == 0` but may
        // still queue a fill via `override_bean_decorated`).
        for (_, fill) in deco_fills {
            fill(&ctx);
        }

        // Run factory-bean post-construct hooks in topological order.
        for pc_fn in factory_pc_fns {
            ctx = pc_fn(ctx)
                .await
                .map_err(|e| BeanError::PostConstruct(e.to_string()))?;
        }

        // Run post-construct hooks for provided/plugin beans, after every
        // factory-bean post-construct. Reads each target by type from the
        // resolved context (pinned overrides honoured). Values pinned from
        // the previous dev-reload cycle skip theirs — same instance, the
        // hook already ran.
        for (tid, pc_fn) in provided_post_constructs {
            if pinned_provided.contains(&tid) {
                continue;
            }
            ctx = pc_fn(ctx)
                .await
                .map_err(|e| BeanError::PostConstruct(e.to_string()))?;
        }

        // ── Lazy beans ──────────────────────────────────────────────────
        if !self.lazy_beans.is_empty() {
            // Validate lazy bean dependencies: all deps must exist in the
            // eagerly-resolved set, provided instances, or other lazy beans.
            let eager_ids: HashSet<TypeId> =
                ctx.base.keys().chain(ctx.overlay.keys()).copied().collect();

            for lazy_reg in &self.lazy_beans {
                for (dep_id, dep_name) in &lazy_reg.dependencies {
                    if !eager_ids.contains(dep_id) && !lazy_type_ids.contains(dep_id) {
                        return Err(BeanError::MissingDependency {
                            bean: lazy_reg.type_name.to_string(),
                            dependency: dep_name.to_string(),
                        });
                    }
                }
            }

            // Validate lazy bean config keys
            let lazy_keys: Vec<_> = self
                .lazy_beans
                .iter()
                .flat_map(|reg| {
                    // Only `Required` keys are presence-validated.
                    reg.config_keys
                        .iter()
                        .filter(|(_, _, kind)| kind.is_required())
                        .map(move |(key, ty_name, _)| (reg.type_name, *key, *ty_name))
                })
                .collect();
            Self::do_validate_config_keys(
                &lazy_keys,
                ctx.try_get::<crate::config::R2eConfig>().as_ref(),
            )?;

            // Build lazy slots from the fully resolved context.
            // Use a shared, mutable map so snapshots can resolve lazy-to-lazy deps.
            let lazy_slots: Arc<RwLock<HashMap<TypeId, Arc<dyn crate::di::lazy::LazyResolve>>>> =
                Arc::new(RwLock::new(HashMap::new()));
            ctx = ctx.with_lazy_slots(Arc::clone(&lazy_slots));
            for lazy_reg in self.lazy_beans {
                // Dev-reload partial rebuild: an unchanged lazy bean keeps
                // its previous slot `Arc` — including any already-resolved
                // value inside it — instead of getting a fresh slot.
                if let Some(plan) = &reuse {
                    if plan.unchanged.contains(&lazy_reg.type_id)
                        && !forced_rebuild.contains(&lazy_reg.type_id)
                    {
                        if let Some(slot) = plan.old_ctx.lazy_slot(lazy_reg.type_id) {
                            lazy_slots
                                .write()
                                .expect("Lazy slots lock poisoned")
                                .insert(lazy_reg.type_id, slot);
                            continue;
                        }
                    }
                }
                let snapshot = ctx.clone();
                let slot = (lazy_reg.slot_factory)(snapshot);
                lazy_slots
                    .write()
                    .expect("Lazy slots lock poisoned")
                    .insert(lazy_reg.type_id, slot);
            }
        }

        // Materialize pre-destroy disposers against the fully resolved graph and
        // stash them on the context. Reversed so disposal runs in reverse
        // registration order (last registered disposes first).
        if !disposer_builders.is_empty() {
            let mut hooks: Vec<crate::plugin::AsyncShutdownHook> =
                disposer_builders.into_iter().map(|d| d(&ctx)).collect();
            hooks.reverse();
            ctx.disposers = std::sync::Mutex::new(hooks);
        }

        Ok(ctx)
    }

    /// Construct beans in topological order (async).
    ///
    /// Factories receive a `BeanContext` (entries behind `Arc`) and return it.
    /// Lazy bean factories may clone the context to capture a dependency
    /// snapshot. When that happens, `Arc::get_mut` fails and new entries go
    /// into the overlay. This two-layer design avoids the `Arc::try_unwrap`
    /// panic that would otherwise occur.
    async fn construct_beans_in_order(
        beans: Vec<BeanRegistration>,
        sorted_order: Vec<usize>,
        entries: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        mut reused_instances: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<BeanContext, BeanError> {
        let mut bean_data: Vec<Option<(TypeId, Factory)>> = beans
            .into_iter()
            .map(|r| Some((r.type_id, r.factory)))
            .collect();

        let mut ctx = BeanContext::new(entries);

        for idx in sorted_order {
            let (type_id, factory) = bean_data[idx].take().unwrap();
            // Dev-reload partial rebuild: an unchanged bean's instance is
            // inserted at its topological position (dependents constructed
            // later read it from the context) — its factory never runs.
            if let Some(inst) = reused_instances.remove(&type_id) {
                ctx = ctx.with_new_entry(type_id, inst);
                continue;
            }
            let (returned_ctx, bean_value) = factory(ctx).await?;
            ctx = returned_ctx.with_new_entry(type_id, bean_value);
        }

        Ok(ctx)
    }
}

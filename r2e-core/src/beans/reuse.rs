use super::{BeanContext, BeanRegistry};
#[cfg(feature = "dev-reload")]
use super::{BeanError, BeanFingerprints, RegMeta};
#[cfg(feature = "dev-reload")]
use crate::config::ConfigKeyKind;
use std::any::{Any, TypeId};
#[cfg(feature = "dev-reload")]
use std::collections::HashMap;
use std::collections::HashSet;
#[cfg(feature = "dev-reload")]
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Monomorphized eager-clone hook stored per registration (a plain fn
/// pointer — zero-sized, no runtime cost outside dev-reload rebuilds).
pub(super) type ReuseCloneFn = fn(&BeanContext) -> Option<Box<dyn Any + Send + Sync>>;

pub(super) fn reuse_clone_of<T: Clone + Send + Sync + 'static>(
    ctx: &BeanContext,
) -> Option<Box<dyn Any + Send + Sync>> {
    ctx.try_get_eager::<T>()
        .map(|b| Box::new(b) as Box<dyn Any + Send + Sync>)
}

/// Reuse stub for volatile registrations (plugin nodes): never reused, so the
/// hook is never consulted — but the field is a plain fn pointer and needs a
/// value. Returning `None` keeps any accidental call safe (treated as "cannot
/// reuse, rebuild").
pub(super) fn reuse_clone_none(_ctx: &BeanContext) -> Option<Box<dyn Any + Send + Sync>> {
    None
}

/// Instructions for a dev-reload partial rebuild: which beans of the
/// previous cycle's resolved graph may be reused instead of reconstructed.
///
/// Built by `build_state()` when the graph fingerprint changed: a bean whose
/// **per-bean** fingerprint is unchanged (constructor tokens, config values,
/// and every transitive dependency's fingerprint) keeps its instance from
/// `old_ctx`; everything else — and every transitive dependent of a changed
/// bean, whose fingerprint changes by propagation — is rebuilt.
#[doc(hidden)]
pub struct ReusePlan {
    /// The fully resolved context of the previous dev-reload cycle.
    pub old_ctx: Arc<BeanContext>,
    /// `TypeId`s whose per-bean fingerprint is identical to the previous
    /// cycle's. Only these are candidates for instance reuse.
    pub unchanged: HashSet<TypeId>,
}

#[cfg(feature = "dev-reload")]
struct FingerprintReg<'a> {
    type_id: TypeId,
    type_name: &'static str,
    dependencies: &'a Vec<(TypeId, &'static str)>,
    config_keys: &'a Vec<(&'static str, &'static str, ConfigKeyKind)>,
    build_version: u64,
    is_lazy: bool,
}

#[cfg(feature = "dev-reload")]
impl RegMeta for FingerprintReg<'_> {
    fn reg_type_id(&self) -> TypeId {
        self.type_id
    }
    fn reg_type_name(&self) -> &'static str {
        self.type_name
    }
    fn reg_dependencies(&self) -> &[(TypeId, &'static str)] {
        self.dependencies.as_slice()
    }
    // Fingerprint snapshots are built after dedup; ordering never consults this.
    fn reg_overridable(&self) -> bool {
        false
    }
}

impl BeanRegistry {
    /// Compute the graph fingerprint without constructing any beans.
    ///
    /// Performs alternative resolution, topological sorting, and computes
    /// per-bean fingerprints from metadata only. This is cheap
    /// and allows `build_state` to compare against the cached fingerprint
    /// before doing the expensive construction step.
    ///
    /// **Note:** This does NOT validate missing dependencies or config keys.
    /// Validation happens in [`resolve()`](Self::resolve) which is called when
    /// the fingerprint changes and a full rebuild is needed.
    ///
    /// Returns `(graph_fingerprint, per_bean_fingerprints)`.
    #[cfg(feature = "dev-reload")]
    pub fn compute_fingerprint(&self) -> Result<(u64, BeanFingerprints), BeanError> {
        // Work on a snapshot of bean metadata to handle deduplication
        // without mutating self (resolve() will do the real dedup later).
        let alt_remove = Self::overridable_indices_to_remove(&self.beans);
        let lazy_alt_remove = Self::overridable_indices_to_remove(&self.lazy_beans);

        let mut beans: Vec<FingerprintReg<'_>> = self
            .beans
            .iter()
            .enumerate()
            .filter(|(i, _)| !alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
                is_lazy: false,
            })
            .collect();

        // Include lazy beans in the fingerprint graph.
        let lazy_regs: Vec<FingerprintReg<'_>> = self
            .lazy_beans
            .iter()
            .enumerate()
            .filter(|(i, _)| !lazy_alt_remove.contains(i))
            .map(|(_, reg)| FingerprintReg {
                type_id: reg.type_id,
                type_name: reg.type_name,
                dependencies: &reg.dependencies,
                config_keys: &reg.config_keys,
                build_version: reg.build_version,
                is_lazy: true,
            })
            .collect();

        beans.extend(lazy_regs);

        // The config is needed both for per-bean fingerprints and for the
        // whole-config component of the graph fingerprint.
        let config = self
            .provided
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        // Seed the graph fingerprint with the ENTIRE config: an edit that no
        // bean declares in `config_keys()` must still invalidate the cached
        // state, or the `R2eConfig` instance inside the cached graph would be
        // served stale. Per-bean fingerprints stay key-scoped, so such an
        // edit rebuilds nothing — the partial-rebuild path just re-provides
        // the fresh config.
        let mut graph_hasher = std::collections::hash_map::DefaultHasher::new();
        match config {
            Some(config) => config.full_fingerprint().hash(&mut graph_hasher),
            None => 0u64.hash(&mut graph_hasher),
        }

        let bean_count = beans.len();
        if bean_count == 0 {
            return Ok((graph_hasher.finish(), Vec::new()));
        }

        // Topological sort (shared generic with resolve(); detects cycles).
        let sorted_order = Self::topological_sort(&beans)?;

        let mut dep_fingerprints: HashMap<TypeId, u64> = HashMap::new();
        let mut per_bean: BeanFingerprints = Vec::new();

        for &idx in &sorted_order {
            let reg = &beans[idx];
            let fp = Self::compute_reg_fingerprint(reg, config, &dep_fingerprints);
            dep_fingerprints.insert(reg.type_id, fp);
            per_bean.push((reg.type_id, reg.type_name, fp));
            fp.hash(&mut graph_hasher);
        }

        Ok((graph_hasher.finish(), per_bean))
    }

    /// Compute a full fingerprint for a bean, incorporating its own config
    /// fingerprint, its `BUILD_VERSION`, and the fingerprints of all its
    /// dependencies (transitively).
    #[cfg(feature = "dev-reload")]
    fn compute_reg_fingerprint(
        reg: &FingerprintReg<'_>,
        config: Option<&crate::config::R2eConfig>,
        dep_fingerprints: &HashMap<TypeId, u64>,
    ) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // 1. Build version (hash of constructor source tokens)
        reg.build_version.hash(&mut hasher);

        // 1b. Registration mode. Switching `#[bean]` to `#[bean(lazy)]`
        // changes graph semantics even when the constructor is unchanged.
        reg.is_lazy.hash(&mut hasher);

        // 2. Config values this bean COPIES.
        //
        // Required, optional and section keys alike are fingerprinted, so
        // editing any of them under `r2e dev` rebuilds the bean and its
        // dependents. `#[live_config]` keys are deliberately excluded: their
        // freshness comes from the registry push, not from a rebuild —
        // fingerprinting them would churn the bean (and drop its in-memory
        // state) on every live edit. Empty lists contribute nothing, so a
        // live-only bean keeps a stable fingerprint across live edits.
        //
        // `Section` entries carry a dotted **prefix** instead of an exact key,
        // so they are hashed separately: the bean copied a whole subtree, and
        // must be rebuilt when any key under it moves.
        if let Some(config) = config {
            let mut exact: Vec<&str> = Vec::new();
            let mut prefixes: Vec<&str> = Vec::new();
            for (key, _, kind) in reg.config_keys.iter() {
                if !kind.is_fingerprinted() {
                    continue;
                }
                if kind.is_prefix() {
                    prefixes.push(key);
                } else {
                    exact.push(key);
                }
            }
            if !exact.is_empty() {
                config.config_fingerprint(&exact).hash(&mut hasher);
            }
            if !prefixes.is_empty() {
                prefixes.sort_unstable();
                prefixes.dedup();
                for prefix in prefixes {
                    config.prefix_fingerprint(prefix).hash(&mut hasher);
                }
            }
        }

        // 3. Fingerprints of all bean dependencies (transitively propagated)
        for (dep_id, _) in reg.dependencies {
            if let Some(&dep_fp) = dep_fingerprints.get(dep_id) {
                dep_fp.hash(&mut hasher);
            }
        }

        hasher.finish()
    }
}

#[cfg(all(test, feature = "dev-reload"))]
mod fingerprint_tests {
    use super::*;

    #[test]
    fn eager_and_lazy_registration_modes_have_distinct_fingerprints() {
        let dependencies = Vec::new();
        let config_keys = Vec::new();
        let eager = FingerprintReg {
            type_id: TypeId::of::<u32>(),
            type_name: "u32",
            dependencies: &dependencies,
            config_keys: &config_keys,
            build_version: 7,
            is_lazy: false,
        };
        let lazy = FingerprintReg {
            is_lazy: true,
            ..eager
        };

        assert_ne!(
            BeanRegistry::compute_reg_fingerprint(&eager, None, &HashMap::new()),
            BeanRegistry::compute_reg_fingerprint(&lazy, None, &HashMap::new()),
        );
    }
}

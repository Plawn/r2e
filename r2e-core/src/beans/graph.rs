use super::{
    BeanError, BeanRegistration, BeanRegistry, LazyBeanRegistration, RegMeta, ServiceConfigDecl,
};
use crate::config::ConfigKeyKind;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet};

impl BeanRegistry {
    /// Shared config-key validation: checks the given triples against an R2eConfig.
    pub(super) fn do_validate_config_keys(
        all_keys: &[(&str, &str, &str)],
        config: Option<&crate::config::R2eConfig>,
    ) -> Result<(), BeanError> {
        if all_keys.is_empty() {
            return Ok(());
        }
        let Some(config) = config else {
            return Ok(());
        };
        let errors = crate::config::validate_keys(config, all_keys);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(BeanError::MissingConfigKeys(
                crate::config::ConfigValidationError { errors },
            ))
        }
    }

    /// Compute the set of indices whose registrations are overridable and
    /// have been superseded by a later registration of the same `TypeId`.
    /// Works uniformly for eager and lazy registrations via [`RegMeta`].
    pub(super) fn overridable_indices_to_remove<R: RegMeta>(regs: &[R]) -> HashSet<usize> {
        if !regs.iter().any(|r| r.reg_overridable()) {
            return HashSet::new();
        }

        let mut type_indices: HashMap<TypeId, Vec<(usize, bool)>> = HashMap::new();
        for (i, reg) in regs.iter().enumerate() {
            type_indices
                .entry(reg.reg_type_id())
                .or_default()
                .push((i, reg.reg_overridable()));
        }

        let mut remove = HashSet::new();
        for indices in type_indices.values() {
            if indices.len() <= 1 {
                continue;
            }
            let last_idx = indices.last().unwrap().0;
            for &(idx, overridable) in indices {
                if idx != last_idx && overridable {
                    remove.insert(idx);
                }
            }
        }
        remove
    }

    /// Remove overridable (default) registrations that have been superseded
    /// by a later (alternative) registration of the same `TypeId`.
    ///
    /// This runs before the global duplicate-check so that the
    /// default/alternative pattern never trips it.
    /// Works uniformly for eager and lazy registrations via [`RegMeta`].
    pub(super) fn resolve_alternatives<R: RegMeta>(regs: &mut Vec<R>) {
        let remove = Self::overridable_indices_to_remove(regs);
        if !remove.is_empty() {
            let mut idx = 0;
            regs.retain(|_| {
                let keep = !remove.contains(&idx);
                idx += 1;
                keep
            });
        }
    }

    /// Check for duplicate bean registrations.
    pub(super) fn check_for_duplicates(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<(), BeanError> {
        let mut seen: HashMap<TypeId, &str> = HashMap::new();
        for reg in beans {
            if entries.contains_key(&reg.type_id) {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
            if seen.insert(reg.type_id, reg.type_name).is_some() {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Check for duplicate lazy registrations, or conflicts with eager beans or provided entries.
    pub(super) fn check_for_lazy_duplicates(
        lazy_beans: &[LazyBeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        beans: &[BeanRegistration],
    ) -> Result<(), BeanError> {
        let eager_ids: HashSet<TypeId> = beans.iter().map(|r| r.type_id).collect();
        let mut seen: HashMap<TypeId, &str> = HashMap::new();
        for reg in lazy_beans {
            if entries.contains_key(&reg.type_id) || eager_ids.contains(&reg.type_id) {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
            if seen.insert(reg.type_id, reg.type_name).is_some() {
                return Err(BeanError::DuplicateBean {
                    type_name: reg.type_name.to_string(),
                });
            }
        }
        Ok(())
    }

    /// Build a map from TypeId to bean index.
    pub(super) fn build_type_index(beans: &[BeanRegistration]) -> HashMap<TypeId, usize> {
        beans
            .iter()
            .enumerate()
            .map(|(i, r)| (r.type_id, i))
            .collect()
    }

    /// Check that all dependencies are available.
    /// `lazy_type_ids` contains TypeIds of lazy beans (also considered "known").
    pub(super) fn check_missing_dependencies(
        beans: &[BeanRegistration],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
        id_to_idx: &HashMap<TypeId, usize>,
        lazy_type_ids: &HashSet<TypeId>,
    ) -> Result<(), BeanError> {
        for reg in beans {
            for (dep_id, dep_name) in &reg.dependencies {
                if !entries.contains_key(dep_id)
                    && !id_to_idx.contains_key(dep_id)
                    && !lazy_type_ids.contains(dep_id)
                {
                    return Err(BeanError::MissingDependency {
                        bean: reg.type_name.to_string(),
                        dependency: dep_name.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// Validate every config declaration reaching this graph — bean keys,
    /// `#[producer(start)]` service keys, and those services' typed
    /// `#[config_section]`s — as ONE aggregated [`BeanError::MissingConfigKeys`].
    ///
    /// Beans and services are validated together on purpose: they fail at the
    /// same moment (graph resolution) for the same reason (a key the app never
    /// set), so splitting them into two early returns only means two boots to
    /// find two typos.
    pub(super) fn validate_all_config(
        beans: &[BeanRegistration],
        services: &[ServiceConfigDecl],
        entries: &HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    ) -> Result<(), BeanError> {
        // Only `Required` keys are presence-validated — `Optional`
        // (`Option<T>`) keys resolve to `None` when absent, `Section` keys are
        // validated through the type-aware validators below, and `Live`
        // (`#[live_config]`) keys start empty and arrive by push.
        let required = |kind: &ConfigKeyKind| kind.is_required();

        let mut all_keys: Vec<(&str, &str, &str)> = beans
            .iter()
            .flat_map(|reg| {
                reg.config_keys
                    .iter()
                    .filter(|(_, _, kind)| required(kind))
                    .map(move |(key, ty_name, _)| (reg.type_name, *key, *ty_name))
            })
            .collect();

        all_keys.extend(services.iter().flat_map(|(_, type_name, keys, _)| {
            keys.iter()
                .filter(|(_, _, kind)| required(kind))
                .map(move |(key, ty_name, _)| (*type_name, *key, *ty_name))
        }));

        let config = entries
            .get(&TypeId::of::<crate::config::R2eConfig>())
            .and_then(|v| v.downcast_ref::<crate::config::R2eConfig>());

        let Some(config) = config else {
            return Ok(());
        };

        let mut errors = crate::config::validate_keys(config, &all_keys);
        for (_, _, _, sections) in services {
            errors.extend(crate::config::validate_declared_sections(sections, config));
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(BeanError::MissingConfigKeys(
                crate::config::ConfigValidationError { errors },
            ))
        }
    }

    /// Perform a topological sort (Kahn's algorithm) over any slice of
    /// registrations. Returns construction order, or a [`BeanError::CyclicDependency`]
    /// listing the nodes left in a cycle. Dependencies pointing outside the
    /// slice (provided instances) are ignored for ordering.
    ///
    /// Shared by [`resolve`](Self::resolve) and (under `dev-reload`)
    /// [`compute_fingerprint`](Self::compute_fingerprint) so both stay in lockstep.
    pub(super) fn topological_sort<R: RegMeta>(nodes: &[R]) -> Result<Vec<usize>, BeanError> {
        let n = nodes.len();
        let id_to_idx: HashMap<TypeId, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, r)| (r.reg_type_id(), i))
            .collect();

        // in_degree = number of deps that are other nodes (not provided).
        let mut in_degree: Vec<usize> = nodes
            .iter()
            .map(|reg| {
                reg.reg_dependencies()
                    .iter()
                    .filter(|(d, _)| id_to_idx.contains_key(d))
                    .count()
            })
            .collect();

        // Dependents: for each node index, which other node indices depend on it.
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, reg) in nodes.iter().enumerate() {
            for (dep_id, _) in reg.reg_dependencies() {
                if let Some(&dep_idx) = id_to_idx.get(dep_id) {
                    dependents[dep_idx].push(i);
                }
            }
        }

        // Seed queue with nodes whose deps are all already provided.
        let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
        let mut sorted_order: Vec<usize> = Vec::with_capacity(n);

        while let Some(idx) = queue.pop() {
            sorted_order.push(idx);
            for &dep_idx in &dependents[idx] {
                in_degree[dep_idx] -= 1;
                if in_degree[dep_idx] == 0 {
                    queue.push(dep_idx);
                }
            }
        }

        // If not all nodes were sorted, there's a cycle. Walk the stuck
        // subgraph (nodes with `in_degree > 0`) to extract one concrete
        // cycle path, so the error reads "A -> B -> C -> A" instead of
        // listing every node tangled in the strongly connected component.
        if sorted_order.len() != n {
            let cycle = Self::find_cycle(nodes, &id_to_idx, &in_degree);
            return Err(BeanError::CyclicDependency { cycle });
        }

        Ok(sorted_order)
    }

    /// Extract one concrete dependency cycle from the subgraph left unsorted
    /// by Kahn's algorithm, as type names ending with a repeat of the first
    /// element (`[A, B, C, A]`).
    ///
    /// After Kahn's algorithm stalls, exactly the unsorted nodes have
    /// `in_degree > 0`, and every cycle lies entirely within them, so the DFS
    /// only follows edges between such nodes. The first back-edge to a node on
    /// the current DFS path closes a cycle.
    fn find_cycle<R: RegMeta>(
        nodes: &[R],
        id_to_idx: &HashMap<TypeId, usize>,
        in_degree: &[usize],
    ) -> Vec<String> {
        // 0 = unvisited, 1 = on the current DFS path, 2 = fully explored.
        const ON_PATH: u8 = 1;
        const DONE: u8 = 2;

        fn dfs<R: RegMeta>(
            i: usize,
            nodes: &[R],
            id_to_idx: &HashMap<TypeId, usize>,
            in_degree: &[usize],
            color: &mut [u8],
            path: &mut Vec<usize>,
        ) -> Option<Vec<usize>> {
            color[i] = ON_PATH;
            path.push(i);
            for (dep_id, _) in nodes[i].reg_dependencies() {
                let Some(&j) = id_to_idx.get(dep_id) else {
                    continue;
                };
                if in_degree[j] == 0 {
                    continue; // sorted node — cannot be part of a cycle
                }
                match color[j] {
                    ON_PATH => {
                        let start = path.iter().position(|&x| x == j).unwrap();
                        let mut cycle = path[start..].to_vec();
                        cycle.push(j);
                        return Some(cycle);
                    }
                    DONE => {}
                    _ => {
                        if let Some(cycle) = dfs(j, nodes, id_to_idx, in_degree, color, path) {
                            return Some(cycle);
                        }
                    }
                }
            }
            path.pop();
            color[i] = DONE;
            None
        }

        let mut color = vec![0u8; nodes.len()];
        let mut path = Vec::new();
        for i in 0..nodes.len() {
            if in_degree[i] > 0 && color[i] == 0 {
                if let Some(cycle) = dfs(i, nodes, id_to_idx, in_degree, &mut color, &mut path) {
                    return cycle
                        .into_iter()
                        .map(|idx| nodes[idx].reg_type_name().to_string())
                        .collect();
                }
            }
        }

        // Unreachable when called after a stalled Kahn sort, but degrade
        // gracefully: report the stuck nodes as before.
        (0..nodes.len())
            .filter(|&i| in_degree[i] > 0)
            .map(|i| nodes[i].reg_type_name().to_string())
            .collect()
    }
}

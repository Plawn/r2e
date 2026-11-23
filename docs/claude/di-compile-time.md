# Reducing DI graph check time

Keep the existing `.register::<T>() … .build_state().await` chain. The cost
target is downstream `cargo check` with dependencies already compiled.

## Method

1. **Push each registration's requirements as one group.** In
   `builder/mod.rs`, `Registered` computes `TCons<Deps, R>`: `R` is a list of
   requirement groups (a bean's `Deps`, a plugin's `Required`, a module's
   `Imports`), never a flat list of beans. A registration costs one cons cell
   whatever its number of dependencies, and no `TAppend` projection. A plugin
   pushes its group through `PushRequired`, which leaves `R` untouched when
   the group is empty — `.when(cond, |b| b.plugin(Health))` needs the builder
   type to stay the same. Keep registering the runtime factory in the original
   call order.
2. **Index the final provisions once at the type level.** In
   `type_list/provision_index.rs`, `ProvisionIndex` maps eight adjacent
   `TCons` cells to one block containing a tuple of eight types and the
   index of the tail. Zero to seven trailing cells use their original list.
   A membership witness selects a slot or continues to the next block.
   Each occurrence has its own witness, including duplicates.
3. **Check the completed graph.** Both `build_state` and `try_build_state`
   require `R: AllGroupsSatisfied<P::Index, W>`, which checks each group with
   `AllSatisfied`. This permits forward references. The solver depth is the
   number of registrations plus the widest group — with a flat list it was the
   total number of dependency edges, which overflowed rustc's stack (SIGBUS)
   between 1 500 and 1 900 edges.
   Materialize the original `P` with `BuildHList`. Runtime sort, overrides,
   hooks, and controller access continue to use the existing paths.

## Reproduce and verify

Run `python3 scripts/bench-di-check.py` on the pinned toolchain. It generates
graphs of 100 and 250 beans where each bean depends on the 1, 5 or 10 beans
registered before it (`--deps`), warms dependencies, disables incremental
compilation, and times three checks per case. To compare a base checkout with
the same workload, run the same script with `--repo /path/to/base`. This
measures the whole consumer crate, including the bean macros. The reported
43 s came from a separate application.

Medians on Apple Silicon with nightly-2026-08-27 (`a8efab7a` is the flat list
before this work; "index only" is the eight-slot index with a flat `R`):

| beans × deps | edges | `a8efab7a` | index only | index + groups |
|---|---|---|---|---|
| 100 × 1 | 99 | 1.38 s | 0.43 s | 0.42 s |
| 100 × 5 | 485 | 16.49 s | 0.56 s | 0.48 s |
| 100 × 10 | 945 | rustc SIGBUS | 0.90 s | 0.55 s |
| 100 × 15 | 1 380 | rustc SIGBUS | 1.32 s | 0.63 s |
| 250 × 1 | 249 | 15.79 s | 1.30 s | 1.22 s |
| 250 × 10 | 2 445 | rustc SIGBUS | rustc SIGBUS | 1.90 s |
| 300 × 5 | 1 485 | rustc SIGBUS | 2.95 s | 2.36 s |
| 500 × 10 | 4 945 | rustc SIGBUS | rustc SIGBUS | 7.54 s |

A crate that only defines 100 beans (no builder chain) checks in about 0.3 s,
so at that size the graph check is within 0.3 s of the floor.

## What is left

- The check at `build_state()` is still superlinear in the number of
  **distinct required types**: about 1.1 s at 300 beans, 4.4 s at 500 (and
  +670 MB of rustc memory), independent of the edge count. Not profiled yet
  (`-Zself-profile` + `summarize`).
- Wider blocks do not help: 16 or 32 slots per block move the 500 × 5 case by
  less than 10 %. Walking the index is not the remaining cost.
- `recursion_limit` now depends on the number of provisions only (the HList
  state), not on dependencies: the default 128 holds up to about 110 beans,
  512 up to about 500.

Run `cargo test -p r2e-core --test builder --test di` and
`cargo test -p r2e-compile-tests --test compile_tests -- compile_fail`.
Check an early and a late provider, every slot in a full block, the short
tail, a genuinely missing bean, and ordinary state access after construction.
Generic downstream helpers are covered by the
[migration guide](../migration/di-provision-index.md).

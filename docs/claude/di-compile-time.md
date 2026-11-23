# Reducing DI graph check time

Keep the existing `.register::<T>() … .build_state().await` chain. The cost
target is downstream `cargo check` with dependencies already compiled.

## Method

1. **Prepend each registration's requirements.** In `builder/mod.rs`,
   `Registered` computes `<Deps as TAppend<R>>::Output`. Concatenation walks
   its left operand, so each registration visits only its own dependencies.
   Keep registering the runtime factory in the original call order.
2. **Index the final provisions once at the type level.** In
   `type_list/provision_index.rs`, `ProvisionIndex` maps eight adjacent
   `TCons` cells to one block containing a tuple of eight types and the
   index of the tail. Zero to seven trailing cells use their original list.
   A membership witness selects a slot or continues to the next block.
   Each occurrence has its own witness, including duplicates.
3. **Check the completed graph.** Both `build_state` and `try_build_state`
   require `R: AllSatisfied<P::Index, W>`. This permits forward references.
   Materialize the original `P` with `BuildHList`. Runtime sort, overrides,
   hooks, and controller access continue to use the existing paths.

## Reproduce and verify

Run `python3 scripts/bench-di-check.py` on the pinned toolchain. It generates
chains of 50, 100, and 250 beans with one dependency on the preceding bean,
warms dependencies, disables incremental compilation, and times three checks
per size. To compare a base checkout with the same workload, run the same
script with `--repo /path/to/base`. On Apple Silicon with nightly-2026-08-27,
the median for 250 beans fell from 15.79 s at `a8efab7a` to 1.30 s with the
index (12.1× faster). This measures the whole consumer crate, including the
bean macros. The reported 43 s came from a separate application.

Run `cargo test -p r2e-core --test builder --test di` and
`cargo test -p r2e-compile-tests --test compile_tests -- compile_fail`.
Check an early and a late provider, every slot in a full block, the short
tail, a genuinely missing bean, and ordinary state access after construction.
Generic downstream helpers are covered by the
[migration guide](../migration/di-provision-index.md).

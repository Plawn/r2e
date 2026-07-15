# Real hot-reload on the lib layout — problem statement & constraint set

## Status

- The canonical `App` trait / `launch!` API is merged (PR #32). Dispatch is correct:
  a concrete `__r2e_server` fn is expanded in the tip crate and Subsecond patches it.
- **But** hot-reload only genuinely applies to code compiled into the **tip crate**
  (the crate that owns `main.rs`). Verified live (2026-07-15, dioxus-cli 0.7.3, macOS):
  a bin-only app hot-patches in ~2 s; the recommended lib layout never serves edits.
- This limitation **predates** the App-trait work: it was introduced silently by the
  Testing-DX refactor (`bab64f7`) which moved app code from `main.rs` into `lib.rs`
  so integration tests could boot the real app. Hot-reload and blueprint testing have
  therefore **never coexisted**.

## The core conflict

| Requirement | Forced by | Consequence |
|---|---|---|
| App + controllers in a **lib** target | cargo: `tests/*.rs` are separate crates that can only link the package's lib target | `#[r2e::test(app = MyApp)]`, `TestApp::boot::<A>()` need `use my_app::MyApp` |
| Editable code in the **tip (bin)** crate | Subsecond: "Changes to crates outside this [tip] crate will be ignored" — dx only rebuilds/remaps the crate owning `main.rs` | controller edits in a lib are compiled into a patch dylib that the live process loads but never dispatches to |

`lib.rs` and `main.rs` of the same package are **two different crates** to the
compiler. The bin imports the lib like any dependency; everything imported is
boot-time code, invisible to patches.

## Observed facts (live experiments — keep these as regression truths)

1. Bin-only crate (App + controller in `main.rs`): handler edit → served response
   changes ~2 s after save. Works on dioxus 0.7.3 **and** 0.7.9 (version skew ruled out).
2. Lib layout (example-app), four configurations tried — `launch!` nested dispatch,
   master-style module-level `__r2e_server`, loop driven from the lib, all crates
   pinned to the dx version — **none** ever served an edited handler string.
3. In the lib layout with `launch!`: `A::build()` **does** re-run per patch (file
   marker grows), the process does not restart (same PID, listener socket identity
   stable), patch dylibs ARE dlopen'd — the re-run just executes the **old** lib code.
4. Generic dispatch monomorphized from `r2e-core` (`__r2e_launch_patch::<A>`) is never
   remapped by the jump table → that was the branch regression, fixed by expanding a
   concrete fn at the call site via `launch!`.

## Constraint set for any candidate solution

A solution is acceptable iff ALL of these hold:

- **C1 — one canonical declaration.** The user writes exactly one `impl App for MyApp`
  (setup/Env/build) in one place. No parallel "dev variant" of the app the user must
  keep in sync by hand.
- **C2 — tests keep booting the real app.** `#[r2e::test(app = MyApp)]` /
  `TestApp::boot::<MyApp>()` keep working, which requires the App (and everything it
  registers) importable from a lib target.
- **C3 — edits to controllers/services/handlers are actually served** after a
  hot-patch (observable via curl), without process restart, with `Env` (pools, buses)
  preserved. Target latency: same order as the bin-only case (~2–5 s).
- **C4 — config re-read per patch.** `application.yaml` edits apply on the next patch
  (no config caching across patches — deliberate decision, see PR #32).
- **C5 — zero cost in prod.** No `dev-reload` code, deps, or indirection compiled in
  when the feature is off; `launch!` must keep degrading to plain `launch::<A>()`.
- **C6 — no per-request regression in prod**, and measured/bounded overhead in dev if
  the solution adds dispatch indirection on the request path.
- **C7 — works with the standard workspace shape** (app package inside a cargo
  workspace, r2e crates as path/registry deps) and with `r2e dev` orchestrating dx.
- **C8 — graceful degradation.** If a patch cannot apply (struct layout change,
  signature change), fall back to what dx already does (full rebuild + restart),
  never serve silently stale code while pretending it patched.

## Candidate directions (with known risks)

### Option A — per-request `subsecond::call` (Dioxus's own pattern) — preferred
Wrap the hot path in `subsecond::call(...)` so every request (or every
router-rebuild) resolves handlers through Subsecond's jump table instead of a
boot-time captured closure. Dioxus fullstack does this for server functions —
it patches code living in ANY workspace crate because dx's patch links the
changed crate's object code and `subsecond::call` re-resolves at call time.
- Open questions: where to put the boundary (per request? per router build? both —
  a `subsecond::call` around `A::build` + router rebuild on patch-notify)?
  Cost of `HotFn` resolution per request in dev; interaction with axum's
  `Router` being built once; whether dx actually rebuilds workspace-lib crates
  into the patch dylib today (VERIFY FIRST — this is the crux; the 2026-07-15
  experiments suggest tip-crate-only REBUILD may also be the limitation, not just
  dispatch. If dx does not even recompile the lib into the patch, Option A alone
  is insufficient).
- First experiment: minimal workspace, app code in a lib, bin does
  `subsecond::call(|| lib::handler())` in a loop — does an edit to the lib
  function change the output under `dx serve --hot-patch`?

### Option B — dev-time "app in the bin" assembly
Make the tip crate *compile* the app source (not import it): a dev bin target
(e.g. `src/bin/dev.rs` or `r2e dev`-generated) doing
`#[path = "../lib.rs"] mod app_src;` / `include!`-style inclusion, so the same
source files are tip-crate code in dev and lib code for tests/prod.
- Satisfies C1 textually (one source), but two compilations of the same code
  (symbol duplication, `#[module]`/macro side effects, `inventory`-style
  registrations run twice if both targets are linked — they are not, bin and
  lib are separate artifacts, so likely fine).
- Risks: path-attribute inclusion is brittle with `mod` trees; proc-macros that
  assume crate-root paths (`crate::…`) keep working (same source, different
  crate name for `crate` — should be fine); IDE/rust-analyzer sees the code
  twice; dx watches the right files (it watches the package dir — OK).
- First experiment: example-app with a generated `src/bin/dev.rs` that
  `#[path]`-mounts `lib.rs` and calls `launch!(app_src::ExampleApp)` — does a
  controller edit hot-patch?

### Option C — wait for upstream
dx "workspace hot-patching" is stated as planned. Zero work, keeps the
documented limitation. Compatible with shipping A or B later.

## Suggested study order

1. Run Option A's first experiment (an afternoon, throwaway workspace) — it
   decides everything: if dx recompiles workspace libs into patches and
   `subsecond::call` re-resolves them, A is the clean fix and B is unnecessary.
2. If A fails at the rebuild level, run B's experiment (the `#[path]` dev-bin) —
   it side-steps rebuild scope entirely by making the code tip-crate.
3. Whichever passes, wire it behind `r2e dev` + `launch!` so C1/C5 hold, then
   re-run the full smoke protocol from PR #32 (handler edit / Env survival /
   config reload) as the acceptance gate.

## References

- PR #32 (canonical App + launch!, dispatch fix, limitation docs)
- `docs/features/09-dev-mode.md` — current documented limitation
- Smoke/debug evidence: 2026-07-15 session (bin-only positive control,
  four-configuration lib-layout matrix, marker-file build() re-run proof)
- Subsecond docs: tip-crate scope; Dioxus fullstack server-fn hot-patching

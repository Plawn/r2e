# Methodology — Subagent implementation + orchestrator audit

> Established 2026-06-11 while implementing Wave 1 of the perf/TPC plan (tasks 544, 534).
> Refined through sessions 3–4 (536, 537, 538) — see "Refinements" at the bottom.
> Read this before resuming the task waves in `HANDOFF-perf-tpc.md`.

## The pattern

One task at a time:

1. **Orchestrator** (the main session, strong model) fetches the Tasker task, sets it
   `in_progress`, and spawns ONE subagent (`model: sonnet`) with a **self-contained prompt**:
   - objective + acceptance criteria (copied from the task, refined),
   - codebase anchor points (file:line), labelled as *hints to verify, not trust*,
   - repo conventions restated (tests in `tests/`, axum only in r2e-http, English, no commits),
   - **cross-task design decisions imposed up front** (e.g. "rt::spawn returns an opaque
     `JobHandle<T>` living in r2e_core::rt" — the 534/535 co-design). Never leave a
     cross-cutting decision to the subagent.
   - a report format: raw data for an orchestrator (files changed, decisions, exact
     verification commands + results, exceptions/deferred items).
2. The subagent explores, reads docs, implements, runs tests — all in **its own context**.
   The orchestrator absorbs only the final report.
3. **Orchestrator audits** (see below), fixes or sends fixes back, re-verifies, then sets
   the task `in_review`; `closed` once committed.

Sequential when tasks touch the same files (534 and 544 both touched `builder.rs`);
`isolation: worktree` only if parallelism is genuinely needed.

## The audit is the load-bearing part

Observed error profile of the implementing model (Sonnet): **reliable on mechanical work,
fallible on implicit invariants**. On Wave 1 it produced structurally good code (clean facade
design, relevant tests, documented exceptions) but left 3 real defects that only the audit
caught:

1. `bind_tcp` resolved addresses via std `to_socket_addrs().next()` — silently dropping
   tokio's multi-address fallback and doing blocking DNS on the async thread. Violated the
   "delegate 1:1, no behavior change" criterion. *Subtle, behavioral, the dangerous kind.*
2. `pub use r2e_core;` re-export in r2e-events instead of direct backend deps — design debt.
3. An orphaned comment left after a refactor — cosmetic, found only on the second full pass.

Audit rules derived from this:

- **Read the FULL diff line by line** for anything touching runtime/behavioral code.
  The agent's report + green tests are NOT sufficient — defects 1 and 2 had green tests.
- A passing `cargo check --workspace` *is* real evidence for API-surface sufficiency of
  wrappers (anything calling a non-exposed method wouldn't compile) — use it as such, but
  it proves nothing about behavioral equivalence.
- **Re-run verification yourself** (tests, `cargo check`), never take the report's word.
- Grep for leftovers the agent's narrative wouldn't mention (`tokio::spawn` residues,
  orphaned comments, stale re-exports).
- Lightweight audit (report + tests + spot-checks) is acceptable ONLY for purely mechanical
  tasks (doc updates, sed-grade renames).
- For the delicate pieces (536 SO_REUSEPORT sharding): orchestrator implements directly,
  OR Sonnet + an additional adversarial review pass on top of the orchestrator's.

## Economics observed (Wave 1)

Two full implementations ≈ 94k + 115k tokens spent in *subagent* contexts; the orchestrator
absorbed only reports + diffs (~a few hundred lines) and stayed fresh for design decisions
and audits across the whole session.

## Tasker hygiene

`in_progress` at spawn → `in_review` after audit+fixes → `closed` after commit.
Note: the Tasker MCP can be slow; a "stuck" call may have landed server-side — on version
conflict, re-fetch before retrying.

## Refinements (sessions 3–4, tasks 536/537/538)

- **Implementing model is Opus** (user's choice since 536). The Wave-1 error profile
  (mechanical work reliable, implicit invariants fallible) still holds in attenuated
  form — the audit is NOT optional with a stronger implementer.
- **Adversarial review pass is mandatory on delicate pieces, regardless of the
  implementing model.** A second Opus agent prompted to REFUTE (find defects, wrong
  claims, things that invalidate the work — not to summarize). It caught 2 real defects
  on 536 and 2 on 537 that the orchestrator's own line-by-line audit had missed, and 3
  retained findings on 538. Findings come back with confidence levels; the orchestrator
  triages — some get rejected as overstated (one on 537).
- **The orchestrator re-runs measurements independently.** New rule from 538: the
  subagent's benchmark run happened under disk-full IO pressure and produced plausible,
  internally-consistent, WRONG numbers (default 1.7–3.7× ahead everywhere) with a
  confident causal narrative fitted to them. Three clean orchestrator re-runs showed a
  much smaller gap and a ranking flip on one run. Green tests + committed tables are not
  evidence for *data*; for any task that produces numbers, the orchestrator re-runs them
  (several times, to characterize variance) before letting a narrative rest on a single
  sample. Prefer reporting variance honestly over a clean story.
- **Self-contained prompt also imposes the report format** ("raw data for an
  orchestrator": files changed, decisions taken where latitude was given, exact
  verification commands + results, exceptions/deferrals/surprises). This has worked
  well — keep it.

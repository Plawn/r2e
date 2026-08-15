# Methodology: the adversarial fix loop

How the W14 tenancy audit fixes were driven to a MERGE verdict (2026-08-14): 9 fix
rounds by a persistent implementation agent, 10 adversarial verification passes by an
independent model, converging on a hard concurrent data structure
(`r2e-tenant/src/map.rs`). This document records the method so it can be replayed on
the next problem of the same shape.

## When to use this

Use the full loop when **all** of these hold:

- The code under repair is *adversarial to review*: lock-free/concurrent logic,
  lifecycle/ownership protocols, anything where a fix can silently introduce a
  neighboring bug of the same class.
- There is a concrete finding list to start from (an audit, a bug report with
  interleavings) — the loop closes findings, it does not discover scope.
- Correctness matters more than wall-clock time. Each round is ~10–15 min of agent
  work plus a verification pass; W14 took 10 passes.

For ordinary fixes (typos, API changes, single-threaded logic), a single fix + one
review pass is enough; the machinery below is overhead.

## The three roles

**Driver** (the main session). Owns the loop: triages each verdict, decides what the
*class* of the defect is (not just the instance), writes the next scoped
instructions, and never lets either agent talk to the other directly — everything is
relayed through the driver, which is where diagnosis quality is enforced. The driver
also owns the safety rails: nothing is committed, the verification gate is fixed, and
the user only sees consolidated outcomes.

**Fixer** — one *persistent* implementation agent (Opus subagent, resumed via
SendMessage every round, never respawned). Persistence is load-bearing: by round 3
the agent holds the whole file's invariant structure in context and starts finding
adjacent bugs on its own (the invalidate reordering, the two-producer amendment).
A fresh agent per round would re-derive the model each time and miss those.

**Verifier** — an independent model with no stake in the fix (codex CLI, read-only).
Independence is the point: the verifier never saw the fix rationale, so it re-derives
interleavings from the code alone. It is invoked per pass with a written prompt:

```bash
codex exec --sandbox workspace-write --skip-git-repo-check "$(cat <prompt-file>)"
# background, explicit 600s timeout (default timeout kills long passes)
```

## The loop

```
findings ──▶ FIX ROUND (fixer) ──▶ gate ──▶ VERIFY PASS (verifier)
                 ▲                                   │
                 │            verdict per item + NEW issues + merge verdict
                 └──── driver triage: class diagnosis, next scoped round ◀──┘
                       (exit when verdict = MERGE with only accepted caveats)
```

### Fix round — instructions to the fixer

Each round message contains, in order:

1. **The verifier's finding verbatim**, including its concrete interleaving. Never
   paraphrase the interleaving — the fixer must be able to re-derive it.
2. **The driver's class diagnosis**: what family of bug this instance belongs to, and
   the structural shape of the fix ("commit the gate inside the same critical section
   that unmaps", "track in-flight work with an RAII counter"). Mark it explicitly as
   arguable: *"my read, agree or argue"*. The fixer pushing back is a feature — in
   round 8 the fixer proved the driver's proposed counter covered only one of two
   producers and widened the fix.
3. **A numbered fix spec** ending with hard invariants that are *grep-checkable*
   ("after this, `commit_dispose` must have ZERO call sites outside a critical
   section").
4. **Docs to update in the same round**: module rustdoc invariants, `llm.txt`, the
   feature doc. Docs are part of the fix, not a follow-up — the verifier checks them
   for overclaim every pass, which is what keeps them honest.
5. **Test requirements**: new tests for the specific interleaving, plus the mutation
   check (below).
6. **The fixed verification gate** (below) and the standing constraints (no commits,
   no `CARGO_TARGET_DIR`).

### The verification gate (identical every round)

```bash
cargo test -p r2e-tenant            # ×3 — concurrency tests must survive rerun
cargo test -p r2e-data-sqlx --features tenant,sqlite
cargo test -p r2e-data-diesel --features tenant,sqlite
cargo check --workspace
cargo clippy -p <crate> --all-targets
```

Running the suite three times is not paranoia: single-run green is meaningless for
scheduler-dependent tests.

### Mutation checks — the test-of-the-test

Every new test must be shown to **fail with only its own fix reverted** (revert,
run, observe the failure, restore). A test that stays green under the mutation is
reported as such, not silently kept. This caught several vacuous tests early
(waiters "passing" because they never ran — fixed with a `settle()` helper +
`!waiter.is_finished()` asserts).

Some windows are genuinely untestable: await-free gaps with no schedulable point
(a CAS relocated to just after its lock; an increment hoisted above a check). The
protocol for those:

1. The fixer declares the gap **honestly** in its report — never claims coverage.
2. The code carries a `// MUST stay inside …` comment at the exact site, naming the
   failure mode.
3. The verifier is explicitly asked to *assess whether the structural argument
   suffices*. Only verifier-accepted gaps become "accepted caveats"; they are listed
   in the final verdict.

### Verify pass — the prompt

One prompt file per pass (kept in the scratchpad, numbered). Structure that proved
to matter:

- **State what the previous pass left open, with the interleaving.** The verifier is
  stateless across passes; the prompt is its memory.
- **Scope hard**: "verify ONLY this and its blast radius — everything you closed
  stays closed unless this round broke it." Without this, passes re-litigate closed
  items and drown the signal.
- **Present the fix as claims to attack**, numbered, with file:line anchors — not as
  a description to admire. Include the author's own reasoning so the verifier can
  attack the *argument*, not just the code ("scrutinize this memory-ordering
  argument: which Ordering is used?").
- **Demand concrete interleavings** for any new finding: "flag only REAL issues with
  a concrete interleaving, not theoretical style points." This is the single most
  important line — it is what kept 10 passes productive instead of devolving into
  style review.
- **Fixed output contract**: verdict per item (`CLOSED` / `CLOSED-WITH-CAVEAT` /
  `NOT CLOSED`, one paragraph, file:line evidence), NEW issues with severity +
  interleaving, then a **one-line FINAL merge verdict for the whole tree**. The
  merge verdict is the loop's exit condition, so it must be demanded explicitly.
- **Read-only**, but allowed to run the test suite.
- End every prompt with a **light final sweep** of the file for "any remaining
  instance of the same class" — this is how each pass found the next, strictly
  narrower instance.

### Driver triage — the part that isn't mechanical

After each verdict, before writing the next round:

- **Name the class, not the instance.** W14's ten passes were all one class ("a
  decision about a slot's fate taken outside the critical section that enforces
  it"). Recognizing that early is what turned point-fixes into structural fixes
  (single CAS site → RAII debt → admission protocol), and structural fixes are what
  made the sequence converge instead of whack-a-mole.
- **Expect the fix to open the next hole, and treat that as progress.** Round 7's
  latch/bump reorder created round 8's finding; round 8's counter created round 9's
  starvation. Each was strictly narrower. A loop that oscillates (same-width
  findings) means the class diagnosis is wrong — stop and rethink.
- **Verify the verifier.** A finding is checked against the code before being
  forwarded (the original audit was itself audited first). A wrong finding sent to
  the fixer wastes a round and erodes trust in the loop.

## Exit criterion

The loop ends when the verifier returns **MERGE** (or MERGE with only
previously-accepted caveats). Not when the fixer says it's done, not when tests are
green — those are necessary per-round conditions, not the exit.

On exit, the driver delivers one consolidated report: original findings → rounds →
final invariant model → test-count progression → the accepted caveats and behaviour
changes, stated plainly. Commits/PR remain the user's decision.

## Anti-patterns observed and avoided

- **Fresh fixer per round** — loses the accumulated invariant model; the fixer's
  best contributions came from context built in earlier rounds.
- **Letting the fixer self-verify** — the fixer honestly believed round 2's
  "atomic insert" was atomic; only the independent pass caught the consumed guard.
- **Unscoped verification** — early prompts without "verify ONLY this" produced
  re-audits of closed items.
- **Accepting "no await between the atomics" as a serialization argument** — the
  verifier refuted it (separate runtime workers); cross-thread claims need a real
  memory-ordering argument (the SeqCst/Dekker derivation), and the prompt should ask
  for one explicitly.
- **Adding testability instrumentation to the hot path** — rejected in round 9
  (a peak counter to catch an unforceable mutation); an untestable-but-commented
  invariant beats a slower hot path.
- **Doc drift** — every round updates docs *and* every pass checks them for
  overclaim; twice a pass flagged a doc claim invalidated by the current fix, which
  forced the next round to make the claim true again rather than soften the doc.

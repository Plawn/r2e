# `Rejection` layout — research record (#1086)

Decision record for the size of `r2e_core::error::Rejection`, the typed hub
every framework failure converts into (`r2e-core/src/error/rejection.rs`).
Shipped 2026-10-12 as the **hot-inline + cold-boxed** layout (56 B). Keep this
file current when the layout changes; the numbers below are the baseline to
beat.

## Problem

Introduced by #1072 (0.5.0) as a plain struct, `Rejection` was **304 B**:
`kind` 1 + `status` 2 + `message: Cow<'static, str>` 24 + `details:
Option<serde_json::Value>` 32 + `headers: HeaderMap` 96 + `source:
Option<Arc<dyn Error>>` 16 + private `opaque: Option<Response>` 128 (a
`Response` embeds its own `HeaderMap`). Before #1072, `Guard::check` returned
`Result<(), Response>` — 128 B, exactly clippy's threshold (the lint fires
strictly above it) — so nobody noticed. Consequences:

- `Result<(), Rejection>` on every guard/extractor/validation path tripped
  `clippy::result_large_err`; `enum E { Rejected(Rejection), … }` — the
  documented envelope pattern — tripped `large_enum_variant` in **user**
  crates. Six `#[allow]`s in the workspace, plus whatever apps added.
- `__maybe_validate` had already been boxed (`Result<(), Box<Rejection>>`,
  commit 67d28d89) to dodge the lint.
- The `Option<Rejection>` finalize slot in generated handlers with managed
  resources inflated the handler future by the hub's size.

Other frameworks: `std::io::Error`, `anyhow::Error`, `hyper::Error`,
`reqwest::Error`, `tonic::Status` are all **8 B** (boxed; tonic boxed in a
recent major after users hit `result_large_err`); `actix_web::Error` 40 B
(`Box<dyn ResponseError>`); axum has no hub (`BoxError`, 16 B). JVM-style
frameworks (Quarkus) hold everything by reference.

## Candidates measured

All measured on the same harness (`release`, 5 rounds, median; synthetic
chain of 5 `#[inline(never)]` async guards returning `Ok`, plus an
`Option<E>` slot held across awaits that mirrors the managed-resource finalize
slot). Sizes on 64-bit.

| | fat (0.6) | A: all boxed | B: piste 2 | C: slim | C3: slim + boxed details | S/O: static/owned |
|---|---|---|---|---|---|---|
| layout | plain struct | `Rejection(Box<Inner>)` | kind/status/message inline, `Option<Box<Extras{details,headers,source,opaque}>>` | fields kept; `headers: Option<Box<HeaderMap>>`, `opaque: Option<Box<Response>>` | C + `details: Option<Box<Value>>` | `enum { Static(&'static Inner), Owned(Box<Inner>), Opaque(Box<..>) }` + `Deref` |
| size (B) | 304 | 8 | 40 | 96 | 72 | 16 |
| `Result<(), E>` (B) | 304 | 8 | 40 | 96 | 72 | 16 |
| future, 5 guards + slot (B) | 328 | 32 | 64 | 120 | 96 | 40 |
| future, 3 guards, no slot (B) | 20 | 20 | 20 | 20 | 20 | 20 |
| happy, 5 guards + slot (ns) | 27.5 | 8.4 | 5.8 | 6.9 | 7.4 | 5.3 |
| happy, 3 guards, no slot (ns) | 7.1 | 2.3 | 3.1 | 3.4 | 3.2 | 3.1 |
| err: bare 401 (ns) | 3.2 | 30.9 | 0.5 | 1.5 | 1.2 | 0.4 |
| err: 404, runtime `format!` message (ns) | 71.5 | 110.5 | 69.1 | 70.4 | 70.4 | 85.6 |
| err: 401 + `WWW-Authenticate` (ns) | 61.9 | 99.4 | 92.1 | 84.2 | 85.9 | 93.4 |
| err: 400 + `source` (ns) | 123.2 | 161.0 | 155.9 | 114.4 | 113.1 | 157.3 |
| deny at guard 1 → `Response` (ns) | 267.9 | 290.0 | 238.0 | 246.6 | 239.8 | 257.9 |
| size depends on `serde_json/preserve_order` | yes | no | no | **yes: 136 B** | no | no |

Reading the table:

- **The hot path barely cares.** Without a retained slot (every route without
  a managed resource: guard failures are projected immediately, never stored)
  the future is 20 B for every layout and the fat hub costs only the 304 B
  `Result` write at return (~4 ns). With the slot, ~20 ns. Between B, C, C3 and
  S/O the differences are noise. Don't justify a layout change with a
  happy-path speedup.
- **Boxing everything (A) is the one bad option**: +28 ns on the bare 401 that
  guards emit under auth-scanner traffic, +40 ns everywhere else.
- **`serde_json/preserve_order`** turns `Option<Value>` from 32 to 72 B. Any
  downstream dependency can enable it by feature unification, so a layout that
  keeps `Option<Value>` inline (C) silently crosses 128 B in some
  applications — the acceptance criterion would fail in the field. The cold
  members must be behind a pointer.
- **`source` is not cold**: nearly every `From` impl in the framework sets it
  (extractor rejections, `SecurityError`, `TenantError`), so a layout that
  boxes it with the cold members (B) adds an allocation to the most common
  framework error path (+33 ns). Keep it inline.
- **Static constants (S/O)** buy sub-nanosecond `unauthenticated()` and a 16 B
  hub but cost a `Box` on every runtime-formatted message and every
  source-carrying rejection, plus `Deref` + copy-on-write machinery and a
  `Response`-is-not-`Sync` wrinkle (the opaque response needs its own
  variant). Not worth it today — see *Future work*.

Two more variants were considered and rejected without benchmarking:
`Rejection(Box<RejectionParts>)` + `Deref` (A with public inner — same
allocation profile as A) and merging only `headers`+`opaque` into one private
box (88 B, still tied to `preserve_order`).

## Decision: B' — hot inline, cold boxed, `source` inline

```rust,ignore
pub struct Rejection {
    pub kind: RejectionKind,              // 1
    pub status: StatusCode,               // 2
    pub message: Cow<'static, str>,       // 24
    pub source: Option<Arc<dyn Error>>,   // 16
    extras: Option<Box<Extras>>,          // 8   → 56 B
}
struct Extras { details: Option<Value>, headers: HeaderMap, opaque: Option<Response> }  // 256 B, allocated on first use
```

- Allocation profile equals C3's: zero on bare statuses, runtime messages and
  source-carrying rejections; one 256 B block for header-bearing, validation
  and opaque rejections (SecurityError sets both `source` and
  `WWW-Authenticate`, so it allocates once).
- 56 B regardless of serde_json features; 72 B of margin under clippy's 128.
  Pinned by `rejection_is_small` (`r2e-core/tests/http/rejection.rs`), which
  also compiles the documented envelope enum and a `Result<(), Rejection>`
  guard under clippy with no allow.
- No `Deref`, no statics, no copy-on-write: an ordinary struct.
- API: `kind`/`status`/`message`/`source` stay public fields (by-value moves
  keep compiling); `details()`/`take_details()`, `headers()`/`headers_mut()`/
  `take_headers()` are the readers; the builder `.details(v)` became
  `.with_details(v)`. `headers()` on a rejection without extras returns a
  shared empty `HeaderMap` (`static NO_HEADERS: LazyLock<HeaderMap>`; an
  empty map allocates nothing).
- **Rule: a new member goes in `Extras`**, never inline.
- `__maybe_validate` is unboxed again; `__managed_finalize_error:
  Option<Rejection>` is 56 B instead of 304.
- One allow survives: `examples/example-app/src/controllers/problem_controller.rs`
  — the RFC 9457 `Problem` envelope is 136 B on its own (several `String`s +
  `Option<Value>`), unrelated to `Rejection`.

Benchmark caveats (from the external review): guards never actually suspend
in the harness, sub-ns numbers lack confidence intervals and allocation counts,
and axum boxes handler futures anyway (a smaller future shrinks the heap
block, it does not remove the allocation). The decision rests on size,
lint removal and the allocation profile, not on the ns columns.

## Future work (not scheduled)

- **S/O with static constants** if a profile ever shows `unauthenticated()`
  / `forbidden()` construction on a hot path: 16 B, 0.4 ns bare 401. Costs:
  one `Box` per runtime message / source-carrying rejection (+15–35 ns on
  cold paths), `Deref<Target = Inner>` + copy-on-write builders, a third
  variant for the opaque response (`Response: !Sync`), by-value field moves
  become `into_inner()`. The harness in this record is the baseline.
- **`Cow<'static, str>` → a 16 B message** (e.g. `Box<str>`/`&'static str`
  enum) would reach 48 B; changes the public `message` type for 8 bytes.
  Not worth a break.
- Validate on real generated routes (0 and N managed resources, pending
  guards, full construct → project → drop incl. `SecurityError`,
  `.source_arc()`, validation, opaque) rather than the synthetic chain, if a
  future change claims a hot-path gain.
- Codex (gpt-6-astra, high) reviewed the candidates on 2026-10-12 and
  concurred with the C-family choice and the `preserve_order` gating; its
  full answer is in the session scratchpad, the substance is above.

# Migration: `Rejection` layout (r2e-core 0.6 → 0.7)

`Rejection`, the typed hub every framework failure converts into, went from
304 bytes to 56 (task #1086). The size mattered because the hub sits in
`Result<T, Rejection>` everywhere — guards, extractors, `__maybe_validate` — and
inside every error envelope `enum E { Rejected(Rejection), … }`, where clippy's
`result_large_err` and `large_enum_variant` (threshold: 128 bytes) fired on
user code that followed the documented pattern. The framework carried six
`#[allow]`s for it; applications had to add their own.

The hot members stay inline **and public**: `kind`, `status`, `message`,
`source`. The cold members — `details`, `headers` and the pre-rendered response
of an `Opaque` rejection — moved into one lazily allocated block, read through
accessors. A bare `401`/`403`/`404`, a runtime-formatted message and a
source-carrying extractor rejection allocate nothing new; a rejection that sets
a header or details pays one allocation, on the error path only. Design record
and measurements: `docs/claude/rejection-layout.md`.

## What breaks

| Before (0.6) | After (0.7) |
|---|---|
| `r.details` (field, `Option<serde_json::Value>`) | `r.details()` → `Option<&Value>`; `r.take_details()` → `Option<Value>` to move it out |
| `r.headers` (field, `HeaderMap`) | `r.headers()` → `&HeaderMap`; `r.headers_mut()`; `r.take_headers()` → `HeaderMap` |
| `.details(value)` builder | `.with_details(value)` (`details()` is now the reader) |
| `let Rejection { details, .. } = r;` | `let details = r.take_details();` first, then destructure the rest |
| `__maybe_validate() -> Result<(), Box<Rejection>>` (generated code only) | `Result<(), Rejection>` |

Everything else compiles unchanged: `r.kind`, `r.status`, `&r.message`,
`r.message.into_owned()`, `r.source`, every constructor, `.header(..)`,
`.source(..)`, `.source_arc(..)`, `take_opaque()`, `is_opaque()`,
`project::<E>()`.

Each break is a compile error that names the moved item (`attempted to take
value of method 'headers'`, `this method takes 0 arguments but 1 was
supplied` for `.details(value)`).

## Envelope `From<Rejection>` impls

Accessors borrow the whole value, so take the cold members **before** moving a
public field out:

```rust,ignore
impl From<Rejection> for Problem {
    fn from(mut r: Rejection) -> Self {
        let errors = r.take_details();                                   // 1. cold members
        let mut p = Problem::new(r.status, r.message.into_owned());     // 2. move the hot ones
        p.code = Some(format!("{:?}", r.kind));
        p.errors = errors;
        p
    }
}
```

The same order applies to a wire that carries headers itself:

```rust,ignore
let headers = rejection.take_headers();
let message = rejection.message.into_owned();
```

## Remove your `#[allow]`s

Delete every `#[allow(clippy::result_large_err)]` and
`#[allow(clippy::large_enum_variant)]` whose justification was `Rejection`:
they now hide real findings. An envelope that is still large on its own (an
RFC 9457 body with several `String`s, for instance) keeps its allow with a
justification naming *that* type.

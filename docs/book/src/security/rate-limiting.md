# Rate Limiting

R2E provides token-bucket rate limiting with three key strategies: global, per-IP, and per-user.

## Setup

Enable the rate-limit feature:

```toml
r2e = { version = "0.3", features = ["rate-limit"] }
```

Provide a `RateLimitRegistry` as a bean. There is no hand-written state struct —
the registry becomes part of the inferred state and rate-limit guards resolve it
from the bean graph by type:

```rust
use r2e::r2e_rate_limit::RateLimitRegistry;

AppBuilder::new()
    .provide(RateLimitRegistry::default())
    // ... other beans ...
    .build_state()
    .await
    // ... plugins, controllers, serve ...
    ;
```

## Rate limit strategies

### Global rate limit (pre-auth)

Shared bucket across all requests to that handler. Runs before JWT validation:

Pre-auth strategies (`global`, `per_ip`) live on the `PreRateLimit` type;
the post-auth per-user strategy lives on `RateLimit`.

```rust
use r2e::r2e_rate_limit::{PreRateLimit, RateLimit};

#[get("/")]
#[pre_guard(PreRateLimit::global(100, 60))]  // 100 requests per 60 seconds total
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

### Per-IP rate limit (pre-auth)

Separate bucket per client IP. Runs before JWT validation:

```rust
#[get("/")]
#[pre_guard(PreRateLimit::per_ip(10, 60))]  // 10 requests per 60 seconds per IP
async fn list(&self) -> Json<Vec<Item>> { /* ... */ }
```

See [Client IP resolution](#client-ip-resolution) for how the IP is determined
and what your proxy must do.

### Per-user rate limit (post-auth)

Separate bucket per authenticated user. Runs after JWT validation:

```rust
#[post("/")]
#[guard(RateLimit::per_user(5, 60))]  // 5 requests per 60 seconds per user
async fn create(&self, body: Json<Request>) -> Json<Response> { /* ... */ }
```

User is identified by the `sub` claim from the JWT token.

**A per-user limit requires an identity.** `RateLimit` / `ConfiguredRateLimit`
declare `DecoratorSpec::REQUIRES_IDENTITY = true`:

- placing one on a route that can never have an identity (no struct-level
  `#[inject(identity)]`, no identity parameter, or an `#[anonymous]` route
  without an `Option<..>` identity parameter) is a **compile error**;
- at runtime, an `Option<..>` identity that came back `None` is answered with
  **401 Unauthorized** (and a warning). Unauthenticated callers never share an
  "anonymous" bucket — that would silently turn a per-user budget into a global
  one.

## Bucket keys

A bucket key is built from **the fully-qualified controller name, the handler
name, and the key strategy**:

```
<crate::module::ControllerName>:<handler>:global
<crate::module::ControllerName>:<handler>:ip:<client-ip>
<crate::module::ControllerName>:<handler>:user:<sub>
```

The controller name is module-qualified (`module_path!()` + type name), so two
same-named controllers in different modules cannot collide. Two controllers
exposing homonymous handlers (`start`, `answer`, `list`, …) likewise get
**independent buckets** — adding a limit to one endpoint never eats the budget
of an unrelated endpoint that happens to share a method name. Each annotated
handler owns its own bucket; there is no way to intentionally share one bucket
across handlers (declare a shared limit as a middleware or a custom guard if you
need that). Bucket keys are an internal detail: renaming or moving a controller
changes them (buckets are per-process and reset on restart anyway).

## Client IP resolution

For `per_ip` limits the client address is resolved in this order:

1. the leftmost entry of the `X-Forwarded-For` header, **when it parses as an IP
   address**;
2. otherwise the **transport peer address** (`ConnectInfo<SocketAddr>`, port stripped);
3. otherwise the literal bucket `unknown` — logged once per process with a
   `tracing::warn!`, because every such request then shares a single bucket and
   "per IP" silently degrades to "global".

### What counts as a parseable entry

The candidate is the leftmost comma-separated entry, trimmed. It is accepted
only if it parses into a `std::net::IpAddr`:

| Header value | Bucket fragment |
|---|---|
| `1.2.3.4` | `1.2.3.4` |
| `1.2.3.4:5678` | `1.2.3.4` (port dropped) |
| `[::1]:8080`, `[::1]`, `0:0:0:0:0:0:0:1` | `::1` (canonical form) |
| `1.2.3.4, 10.0.0.1` | `1.2.3.4` (leftmost only) |
| `unknown`, `not-an-ip`, `'; DROP TABLE …`, empty | *treated as absent* → peer address |

Anything unparseable is treated as **absent**, so a client cannot mint a fresh
bucket per garbage value, and cannot suppress the peer fallback either. Entries
to the right of a malformed one are never used to "repair" it — they come from
the same untrusted hop. The bucket string is always the canonical `IpAddr`
`Display` form, so two spellings of one address share one bucket.

### Deployment: the header must be overwritten, not appended

`X-Forwarded-For` is client-controlled unless something in front rewrites it.

- **Proxy that overwrites** (`proxy_set_header X-Forwarded-For $remote_addr;` in
  nginx, or Traefik/ALB configured to replace): correct — the leftmost entry is
  the real client and cannot be forged.
- **Proxy that appends** (`$proxy_add_x_forwarded_for`, the nginx default
  recipe): a client can prepend its own `X-Forwarded-For: 1.2.3.4` and get a
  fresh bucket per forged value, defeating the limit. Either switch the proxy to
  overwrite, or use `peer_ip_only()` / `trust-forwarded-for: false` — but note
  that behind a proxy the peer address is the proxy, so all traffic then shares
  one bucket.
- **No proxy at all** (the app is exposed directly): use `peer_ip_only()`. The
  peer address is un-forgeable, and trusting the header here would let any
  client mint unlimited buckets.

```rust
// Directly exposed: ignore X-Forwarded-For entirely.
#[pre_guard(PreRateLimit::per_ip(10, 60).peer_ip_only())]
```

The peer address is available under `serve_auto` / the sharded serve path /
HTTP-3, and in `TestServer` (live TCP). It is **not** available under
`TestApp`'s in-process dispatch (no socket) — per-IP limits there fall back to
`unknown` unless the test sets `X-Forwarded-For` explicitly.

Guards written by hand can read the same information from the guard context:

```rust
ctx.forwarded_for()  // Option<&str>    — leftmost XFF entry, RAW and unvalidated
ctx.forwarded_ip()   // Option<IpAddr>  — the same entry, parsed (None if malformed)
ctx.peer_ip()        // Option<IpAddr>  — transport peer, port stripped
ctx.client_ip()      // Option<ClientIp> — parsed XFF first, else peer
```

`ClientIp` is `Forwarded(IpAddr)` / `Peer(IpAddr)`: it tells you *where* the
address came from (so a guard can decide how much to trust it), and its
`Display`/`ip()` give the canonical address. Never key anything on
`forwarded_for()` — use `forwarded_ip()` or `client_ip()`.

## Configurable budgets

`PreRateLimit` / `RateLimit` take literal budgets. To make a limit tunable per
environment, use the config-resolved specs `ConfiguredPreRateLimit` /
`ConfiguredRateLimit`: they read their budget from `R2eConfig` at controller
registration, with the literal arguments as fallback defaults.

```rust
use r2e::r2e_rate_limit::{ConfiguredPreRateLimit, ConfiguredRateLimit};

#[post("/start")]
#[pre_guard(ConfiguredPreRateLimit::per_ip("rate-limit.public").defaults(30, 60))]
async fn start(&self) -> Json<Session> { /* ... */ }

#[post("/heavy")]
#[guard(ConfiguredRateLimit::per_user("rate-limit.api").defaults(5, 60))]
async fn heavy(&self) -> Json<Report> { /* ... */ }
```

```yaml
rate-limit:
  public:
    max: 30                    # default: the spec's `defaults(...)` value
    window-secs: 60            # default: the spec's `defaults(...)` value
    enabled: true              # false → the guard always allows (handy in `application-test.yaml`)
    trust-forwarded-for: true  # false → peer address only, like `peer_ip_only()`
  api:
    max: 5
    window-secs: 60
```

Keys are read under the prefix passed to the constructor, so several sites can
share one prefix or each own its own. Any key may be omitted; the spec's
defaults apply.

### Malformed configuration fails startup

The default applies **only when the key is absent**. A key that is present but
not convertible to its type (`max: plenty`, `enabled: yes-please`) aborts
startup with a message naming the key — it never silently reinstates the
default budget, because a security limit that quietly reverts is worse than one
that refuses to boot. Specs are built once, at controller registration, so the
failure happens at boot, not on the first request.

A `window-secs` of `0` is rejected the same way, in every path: the literal
constructors (`PreRateLimit::per_ip(10, 0)`, `RateLimit::per_user(5, 0)`,
`.defaults(5, 0)`) panic where they are written, and `<prefix>.window-secs: 0`
aborts startup. A zero-length window makes the refill rate infinite — the bucket
returns to capacity on every request, which disables the limit entirely.

These are **separate spec types**, not extra constructors on `PreRateLimit`:
a `DecoratorSpec` declares its bean dependencies at the type level, and the
config-resolved variants additionally depend on `R2eConfig`.

## Key classification

| Strategy | Attribute | Runs when | Needs identity |
|----------|-----------|-----------|---------------|
| `PreRateLimit::global(max, window)` | `#[pre_guard(...)]` | Before JWT | No |
| `PreRateLimit::per_ip(max, window)` | `#[pre_guard(...)]` | Before JWT | No |
| `RateLimit::per_user(max, window)` | `#[guard(...)]` | After JWT | Yes (enforced) |
| `ConfiguredPreRateLimit::{global,per_ip}(prefix)` | `#[pre_guard(...)]` | Before JWT | No |
| `ConfiguredRateLimit::per_user(prefix)` | `#[guard(...)]` | After JWT | Yes (enforced) |

"Enforced" = `REQUIRES_IDENTITY = true`: a compile error where no identity can
exist, a 401 where an optional identity is missing.

## Combining rate limits

```rust
#[post("/upload")]
#[pre_guard(PreRateLimit::global(1000, 60))]  // 1000 total uploads/min
#[pre_guard(PreRateLimit::per_ip(50, 60))]    // 50 uploads/min per IP
#[guard(RateLimit::per_user(10, 60))]         // 10 uploads/min per user
async fn upload(&self, body: Bytes) -> Result<(), HttpError> { /* ... */ }
```

## Response on rate limit exceeded

When a rate limit is exceeded, R2E returns:

```
HTTP/1.1 429 Too Many Requests
```

A per-user limit reached without an identity returns `401 Unauthorized`
instead — see [Per-user rate limit](#per-user-rate-limit-post-auth).

## Custom rate limit backend

The default backend is `InMemoryRateLimiter` (DashMap-based).

**It is single-process.** Buckets live in this process's memory, so N replicas
behind a load balancer allow up to N × `max` requests per window, and a restart
resets every bucket. Treat it as protection against a single abusive client, not
as a cluster-wide quota. Budgets are *not* frozen at the first call: when a call
presents a different `max`/`window_secs` (config change, two sites on one key)
the bucket is re-tuned in place and its token count clamped to the new maximum.
A `window_secs` of 0 can never come from this crate's constructors; should a
hand-rolled guard pass one, the bucket simply never refills (fail closed).

For a distributed limit, implement the `RateLimitBackend` trait over a shared
store and provide it as the registry's backend:

```rust
use r2e::r2e_rate_limit::{RateLimitBackend, RateLimitRegistry};

struct RedisRateLimiter { /* ... */ }

impl RateLimitBackend for RedisRateLimiter {
    fn try_acquire(&self, key: &str, max: u64, window_secs: u64) -> bool {
        // Return true if allowed, false if exceeded.
        todo!()
    }
}

AppBuilder::new().provide(RateLimitRegistry::new(RedisRateLimiter { /* ... */ }));
```

`try_acquire` is synchronous and runs inline in the guard — keep it cheap, or
front it with a local cache.

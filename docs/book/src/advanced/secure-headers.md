# Secure Headers

The `SecureHeaders` plugin automatically adds security-related HTTP headers to every response. It ships with sensible defaults and provides a builder API for full customization.

## Quick start

```rust
use r2e::prelude::*;

AppBuilder::new()
    .build_state::<AppState, _, _>()
    .await
    .with(SecureHeaders::default())   // sensible defaults
    .serve("0.0.0.0:3000")
    .await
    .unwrap();
```

## Default headers

When using `SecureHeaders::default()`, the following headers are added to every response:

| Header | Default Value | Purpose |
|--------|---------------|---------|
| `X-Content-Type-Options` | `nosniff` | Prevents MIME-type sniffing |
| `X-Frame-Options` | `DENY` | Blocks page embedding in iframes |
| `Strict-Transport-Security` | `max-age=31536000; includeSubDomains` | Enforces HTTPS for 1 year |
| `X-XSS-Protection` | `0` | Disables legacy XSS filter (modern best practice) |
| `Referrer-Policy` | `strict-origin-when-cross-origin` | Controls referrer information sent with requests |

Two additional headers are supported but **not enabled by default** (they have no default value):

| Header | Builder Method |
|--------|---------------|
| `Content-Security-Policy` | `.content_security_policy(...)` |
| `Permissions-Policy` | `.permissions_policy(...)` |

## Builder API

Use `SecureHeaders::builder()` to customize which headers are sent and their values.

### Builder methods

| Method | Signature | Default | Description |
|--------|-----------|---------|-------------|
| `content_type_options` | `(enabled: bool)` | `true` | Enable/disable `X-Content-Type-Options: nosniff` |
| `frame_options` | `(value: impl Into<String>)` | `"DENY"` | Set `X-Frame-Options` value (`"DENY"`, `"SAMEORIGIN"`) |
| `no_frame_options` | `()` | -- | Remove the `X-Frame-Options` header entirely |
| `hsts` | `(enabled: bool)` | `true` | Enable/disable `Strict-Transport-Security` |
| `hsts_max_age` | `(seconds: u64)` | `31536000` (1 year) | Set the HSTS `max-age` directive in seconds |
| `hsts_include_subdomains` | `(include: bool)` | `true` | Include or exclude `includeSubDomains` in HSTS |
| `xss_protection` | `(enabled: bool)` | `true` | Enable/disable `X-XSS-Protection: 0` |
| `referrer_policy` | `(value: impl Into<String>)` | `"strict-origin-when-cross-origin"` | Set the `Referrer-Policy` value |
| `content_security_policy` | `(value: impl Into<String>)` | *not set* | Set `Content-Security-Policy` |
| `permissions_policy` | `(value: impl Into<String>)` | *not set* | Set `Permissions-Policy` |
| `build` | `()` | -- | Finalize and produce the `SecureHeaders` plugin |

### Header details

**`X-Content-Type-Options`** -- When enabled, sends `nosniff` which tells browsers not to guess the MIME type of a response, preventing MIME-confusion attacks.

**`X-Frame-Options`** -- Controls whether the page can be embedded in `<iframe>`, `<frame>`, or `<object>` elements. Common values are `DENY` (block all framing) and `SAMEORIGIN` (allow same-origin framing only). Use `no_frame_options()` to omit entirely (e.g., when relying on CSP `frame-ancestors` instead).

**`Strict-Transport-Security` (HSTS)** -- Instructs browsers to only connect via HTTPS. The `max-age` directive sets how long (in seconds) the browser remembers this policy. `includeSubDomains` extends the policy to all subdomains. Disable this for development servers or HTTP-only environments with `.hsts(false)`.

**`X-XSS-Protection`** -- Set to `0` by modern best practice. The legacy XSS auditor in older browsers could itself introduce vulnerabilities, so the recommended setting is to disable it and rely on CSP instead.

**`Referrer-Policy`** -- Controls how much referrer information is sent with requests. Common values: `no-referrer`, `origin`, `strict-origin`, `strict-origin-when-cross-origin`, `same-origin`.

**`Content-Security-Policy` (CSP)** -- A powerful header that restricts which resources the browser may load. Not set by default because it is highly application-specific.

**`Permissions-Policy`** -- Controls which browser features (camera, microphone, geolocation, etc.) the page may use. Not set by default because it depends on your application's needs.

## Examples

### Using all defaults

```rust
.with(SecureHeaders::default())
```

### Allowing iframe embedding from the same origin

```rust
.with(SecureHeaders::builder()
    .frame_options("SAMEORIGIN")
    .build())
```

### Adding a Content Security Policy

```rust
.with(SecureHeaders::builder()
    .content_security_policy("default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:")
    .build())
```

### Strict production configuration

```rust
.with(SecureHeaders::builder()
    .hsts_max_age(63072000)                          // 2 years
    .hsts_include_subdomains(true)
    .content_security_policy("default-src 'self'")
    .permissions_policy("camera=(), microphone=(), geolocation=()")
    .frame_options("DENY")
    .build())
```

### Development configuration (disable HSTS)

```rust
.with(SecureHeaders::builder()
    .hsts(false)
    .build())
```

### Disabling specific headers

```rust
.with(SecureHeaders::builder()
    .content_type_options(false)   // remove X-Content-Type-Options
    .no_frame_options()            // remove X-Frame-Options
    .xss_protection(false)         // remove X-XSS-Protection
    .build())
```

### Complete custom example

```rust
use r2e::prelude::*;

#[tokio::main]
async fn main() {
    let secure = SecureHeaders::builder()
        .content_type_options(true)
        .frame_options("SAMEORIGIN")
        .hsts(true)
        .hsts_max_age(63072000)
        .hsts_include_subdomains(true)
        .xss_protection(true)
        .referrer_policy("no-referrer")
        .content_security_policy("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'")
        .permissions_policy("camera=(), microphone=(), geolocation=()")
        .build();

    AppBuilder::new()
        .build_state::<AppState, _, _>()
        .await
        .with(Tracing)
        .with(ErrorHandling)
        .with(secure)
        .with(Health)
        .serve("0.0.0.0:3000")
        .await
        .unwrap();
}
```

## How it works

`SecureHeaders` implements the `Plugin` trait. When installed, it wraps the router with an Axum middleware layer that appends the configured headers to every outgoing response. Headers are collected once at startup and stored in an `Arc`, so the per-request overhead is minimal (just cloning header values into the response).

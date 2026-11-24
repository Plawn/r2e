# QUIC and HTTP/3

With the `quic` feature, R2E serves HTTP/3 over QUIC next to the regular TCP
server. Both endpoints share the same routes, so guards, interceptors,
middleware and error handling behave the same. Raw QUIC streams are available
for custom protocols.

## Setup

```toml
r2e = { version = "0.15", features = ["quic"] }
```

`quic` is **not** part of `full`: it pulls in quinn, rustls and h3.

## Configuration (`server.quic.*`)

```yaml
server:
  host: 0.0.0.0
  port: 3000
  quic:
    port: 4433               # UDP port for QUIC/HTTP3
    cert: certs/server.crt   # PEM certificate chain
    key: certs/server.key    # PEM private key
    alt_svc_max_age: 3600    # Alt-Svc max-age in seconds
```

| Key | Required | Default | Description |
|---|---|---|---|
| `server.quic.port` | yes | — | UDP port of the QUIC endpoint |
| `server.quic.cert` | yes | — | Path to the PEM certificate chain |
| `server.quic.key` | yes | — | Path to the PEM private key |
| `server.quic.alt_svc_max_age` | no | `3600` | `Alt-Svc` max-age (seconds) |

If `port` is set but `cert` or `key` is missing, the error is logged and the
app runs TCP only.

## Automatic mode

With the config in place, `serve_auto()` (or `serve(addr)`) starts both
endpoints. Nothing else changes in your code:

```rust
AppBuilder::new()
    .load_config::<AppConfig>()
    .build_state()
    .await
    .register_controller::<MyController>()
    .serve_auto()
    .await?;
```

Every TCP response then carries an `Alt-Svc` header that tells browsers HTTP/3
is available:

```text
Alt-Svc: h3=":4433"; ma=3600
```

HTTP/3 request bodies are streamed with backpressure. They are limited to
`DEFAULT_MAX_BODY_SIZE` (2 MiB); a larger `Content-Length` is rejected with
413.

## Manual HTTP/3 server

For tests or custom setups, the building blocks live in `r2e::http::quic`:

```rust
use r2e::http::quic;

let config = quic::build_server_config(&cert_pem, &key_pem)?;
quic::serve_h3(router, "0.0.0.0:4433".parse()?, config, shutdown_signal()).await?;
```

| Function | Description |
|---|---|
| `build_server_config(cert_pem, key_pem)` | `quinn::ServerConfig` with ALPN `h3` |
| `build_server_config_with_alpn(cert, key, alpn)` | Custom ALPN protocols |
| `build_server_config_from_files(cert_path, key_path)` | Load PEM files from disk |
| `serve_h3(router, addr, config, shutdown)` | Bind, serve, close |
| `serve_h3_with_endpoint(router, endpoint, shutdown)` | Serve on a pre-bound endpoint (you close it) |
| `apply_alt_svc(router, port, max_age)` | Add the `Alt-Svc` middleware |

`serve_h3_with_endpoint` is handy in tests: bind on port 0 first, read the
address, then serve, with no port race.

## Raw QUIC streams

```rust
use r2e::http::quic::{build_server_config_with_alpn, QuicConnection, QuicEndpoint};

let config = build_server_config_with_alpn(&cert, &key, vec![b"my-proto".to_vec()])?;
let endpoint = QuicEndpoint::bind("0.0.0.0:4433".parse()?, config)?;

while let Some(incoming) = endpoint.accept().await {
    r2e::rt::spawn(async move {
        let conn = QuicConnection::new(incoming.await.unwrap());
        let (mut send, mut recv) = conn.accept_bi().await.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = recv.read(&mut buf).await.unwrap().unwrap();
        send.write_all(&buf[..n]).await.unwrap();
        send.finish().unwrap();
    });
}
```

`QuicConnection` offers `accept_bi`, `open_bi`, `accept_uni` and `open_uni`.
The `quinn` crate is re-exported as `r2e::http::quic::quinn`.

## Limitations

- The QUIC endpoint is not sharded: `server.workers` applies to TCP only.
- Under `r2e dev`, the QUIC endpoint is kept across hot-patches. A certificate
  change needs a full restart.

## See also

- Full guide: [docs/features/18-quic.md](https://github.com/plawn/r2e/blob/master/docs/features/18-quic.md)
- [Sharded Serving](./sharded-serving.md)

# HTTP Proxy

R2E provides native HTTP proxy primitives: **CONNECT tunnelling** via the `#[connect]` attribute, the `TcpTunnel` type for bidirectional TCP streams, and a **forward proxy** layer for intercepting absolute-URI requests. Enable the `proxy` feature flag to use them.

## Setup

Add the `proxy` feature to your `r2e` dependency:

```toml
[dependencies]
r2e = { version = "0.1", features = ["proxy"] }
```

Import the prelude — it re-exports `TcpTunnel`, `TcpTunnelRead`, and `TcpTunnelWrite`:

```rust
use r2e::prelude::*;
```

## CONNECT tunnelling

Axum's router does not support `Method::CONNECT`. R2E intercepts CONNECT requests with a Tower layer before they reach the router, upgrades the connection to raw TCP, and hands you a `TcpTunnel`.

### Declaring a CONNECT handler

Annotate a controller method with `#[connect]` and accept a `TcpTunnel` parameter:

```rust
#[derive(Controller)]
#[controller(path = "/", state = AppState)]
pub struct ProxyController;

#[routes]
impl ProxyController {
    #[connect]
    async fn handle_tunnel(&self, tunnel: TcpTunnel) {
        let target = tunnel.target(); // "host:port"
        if let Ok(mut upstream) = tokio::net::TcpStream::connect(&target).await {
            let _ = tunnel.pipe(&mut upstream).await;
        }
    }
}
```

The framework:
1. Extracts the authority (`host:port`) from the CONNECT request URI
2. Responds with `200 OK` to the client
3. Upgrades the connection
4. Wraps the upgraded stream in `TcpTunnel` and calls your method

### TcpTunnel API

| Method | Description |
|--------|-------------|
| `host()` | Target hostname (`&str`) |
| `port()` | Target port (`u16`) |
| `target()` | `"host:port"` string |
| `pipe(stream)` | Bidirectional copy to another `AsyncRead + AsyncWrite`. Returns `(client→upstream, upstream→client)` byte counts |
| `split()` | Split into `TcpTunnelRead` + `TcpTunnelWrite` halves |
| `into_inner()` | Unwrap the raw `TokioIo<Upgraded>` |

`TcpTunnel` implements `AsyncRead` and `AsyncWrite`, so you can use it directly with `tokio::io::copy_bidirectional` or any other IO combinator.

### Accepting additional extractors

CONNECT handlers do not support extra extractors — the connection metadata is carried by `TcpTunnel` itself (host, port).

## Forward proxy

Forward proxy requests use absolute URIs (`GET http://example.com/path HTTP/1.1`). Axum routes by relative path, so these fall through. `ForwardProxyLayer` intercepts them before the router.

### Adding a forward proxy layer

Use `with_layer_fn` on the builder:

```rust
use r2e::proxy::ForwardProxyLayer;

let app = AppBuilder::new()
    .with_state(state)
    .with_layer_fn(|router| {
        let handler = std::sync::Arc::new(|req: r2e::http::Request| {
            Box::pin(async move {
                // Forward the request to the target, return the response
                forward_request(req).await
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>>
        });
        router.layer(ForwardProxyLayer::new(handler))
    });
```

The layer checks whether the request has a scheme in its URI (`http://...`) and is not a CONNECT. If so, it dispatches to your handler instead of the Axum router.

## Combining CONNECT and forward proxy

A full HTTP proxy typically handles both CONNECT (for HTTPS tunnelling) and forward proxy (for plain HTTP). Use both together:

```rust
#[derive(Controller)]
#[controller(path = "/", state = AppState)]
pub struct FullProxyController;

#[routes]
impl FullProxyController {
    #[connect]
    async fn tunnel(&self, tunnel: TcpTunnel) {
        if let Ok(mut upstream) = tokio::net::TcpStream::connect(&tunnel.target()).await {
            let _ = tunnel.pipe(&mut upstream).await;
        }
    }
}

// In your app setup:
let app = AppBuilder::new()
    .with_state(state)
    .register_controller::<FullProxyController>()
    .with_layer_fn(|router| {
        router.layer(ForwardProxyLayer::new(forward_handler))
    });
```

## Summary

| Type | Purpose |
|------|---------|
| `#[connect]` | Declare a CONNECT tunnel handler on a controller method |
| `TcpTunnel` | Bidirectional TCP stream after CONNECT upgrade |
| `TcpTunnelRead` / `TcpTunnelWrite` | Split halves of a tunnel |
| `ForwardProxyLayer` | Tower layer intercepting absolute-URI requests |
| `ConnectLayer` | Tower layer intercepting CONNECT (installed automatically by the framework) |

## Next steps

- [WebSocket](./websocket.md) -- similar upgrade-based real-time pattern
- [Custom Plugins](../advanced/custom-plugins.md) -- wrap proxy logic in a reusable plugin

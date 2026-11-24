# example-grpc

A Tonic gRPC service next to an HTTP API, with guards, interceptors,
reflection and grpc-web.

What it shows:

- `.proto` files under `proto/` (`greeter.proto`: `SayHello`,
  `SayHelloAdmin`; `ticker.proto`: server-streaming `Watch`) compiled by
  `build.rs` with `r2e_grpc_build::compile()` and included with
  `include_protos!()`
- `GreeterService` implemented with `#[grpc_routes]`, with a `LogCalls`
  interceptor built as a `#[derive(DecoratorBean)]`
- Server reflection (`grpc-reflection` feature) and grpc-web (`grpc-web`
  feature)
- An HTTP health route on the same app

Building needs `protoc` on the `PATH` (e.g. `apt install protobuf-compiler` or
`brew install protobuf`).

## Running

```bash
cargo run -p example-grpc
```

HTTP on `http://localhost:3000` (`GET /api/health`), gRPC on `localhost:50051`:

```bash
grpcurl -plaintext localhost:50051 list
grpcurl -plaintext -d '{"name":"r2e"}' localhost:50051 greeter.Greeter/SayHello
```

Check the exact service names with `list`; they come from the `package` in
the `.proto` files.

## Testing

```bash
cargo test -p example-grpc
```

`tests/` covers guards, interceptors, modules, reflection, serving, the
in-process test client (`app.grpc()`) and grpc-web.

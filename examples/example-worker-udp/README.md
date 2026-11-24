# example-worker-udp

Per-worker services under sharded serving: a UDP echo server running on every
worker, next to an HTTP API that inspects and controls the workers.

It requires `server.workers: per-core` (set in `application.yaml`).

What it shows:

- `worker_local` / `WorkerLocal`: state owned by each worker
- `per_worker_service`: a service started once per worker
- `reuseport_udp` + `adopt_udp`: one `SO_REUSEPORT` UDP socket per worker
- `Mailboxes<Cmd>` with `ask_all` / `broadcast_with`: commands sent to every
  worker from an HTTP handler
- `WorkerSet`, `WorkerInfo`, the `WorkerHealth` plugin and a `WorkerCollector`
  for Prometheus
- A `#[scheduled(every = 10)]` job aggregating per-worker stats

Everything is in `src/main.rs` (`#[r2e::main]` + `r2e::launch!(UdpApp)`).

## Running

```bash
cargo run -p example-worker-udp
```

HTTP on `http://127.0.0.1:3000`, UDP echo on port `UDP_PORT` (default 4433):

```bash
echo hello | nc -u -w1 127.0.0.1 4433

curl localhost:3000/ping
curl localhost:3000/whoami        # which worker answered
curl localhost:3000/stats         # counters aggregated across shards
curl -X POST localhost:3000/stats/reset
curl localhost:3000/workers       # lifecycle + crossing counters per worker
curl localhost:3000/metrics
```

See `docs/features/19-sharded-serving.md` § "Per-worker services".

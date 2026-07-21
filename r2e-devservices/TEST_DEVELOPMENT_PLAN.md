# r2e-devservices — Test Development Plan

## Coverage Gaps (llvm-cov 2026-07-21)

- **Line coverage**: 11.4% (54/473)
- **Function coverage**: 14.1% (11/78)

> All services require Docker. Tests are integration-level (testcontainers).

| File | Covered | Total | Line % | Uncovered |
|------|---------|-------|--------|-----------|
| `src/ryuk.rs` | 4 | 156 | 2.6% | 152 |
| `src/openfga.rs` | 0 | 137 | 0.0% | 137 |
| `src/common.rs` | 50 | 180 | 27.8% | 130 |

### `src/ryuk.rs` — 152 uncovered lines

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L24-31 | `ensure_lease()` (skip when KEEP enabled, OnceCell init) | Test Ryuk lease acquisition lifecycle (requires Docker) |
| L33-96 | `start()`: container creation, port resolution, TCP connection retry loop, deadline panic | Test Ryuk start with retry, port resolution, connection handshake (ACK protocol) |
| L98-143 | `connect()`: TCP connect with timeout, filter write, ACK read, retry on failure | Unit-testable with a mock TCP listener: test filter write + ACK response, timeout, non-ACK response |
| L145-183 | `docker_socket()`: env var resolution (`R2E_DEVSERVICES_DOCKER_SOCKET`, `DOCKER_HOST`), candidate paths, XDG/HOME fallback | Unit-testable: test socket path resolution with various env var combos |
| L185-194 | `existing_socket()`: path existence check | Unit-testable: test with existing vs missing path |

### `src/openfga.rs` — 137 uncovered lines

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L51-61 | `base_image()` construction | Trivially covered by any start test |
| L62-135 | `DevOpenFga::start()`, `shared()`, `start_shared()`, `from_request()`, `from_container()` | Test isolated + shared container lifecycle (requires Docker + OpenFGA image) |
| L137-140 | `grpc_endpoint()`, `http_endpoint()` accessors | Covered once any start test exists |

> Note: `tests/openfga.rs` exists with 1 test (`openfga_dev_service_boots_and_bootstraps`) but the llvm-cov run may not have executed it (Docker dependency). The `create_store`/`write_model`/`write_tuples` HTTP bootstrap helpers on `DevOpenFga` are also uncovered.

### `src/common.rs` — 130 uncovered lines

| Lines | Code path | Missing test |
|-------|-----------|--------------|
| L43-53 | `SharedIdentity::name()`, `label()` | Covered by inline tests (fingerprint) but the label application path is not |
| L55-76 | `label_isolated()`, `managed_labels()` | Test that labels are applied correctly to container requests |
| L78-91 | `session_scope()`: env var `R2E_DEVSERVICES_SESSION` → fingerprint, fallback to cwd | Unit-testable: test scope derivation with/without env var |
| L93-104 | `keep_enabled()`, `truthy_env()` | Unit-testable: test truthy parsing ("1", "true", "yes", "on", "false", "") |
| L106-136 | `start_with_retry()`: retry loop on 409 name conflict, deadline panic | Requires Docker or a mock of `ContainerRequest::start()` |
| L138-193 | `cleanup()`: Docker API filter query, container state matching (active/legacy/same-scope/same-config), force remove | Requires Docker or a mocked Docker client |
| L196-222 | `wait_tcp_ready()`: TCP probe loop with deadline | Unit-testable with a delayed TCP listener |

### Testability notes

- **Unit-testable without Docker**: `docker_socket()`, `existing_socket()`, `truthy_env()`, `session_scope()`, `fingerprint()`, `wait_tcp_ready()` (with local TCP listener), `connect()` (with mock TCP server)
- **Requires Docker**: `start_with_retry()`, `cleanup()`, `ensure_lease()`, all `DevOpenFga`/`DevPostgres`/`DevRedis` lifecycle
- Existing tests (`tests/dev_services.rs`, `tests/openfga.rs`) cover the start+listen happy path but not error/retry paths

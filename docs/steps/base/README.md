# R2E Implementation Plan — Step Index

## Overview

| Step | File | Description |
|------|------|-------------|
| 0 | [00-workspace-setup.md](./00-workspace-setup.md) | Cargo multi-crate workspace setup |
| 1 | [01-r2e-core-fondations.md](./01-r2e-core-fondations.md) | AppState, AppBuilder, error handling, Controller trait |
| 2 | [02-macros-route-attributes.md](./02-macros-route-attributes.md) | `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` macros |
| 3 | [03-macros-controller.md](./03-macros-controller.md) | `#[controller]` macro — parsing + code generation |
| 4 | [04-r2e-security.md](./04-r2e-security.md) | JWT, JWKS cache, `AuthenticatedUser` extractor |
| 5 | [05-router-assembly.md](./05-router-assembly.md) | Router assembly, Tower layers, serve helper |
| 6 | [06-example-app.md](./06-example-app.md) | Complete demo application |
| 7 | [07-extensions-futures.md](./07-extensions-futures.md) | Extensions outside v0.1 scope |

## Dependency Graph

```
Step 0 (Workspace)
  │
  ├──→ Step 1 (Core)
  │       │
  │       ├──→ Step 5 (Router assembly)
  │       │       │
  │       └──→ Step 4 (Security) ──→ Step 6 (Example app)
  │                                       ↑
  ├──→ Step 2 (Route attributes)          │
  │       │                               │
  │       └──→ Step 3 (Controller) ───────┘
  │
  └──→ Step 7 (Future extensions — post v0.1)
```

## Possible Parallelization

After step 0:

- **Branch A**: Step 1 → Step 5
- **Branch B**: Step 2 → Step 3
- **Branch C**: Step 1 → Step 4

Branches A, B, and C can progress in parallel. Step 6 (example-app) requires the convergence of all three branches.

## Overall Validation Criteria

The example-app compiles and responds correctly:

```bash
cargo run -p example-app
# In another terminal:
curl http://localhost:3000/health          # → "OK"
curl -H "Authorization: Bearer <jwt>" \
     http://localhost:3000/users           # → [...users...]
```

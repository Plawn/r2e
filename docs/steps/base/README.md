# Plan d'implementation Quarlus — Index des etapes

## Vue d'ensemble

| Etape | Fichier | Description |
|-------|---------|-------------|
| 0 | [00-workspace-setup.md](./00-workspace-setup.md) | Setup du workspace Cargo multi-crates |
| 1 | [01-quarlus-core-fondations.md](./01-quarlus-core-fondations.md) | AppState, AppBuilder, error handling, trait Controller |
| 2 | [02-macros-route-attributes.md](./02-macros-route-attributes.md) | Macros `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]` |
| 3 | [03-macros-controller.md](./03-macros-controller.md) | Macro `#[controller]` — parsing + generation de code |
| 4 | [04-quarlus-security.md](./04-quarlus-security.md) | JWT, JWKS cache, extracteur `AuthenticatedUser` |
| 5 | [05-router-assembly.md](./05-router-assembly.md) | Assemblage du router, layers Tower, serve helper |
| 6 | [06-example-app.md](./06-example-app.md) | Application de demonstration complete |
| 7 | [07-extensions-futures.md](./07-extensions-futures.md) | Extensions hors scope v0.1 |

## Graphe de dependances

```
Etape 0 (Workspace)
  │
  ├──→ Etape 1 (Core)
  │       │
  │       ├──→ Etape 5 (Router assembly)
  │       │       │
  │       └──→ Etape 4 (Security) ──→ Etape 6 (Example app)
  │                                       ↑
  ├──→ Etape 2 (Route attributes)         │
  │       │                               │
  │       └──→ Etape 3 (Controller) ──────┘
  │
  └──→ Etape 7 (Extensions futures — post v0.1)
```

## Parallelisation possible

Apres l'etape 0 :

- **Branche A** : Etape 1 → Etape 5
- **Branche B** : Etape 2 → Etape 3
- **Branche C** : Etape 1 → Etape 4

Les branches A, B et C peuvent avancer en parallele. L'etape 6 (example-app) requiert la convergence des trois branches.

## Critere de validation global

L'example-app compile et repond correctement :

```bash
cargo run -p example-app
# Dans un autre terminal :
curl http://localhost:3000/health          # → "OK"
curl -H "Authorization: Bearer <jwt>" \
     http://localhost:3000/users           # → [...users...]
```

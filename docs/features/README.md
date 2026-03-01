# R2E — Guide des fonctionnalites

## Vue d'ensemble

R2E fournit 13 fonctionnalites principales, chacune documentee dans un fichier dedie.

| # | Fonctionnalite | Fichier | Crate |
|---|---------------|---------|-------|
| 1 | Configuration | [01-configuration.md](./01-configuration.md) | `r2e-core` |
| 2 | Validation | [02-validation.md](./02-validation.md) | `r2e-core` |
| 3 | Gestion d'erreurs | [03-error-handling.md](./03-error-handling.md) | `r2e-core` |
| 4 | Intercepteurs | [04-intercepteurs.md](./04-intercepteurs.md) | `r2e-macros` |
| 5 | OpenAPI | [05-openapi.md](./05-openapi.md) | `r2e-openapi` |
| 6 | Data / Repository | [06-data-repository.md](./06-data-repository.md) | `r2e-data` |
| 7 | Evenements | [07-evenements.md](./07-evenements.md) | `r2e-events` |
| 8 | Scheduling | [08-scheduling.md](./08-scheduling.md) | `r2e-scheduler` |
| 9 | Mode developpement | [09-dev-mode.md](./09-dev-mode.md) | `r2e-core` |
| 10 | Hooks de cycle de vie | [10-lifecycle-hooks.md](./10-lifecycle-hooks.md) | `r2e-core` |
| 11 | Securite JWT / Roles | [11-securite-jwt.md](./11-securite-jwt.md) | `r2e-security` |
| 12 | Testing | [12-testing.md](./12-testing.md) | `r2e-test` |
| 13 | Cycle de vie, DI & Performance | [13-lifecycle-injection-performance.md](./13-lifecycle-injection-performance.md) | `r2e-core` / `r2e-macros` |

## Architecture des crates

```
r2e-macros       Proc-macro. #[derive(Controller)] + #[routes] generent le code Axum.
r2e-core         Runtime. AppBuilder, Controller, HttpError, config, validation, cache, rate limiter.
r2e-security     JWT/JWKS, AuthenticatedUser, #[roles].
r2e-data         Entity, QueryBuilder, Pageable, Page, Repository CRUD.
r2e-events       EventBus trait + LocalEventBus (pub/sub type).
r2e-scheduler    Taches planifiees (intervalle, cron) avec CancellationToken.
r2e-openapi      Generation de spec OpenAPI 3.0.3 + Swagger UI.
r2e-test         TestApp (client HTTP in-process) + TestJwt.
r2e-cli          CLI (r2e new, r2e dev).
```

## Demarrage rapide

```bash
# Lancer l'application de demonstration
cargo run -p example-app

# Dans un autre terminal
curl http://localhost:3000/health           # → "OK"
curl http://localhost:3000/openapi.json     # → spec OpenAPI
curl http://localhost:3000/docs             # → interface de documentation API
```

# Quarlus — Guide des fonctionnalites

## Vue d'ensemble

Quarlus fournit 12 fonctionnalites principales, chacune documentee dans un fichier dedie.

| # | Fonctionnalite | Fichier | Crate |
|---|---------------|---------|-------|
| 1 | Configuration | [01-configuration.md](./01-configuration.md) | `quarlus-core` |
| 2 | Validation | [02-validation.md](./02-validation.md) | `quarlus-core` |
| 3 | Gestion d'erreurs | [03-error-handling.md](./03-error-handling.md) | `quarlus-core` |
| 4 | Intercepteurs | [04-intercepteurs.md](./04-intercepteurs.md) | `quarlus-macros` |
| 5 | OpenAPI | [05-openapi.md](./05-openapi.md) | `quarlus-openapi` |
| 6 | Data / Repository | [06-data-repository.md](./06-data-repository.md) | `quarlus-data` |
| 7 | Evenements | [07-evenements.md](./07-evenements.md) | `quarlus-events` |
| 8 | Scheduling | [08-scheduling.md](./08-scheduling.md) | `quarlus-scheduler` |
| 9 | Mode developpement | [09-dev-mode.md](./09-dev-mode.md) | `quarlus-core` |
| 10 | Hooks de cycle de vie | [10-lifecycle-hooks.md](./10-lifecycle-hooks.md) | `quarlus-core` |
| 11 | Securite JWT / Roles | [11-securite-jwt.md](./11-securite-jwt.md) | `quarlus-security` |
| 12 | Testing | [12-testing.md](./12-testing.md) | `quarlus-test` |

## Architecture des crates

```
quarlus-macros       Proc-macro. Parse les blocs controller! {} et genere le code Axum.
quarlus-core         Runtime. AppBuilder, Controller, AppError, config, validation, cache, rate limiter.
quarlus-security     JWT/JWKS, AuthenticatedUser, #[roles].
quarlus-data         Entity, QueryBuilder, Pageable, Page, Repository CRUD.
quarlus-events       EventBus in-process (pub/sub type).
quarlus-scheduler    Taches planifiees (intervalle, cron) avec CancellationToken.
quarlus-openapi      Generation de spec OpenAPI 3.0.3 + Swagger UI.
quarlus-test         TestApp (client HTTP in-process) + TestJwt.
quarlus-cli          CLI (quarlus new, quarlus dev).
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

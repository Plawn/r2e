# Remediations (r2e + r2e-core)

## 1) with_config est un no-op
- Probleme: `AppBuilder::with_config` stocke `R2eConfig` mais ne le propage jamais (pas d'extension Axum ni dans l'etat).
- Remediation:
  - Option A (non-breaking): injecter `R2eConfig` comme extension Axum au moment de `build()`.
    - Exemple: lors de `build_inner`, si `shared.config` est `Some`, appeler `app = app.layer(Extension(config.clone()))` ou `app = app.with_state(...)` + `router.layer(Extension(config))` selon le pattern utilise ailleurs.
  - Option B (API explicite): supprimer `shared.config` + `with_config` si non utilise, et mettre a jour les docs/examples qui le mentionnent.
- Fichiers: `r2e-core/src/builder.rs`, docs/examples qui appellent `.with_config`.

## 2) Feature `full` n'inclut pas `validation`
- Probleme: la doc annonce "All of the above" mais `full` n'active pas `validation`.
- Remediation:
  - Ajouter `validation` dans la liste `full`.
  - Verifier si la doc doit preciser que `validation` est un feature de `r2e-core` (sinon corriger le tableau).
- Fichiers: `r2e/Cargo.toml`, `r2e/src/lib.rs` (doc table si besoin), docs associees.

## 3) API legacy de plugins differes toujours exposee
- Probleme: `DeferredPlugin`/`DeferredInstallContext`/`with_plugin` sont deprecies mais toujours exposes et utilises par le builder.
- Remediation:
  - Option A (breaking): supprimer les types et le chemin legacy, retirer `allow(deprecated)` et la logique d'installation legacy.
  - Option B (compat): garder mais isoler dans un module `legacy` et ne plus re-exporter au top-level, ajouter un avertissement de migration clair dans les docs.
- Fichiers: `r2e-core/src/plugin.rs`, `r2e-core/src/builder.rs`, `r2e-core/src/lib.rs`.

## 4) Warning scheduler mentionne une API depassee
- Probleme: le warning conseille `.with_plugin(Scheduler)` alors que l'API recommande `.plugin(Scheduler)`.
- Remediation:
  - Mettre a jour le message de warning pour pointer vers `.plugin(Scheduler)`.
- Fichier: `r2e-core/src/builder.rs`.

## 5) Champs morts dans le broadcaster WS
- Probleme: `sender_id`/`client_id` ne sont jamais utilises; `allow(dead_code)` masque le probleme.
- Remediation:
  - Option A: implementer l'exclusion de l'emetteur (ex: `send_from(sender_id, msg)` et filtrage cote receiver).
  - Option B: supprimer `sender_id`/`client_id` et les `allow(dead_code)` associes.
- Fichier: `r2e-core/src/ws.rs`.

## 6) `#[allow(unused_variables)]` global sur `WsHandler`
- Probleme: le lint est desactive pour toute la trait, alors qu'il suffit pour les methodes par defaut.
- Remediation:
  - Remplacer par des arguments nommes `_ws` ou `_msg` dans les implementations par defaut, ou appliquer `#[allow(unused_variables)]` uniquement sur `on_connect`/`on_close`.
- Fichier: `r2e-core/src/ws.rs`.

# Design Doc: Option 1 — Registre interne de “route macros”

## Résumé
Remplacer la gestion actuelle “hard-codée” des attributs route/middleware/guards dans `r2e-macros` par un registre interne de plugins statiques. Les comportements restent compilés dans `r2e-macros`, mais deviennent modulaires, testables et plus simples à étendre sans toucher le cœur à chaque ajout.

## Contexte actuel
- Parsing des attributs dispersé et codé en dur dans `r2e-macros/src/routes_parsing.rs` et `r2e-macros/src/extract/route.rs`.
- Liste des attributs stripée manuellement dans `strip_route_attrs`.
- Ajout d’un nouvel attribut = modifications croisées parsing + codegen + no-op macro dans `r2e-macros/src/lib.rs`.

## Objectifs
- Centraliser la déclaration des attributs supportés.
- Rendre l’ajout d’un nouvel attribut localisé et prévisible.
- Réduire le couplage entre parsing et codegen.
- Garder le comportement actuel inchangé (compatibilité API).

## Non-objectifs
- Extension externe par crates tierces (Option 2).
- Refacto profonde des macros de derive (`Controller`).
- Changement de l’API publique des macros.

## Proposition

### 1) Nouveau module de registry
Créer `r2e-macros/src/plugin.rs` avec:
- Un trait `RoutePlugin`.
- Un `Registry` statique contenant la liste des plugins internes.

Responsabilités du plugin:
- Déclarer ses attributs (nom + type, ex: route, sse, ws, intercept, guard).
- Parser ses attributs à partir d’un `Vec<Attribute>`.
- Injecter un “payload” dans la définition interne (`RouteMethod`, `SseMethod`, etc.).
- Fournir un hook de codegen optionnel.

### 2) Nouvelles structures de données
Étendre `r2e-macros/src/types.rs` avec une structure générique d’extensions:
- `RouteExtension`: type + payload typé par plugin.
- `RouteMethod.extensions: Vec<RouteExtension>`.
- Même principe pour `SseMethod`, `WsMethod`, `ScheduledMethod` si besoin.

L’idée: le cœur construit un modèle commun, puis les plugins enrichissent.

### 3) Parsing orchestré
`r2e-macros/src/routes_parsing.rs` devient un orchestrateur:
1. Collecte `attrs` du handler.
2. `Registry::parse_all(attrs)` retourne:
   - `route_kind` (http/ws/sse/consumer/scheduled/other).
   - une liste d’extensions (payloads).
   - des erreurs de validation si conflit.
3. Construction de `RouteMethod` avec `extensions`.

### 4) Codegen délégué
Dans `r2e-macros/src/codegen/*`, ajouter un hook:
- Avant génération handlers/wrapping, itérer sur `extensions` pour injecter wrapping, config de guards, layers, etc.

### 5) Source of truth pour les attrs
Le registry devient la source unique des attributs supportés.
- `strip_route_attrs` est remplacé par `Registry::strip_known_attrs()`.
- Plus besoin de maintenir la liste en plusieurs endroits.

## Architecture proposée

### Trait `RoutePlugin` (conceptuel)
- `fn name() -> &'static str`
- `fn attributes() -> &'static [&'static str]`
- `fn parse(&self, attrs: &[Attribute]) -> Result<Option<RouteExtension>>`
- `fn apply_code_gen(&self, ext: &RouteExtension, ctx: &mut CodegenCtx)`

### Registry
- `static PLUGINS: &[&dyn RoutePlugin]`
- `parse_all(attrs)`:
  - dispatch vers chaque plugin
  - merge des extensions
  - validation de conflits
- `strip_known_attrs(attrs)`:
  - enlève tous les attrs gérés par les plugins

### CodegenCtx
- Contexte minimal (handler, method, state, etc.)
- Utilisé par les plugins pour injecter wrapping ou config

## Plan de migration (incremental)
1. Créer `plugin.rs` et le Registry.
2. Migrer les attributs interop.
3. Migrer les routes HTTP.
4. Migrer SSE et WS.
5. Migrer scheduled et consumer.
6. Remplacer `strip_route_attrs`.

À chaque étape, tests + parité avec le comportement actuel.

## Tests
- Tests unitaires de parsing par plugin.
- Tests d’intégration macro: mêmes snapshots codegen.
- Ajouter des tests ciblés pour conflits (ex: `#[managed]` + `#[transactional]`).

## Risques
- Plus de complexité dans le core (registry + dispatch).
- Codegen multi-pass si plugins nécessitent des hooks à différents moments.
- Régression silencieuse si un plugin oublie de déclarer un attr.

Mitigation:
- Tests de snapshot (golden tests).
- Avertissement compile-time si attr inconnue (optionnel).

## Impact
- Aucun changement API pour l’utilisateur.
- Refacto interne uniquement, mais facilite l’ajout d’attributs futurs.

## Fichiers impactés
- `r2e-macros/src/plugin.rs` (nouveau)
- `r2e-macros/src/routes_parsing.rs`
- `r2e-macros/src/types.rs`
- `r2e-macros/src/extract/route.rs` (déplacé ou démantelé)
- `r2e-macros/src/codegen/*`

# Audit r2e-macros — Rapport detaille

**Date :** 2026-02-23
**Scope :** `r2e-macros/src/` — 33 fichiers source, ~2 500+ lignes de codegen proc-macro
**Dependances :** `syn`, `quote`, `proc-macro2`, `proc-macro-crate`

---

## Table des matieres

1. [Resume executif](#1-resume-executif)
2. [Problemes critiques](#2-problemes-critiques)
3. [Problemes de robustesse](#3-problemes-de-robustesse)
4. [Duplication de code](#4-duplication-de-code)
5. [Ameliorations structurelles](#5-ameliorations-structurelles)
6. [Problemes mineurs](#6-problemes-mineurs)
7. [Analyse fichier par fichier](#7-analyse-fichier-par-fichier)
8. [Recommandations prioritaires](#8-recommandations-prioritaires)

---

## 1. Resume executif

Le crate `r2e-macros` est fonctionnel et bien structure dans l'ensemble. L'architecture plugin pour l'extraction des attributs (`extract/plugins.rs`) est propre, et les messages d'erreur dans le parsing derive sont de bonne qualite.

Cependant, l'audit revele **3 problemes critiques** (guard gRPC inoperant, zero tests, `unwrap()` dans le code genere), **~420 lignes de duplication reductible** (~17% du crate), et plusieurs cas ou les erreurs sont avalees silencieusement au lieu de produire des diagnostics clairs.

**Metriques cles :**

| Metrique | Valeur |
|---|---|
| Fichiers source | 33 |
| Tests | 0 |
| `unwrap()` dans le code genere | 2 (runtime panic) |
| Fonctions dupliquees | 8 patterns identifies |
| Lignes de duplication estimees | ~420 (~17%) |
| Erreurs silencieusement avalees | 3 cas |

---

## 2. Problemes critiques

### 2.1 `#[roles]` silencieusement ignore sur les methodes gRPC

**Fichier :** `grpc_codegen/trait_impl.rs:170-188`
**Severite :** Critique — trou de securite silencieux

Le guard de roles gRPC cree un `GrpcRolesGuard` et un `GrpcGuardContext` mais **n'appelle jamais la methode `check()`**. Le bloc genere n'a aucun effet. Un `#[roles("admin")]` sur une methode gRPC compile sans erreur mais ne protege rien.

```rust
// Code genere actuel — le guard n'est jamais execute
{
    let __guard = GrpcRolesGuard::new(vec![...]);
    let __ctx = GrpcGuardContext { ... };
    // MANQUANT: __guard.check(&self.state, &__ctx).await?;
}
```

**Impact :** Tout endpoint gRPC annote avec `#[roles(...)]` est accessible sans restriction de role.

**Correction :** Ajouter l'appel a `check()` ou, si le guard gRPC n'est pas encore pret, emettre un `compile_error!` quand `#[roles]` est utilise sur une methode gRPC.

---

### 2.2 Zero tests

**Severite :** Critique — risque de regression

Le crate n'a aucun fichier de test (`tests/` absent). Pour un crate proc-macro de cette complexite, cela signifie :

- Aucune validation des messages d'erreur de parsing
- Aucune verification des edge cases (structs generiques, tuple structs, 0 champs)
- Aucun test de non-regression sur les 5 branches de `generate_single_handler`
- Impossible de refactorer en confiance

**Correction recommandee :**

1. **Tests trybuild** pour les cas d'erreur (attributs manquants, combinaisons invalides)
2. **Tests d'expansion** (`cargo expand` ou `macrotest`) pour les happy paths
3. **Tests de compilation** pour verifier que le code genere compile correctement

---

### 2.3 `unwrap()` emis dans le code genere — panics runtime

**Severite :** Critique

Deux `unwrap()` sont emis dans le code que l'utilisateur compilera, pouvant provoquer des panics au runtime :

#### 2.3.1 `serde_json::to_value` dans les schemas OpenAPI

**Fichier :** `codegen/controller_impl.rs:437`

```rust
// Code genere
serde_json::to_value(__schema).unwrap()
```

Si `schemars` produit un schema non-serialisable par `serde_json`, panic au runtime. Peu probable mais non-impossible.

**Correction :** Utiliser `.unwrap_or_default()` ou `.unwrap_or_else(|_| serde_json::Value::Null)`.

#### 2.3.2 `StatusCode::from_u16` dans `#[derive(ApiError)]`

**Fichier :** `api_error_derive.rs:660`

```rust
// Code genere pour #[error(status = 999)]
StatusCode::from_u16(999u16).unwrap()  // PANIC: 999 n'est pas un status HTTP valide
```

**Correction :** Valider le code de status pendant l'expansion du macro et emettre un `compile_error!` si le code est hors range (100-999) ou non-standard.

---

## 3. Problemes de robustesse

### 3.1 Matching de types par string fragile

**Fichier :** `codegen/controller_impl.rs:325`
**Severite :** Moyenne

```rust
let ty_str = quote!(#ty).to_string();
if ty_str.contains("Path") {  // Matche PathBuf, UserPath, FilePathInfo...
```

Utilise `contains("Path")` au lieu de verifier le dernier segment du type path. Tout type contenant "Path" dans son nom sera incorrectement identifie comme un extracteur `Path<T>`.

**Correction :** Extraire le dernier segment du `syn::Type` et comparer `segment.ident == "Path"` exactement.

---

### 3.2 `.ok()` silencieux sur le parsing du roles guard

**Fichier :** `extract/route.rs:92`
**Severite :** Moyenne

```rust
syn::parse2(tokens).ok()  // Erreur de parsing silencieusement avalee
```

Si l'expression du `RolesGuard` genere echoue a parser (ce qui indiquerait un bug dans le macro), `.ok()` avale l'erreur et retourne `None`. Le guard de roles disparait sans laisser de trace.

**Correction :** Utiliser `.expect("internal: RolesGuard expression should always parse")` ou propager comme `syn::Result`.

---

### 3.3 Bug de documentation : `delay` vs `initial_delay`

**Fichier :** `lib.rs:429`
**Severite :** Moyenne

```rust
// Documentation dans lib.rs (INCORRECT) :
/// #[scheduled(every = 60, delay = 10)]

// Parsing reel dans extract/scheduled.rs (CORRECT) :
// Attend: #[scheduled(every = 60, initial_delay = 10)]
```

Les utilisateurs qui copient l'exemple de la doc obtiendront une erreur de compilation (ou pire, l'attribut sera silencieusement ignore).

**Correction :** Remplacer `delay = 10` par `initial_delay = 10` dans la doc.

---

### 3.4 `#[derive(Params)]` — champs sans annotation

**Fichier :** `params_derive.rs:126`
**Severite :** Moyenne

Un champ sans `#[path]`, `#[query]`, `#[header]`, ou `#[params]` est silencieusement ignore par le macro. Le struct init genere omet ce champ, ce qui produit un "missing field" a la compilation — l'erreur pointe vers le code genere, pas vers le champ non-annote de l'utilisateur.

**Correction :** Detecter les champs sans annotation source et emettre un `compile_error!` clair :
```
field `foo` has no source annotation (#[path], #[query], #[header], or #[params])
```

---

### 3.5 `from_multipart.rs` — type d'erreur incorrect

**Fichier :** `from_multipart.rs:94`
**Severite :** Moyenne

```rust
v.parse().map_err(|e: Box<dyn ::std::fmt::Display>| { ... })
```

Le code genere annote l'erreur de `str::parse()` comme `Box<dyn Display>`, mais `parse()` retourne `FromStr::Err` qui n'est pas forcement ce type. Provoque une erreur de compilation pour tout type dont `FromStr::Err` n'est pas `Box<dyn Display>`.

**Correction :** Utiliser `map_err(|e| e.to_string())` ou ne pas annoter le type d'erreur et laisser l'inference fonctionner.

---

### 3.6 `unwrap()` interne dans le macro pour cron config

**Fichier :** `codegen/controller_impl.rs:772`
**Severite :** Basse (defensive)

```rust
let cron_expr = sm.config.cron.as_ref().unwrap();
```

Si la logique de parsing change et que `every` et `cron` sont tous deux `None`, ceci panic le compilateur au lieu de produire un diagnostic. Le parsing dans `extract/scheduled.rs` valide ce cas, mais le code devrait etre defensif.

**Correction :** Remplacer par un `compile_error!` ou un `.expect("internal: cron must be Some when every is None")`.

---

## 4. Duplication de code

~420 lignes de duplication reductible, soit environ 17% du crate.

### 4.1 `unwrap_option_type` — 4 copies

| Fichier | Lignes |
|---|---|
| `routes_parsing.rs:18` | ~15 |
| `grpc_routes_parsing.rs:34` | ~15 |
| `params_derive.rs:565` | ~15 |
| `config_derive.rs:58-65` (variante `is_option_type` + `option_inner_type`) | ~15 |

**Correction :** Extraire dans un `util.rs` partage.

---

### 4.2 `extract_identity_param` — 2 copies

| Fichier | Lignes |
|---|---|
| `routes_parsing.rs:36` | ~20 |
| `grpc_routes_parsing.rs:50` | ~20 |

Seule difference : le message d'erreur. Devrait etre une seule fonction parametree.

---

### 4.3 Resolution de crate path — 4 fonctions identiques

**Fichier :** `crate_path.rs`

Les fonctions `r2e_core_path`, `r2e_security_path`, `r2e_scheduler_path`, `r2e_grpc_path` partagent exactement la meme structure (~20 lignes chacune) :

```rust
fn r2e_xxx_path() -> TokenStream {
    if let Ok(found) = crate_name("r2e") { ... }
    else if let Ok(found) = crate_name("r2e-xxx") { ... }
    else { ... }
}
```

**Correction :** Factoriser en une seule fonction generique :

```rust
fn resolve_crate(facade_subpath: Option<&str>, direct_crate: &str) -> TokenStream { ... }
```

---

### 4.4 Init des champs config — 2 copies dans derive_codegen.rs

**Fichier :** `derive_codegen.rs`

| Fonction | Lignes |
|---|---|
| `generate_extractor` (lignes 173-199) | ~25 |
| `generate_stateful_construct` (lignes 284-303) | ~20 |

Code quasi-identique pour initialiser les champs `#[config]`. Seule difference : l'extractor retourne une `HttpError` response, `StatefulConstruct` fait un `panic!`.

---

### 4.5 Traitement des parametres bean/producer — 2 copies

| Fichier | Lignes |
|---|---|
| `bean_attr.rs:42-81` | ~40 |
| `producer_attr.rs:66-113` | ~48 |

Meme boucle sur les parametres de constructeur : detection `#[config("key")]` vs dependance reguliere, accumulation de build args, tracking de l'etat config.

---

### 4.6 Generation du guard context — 3 copies dans handlers.rs

**Fichier :** `codegen/handlers.rs`

| Location | Lignes |
|---|---|
| `generate_single_handler` (via `generate_guard_context`) | helper partage |
| SSE handler (lignes 672-702) | ~30 inline |
| WS handler (lignes 840-870) | ~30 inline |

Les handlers SSE et WS dupliquent la logique de creation du guard context au lieu de reutiliser `generate_guard_context`.

---

### 4.7 Generation middleware/layer — 3 copies dans controller_impl.rs

**Fichier :** `codegen/controller_impl.rs`

| Location | Lignes |
|---|---|
| `generate_route_registrations` (lignes 216-232) | ~16 |
| `generate_sse_route_registrations` (lignes 611-630) | ~20 |
| `generate_ws_route_registrations` (lignes 697-713) | ~16 |

Pattern identique pour appliquer les layers et middlewares a une route.

---

### 4.8 Branches d'extraction query params — 3 branches dans params_derive.rs

**Fichier :** `params_derive.rs:316-385`

Trois branches quasi-identiques pour `DefaultValue::Trait`, `DefaultValue::Expr`, et `None`. Chacune duplique la logique de lookup par cle, parsing, et construction d'erreur. Seul le fallback "missing" differe.

---

## 5. Ameliorations structurelles

### 5.1 Injection par acces direct aux champs du state

**Fichier :** `derive_codegen.rs:156`

```rust
// Code genere actuel
#field_name: __state.#field_name.clone()
```

Ceci couple les noms des champs du controller a ceux du state struct. Le pattern Axum idiomatique utilise `FromRef` :

```rust
// Pattern idiomatique
#field_name: <#field_type as ::axum::extract::FromRef<__State>>::from_ref(__state)
```

Avantage : decouple les noms, compatible avec `#[derive(FromRef)]`.

**Impact :** Breaking change pour les utilisateurs existants. A considerer pour une version majeure.

---

### 5.2 `generate_single_handler` trop complexe

**Fichier :** `codegen/handlers.rs:413-568`
**Complexite :** ~155 lignes, 5 branches

| Case | Condition |
|---|---|
| 1a | Guards + managed params |
| 1b | Guards, pas de managed |
| 2a | Pas de guards, managed params |
| 2b | Pas de guards, pas de managed, interceptors ou validation |
| 3 | Cas simple (rien) |

Chaque branche genere une signature et un corps de handler legerement differents. La logique pourrait etre decomposee en helpers composables :

```
build_signature(has_guards, has_managed) -> TokenStream
build_guard_block(guards, context) -> TokenStream
build_managed_acquire(managed_params) -> TokenStream
build_validation(validatable_params) -> TokenStream
build_body(method_call, interceptors) -> TokenStream
```

---

### 5.3 Handlers SSE/WS — fonctionnalites manquantes

Les handlers SSE et WS ne supportent pas :

- **Intercepteurs** : l'attribut `#[intercept]` est parse mais le code de wrapping n'est jamais genere
- **Validation** : les parametres ne sont pas valides automatiquement (contrairement aux handlers route)

Les handlers SSE/WS dupliquent aussi `extra_params`, `call_args`, etc. en inline au lieu de reutiliser `extract_handler_params` et `build_handler_params`.

---

### 5.4 gRPC — variables mortes et imports inutilises

**Fichier :** `grpc_codegen/trait_impl.rs`

- `r2e_security_path` est importe (ligne 6) mais la variable `security_krate` (ligne 167) n'est jamais utilisee dans le code genere — c'est une variable morte.
- L'import de `r2e_security_path` devrait etre conditionnel.

---

### 5.5 Intercepteur context : state `&()` pour les scheduled tasks

**Fichier :** `codegen/wrapping.rs:198`

```rust
// Code genere pour les scheduled tasks
state: &(),
```

Les intercepteurs sur les scheduled tasks recoivent `&()` comme state au lieu du state applicatif reel. Si un intercepteur tente d'acceder au state (ex: logging avec context), il recoit un type unit.

**Correction :** Passer le state reel ou documenter la limitation.

---

## 6. Problemes mineurs

### 6.1 Commentaire en francais dans le code source

**Fichier :** `route.rs:25`

```rust
/// Parse le path depuis les arguments d'un attribut
```

Le reste du code et des commentaires sont en anglais. Inconsistance mineure.

---

### 6.2 `#[routes(something)]` — argument silencieusement ignore

**Fichier :** `lib.rs:150`

Le parametre `_args` est discard. Si un utilisateur ecrit `#[routes(prefix = "/api")]`, aucun warning n'est emis.

**Correction :** Emettre un `compile_error!` si des arguments sont passes a `#[routes]`.

---

### 6.3 `is_result_type` fragile

**Fichier :** `codegen/handlers.rs:572-581`

Ne verifie que le dernier segment du path. `std::result::Result`, `anyhow::Result`, ou un type alias `MyResult` ne sont pas detectes. C'est une limitation connue des proc-macros (pas de resolution de types), mais merite d'etre documentee.

---

### 6.4 `humanize_ident` — casse incorrecte pour acronymes

**Fichier :** `api_error_derive.rs`

Pour un variant nomme `IOError`, la fonction produit `"I o error"` (espace entre I et O). Devrait detecter les sequences d'uppercase consecutives comme un seul mot.

---

### 6.5 `WsParam::ty` jamais lu

**Fichier :** `types.rs:112`

```rust
pub struct WsParam {
    pub index: usize,
    #[allow(dead_code)]
    pub ty: syn::Type,     // Jamais lu — a utiliser ou supprimer
    pub is_ws_stream: bool,
}
```

---

### 6.6 `Cacheable` derive assume `serde_json` et `bytes` disponibles

**Fichier :** `cacheable_derive.rs:23-29`

Le code genere reference `serde_json::to_vec` et `bytes::Bytes` directement. Si l'utilisateur n'a pas ces crates, il obtiendra des erreurs "not found" confuses. Le code genere devrait passer par les re-exports de `r2e_core`.

---

### 6.7 `bean_derive.rs` — `.unwrap()` sur field ident

**Fichier :** `bean_derive.rs:48`

```rust
let field_name = field.ident.as_ref().unwrap();
```

Garanti safe car on verifie `Fields::Named` avant, mais un `ok_or_else` avec une `syn::Error` serait plus defensif.

---

### 6.8 `bean_state_derive.rs` — dedup par stringification

**Fichier :** `bean_state_derive.rs:136-138`

```rust
fn type_to_string(ty: &Type) -> String {
    quote!(#ty).to_string().replace(' ', "")
}
```

La cle de dedup depend du formatage de `quote!` qui pourrait changer entre versions. Acceptable en pratique mais fragile en theorie.

---

## 7. Analyse fichier par fichier

| Fichier | Etat | Findings |
|---|---|---|
| `lib.rs` | OK | Bug doc `delay` vs `initial_delay`, args de `#[routes]` ignores |
| `types.rs` | OK | `WsParam::ty` dead code |
| `route.rs` | OK | Commentaire francais |
| `crate_path.rs` | Duplication | 4 fonctions identiques a factoriser |
| `derive_parsing.rs` | Bon | Messages d'erreur excellents, span mineur |
| `derive_codegen.rs` | Duplication | Config init duplique, inject via champs directs |
| `routes_parsing.rs` | Duplication | `unwrap_option_type`, `extract_identity_param` |
| `routes_codegen.rs` | OK | Delegation propre aux sous-modules |
| `codegen/handlers.rs` | Complexe | 5 branches, guard context duplique SSE/WS |
| `codegen/controller_impl.rs` | Mixed | `unwrap()` genere, `contains("Path")`, middleware duplique |
| `codegen/wrapping.rs` | OK | State `&()` pour scheduled tasks |
| `extract/route.rs` | Robustesse | `.ok()` silencieux sur roles guard |
| `extract/plugins.rs` | Bon | Architecture propre |
| `extract/managed.rs` | Bon | Messages d'erreur clairs |
| `extract/scheduled.rs` | Bon | Bonne validation |
| `extract/consumer.rs` | Bon | Diagnostics utiles |
| `bean_attr.rs` | Duplication | Boucle params partagee avec producer |
| `producer_attr.rs` | Duplication | Idem |
| `bean_derive.rs` | OK | `unwrap()` mineur |
| `bean_state_derive.rs` | OK | Dedup par string fragile |
| `type_list_gen.rs` | Bon | Simple et correct |
| `cacheable_derive.rs` | Minor | Assume deps disponibles |
| `config_derive.rs` | OK | Propre |
| `from_multipart.rs` | Bug | Type d'erreur incorrect |
| `params_derive.rs` | Duplication+Bug | Branches dupliquees, champs sans annotation |
| `api_error_derive.rs` | Critique+Minor | `unwrap()` genere, `humanize_ident` |
| `grpc_routes_parsing.rs` | Duplication | Copies de routes_parsing |
| `grpc_codegen/mod.rs` | OK | Fix de nommage en cours |
| `grpc_codegen/trait_impl.rs` | Critique | Roles guard inoperant, vars mortes |
| `grpc_codegen/service_impl.rs` | OK | Convention tonic assumee |

---

## 8. Recommandations prioritaires

### Priorite 1 — Securite et correctness

| # | Action | Fichier(s) | Effort |
|---|---|---|---|
| 1 | Fixer le guard roles gRPC (appeler `check()`) | `grpc_codegen/trait_impl.rs` | Petit |
| 2 | Valider les status codes numeriques a la compilation dans `ApiError` | `api_error_derive.rs` | Petit |
| 3 | Remplacer `unwrap()` dans le code genere par des alternatives safe | `controller_impl.rs`, `api_error_derive.rs` | Petit |
| 4 | Corriger le type d'erreur `from_multipart.rs` | `from_multipart.rs` | Petit |

### Priorite 2 — Robustesse et diagnostics

| # | Action | Fichier(s) | Effort |
|---|---|---|---|
| 5 | Remplacer `contains("Path")` par une comparaison exacte du segment | `controller_impl.rs` | Petit |
| 6 | Propager l'erreur au lieu de `.ok()` sur le roles guard | `extract/route.rs` | Petit |
| 7 | Diagnostiquer les champs sans annotation dans `Params` | `params_derive.rs` | Petit |
| 8 | Corriger la doc `delay` → `initial_delay` | `lib.rs` | Trivial |
| 9 | Emettre un diagnostic si `#[routes]` recoit des arguments | `lib.rs` | Trivial |

### Priorite 3 — Reduction de la duplication

| # | Action | Fichier(s) | Effort |
|---|---|---|---|
| 10 | Creer `util.rs` avec `unwrap_option_type`, `extract_identity_param` | 4+ fichiers | Moyen |
| 11 | Factoriser `crate_path.rs` en une seule fonction generique | `crate_path.rs` | Petit |
| 12 | Unifier la boucle de traitement params dans bean/producer | `bean_attr.rs`, `producer_attr.rs` | Moyen |
| 13 | Extraire les helpers de guard context pour SSE/WS | `codegen/handlers.rs` | Moyen |

### Priorite 4 — Tests et structure

| # | Action | Fichier(s) | Effort |
|---|---|---|---|
| 14 | Ajouter des tests trybuild pour les cas d'erreur | Nouveau `tests/` | Grand |
| 15 | Ajouter des tests d'expansion pour les happy paths | Nouveau `tests/` | Grand |
| 16 | Refactorer `generate_single_handler` en helpers composables | `codegen/handlers.rs` | Grand |
| 17 | Aligner SSE/WS handlers sur les fonctionnalites des route handlers | `codegen/handlers.rs` | Grand |

### Priorite 5 — Ameliorations futures (breaking changes potentiels)

| # | Action | Fichier(s) | Effort |
|---|---|---|---|
| 18 | Migrer l'injection vers `FromRef` au lieu de l'acces direct aux champs | `derive_codegen.rs` | Grand |
| 19 | Passer le vrai state aux intercepteurs des scheduled tasks | `codegen/wrapping.rs` | Moyen |

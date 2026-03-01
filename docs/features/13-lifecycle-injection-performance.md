# Feature 13 — Cycle de vie, injection de dependances et implications de performance

## Vue d'ensemble

Ce document decrit le cycle de vie complet d'une application R2E — du demarrage a l'arret — ainsi que le fonctionnement interne de l'injection de dependances et ses implications sur la performance.

---

## 1. Cycle de vie de l'application

### 1.1 Phase d'assemblage (`AppBuilder`)

Tout commence par la construction fluide via `AppBuilder` :

```rust
AppBuilder::new()
    .with_state(services)           // 1. Etat applicatif
    .with_config(config)            // 2. Configuration
    .with_cors()                    // 3. Layers Tower
    .with_tracing()
    .with_health()
    .with_error_handling()
    .with_openapi(openapi_config)   // 4. Documentation
    .with_scheduler(|s| {           // 5. Taches planifiees
        s.register::<ScheduledJobs>();
    })
    .on_start(|state| async { Ok(()) })  // 6. Hooks
    .on_stop(|| async { })
    .register_controller::<UserController>()  // 7. Controllers
    .serve("0.0.0.0:3000")         // 8. Lancement
    .await?;
```

`AppBuilder` accumule les elements sans rien executer. L'assemblage se fait lors de l'appel a `build()` ou `serve()`.

### 1.2 Construction interne (`build_inner`)

La methode `build_inner()` produit un tuple `(Router, StartupHooks, ShutdownHooks, ConsumerRegs, State)` :

1. **Creation du Router Axum** — un `Router<T>` vide
2. **Fusion des routes** — chaque controller enregistre ses routes via `Controller::routes()`
3. **OpenAPI** (si active) — invocation du builder OpenAPI avec les metadonnees collectees, ajout des routes `/openapi.json` et `/docs`
4. **Routes systeme** — `/health` et `/__r2e_dev/*` si actives
5. **Application de l'etat** — `router.with_state(state.clone())` : un seul clone a la construction
6. **Empilement des layers** — appliques dans l'ordre inverse de declaration (le dernier ajoute est le plus externe)

### 1.3 Ordre des layers Tower

Les layers s'empilent de l'interieur vers l'exterieur. A l'execution, la requete les traverse dans l'ordre inverse :

```
Requete HTTP entrante
        |
        v
 [TraceLayer]          -- log de la requete/reponse (le plus externe)
 [CatchPanicLayer]     -- capture les panics → JSON 500
 [CorsLayer]           -- validation CORS (le plus proche du handler)
        |
        v
   Handler Axum
```

**Implication** : `TraceLayer` voit toutes les requetes, y compris celles rejetees par CORS. Les panics dans le handler sont capturees par `CatchPanicLayer` et converties en reponse JSON 500 propre.

### 1.4 Sequence de demarrage (`serve`)

```
serve(addr)
    |
    |-- 1. build_inner() → Router + Hooks + ConsumerRegs + State
    |
    |-- 2. Enregistrement des consumers d'evenements
    |       Pour chaque controller avec #[consumer] :
    |         → Controller::register_consumers(state.clone())
    |         → Subscribe les handlers sur l'EventBus
    |
    |-- 3. Execution des hooks on_start (dans l'ordre d'enregistrement)
    |       Chaque hook recoit state.clone()
    |       Un hook qui echoue arrete le demarrage
    |
    |-- 4. Binding TCP sur l'adresse
    |
    |-- 5. axum::serve() avec graceful shutdown
    |       Le serveur accepte des connexions
    |       En arriere-plan : taches planifiees actives
    |
    |-- 6. Signal d'arret (Ctrl-C / SIGTERM)
    |       → Arret des nouvelles connexions
    |       → Attente de la fin des requetes en cours
    |
    |-- 7. Execution des hooks on_stop (dans l'ordre d'enregistrement)
    |
    └-- 8. Arret
```

### 1.5 Arret gracieux

Le scheduler utilise un `CancellationToken` (de `tokio-util`) qui est annule dans un hook `on_stop` enregistre par `with_scheduler`. Chaque tache planifiee surveille ce token via `tokio::select!` et s'arrete proprement.

Les requetes HTTP en cours sont completees avant la fermeture (comportement par defaut d'Axum).

### 1.6 Grace period de shutdown

Par defaut le processus attend indefiniment la fin des hooks de shutdown. `shutdown_grace_period(Duration)` definit un delai maximum :

```rust
AppBuilder::new()
    .with_state(services)
    .shutdown_grace_period(Duration::from_secs(5))
    .serve("0.0.0.0:3000").await?;
```

Si les hooks (plugin + utilisateur) ne terminent pas dans le delai, le processus force l'arret via `process::exit(1)`. Cela garantit qu'un hook bloquant ne laisse pas le processus suspendu indefiniment.

---

## 2. Cycle de vie d'une requete HTTP

### 2.1 Vue d'ensemble

```
Requete HTTP
    |
    v
[Layers Tower]  ← TraceLayer → CatchPanicLayer → CorsLayer
    |
    v
[Routage Axum]  ← correspondance path + method
    |
    v
[Extraction]    ← pipeline d'extracteurs Axum
    |
    +-- State (si handler guarde)
    +-- HeaderMap (si handler guarde)
    +-- __R2eExtract_<Name>  ← construction du controller
    |       +-- #[inject(identity)] : FromRequestParts (async)
    |       +-- #[inject]           : state.field.clone() (sync)
    |       +-- #[config("key")]    : config.get(key) (sync)
    +-- Params handler (Json, Path, Query, etc.)
    +-- #[inject(identity)] param (si param-level)
    |
    v
[Guards]        ← execution sequentielle, short-circuit sur erreur
    |
    +-- RateLimitGuard → 429 Too Many Requests
    +-- RolesGuard     → 403 Forbidden
    +-- Custom Guards  → reponse custom
    |
    v
[Intercepteurs] ← chain around() monomorphisee
    |
    +-- Logged (entering)
    +-- Timed (start)
    +-- User interceptors
    +-- Cache (lookup)
    |       +-- Hit  → retour immediat
    |       +-- Miss → continue
    |
    v
[Corps du handler]
    |
    +-- transactional (begin tx)
    +-- logique metier
    +-- transactional (commit/rollback)
    |
    v
[Post-traitement]
    |
    +-- Cache (store si miss)
    +-- CacheInvalidate (clear group)
    +-- Timed (log elapsed)
    +-- Logged (exiting)
    |
    v
Reponse HTTP
```

### 2.2 Extraction du controller

L'extracteur genere `__R2eExtract_<Name>` implemente `FromRequestParts<State>`. Il construit le controller en trois phases :

**Phase 1 — Identity (async, faillible)**

```rust
let user = <AuthenticatedUser as FromRequestParts<State>>
    ::from_request_parts(parts, state)
    .await
    .map_err(IntoResponse::into_response)?;
```

C'est la seule phase asynchrone. Pour `AuthenticatedUser`, cela implique :
- Extraction du header `Authorization: Bearer <token>`
- Validation JWT (signature cryptographique)
- Lookup JWKS si la cle n'est pas en cache (potentiellement un appel reseau)
- Construction de l'objet `AuthenticatedUser`

Si l'extraction echoue, la requete est immediatement rejetee (401).

**Phase 2 — Inject (sync, infaillible)**

```rust
user_service: state.user_service.clone(),
pool: state.pool.clone(),
```

Chaque champ `#[inject]` est clone depuis l'etat. Operation purement synchrone.

**Phase 3 — Config (sync, panic si absent)**

```rust
greeting: {
    let cfg = <R2eConfig as FromRef<State>>::from_ref(state);
    cfg.get("app.greeting").unwrap_or_else(|e| panic!(...))
}
```

Extraction de `R2eConfig` depuis l'etat via `FromRef`, puis lookup dans le `HashMap`.

### 2.3 Deux modes de handler

**Mode simple** (sans guards) — le handler retourne directement le type de la methode :

```rust
async fn __r2e_UserController_list(
    ctrl_ext: __R2eExtract_UserController,
    // ... params
) -> Json<Vec<User>> {
    let ctrl = ctrl_ext.0;
    ctrl.list().await
}
```

**Mode guarde** (avec `#[roles]`, `#[rate_limited]`, `#[guard]`) — le handler retourne `Response` pour permettre le short-circuit :

```rust
async fn __r2e_UserController_admin_list(
    State(state): State<Services>,
    headers: HeaderMap,
    ctrl_ext: __R2eExtract_UserController,
) -> Response {
    let guard_ctx = GuardContext {
        method_name: "admin_list",
        controller_name: "UserController",
        headers: &headers,
        identity: guard_identity(&ctrl_ext.0),  // Option<&AuthenticatedUser>
    };

    // Short-circuit si le guard echoue
    if let Err(resp) = Guard::check(&RolesGuard { required_roles: &["admin"] }, &state, &guard_ctx) {
        return resp;
    }

    let ctrl = ctrl_ext.0;
    IntoResponse::into_response(ctrl.admin_list().await)
}
```

**Implications** : en mode guarde, Axum extrait aussi `State` et `HeaderMap` en plus de l'extracteur du controller. L'extraction de l'etat est un clone supplementaire (mais cheap — c'est un clone de `Arc` interne).

---

## 3. Injection de dependances : les trois scopes

### 3.1 `#[inject]` — Scope applicatif

| Propriete | Valeur |
|-----------|--------|
| Resolution | Compile-time (codegen) |
| Moment | A chaque requete |
| Operation | `state.field.clone()` |
| Prerequis | `Clone + Send + Sync` |
| Faillible | Non |
| Async | Non |

**Code genere :**
```rust
field_name: __state.field_name.clone()
```

**Patterns courants :**

| Type | Cout du clone | Mecanisme |
|------|--------------|-----------|
| `Arc<T>` | O(1) — increment atomique du refcount | Partage immutable |
| `SqlxPool` | O(1) — `Arc` interne | Pool de connexions |
| `EventBus` | O(1) — `Arc<RwLock<HashMap>>` | Bus d'evenements |
| `RateLimitRegistry` | O(1) — `Arc` interne | Registre de limiteurs |
| `R2eConfig` | O(n) — clone du `HashMap` | Configuration |

**Bonne pratique** : envelopper les services lourds dans `Arc<T>` pour que le clone soit un simple increment de reference atomique. Le framework n'impose pas `Arc`, mais les types fournis (`SqlxPool`, `EventBus`, etc.) l'utilisent deja en interne.

### 3.2 `#[inject(identity)]` — Scope requete

| Propriete | Valeur |
|-----------|--------|
| Resolution | Compile-time (codegen) |
| Moment | A chaque requete |
| Operation | `FromRequestParts::from_request_parts()` |
| Prerequis | `FromRequestParts<State>` + `Identity` |
| Faillible | Oui (reponse d'erreur) |
| Async | Oui |

**Code genere :**
```rust
let user = <AuthenticatedUser as FromRequestParts<State>>
    ::from_request_parts(__parts, __state)
    .await
    .map_err(IntoResponse::into_response)?;
```

**Deux emplacements possibles :**

- **Sur le struct** — le controller requiert toujours l'identity. Pas de `StatefulConstruct`.
- **Sur un parametre handler** — seuls les handlers annotes requierent l'identity. `StatefulConstruct` est genere.

**Cout** : c'est le scope le plus cher. Pour `AuthenticatedUser`, chaque requete implique une validation JWT avec verification de signature cryptographique.

### 3.3 `#[config("key")]` — Scope applicatif (lookup)

| Propriete | Valeur |
|-----------|--------|
| Resolution | Compile-time (codegen) |
| Moment | A chaque requete |
| Operation | `FromRef` + `HashMap::get()` |
| Prerequis | `FromConfigValue` |
| Faillible | Panic si cle absente |
| Async | Non |

**Code genere :**
```rust
field_name: {
    let __cfg = <R2eConfig as FromRef<State>>::from_ref(__state);
    __cfg.get("app.greeting").unwrap_or_else(|e| panic!(...))
}
```

**Attention** : la config est clonee depuis l'etat (via `FromRef`), puis une lookup HashMap est effectuee. Si la cle n'existe pas, le handler **panic** (et `CatchPanicLayer` convertit en 500).

### 3.4 Schema recapitulatif

```
                    ┌─────────────────────────────────────────────┐
                    │           Etat applicatif (State)            │
                    │                                             │
                    │  user_service: UserService  ←── Arc interne │
                    │  pool: SqlitePool           ←── Arc interne │
                    │  jwt_validator: Arc<JwtValidator>            │
                    │  event_bus: EventBus         ←── Arc interne │
                    │  config: R2eConfig       ←── HashMap     │
                    │  rate_limiter: RateLimitRegistry             │
                    └──────────────┬──────────────────────────────┘
                                   │
                    ┌──────────────┴──────────────────────────────┐
                    │         Requete HTTP entrante                │
                    └──────────────┬──────────────────────────────┘
                                   │
            ┌──────────────────────┼──────────────────────────┐
            │                      │                          │
    #[inject]              #[inject(identity)]         #[config("key")]
    state.field.clone()    FromRequestParts(async)     config.get(key)
    ↓                      ↓                           ↓
    O(1) si Arc            Validation JWT              O(1) HashMap
    Sync, infaillible      Async, faillible (401)      Sync, panic si absent
```

---

## 4. Construction hors contexte HTTP : `StatefulConstruct`

Le trait `StatefulConstruct<S>` permet de construire un controller depuis l'etat seul, sans requete HTTP. Il est genere automatiquement par `#[derive(Controller)]` **uniquement** quand le struct n'a pas de champ `#[inject(identity)]`.

### 4.1 Utilisation par les consumers

```rust
// Code genere par #[routes] pour #[consumer(bus = "event_bus")]
event_bus.subscribe(move |event: Arc<UserCreatedEvent>| {
    let state = state.clone();
    async move {
        let ctrl = <MyController as StatefulConstruct<State>>::from_state(&state);
        ctrl.on_user_created(event).await;
    }
}).await;
```

### 4.2 Utilisation par les taches planifiees

```rust
// Code genere par #[routes] pour #[scheduled(every = 30)]
scheduler.add_task(ScheduledTask {
    name: "MyController_cleanup",
    schedule: Schedule::Every(Duration::from_secs(30)),
    task: Box::new(move |state: State| {
        Box::pin(async move {
            let ctrl = <MyController as StatefulConstruct<State>>::from_state(&state);
            ctrl.cleanup().await;
        })
    }),
});
```

### 4.3 Le pattern controller mixte

Avec `#[inject(identity)]` sur les **parametres** handler (et non sur le struct), le controller conserve `StatefulConstruct` tout en permettant des endpoints proteges :

```rust
#[derive(Controller)]
#[controller(path = "/api", state = Services)]
pub struct MixedController {
    #[inject] user_service: UserService,
    // Pas de #[inject(identity)] ici → StatefulConstruct genere
}

#[routes]
impl MixedController {
    #[get("/public")]
    async fn public_data(&self) -> Json<Vec<Data>> { ... }

    #[get("/me")]
    async fn me(&self, #[inject(identity)] user: AuthenticatedUser) -> Json<AuthenticatedUser> {
        Json(user)
    }

    #[scheduled(every = 60)]
    async fn cleanup(&self) { ... }  // Fonctionne car StatefulConstruct existe
}
```

---

## 5. Guards et le trait `Identity`

### 5.1 Architecture

```rust
// r2e-core
pub trait Identity: Send + Sync {
    fn sub(&self) -> &str;
    fn roles(&self) -> &[String];
}

pub struct GuardContext<'a, I: Identity> {
    pub method_name: &'static str,
    pub controller_name: &'static str,
    pub headers: &'a HeaderMap,
    pub identity: Option<&'a I>,
}

pub trait Guard<S, I: Identity>: Send + Sync {
    fn check(&self, state: &S, ctx: &GuardContext<'_, I>) -> Result<(), Response>;
}
```

Le trait `Identity` decouple les guards du type concret `AuthenticatedUser`. Les guards built-in (`RolesGuard`, `RateLimitGuard`) sont generiques sur `I: Identity`.

### 5.2 Source de l'identity pour les guards

Deux cas dans le code genere :

**Cas A** — Identity sur un parametre handler :

```rust
// Le param est deja extrait par Axum
let guard_ctx = GuardContext {
    identity: Some(&__arg_0),  // reference directe au param
    ...
};
```

**Cas B** — Identity sur le struct (ou absente) :

```rust
// Appel a la fonction du meta-module
let guard_ctx = GuardContext {
    identity: __r2e_meta_Name::guard_identity(&ctrl_ext.0),
    ...
};
```

Quand il n'y a pas d'identity du tout, `guard_identity` retourne `None` et le type est `NoIdentity`. Un guard comme `RolesGuard` retourne alors 403 "No identity available for role check".

---

## 6. Implications de performance

### 6.1 Cout par requete — decomposition

| Etape | Type | Cout typique | Notes |
|-------|------|-------------|-------|
| Layers Tower | Sync | ~1 us | Tracing, CORS, error handling |
| Routage Axum | Sync | ~1 us | Radix tree matching |
| **Clone des champs `#[inject]`** | Sync | **~10-50 ns par champ** | Si types `Arc` (refcount atomique) |
| **Lookup config** | Sync | **~50 ns par champ** | HashMap lookup + type conversion |
| **Validation JWT** | Async | **~10-50 us** | Verification de signature cryptographique |
| **Lookup JWKS (cache miss)** | Async | **~50-200 ms** | Appel HTTP au provider OIDC |
| Guard rate limit | Sync | ~100 ns | Token bucket check |
| Guard roles | Sync | ~50 ns | Iteration sur le tableau de roles |
| Intercepteurs | Async | ~100 ns d'overhead | Monomorphises, zero vtable |
| Logique metier | Async | Variable | I/O database, services externes |

### 6.2 Les operations critiques en detail

#### Clone de l'etat (`#[inject]`)

Le clone se fait a chaque requete pour chaque champ `#[inject]`. C'est le mecanisme d'Axum : l'extracteur `FromRequestParts` recoit une reference immutable a l'etat et doit en produire une copie locale.

**Recommandation** : utiliser `Arc<T>` pour les services couteux a cloner. Le framework le fait deja pour `SqlxPool`, `EventBus`, et `RateLimitRegistry`.

```rust
// Bon : Arc<T> → clone O(1)
#[derive(Clone)]
pub struct Services {
    pub user_service: UserService,       // contient Arc<RwLock<Vec<User>>>
    pub jwt_validator: Arc<JwtValidator>,
    pub pool: SqlitePool,                // Arc interne
}

// Mauvais : si UserService contenait Vec<User> directement → clone O(n) par requete
```

**Anti-pattern** : stocker `R2eConfig` comme champ `#[inject]` plutot que via `#[config]`. Le `R2eConfig` est un `HashMap<String, ConfigValue>` — son clone copie l'integralite de la map a chaque requete. Preferer `#[config("key")]` qui ne clone que la valeur demandee, ou stocker la config comme `Arc<R2eConfig>`.

#### Validation JWT (`#[inject(identity)]`)

C'est generalement l'operation la plus couteuse de l'extraction. Elle comprend :

1. **Parsing du header** — O(1), negligeable
2. **Decode du JWT** — parsing base64 + JSON, ~1 us
3. **Verification de signature** — RSA/ECDSA, ~10-50 us selon l'algorithme
4. **Lookup de la cle** (JWKS mode) :
   - Cache hit : lecture RwLock, ~100 ns
   - Cache miss : requete HTTP au JWKS endpoint, ~50-200 ms
5. **Construction d'`AuthenticatedUser`** — allocation, negligeable

**Optimisations possibles** :

- **Pre-rechauffer le cache JWKS** dans un hook `on_start` (premiere requete sans latence)
- **Cle statique en dev** (`JwtValidator::new_with_static_key`) evite le JWKS entierement
- **Le cache JWKS est partage** via `Arc<RwLock>` — un seul refresh meme sous charge

**Struct-level vs param-level** : quand l'identity est sur le struct, elle est extraite pour **toutes** les requetes vers ce controller, meme les endpoints qui n'en ont pas besoin. Le pattern param-level (`#[inject(identity)]` sur le param) permet d'eviter cette extraction pour les endpoints publics.

#### Lookup de configuration (`#[config]`)

Chaque champ `#[config("key")]` effectue :

1. `FromRef` extraction de `R2eConfig` — clone du `HashMap`, O(n) ou n = nombre de cles
2. `config.get(key)` — O(1) lookup + conversion de type

**Le clone de la config est le point d'attention**. Si la config contient 100 cles, c'est 100 allocations par champ `#[config]` par requete.

**Recommandation** : pour les controllers a forte charge, preferer injecter les valeurs de config dans l'etat au demarrage plutot que via `#[config]` :

```rust
// Plutot que :
#[config("app.greeting")] greeting: String,

// Considerer :
#[inject] greeting: Arc<String>,  // pre-construit dans l'etat
```

### 6.3 Intercepteurs : zero-cost abstraction

Les intercepteurs utilisent le trait `Interceptor<R>` qui est **monomorphise** par le compilateur Rust :

```rust
pub trait Interceptor<R> {
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F)
        -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send;
}
```

- **Pas de `dyn` dispatch** — le type concret de l'intercepteur est connu au compile-time
- **Imbrication en closures** — LLVM optimise les closures nested en code lineaire
- **`InterceptorContext` est `Copy`** — capture par valeur dans chaque closure async

Le cout reel des intercepteurs est celui de leur **logique metier** (logging, timing, cache lookup), pas du mecanisme d'`around`.

### 6.4 Guards : execution synchrone

Les guards sont executes de maniere synchrone dans un handler async. Ils ne bloquent pas le runtime Tokio car ils sont typiquement O(1) :

- `RolesGuard` — iteration sur un petit slice de roles
- `RateLimitGuard` — acces a un `DashMap` (lock-free pour les lectures)

**Attention** : un guard custom qui ferait de l'I/O bloquerait le runtime. Les guards doivent rester rapides et synchrones.

### 6.5 Comparaison struct-level vs param-level identity

| Aspect | Struct-level `#[inject(identity)]` | Param-level `#[inject(identity)]` |
|--------|-----------------------------------|----------------------------------|
| Extraction JWT | A chaque requete, tous endpoints | Seulement endpoints annotes |
| `StatefulConstruct` | Non genere | Genere |
| Consumers / Schedulers | Impossible | Possible |
| Acces identity dans self | `self.user` (toujours disponible) | Non disponible dans self |
| Guard context | Via `guard_identity(&ctrl)` | Via reference au param |
| Overhead endpoint public | Validation JWT inutile | Aucun overhead JWT |

**Recommandation** : utiliser le pattern param-level pour les controllers qui melangent endpoints publics et proteges. Reserver le struct-level pour les controllers entierement proteges ou l'identity est utilisee dans la majorite des methodes.

### 6.6 Taches planifiees : cout de construction

Chaque execution d'une tache planifiee appelle `StatefulConstruct::from_state`, qui clone les champs `#[inject]` et lookup les champs `#[config]`. Pour les taches a haute frequence (e.g., `every = 1`), ce cout est identique a celui d'une requete HTTP (moins l'extraction identity).

**Recommandation** : pour les taches a tres haute frequence, reduire le nombre de champs injectes au minimum necessaire.

---

## 7. Resume des regles d'or

1. **Envelopper les services dans `Arc<T>`** — le clone par requete devient un simple increment atomique
2. **Preferer `#[inject(identity)]` param-level** pour les controllers mixtes — evite la validation JWT sur les endpoints publics
3. **Limiter le nombre de champs `#[config]`** — chaque champ clone l'ensemble de `R2eConfig`
4. **Pre-rechauffer le cache JWKS** au demarrage si le temps de premiere requete compte
5. **Les intercepteurs sont gratuits** en termes d'overhead de dispatch — le cout est dans leur logique interne
6. **Les guards doivent rester synchrones et O(1)** — pas d'I/O dans un guard
7. **Un controller par responsabilite** — evite d'injecter des dependances inutiles qui sont clonees a chaque requete

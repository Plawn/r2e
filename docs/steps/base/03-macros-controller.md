# Etape 3 — r2e-macros : macro `#[controller]`

## Objectif

Implementer la macro `#[controller]` — la piece maitresse du framework. Elle transforme un `impl` block annote en handlers Axum complets avec extraction de state et de donnees request-scoped.

## Fichiers a creer/modifier

```
r2e-macros/src/
  lib.rs              # Ajout de #[controller]
  controller.rs       # Logique principale de la macro
  parsing.rs          # Extraction des champs inject/identity et des methodes routes
  codegen.rs          # Generation de code (handlers + impl Controller)
```

## 1. Vue d'ensemble du pipeline

```
Input:                              Output:
#[controller]                       1. Struct originale (inchangee)
impl UserResource {                 2. impl UserResource { methodes originales }
    #[inject]                       3. Handlers Axum libres (fonctions async)
    user_service: UserService,      4. impl Controller<Services> for UserResource
                                    5. fn routes() -> Router
    #[identity]
    user: AuthenticatedUser,

    #[get("/users")]
    async fn list(&self) -> ...
}
```

## 2. Phase de parsing (`parsing.rs`)

### Entree

La macro recoit un `impl` block complet via `syn::parse`.

### Donnees a extraire

```rust
pub struct ControllerDef {
    /// Nom du type (ex: UserResource)
    pub name: syn::Ident,

    /// Champs #[inject] — app-scoped
    pub injected_fields: Vec<InjectedField>,

    /// Champs #[identity] — request-scoped
    pub identity_fields: Vec<IdentityField>,

    /// Methodes annotees avec un attribut de route
    pub route_methods: Vec<RouteMethod>,

    /// Methodes non-annotees (helpers prives, etc.)
    pub other_methods: Vec<syn::ImplItemFn>,
}

pub struct InjectedField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct IdentityField {
    pub name: syn::Ident,
    pub ty: syn::Type,
}

pub struct RouteMethod {
    pub method: HttpMethod,    // Get, Post, Put, Delete, Patch
    pub path: String,          // "/users/:id"
    pub fn_item: syn::ImplItemFn,
}
```

### Logique de parsing

1. **Identifier le type** : extraire `Self` type du `impl` block
2. **Classifier les items** :
   - Item avec `#[inject]` → `InjectedField`
   - Item avec `#[identity]` → `IdentityField`
   - Methode avec `#[get(...)]` / `#[post(...)]` / etc. → `RouteMethod`
   - Autre methode → `other_methods`

### Gestion des champs dans un `impl` block

Probleme : Rust standard ne permet pas de declarer des champs dans un `impl` block. Deux strategies :

**Strategie A** — Syntaxe custom dans le `impl` block :
La macro parse des declarations `field: Type` dans le bloc et les retire du code genere. L'utilisateur ecrit :

```rust
#[controller]
impl UserResource {
    #[inject]
    user_service: UserService,
    // ...
}
```

La macro doit parser ces items manuellement car `syn` ne les reconnaitra pas comme des `ImplItem` valides. Utiliser `syn::parse::Parse` custom.

**Strategie B (alternative)** — Struct separee + attributs :
L'utilisateur definit la struct normalement et `#[controller]` s'applique sur le `impl`. Les injections sont declarees sur la struct :

```rust
#[injectable]
struct UserResource {
    #[inject]
    user_service: UserService,
    #[identity]
    user: AuthenticatedUser,
}

#[controller]
impl UserResource {
    #[get("/users")]
    async fn list(&self) -> Json<Vec<User>> { ... }
}
```

> **Recommandation** : Strategie A pour rester fidele a la DX cible du plan, meme si elle requiert un parsing custom.

## 3. Phase de generation de code (`codegen.rs`)

### Pour chaque `RouteMethod`, generer un handler Axum

Entree :
```rust
#[get("/users")]
async fn list(&self) -> Json<Vec<User>>
```

Sortie :
```rust
async fn __r2e_handler_list(
    axum::extract::State(state): axum::extract::State<AppState<Services>>,
    user: AuthenticatedUser,    // chaque #[identity] field
) -> impl axum::response::IntoResponse {
    let controller = UserResource {
        user_service: state.get().user_service.clone(),  // chaque #[inject] field
        user,                                             // chaque #[identity] field
    };
    controller.list().await
}
```

### Regles de generation

| Source | Dans le handler |
|--------|----------------|
| `#[inject] foo: Foo` | `foo: state.get().foo.clone()` |
| `#[identity] bar: Bar` | Parametre d'extraction : `bar: Bar` |
| Parametres de la methode (hors `&self`) | Parametres d'extraction supplementaires (ex: `Path(id): Path<u64>`, `Json(body): Json<T>`) |

### Generer `impl Controller<T>` + `fn routes()`

```rust
impl Controller<Services> for UserResource {
    fn routes() -> axum::Router<AppState<Services>> {
        axum::Router::new()
            .route("/users", axum::routing::get(__r2e_handler_list))
            .route("/users/:id", axum::routing::get(__r2e_handler_get_by_id))
            // ...
    }
}
```

### Le type `Services` (AppState inner)

Le type concret de l'AppState inner doit etre connu par la macro. Deux options :

**Option 1** — Parametre de la macro : `#[controller(state = Services)]`
**Option 2** — Convention de nommage / type alias global
**Option 3** — Inference depuis les champs `#[inject]` (complexe)

> **Recommandation** : Option 1 pour la clarte.

## 4. Gestion des parametres de methode

Les methodes de controller peuvent avoir des parametres au-dela de `&self` :

```rust
#[get("/users/:id")]
async fn get_by_id(&self, Path(id): Path<u64>) -> Json<User>
```

Ces parametres doivent etre **transferes tels quels** comme parametres du handler Axum genere. La macro doit :

1. Retirer `&self` de la liste des parametres
2. Conserver tous les autres parametres comme parametres du handler
3. Ajouter les parametres d'extraction identity en plus

## 5. Integration dans `lib.rs`

```rust
mod controller;
mod parsing;
mod codegen;

#[proc_macro_attribute]
pub fn controller(args: TokenStream, input: TokenStream) -> TokenStream {
    controller::expand(args, input)
}

// Macros auxiliaires
#[proc_macro_attribute]
pub fn inject(_args: TokenStream, input: TokenStream) -> TokenStream {
    input // no-op, lu par #[controller]
}

#[proc_macro_attribute]
pub fn identity(_args: TokenStream, input: TokenStream) -> TokenStream {
    input // no-op, lu par #[controller]
}
```

## Critere de validation

Test de compilation end-to-end (sans serveur) :

```rust
use r2e_macros::{controller, inject, identity, get};
use r2e_core::{AppState, Controller};

#[derive(Clone)]
struct Services {
    greeting: String,
}

struct HelloController;

#[controller(state = Services)]
impl HelloController {
    #[inject]
    greeting: String,

    #[get("/hello")]
    async fn hello(&self) -> String {
        self.greeting.clone()
    }
}

// Verifie que Controller est implemente
let router = HelloController::routes();
```

## Dependances entre etapes

- Requiert : etape 0, etape 1 (AppState, Controller trait), etape 2 (attributs de route)
- Bloque : etape 5 (example-app)

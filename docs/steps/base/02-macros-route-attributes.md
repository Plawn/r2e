# Etape 2 — r2e-macros : attributs de route

## Objectif

Implementer les macros d'attribut `#[get]`, `#[post]`, `#[put]`, `#[delete]`, `#[patch]`. Ces macros ne transforment pas le code elles-memes — elles **marquent** les methodes pour que la macro `#[controller]` (etape 3) puisse les identifier et generer les handlers Axum.

## Fichiers a creer/modifier

```
r2e-macros/src/
  lib.rs             # Point d'entree proc-macro, declare les attributs
  route.rs           # Parsing des attributs de route
```

## 1. Attributs de route (`route.rs`)

Chaque attribut de route capture le **path HTTP** et le **method HTTP**.

### Parsing

```rust
pub struct RouteAttribute {
    pub method: HttpMethod,
    pub path: String,
}

pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
}
```

Le path est extrait depuis les arguments de l'attribut :

```rust
// #[get("/users/:id")]  →  RouteAttribute { method: Get, path: "/users/:id" }
```

### Implementation des macros

Chaque macro (`#[get]`, `#[post]`, etc.) est un **attribute proc-macro** qui :

1. Parse le path depuis `TokenStream` d'arguments
2. Annote la methode avec un attribut custom reconnaissable (par ex. `#[r2e_route(method = "GET", path = "/users")]`)
3. Retourne la methode inchangee (la transformation reelle est faite par `#[controller]`)

```rust
#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    route_attribute(HttpMethod::Get, args, input)
}

#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    route_attribute(HttpMethod::Post, args, input)
}

// ... idem pour put, delete, patch
```

### Strategie d'annotation

**Option A** — Attribut inerte : les macros `#[get]` etc. re-emettent la methode avec un attribut `#[doc(hidden)]` contenant le metadata route encodee. La macro `#[controller]` lit ensuite cet attribut.

**Option B (recommandee)** — Pas de transformation : les macros `#[get]` etc. sont des **no-op** purs. La macro `#[controller]` parse directement les attributs `get`, `post`, etc. sur les methodes du `impl` block.

L'option B est plus simple car `#[controller]` recoit le `impl` block complet avec tous les attributs intact.

## 2. Point d'entree (`lib.rs`)

```rust
extern crate proc_macro;
use proc_macro::TokenStream;

mod route;

#[proc_macro_attribute]
pub fn get(args: TokenStream, input: TokenStream) -> TokenStream {
    // No-op : retourne input tel quel
    // Le path est lu par #[controller]
    input
}

#[proc_macro_attribute]
pub fn post(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn put(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn delete(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn patch(args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
```

> **Note** : Si les macros sont des no-op, elles doivent quand meme exister comme proc-macro attributes pour que le compilateur ne rejette pas `#[get("/path")]` comme attribut inconnu.

## Critere de validation

```rust
use r2e_macros::get;

struct Foo;

impl Foo {
    #[get("/hello")]
    async fn hello(&self) -> String {
        "hello".to_string()
    }
}
```

Compile sans erreur (l'attribut est accepte et ne modifie pas la methode).

## Dependances entre etapes

- Requiert : etape 0
- Bloque : etape 3 (macro controller)

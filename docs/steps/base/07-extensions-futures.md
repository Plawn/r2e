# Etape 7 — Extensions futures (hors scope v0.1)

## Objectif

Documenter les extensions envisagees pour les versions suivantes. Ces fonctionnalites ne sont **pas bloquantes** pour le livrable initial.

---

## 1. `#[roles("admin", "manager")]`

### Description

Macro d'attribut sur une methode de controller qui restreint l'acces aux utilisateurs ayant au moins un des roles specifies.

### Implementation envisagee

```rust
#[get("/admin/users")]
#[roles("admin")]
async fn admin_list(&self) -> Json<Vec<User>> { ... }
```

La macro `#[controller]` genere un guard supplementaire dans le handler :

```rust
if !user.roles.iter().any(|r| ["admin"].contains(&r.as_str())) {
    return Err(AppError::Forbidden("Insufficient roles".into()));
}
```

### Complexite

Faible — ajout d'un attribut supplementaire a parser dans la macro controller.

---

## 2. `#[transactional]`

### Description

Enveloppe l'execution de la methode dans une transaction SQL. Commit automatique en cas de succes, rollback en cas d'erreur.

### Prerequis

- Integration SQLx dans `quarlus-core`
- Pool de connexion dans l'AppState
- Trait `Transactional` pour les services

### Implementation envisagee

```rust
#[post("/users")]
#[transactional]
async fn create(&self, Json(body): Json<CreateUser>) -> Json<User> {
    // Tout est dans une transaction
    self.user_service.create(&body).await?
}
```

### Complexite

Moyenne — necessite de passer un `Transaction` ou `&mut PgConnection` aux services.

---

## 3. `#[config("app.database.url")]`

### Description

Injection de valeurs de configuration au compile-time ou au runtime depuis un fichier `application.yaml` ou des variables d'environnement.

### Implementation envisagee

```rust
#[controller(state = Services)]
impl MyController {
    #[config("app.greeting")]
    greeting: String,

    #[get("/hello")]
    async fn hello(&self) -> String {
        self.greeting.clone()
    }
}
```

### Complexite

Moyenne — necessite un systeme de configuration (serde_yaml, dotenv, etc.) integre a l'AppBuilder.

---

## 4. Generation OpenAPI automatique

### Description

Generer une spec OpenAPI 3.x a partir des controllers annotes.

### Implementation envisagee

- Extraire les routes, methodes HTTP, types de requete/reponse
- Generer un `openapi.json` ou le servir sur `/openapi.json`
- Integrer une interface de documentation API sur `/docs`

### Complexite

Elevee — necessite l'introspection des types Serde pour generer les schemas JSON.

---

## 5. Dev mode / Hot reload

### Description

Recompilation et redemarrage automatique lors de modifications de fichiers source.

### Implementation envisagee

- Utilisation de `cargo-watch` ou `watchexec` en externe
- Ou integration d'un watcher dans le binaire de dev

### Complexite

Faible si externe (juste de la documentation), elevee si integre.

---

## 6. Middleware custom declaratif

### Description

Permettre de declarer des middlewares Tower via des macros :

```rust
#[middleware]
async fn log_request(req: Request, next: Next) -> Response {
    println!("→ {} {}", req.method(), req.uri());
    let response = next.run(req).await;
    println!("← {}", response.status());
    response
}
```

### Complexite

Moyenne — wrapper autour des layers Tower.

---

## Priorite suggeree

| Extension | Priorite | Effort |
|-----------|----------|--------|
| `#[roles]` | Haute | Faible |
| `#[config]` | Haute | Moyen |
| `#[transactional]` | Moyenne | Moyen |
| OpenAPI | Moyenne | Eleve |
| Middleware custom | Basse | Moyen |
| Hot reload | Basse | Faible (externe) |

# Feature 6 — Data / Repository

## Objectif

Fournir des abstractions pour l'acces aux donnees : trait `Entity` pour modeliser les tables, `QueryBuilder` pour construire des requetes SQL de maniere fluide, et `Pageable`/`Page<T>` pour la pagination.

## Concepts cles

### Entity

Trait representant une entite persistee en base. Definit le nom de la table, les colonnes, et la colonne d'identifiant.

### QueryBuilder

Builder fluide pour construire des requetes SELECT avec conditions, tri, et pagination.

### Pageable / Page\<T\>

`Pageable` est un struct extractible depuis les query params pour la pagination. `Page<T>` est le conteneur de reponse paginee.

## Utilisation

### 1. Ajouter la dependance

```toml
[dependencies]
quarlus-data = { path = "../quarlus-data", features = ["sqlite"] }
```

### 2. Definir une entite

```rust
use quarlus_data::Entity;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct UserEntity {
    pub id: i64,
    pub name: String,
    pub email: String,
}

impl Entity for UserEntity {
    type Id = i64;

    fn table_name() -> &'static str {
        "users"
    }

    fn id_column() -> &'static str {
        "id"
    }

    fn columns() -> &'static [&'static str] {
        &["id", "name", "email"]
    }

    fn id(&self) -> &i64 {
        &self.id
    }
}
```

### 3. Utiliser QueryBuilder

```rust
use quarlus_data::QueryBuilder;

// Requete simple
let (sql, params) = QueryBuilder::new("users")
    .build_select("*");
// → "SELECT * FROM users", []

// Requete avec conditions
let (sql, params) = QueryBuilder::new("users")
    .where_eq("email", "alice@example.com")
    .where_like("name", "%ali%")
    .order_by("id", true)
    .limit(10)
    .offset(20)
    .build_select("id, name, email");
// → "SELECT id, name, email FROM users WHERE email = ? AND name LIKE ? ORDER BY id ASC LIMIT 10 OFFSET 20"
// → params: ["alice@example.com", "%ali%"]

// Requete COUNT
let (sql, params) = QueryBuilder::new("users")
    .where_eq("active", "true")
    .build_count();
// → "SELECT COUNT(*) FROM users WHERE active = ?"
```

### Methodes de condition disponibles

| Methode | SQL genere | Exemple |
|---------|-----------|---------|
| `where_eq(col, val)` | `col = ?` | `.where_eq("status", "active")` |
| `where_not_eq(col, val)` | `col != ?` | `.where_not_eq("role", "admin")` |
| `where_like(col, pat)` | `col LIKE ?` | `.where_like("name", "%alice%")` |
| `where_gt(col, val)` | `col > ?` | `.where_gt("age", "18")` |
| `where_lt(col, val)` | `col < ?` | `.where_lt("price", "100")` |
| `where_in(col, vals)` | `col IN (?, ?, ...)` | `.where_in("id", &["1", "2", "3"])` |
| `where_null(col)` | `col IS NULL` | `.where_null("deleted_at")` |
| `where_not_null(col)` | `col IS NOT NULL` | `.where_not_null("email")` |

### 4. Pagination avec Pageable et Page\<T\>

`Pageable` s'extrait depuis les query params :

```rust
use axum::extract::Query;
use quarlus_data::{Pageable, Page};

#[get("/data/users")]
async fn list(
    &self,
    Query(pageable): Query<Pageable>,
) -> Result<axum::Json<Page<UserEntity>>, quarlus_core::AppError> {
    let qb = QueryBuilder::new("users")
        .order_by("id", true)
        .limit(pageable.size)
        .offset(pageable.offset());

    let (sql, _params) = qb.build_select("id, name, email");
    let rows: Vec<UserEntity> = sqlx::query_as(&sql)
        .fetch_all(&self.pool)
        .await?;

    let (count_sql, _) = QueryBuilder::new("users").build_count();
    let total: (i64,) = sqlx::query_as(&count_sql)
        .fetch_one(&self.pool)
        .await?;

    Ok(axum::Json(Page::new(rows, &pageable, total.0 as u64)))
}
```

### Parametres de Pageable

| Parametre | Type | Defaut | Description |
|-----------|------|--------|-------------|
| `page` | `u64` | `0` | Numero de page (commence a 0) |
| `size` | `u64` | `20` | Nombre d'elements par page |
| `sort` | `Option<String>` | `None` | Champ de tri (optionnel) |

### Exemple de requete HTTP

```bash
curl "http://localhost:3000/data/users?page=0&size=10"
```

### Reponse Page\<T\>

```json
{
    "content": [
        {"id": 1, "name": "Alice", "email": "alice@example.com"},
        {"id": 2, "name": "Bob", "email": "bob@example.com"}
    ],
    "page": 0,
    "size": 10,
    "total_elements": 3,
    "total_pages": 1
}
```

### 5. Recherche avec QueryBuilder

```rust
#[derive(serde::Deserialize)]
struct SearchParams {
    name: Option<String>,
    email: Option<String>,
}

#[get("/data/users/search")]
async fn search(
    &self,
    Query(params): Query<SearchParams>,
) -> Result<axum::Json<Vec<UserEntity>>, quarlus_core::AppError> {
    let mut qb = QueryBuilder::new("users");

    if let Some(ref name) = params.name {
        qb = qb.where_like("name", &format!("%{name}%"));
    }
    if let Some(ref email) = params.email {
        qb = qb.where_eq("email", email);
    }

    let (sql, params_vec) = qb.order_by("id", true).build_select("id, name, email");

    let mut query = sqlx::query_as::<_, UserEntity>(&sql);
    for p in &params_vec {
        query = query.bind(p);
    }

    let rows = query.fetch_all(&self.pool).await?;
    Ok(axum::Json(rows))
}
```

## Critere de validation

```bash
# Liste paginee
curl "http://localhost:3000/data/users?page=0&size=2"
# → {"content":[...],"page":0,"size":2,"total_elements":3,"total_pages":2}

# Recherche par nom
curl "http://localhost:3000/data/users/search?name=Alice"
# → [{"id":1,"name":"Alice","email":"alice@example.com"}]

# Recherche par email exact
curl "http://localhost:3000/data/users/search?email=bob@example.com"
# → [{"id":2,"name":"Bob","email":"bob@example.com"}]
```

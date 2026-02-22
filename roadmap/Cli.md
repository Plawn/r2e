# Plan d'implémentation — CLI Enrichi

## Contexte

Le CLI actuel (`r2e-cli/`) est fonctionnel mais minimaliste :
- `r2e new <name>` : scaffold un projet vide ✅
- `r2e generate controller <name>` : génère un controller vide ✅
- `r2e generate service <name>` : génère un service vide ✅
- `r2e add <extension>` : ajoute une dépendance au Cargo.toml ✅
- `r2e dev` : lance `cargo watch` ✅

**Ce qui manque** :
- Génération de CRUD complet (controller + service + entity + tests)
- Scaffold de projets avec options (DB, auth, OpenAPI, etc.)
- Génération d'entities depuis une DB existante (reverse engineering)
- Génération depuis un spec OpenAPI
- `r2e config:show` pour lister la configuration connue
- `r2e routes` pour lister les routes déclarées
- `r2e doctor` pour diagnostiquer les problèmes courants
- Templates interactifs (prompts utilisateur)

## Architecture cible

```
r2e-cli/
  ├── Cargo.toml
  └── src/
      ├── main.rs                        ← Enrichir avec nouvelles commandes
      └── commands/
          ├── mod.rs
          ├── new_project.rs             ← ENRICHIR (options interactives)
          ├── add.rs                     ← Existant (ok)
          ├── dev.rs                     ← ENRICHIR (watch + reload)
          ├── generate.rs                ← ENRICHIR (CRUD, entity, test, middleware)
          ├── doctor.rs                  ← NOUVEAU
          ├── routes.rs                  ← NOUVEAU
          ├── config_show.rs             ← NOUVEAU
          └── templates/                 ← NOUVEAU (templates embarqués)
              ├── mod.rs
              ├── project.rs             ← Templates de projet
              ├── controller.rs          ← Templates de controller
              ├── crud.rs                ← Templates CRUD complet
              ├── entity.rs              ← Templates entity/repository
              └── test.rs                ← Templates de test
```

---

## Étape 1 — Restructurer les templates avec un système propre

**Fichier** : `r2e-cli/src/commands/templates/mod.rs`

### Objectif
Centraliser les templates de code dans un module dédié avec des fonctions de rendu
paramétrées, au lieu de hardcoder des strings dans chaque commande.

### Implémentation du template engine minimal
```rust
// r2e-cli/src/commands/templates/mod.rs

pub mod project;
pub mod controller;
pub mod crud;
pub mod entity;
pub mod test;

/// Simple template rendering: replaces {{key}} with value.
pub fn render(template: &str, vars: &[(&str, &str)]) -> String {
    let mut output = template.to_string();
    for (key, value) in vars {
        output = output.replace(&format!("{{{{{}}}}}", key), value);
    }
    output
}

/// Convert PascalCase to snake_case.
pub fn to_snake_case(name: &str) -> String {
    let mut result = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                result.push('_');
            }
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}

/// Convert snake_case to PascalCase.
pub fn to_pascal_case(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Compute a plural form (simple English rules).
pub fn pluralize(name: &str) -> String {
    if name.ends_with('s') || name.ends_with("sh") || name.ends_with("ch") {
        format!("{name}es")
    } else if name.ends_with('y') && !name.ends_with("ey") {
        format!("{}ies", &name[..name.len()-1])
    } else {
        format!("{name}s")
    }
}
```

### Validation
```bash
cargo check -p r2e-cli
```

---

## Étape 2 — Enrichir `r2e new` avec des options interactives

**Fichier** : `r2e-cli/src/commands/new_project.rs`

### Objectif
Proposer un wizard interactif au scaffold de projet, comme `cargo shuttle init`
ou `quarkus create app`.

### Nouvelles options CLI
```rust
// main.rs — enrichir la commande New
#[derive(Subcommand)]
enum Commands {
    /// Create a new R2E project
    New {
        /// Project name
        name: String,
        /// Include database support (sqlite, postgres, mysql)
        #[arg(long)]
        db: Option<String>,
        /// Include JWT/OIDC security
        #[arg(long)]
        auth: bool,
        /// Include OpenAPI docs
        #[arg(long)]
        openapi: bool,
        /// Include Prometheus metrics
        #[arg(long)]
        metrics: bool,
        /// Include all features
        #[arg(long)]
        full: bool,
        /// Skip interactive prompts (use defaults)
        #[arg(long)]
        no_interactive: bool,
    },
    // ...
}
```

### Mode interactif
Utiliser la crate `dialoguer` (ajouter comme dépendance) pour les prompts.

```rust
// new_project.rs

use dialoguer::{Confirm, MultiSelect, Select};

pub struct ProjectOptions {
    pub name: String,
    pub db: Option<DbKind>,
    pub auth: bool,
    pub openapi: bool,
    pub metrics: bool,
    pub scheduler: bool,
    pub events: bool,
}

#[derive(Debug, Clone)]
pub enum DbKind {
    Sqlite,
    Postgres,
    Mysql,
}

pub fn run(name: &str, cli_opts: &CliNewOpts) -> Result<(), Box<dyn std::error::Error>> {
    let opts = if cli_opts.full {
        ProjectOptions {
            name: name.to_string(),
            db: Some(DbKind::Sqlite),
            auth: true,
            openapi: true,
            metrics: true,
            scheduler: true,
            events: true,
        }
    } else if cli_opts.no_interactive || cli_opts.has_any_flag() {
        // Use CLI flags directly
        ProjectOptions::from_cli(name, cli_opts)
    } else {
        // Interactive mode
        prompt_options(name)?
    };

    generate_project(&opts)
}

fn prompt_options(name: &str) -> Result<ProjectOptions, Box<dyn std::error::Error>> {
    println!("{}", "Creating a new R2E project".blue().bold());
    println!();

    // Database selection
    let db_choices = &["None", "SQLite", "PostgreSQL", "MySQL"];
    let db_idx = Select::new()
        .with_prompt("Database")
        .items(db_choices)
        .default(0)
        .interact()?;
    let db = match db_idx {
        1 => Some(DbKind::Sqlite),
        2 => Some(DbKind::Postgres),
        3 => Some(DbKind::Mysql),
        _ => None,
    };

    // Features selection
    let feature_choices = &[
        "JWT/OIDC Authentication",
        "OpenAPI Documentation",
        "Prometheus Metrics",
        "Task Scheduling",
        "Event Bus",
    ];
    let selected = MultiSelect::new()
        .with_prompt("Select features (space to toggle, enter to confirm)")
        .items(feature_choices)
        .interact()?;

    Ok(ProjectOptions {
        name: name.to_string(),
        db,
        auth: selected.contains(&0),
        openapi: selected.contains(&1),
        metrics: selected.contains(&2),
        scheduler: selected.contains(&3),
        events: selected.contains(&4),
    })
}
```

### Génération conditionnelle du projet

```rust
fn generate_project(opts: &ProjectOptions) -> Result<(), Box<dyn std::error::Error>> {
    let project_dir = Path::new(&opts.name);
    if project_dir.exists() {
        return Err(format!("Directory '{}' already exists", opts.name).into());
    }

    fs::create_dir_all(project_dir.join("src/controllers"))?;

    // 1. Cargo.toml — avec les bonnes features
    let features = build_feature_list(opts);
    let cargo_toml = templates::project::cargo_toml(&opts.name, &features, &opts.db);
    fs::write(project_dir.join("Cargo.toml"), cargo_toml)?;

    // 2. State — avec les champs correspondants aux features
    let state_rs = templates::project::state_rs(opts);
    fs::write(project_dir.join("src/state.rs"), state_rs)?;

    // 3. main.rs — avec le builder adapté
    let main_rs = templates::project::main_rs(opts);
    fs::write(project_dir.join("src/main.rs"), main_rs)?;

    // 4. Hello controller
    let hello = templates::controller::basic("HelloController", "/", "AppState");
    fs::write(project_dir.join("src/controllers/hello.rs"), hello)?;
    fs::write(project_dir.join("src/controllers/mod.rs"), "pub mod hello;\n")?;

    // 5. application.yaml
    let yaml = templates::project::application_yaml(opts);
    fs::write(project_dir.join("application.yaml"), yaml)?;

    // 6. Si DB → créer le dossier migrations/
    if opts.db.is_some() {
        fs::create_dir_all(project_dir.join("migrations"))?;
    }

    // 7. .gitignore
    fs::write(project_dir.join(".gitignore"), "/target\n")?;

    // Print summary
    println!();
    println!("{} Project '{}' created!", "✓".green(), opts.name.green());
    println!();
    println!("  {}", format!("cd {}", opts.name).cyan());
    println!("  {}", "cargo run".cyan());
    println!();

    if opts.openapi {
        println!("  📖 API docs: {}", "http://localhost:8080/docs".cyan());
    }
    if opts.metrics {
        println!("  📊 Metrics:  {}", "http://localhost:8080/metrics".cyan());
    }
    println!("  🏥 Health:   {}", "http://localhost:8080/health".cyan());

    Ok(())
}
```

### Templates de projet
**Fichier** : `r2e-cli/src/commands/templates/project.rs`

```rust
pub fn cargo_toml(name: &str, features: &[&str], db: &Option<DbKind>) -> String {
    let r2e_features = if features.is_empty() {
        String::new()
    } else {
        format!(", features = [{}]", features.iter()
            .map(|f| format!("\"{}\"", f))
            .collect::<Vec<_>>()
            .join(", "))
    };

    let db_dep = match db {
        Some(DbKind::Sqlite) => "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"sqlite\"] }",
        Some(DbKind::Postgres) => "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"postgres\"] }",
        Some(DbKind::Mysql) => "\nsqlx = { version = \"0.8\", features = [\"runtime-tokio\", \"mysql\"] }",
        None => "",
    };

    format!(r#"[package]
name = "{name}"
version = "0.1.0"
edition = "2021"

[dependencies]
r2e = {{ version = "0.1"{r2e_features} }}
tokio = {{ version = "1", features = ["full"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"{db_dep}
"#)
}

pub fn main_rs(opts: &ProjectOptions) -> String {
    // Générer un main.rs adapté aux features sélectionnées
    // Inclure conditionnellement : Security setup, OpenAPI plugin, Prometheus, etc.
    // ...
}

pub fn state_rs(opts: &ProjectOptions) -> String {
    // Générer un state.rs avec les champs nécessaires
    // Si DB → inclure pool
    // Si events → inclure event_bus
    // etc.
    // ...
}

pub fn application_yaml(opts: &ProjectOptions) -> String {
    let mut yaml = format!("app:\n  name: \"{}\"\n  port: 8080\n", opts.name);

    if opts.db.is_some() {
        yaml.push_str("\ndatabase:\n");
        match &opts.db {
            Some(DbKind::Sqlite) => yaml.push_str("  url: \"sqlite:data.db?mode=rwc\"\n"),
            Some(DbKind::Postgres) => yaml.push_str("  url: \"${DATABASE_URL}\"\n  pool-size: 10\n"),
            Some(DbKind::Mysql) => yaml.push_str("  url: \"${DATABASE_URL}\"\n  pool-size: 10\n"),
            None => {}
        }
    }

    if opts.auth {
        yaml.push_str("\nsecurity:\n  jwt:\n    issuer: \"my-app\"\n    audience: \"my-app\"\n    jwks-url: \"${JWKS_URL}\"\n");
    }

    yaml
}
```

### Dépendances Cargo à ajouter au CLI
```toml
dialoguer = "0.11"
```

### Validation
```bash
cargo build -p r2e-cli
./target/debug/r2e new test-project --full
cd test-project && cargo check
```

---

## Étape 3 — Génération CRUD complète

**Fichier** : `r2e-cli/src/commands/generate.rs` (enrichir)

### Objectif
Avec `r2e generate crud User`, générer automatiquement :
1. Un model (`src/models/user.rs`)
2. Un service (`src/services/user_service.rs`)
3. Un controller (`src/controllers/user_controller.rs`)
4. Un test d'intégration (`tests/user_test.rs`)

### Nouvelles sous-commandes generate
```rust
// main.rs
#[derive(Subcommand)]
enum GenerateKind {
    /// Generate a new controller
    Controller { name: String },
    /// Generate a new service
    Service { name: String },
    /// Generate a complete CRUD (controller + service + model + tests)
    Crud {
        /// Entity name in PascalCase (e.g. User, BlogPost)
        name: String,
        /// Fields in format "name:type" (e.g. "name:String email:String age:i64")
        #[arg(long, num_args = 1..)]
        fields: Vec<String>,
    },
    /// Generate a model/entity
    Entity {
        name: String,
        #[arg(long, num_args = 1..)]
        fields: Vec<String>,
    },
    /// Generate an integration test
    Test { name: String },
    /// Generate a middleware/interceptor
    Middleware { name: String },
}
```

### Parsing des champs
```rust
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,        // snake_case
    pub rust_type: String,   // String, i64, f64, bool, Option<String>, etc.
    pub is_optional: bool,
}

fn parse_fields(fields: &[String]) -> Result<Vec<Field>, String> {
    fields.iter().map(|f| {
        let parts: Vec<&str> = f.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid field format '{}'. Expected 'name:Type'", f));
        }
        let name = parts[0].to_string();
        let rust_type = parts[1].to_string();
        let is_optional = rust_type.starts_with("Option<");
        Ok(Field { name, rust_type, is_optional })
    }).collect()
}
```

### Templates CRUD
**Fichier** : `r2e-cli/src/commands/templates/crud.rs`

```rust
use super::{to_snake_case, to_pascal_case, pluralize, render};

/// Generate the model struct.
pub fn model(entity_name: &str, fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let fields_str: String = fields.iter().map(|f| {
        format!("    pub {}: {},\n", f.name, f.rust_type)
    }).collect();

    let create_fields: String = fields.iter()
        .filter(|f| f.name != "id")
        .map(|f| format!("    pub {}: {},\n", f.name, f.rust_type))
        .collect();

    format!(r#"use serde::{{Deserialize, Serialize}};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct {entity_name} {{
    pub id: i64,
{fields_str}}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Create{entity_name}Request {{
{create_fields}}}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Update{entity_name}Request {{
{create_fields}}}
"#)
}

/// Generate the service.
pub fn service(entity_name: &str, _fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(r#"use crate::models::{snake}::{{{entity_name}, Create{entity_name}Request, Update{entity_name}Request}};
use r2e::prelude::*;
use sqlx::SqlitePool;

#[derive(Clone)]
pub struct {entity_name}Service {{
    pool: SqlitePool,
}}

#[bean]
impl {entity_name}Service {{
    pub fn new(pool: SqlitePool) -> Self {{
        Self {{ pool }}
    }}

    pub async fn list(&self) -> Vec<{entity_name}> {{
        sqlx::query_as!(
            {entity_name},
            "SELECT * FROM {plural}"
        )
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default()
    }}

    pub async fn get_by_id(&self, id: i64) -> Option<{entity_name}> {{
        sqlx::query_as!(
            {entity_name},
            "SELECT * FROM {plural} WHERE id = ?",
            id
        )
        .fetch_optional(&self.pool)
        .await
        .unwrap_or(None)
    }}

    pub async fn create(&self, req: Create{entity_name}Request) -> {entity_name} {{
        // TODO: implement insert
        todo!("Implement create")
    }}

    pub async fn update(&self, id: i64, req: Update{entity_name}Request) -> Option<{entity_name}> {{
        // TODO: implement update
        todo!("Implement update")
    }}

    pub async fn delete(&self, id: i64) -> bool {{
        let result = sqlx::query!("DELETE FROM {plural} WHERE id = ?", id)
            .execute(&self.pool)
            .await;
        matches!(result, Ok(r) if r.rows_affected() > 0)
    }}
}}
"#)
}

/// Generate the controller.
pub fn controller(entity_name: &str, _fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(r#"use crate::models::{snake}::{{{entity_name}, Create{entity_name}Request, Update{entity_name}Request}};
use crate::services::{snake}_service::{entity_name}Service;
use r2e::prelude::*;

#[derive(Controller)]
#[controller(path = "/{plural}", state = AppState)]
pub struct {entity_name}Controller {{
    #[inject]
    service: {entity_name}Service,
}}

#[routes]
impl {entity_name}Controller {{
    /// List all {plural}
    #[get("/")]
    async fn list(&self) -> Json<Vec<{entity_name}>> {{
        Json(self.service.list().await)
    }}

    /// Get a single {snake} by ID
    #[get("/{{id}}")]
    async fn get_by_id(&self, Path(id): Path<i64>) -> Result<Json<{entity_name}>, HttpError> {{
        self.service.get_by_id(id).await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("{entity_name} not found".into()))
    }}

    /// Create a new {snake}
    #[post("/")]
    async fn create(&self, Json(body): Json<Create{entity_name}Request>) -> Json<{entity_name}> {{
        Json(self.service.create(body).await)
    }}

    /// Update an existing {snake}
    #[put("/{{id}}")]
    async fn update(
        &self,
        Path(id): Path<i64>,
        Json(body): Json<Update{entity_name}Request>,
    ) -> Result<Json<{entity_name}>, HttpError> {{
        self.service.update(id, body).await
            .map(Json)
            .ok_or_else(|| HttpError::NotFound("{entity_name} not found".into()))
    }}

    /// Delete a {snake}
    #[delete("/{{id}}")]
    async fn delete(&self, Path(id): Path<i64>) -> Result<Json<&'static str>, HttpError> {{
        if self.service.delete(id).await {{
            Ok(Json("deleted"))
        }} else {{
            Err(HttpError::NotFound("{entity_name} not found".into()))
        }}
    }}
}}
"#)
}

/// Generate an integration test.
pub fn integration_test(entity_name: &str, _fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    format!(r#"use r2e_test::{{TestApp, TestJwt}};
use serde_json::json;

// TODO: adapt imports to your project structure
// use crate::models::{snake}::{entity_name};

#[tokio::test]
async fn test_list_{plural}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.get("/{plural}").await;
    // resp.assert_ok();
    todo!("Setup TestApp and test list endpoint");
}}

#[tokio::test]
async fn test_create_{snake}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let body = json!({{ /* fields */ }});
    // let resp = app.post("/{plural}", &body).await;
    // resp.assert_status(201);
    todo!("Setup TestApp and test create endpoint");
}}

#[tokio::test]
async fn test_get_{snake}_not_found() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.get("/{plural}/999").await;
    // resp.assert_status(404);
    todo!("Setup TestApp and test 404");
}}

#[tokio::test]
async fn test_delete_{snake}() {{
    // let app = TestApp::from_builder(/* your builder */);
    // let resp = app.delete("/{plural}/1").await;
    // resp.assert_ok();
    todo!("Setup TestApp and test delete");
}}
"#)
}

/// Generate a SQL migration for the entity.
pub fn migration(entity_name: &str, fields: &[Field]) -> String {
    let snake = to_snake_case(entity_name);
    let plural = pluralize(&snake);

    let columns: String = fields.iter().map(|f| {
        let sql_type = rust_type_to_sql(&f.rust_type);
        let nullable = if f.is_optional { "" } else { " NOT NULL" };
        format!("    {} {}{}", f.name, sql_type, nullable)
    }).collect::<Vec<_>>().join(",\n");

    format!(r#"-- Create {plural} table
CREATE TABLE IF NOT EXISTS {plural} (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
{columns}
);
"#)
}

fn rust_type_to_sql(rust_type: &str) -> &str {
    let inner = rust_type.strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(rust_type);

    match inner {
        "String" | "&str" => "TEXT",
        "i32" | "i64" | "u32" | "u64" | "usize" => "INTEGER",
        "f32" | "f64" => "REAL",
        "bool" => "BOOLEAN",
        _ => "TEXT",
    }
}
```

### Commande CRUD complète
**Fichier** : `r2e-cli/src/commands/generate.rs` (enrichir)

```rust
pub fn crud(name: &str, raw_fields: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let fields = parse_fields(raw_fields)?;
    let snake = to_snake_case(name);
    let plural = pluralize(&snake);

    println!("{} Generating CRUD for '{}'", "→".blue(), name.green());

    // 1. Model
    let model_dir = Path::new("src/models");
    fs::create_dir_all(model_dir)?;
    let model_path = model_dir.join(format!("{snake}.rs"));
    fs::write(&model_path, templates::crud::model(name, &fields))?;
    update_mod_rs(model_dir, &snake)?;
    println!("  {} {}", "✓".green(), model_path.display());

    // 2. Service
    let service_dir = Path::new("src/services");
    fs::create_dir_all(service_dir)?;
    let service_path = service_dir.join(format!("{snake}_service.rs"));
    fs::write(&service_path, templates::crud::service(name, &fields))?;
    update_mod_rs(service_dir, &format!("{snake}_service"))?;
    println!("  {} {}", "✓".green(), service_path.display());

    // 3. Controller
    let controller_dir = Path::new("src/controllers");
    fs::create_dir_all(controller_dir)?;
    let controller_path = controller_dir.join(format!("{snake}_controller.rs"));
    fs::write(&controller_path, templates::crud::controller(name, &fields))?;
    update_mod_rs(controller_dir, &format!("{snake}_controller"))?;
    println!("  {} {}", "✓".green(), controller_path.display());

    // 4. Migration (si dossier migrations/ existe)
    if Path::new("migrations").exists() {
        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let migration_path = Path::new("migrations")
            .join(format!("{}_create_{plural}.sql", timestamp));
        fs::write(&migration_path, templates::crud::migration(name, &fields))?;
        println!("  {} {}", "✓".green(), migration_path.display());
    }

    // 5. Test
    let test_dir = Path::new("tests");
    fs::create_dir_all(test_dir)?;
    let test_path = test_dir.join(format!("{snake}_test.rs"));
    fs::write(&test_path, templates::crud::integration_test(name, &fields))?;
    println!("  {} {}", "✓".green(), test_path.display());

    // Summary
    println!();
    println!("{} CRUD generated for '{}'!", "✓".green().bold(), name.green());
    println!();
    println!("  Next steps:");
    println!("  1. Register the controller in main.rs:");
    println!("     {}", format!(".register_controller::<{name}Controller>()").cyan());
    println!("  2. Add {name}Service to your state struct");
    println!("  3. Run migrations if applicable");
    println!("  4. Run `cargo check` to verify");

    Ok(())
}

/// Update or create a mod.rs file to include a new module.
fn update_mod_rs(dir: &Path, module_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mod_path = dir.join("mod.rs");
    let mod_line = format!("pub mod {module_name};\n");

    if mod_path.exists() {
        let existing = fs::read_to_string(&mod_path)?;
        if !existing.contains(&mod_line) {
            fs::write(&mod_path, format!("{existing}{mod_line}"))?;
        }
    } else {
        fs::write(&mod_path, &mod_line)?;
    }
    Ok(())
}
```

### Exemple d'utilisation
```bash
r2e generate crud BlogPost --fields "title:String content:String author:String published:bool"
```

Produit :
```
→ Generating CRUD for 'BlogPost'
  ✓ src/models/blog_post.rs
  ✓ src/services/blog_post_service.rs
  ✓ src/controllers/blog_post_controller.rs
  ✓ migrations/20260211120000_create_blog_posts.sql
  ✓ tests/blog_post_test.rs

✓ CRUD generated for 'BlogPost'!

  Next steps:
  1. Register the controller in main.rs:
     .register_controller::<BlogPostController>()
  2. Add BlogPostService to your state struct
  3. Run migrations if applicable
  4. Run `cargo check` to verify
```

### Validation
```bash
cargo build -p r2e-cli
# Créer un projet test et générer un CRUD
./target/debug/r2e new test-crud --db sqlite --no-interactive
cd test-crud
../target/debug/r2e generate crud User --fields "name:String email:String"
cargo check
```

---

## Étape 4 — Commande `r2e doctor`

**Fichier** : `r2e-cli/src/commands/doctor.rs`

### Objectif
Diagnostiquer les problèmes courants d'un projet R2E : dépendances manquantes,
versions incompatibles, config manquante, etc.

### Implémentation
```rust
use colored::Colorize;
use std::path::Path;

#[derive(Debug)]
enum CheckResult {
    Ok(String),
    Warning(String),
    Error(String),
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    println!("{}", "R2E Doctor — Checking project health".bold());
    println!();

    let mut issues = 0;

    // 1. Check Cargo.toml exists
    check("Cargo.toml exists", || {
        if Path::new("Cargo.toml").exists() {
            CheckResult::Ok("Found".into())
        } else {
            CheckResult::Error("Not in a Rust project directory".into())
        }
    }, &mut issues);

    // 2. Check r2e dependency
    check("R2E dependency", || {
        let content = std::fs::read_to_string("Cargo.toml").unwrap_or_default();
        if content.contains("r2e") {
            CheckResult::Ok("Found".into())
        } else {
            CheckResult::Error("r2e not found in dependencies".into())
        }
    }, &mut issues);

    // 3. Check application.yaml
    check("Configuration file", || {
        if Path::new("application.yaml").exists() {
            CheckResult::Ok("application.yaml found".into())
        } else {
            CheckResult::Warning("application.yaml not found (optional)".into())
        }
    }, &mut issues);

    // 4. Check src/controllers/ directory
    check("Controllers directory", || {
        if Path::new("src/controllers").exists() {
            let count = std::fs::read_dir("src/controllers")
                .map(|dir| dir.filter(|e| {
                    e.as_ref().map(|e| e.path().extension()
                        .map(|ext| ext == "rs").unwrap_or(false))
                        .unwrap_or(false)
                }).count())
                .unwrap_or(0);
            CheckResult::Ok(format!("{} controller files", count))
        } else {
            CheckResult::Warning("src/controllers/ not found".into())
        }
    }, &mut issues);

    // 5. Check Rust toolchain
    check("Rust toolchain", || {
        match std::process::Command::new("rustc").arg("--version").output() {
            Ok(output) => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                CheckResult::Ok(version)
            }
            Err(_) => CheckResult::Error("rustc not found".into()),
        }
    }, &mut issues);

    // 6. Check cargo-watch (pour r2e dev)
    check("cargo-watch (for r2e dev)", || {
        match std::process::Command::new("cargo-watch").arg("--version").output() {
            Ok(output) => {
                let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
                CheckResult::Ok(version)
            }
            Err(_) => CheckResult::Warning(
                "Not installed. Run: cargo install cargo-watch".into()
            ),
        }
    }, &mut issues);

    // 7. Check migrations directory if data feature is used
    check("Migrations directory", || {
        let content = std::fs::read_to_string("Cargo.toml").unwrap_or_default();
        if content.contains("r2e-data") || content.contains("\"data\"") {
            if Path::new("migrations").exists() {
                let count = std::fs::read_dir("migrations")
                    .map(|dir| dir.count())
                    .unwrap_or(0);
                CheckResult::Ok(format!("{} migration files", count))
            } else {
                CheckResult::Warning("Data feature used but no migrations/ directory".into())
            }
        } else {
            CheckResult::Ok("Data feature not used (skipped)".into())
        }
    }, &mut issues);

    // 8. Check that src/main.rs has serve()
    check("Application entrypoint", || {
        let content = std::fs::read_to_string("src/main.rs").unwrap_or_default();
        if content.contains(".serve(") || content.contains("serve(") {
            CheckResult::Ok("serve() call found in main.rs".into())
        } else {
            CheckResult::Warning("No .serve() call found in main.rs".into())
        }
    }, &mut issues);

    println!();
    if issues == 0 {
        println!("{}", "All checks passed! ✓".green().bold());
    } else {
        println!("{}", format!("{} issue(s) found", issues).yellow().bold());
    }

    Ok(())
}

fn check<F>(name: &str, f: F, issues: &mut usize)
where
    F: FnOnce() -> CheckResult,
{
    let result = f();
    match &result {
        CheckResult::Ok(msg) => {
            println!("  {} {} — {}", "✓".green(), name, msg.dimmed());
        }
        CheckResult::Warning(msg) => {
            println!("  {} {} — {}", "⚠".yellow(), name, msg.yellow());
            *issues += 1;
        }
        CheckResult::Error(msg) => {
            println!("  {} {} — {}", "✗".red(), name, msg.red());
            *issues += 1;
        }
    }
}
```

### Enregistrer dans main.rs
```rust
Commands::Doctor => doctor::run(),
```

### Validation
```bash
cargo build -p r2e-cli
cd example-app && ../target/debug/r2e doctor
```

---

## Étape 5 — Commande `r2e routes`

**Fichier** : `r2e-cli/src/commands/routes.rs`

### Objectif
Lister toutes les routes déclarées dans le projet en parsant les fichiers source.
C'est un parsing statique (pas de compilation nécessaire).

### Implémentation
```rust
use colored::Colorize;
use std::fs;
use std::path::Path;

#[derive(Debug)]
struct Route {
    method: String,
    path: String,
    handler: String,
    file: String,
    line: usize,
    roles: Option<String>,
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let controllers_dir = Path::new("src/controllers");
    if !controllers_dir.exists() {
        return Err("src/controllers/ directory not found".into());
    }

    let mut routes = Vec::new();

    // Walk all .rs files in src/controllers/
    for entry in fs::read_dir(controllers_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map_or(false, |ext| ext == "rs") && path.file_name() != Some("mod.rs".as_ref()) {
            parse_routes_from_file(&path, &mut routes)?;
        }
    }

    if routes.is_empty() {
        println!("{}", "No routes found.".dimmed());
        return Ok(());
    }

    // Sort by path
    routes.sort_by(|a, b| a.path.cmp(&b.path));

    // Print table
    println!("{}", "Declared routes:".bold());
    println!();
    println!("  {:<8} {:<35} {:<25} {}",
        "METHOD".dimmed(), "PATH".dimmed(), "HANDLER".dimmed(), "FILE".dimmed());
    println!("  {}", "─".repeat(80).dimmed());

    for route in &routes {
        let method_colored = match route.method.as_str() {
            "GET" => route.method.green(),
            "POST" => route.method.blue(),
            "PUT" => route.method.yellow(),
            "DELETE" => route.method.red(),
            "PATCH" => route.method.magenta(),
            _ => route.method.normal(),
        };

        let roles_str = route.roles.as_deref().unwrap_or("");
        let handler_str = if roles_str.is_empty() {
            route.handler.clone()
        } else {
            format!("{} 🔒{}", route.handler, roles_str)
        };

        println!("  {:<8} {:<35} {:<25} {}:{}",
            method_colored,
            route.path,
            handler_str,
            route.file,
            route.line,
        );
    }

    println!();
    println!("  {} routes total", routes.len());

    Ok(())
}

fn parse_routes_from_file(path: &Path, routes: &mut Vec<Route>) -> Result<(), Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let filename = path.file_name().unwrap().to_string_lossy().to_string();

    // Trouver le #[controller(path = "...")]
    let base_path = extract_controller_path(&content).unwrap_or_default();

    // Parser les attributs #[get("/...")], #[post("/...")], etc.
    let mut current_roles: Option<String> = None;

    for (line_num, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Detect #[roles("...")]
        if trimmed.starts_with("#[roles(") {
            current_roles = extract_string_arg(trimmed, "roles");
        }

        // Detect route macros
        for method in &["get", "post", "put", "delete", "patch"] {
            let pattern = format!("#[{}(", method);
            if trimmed.starts_with(&pattern) {
                if let Some(route_path) = extract_string_arg(trimmed, method) {
                    // Find the handler name (next `async fn` or `fn` line)
                    let handler = find_next_fn_name(&content, line_num);

                    let full_path = if base_path.is_empty() {
                        route_path.clone()
                    } else {
                        format!("{}{}", base_path, route_path)
                    };

                    routes.push(Route {
                        method: method.to_uppercase(),
                        path: full_path,
                        handler: handler.unwrap_or_else(|| "?".to_string()),
                        file: filename.clone(),
                        line: line_num + 1,
                        roles: current_roles.take(),
                    });
                }
            }
        }

        // Reset roles if we hit a line that's not a macro attribute
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            current_roles = None;
        }
    }

    Ok(())
}

fn extract_controller_path(content: &str) -> Option<String> {
    // Chercher #[controller(path = "...")] et extraire le path
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.contains("controller(") && trimmed.contains("path") {
            // Simple regex-free extraction
            if let Some(start) = trimmed.find("path") {
                let rest = &trimmed[start..];
                if let Some(quote_start) = rest.find('"') {
                    let after_quote = &rest[quote_start + 1..];
                    if let Some(quote_end) = after_quote.find('"') {
                        return Some(after_quote[..quote_end].to_string());
                    }
                }
            }
        }
    }
    None
}

fn extract_string_arg(line: &str, attr_name: &str) -> Option<String> {
    let pattern = format!("#[{}(", attr_name);
    if let Some(start) = line.find(&pattern) {
        let rest = &line[start + pattern.len()..];
        if let Some(quote_start) = rest.find('"') {
            let after_quote = &rest[quote_start + 1..];
            if let Some(quote_end) = after_quote.find('"') {
                return Some(after_quote[..quote_end].to_string());
            }
        }
    }
    None
}

fn find_next_fn_name(content: &str, from_line: usize) -> Option<String> {
    for line in content.lines().skip(from_line + 1).take(5) {
        let trimmed = line.trim();
        if trimmed.contains("async fn ") || trimmed.contains("fn ") {
            let fn_start = trimmed.find("fn ").map(|i| i + 3)?;
            let rest = &trimmed[fn_start..];
            let fn_end = rest.find('(').unwrap_or(rest.len());
            return Some(rest[..fn_end].to_string());
        }
    }
    None
}
```

### Exemple de sortie
```
Declared routes:

  METHOD   PATH                                HANDLER                   FILE
  ────────────────────────────────────────────────────────────────────────────────
  GET      /health                             health_handler            (built-in)
  GET      /users                              list                      user_controller.rs:15
  GET      /users/{id}                         get_by_id                 user_controller.rs:20
  POST     /users                              create 🔒admin            user_controller.rs:26
  DELETE   /users/{id}                         delete 🔒admin            user_controller.rs:32
  GET      /api/public                         public_data               mixed_controller.rs:8
  GET      /api/me                             me                        mixed_controller.rs:13

  7 routes total
```

### Validation
```bash
cargo build -p r2e-cli
cd example-app && ../target/debug/r2e routes
```

---

## Étape 6 — Enrichir `r2e dev` avec un vrai watch

**Fichier** : `r2e-cli/src/commands/dev.rs`

### Objectif actuel
Le `dev` actuel fait juste `cargo watch -x run`. L'enrichir pour :
- Détecter automatiquement le binaire à lancer
- Afficher les routes au démarrage
- Ouvrir le navigateur automatiquement (optionnel)
- Surveiller aussi les fichiers YAML

### Implémentation enrichie
```rust
use colored::Colorize;
use std::process::Command;

pub fn run(open_browser: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Check cargo-watch is installed
    if Command::new("cargo-watch").arg("--version").output().is_err() {
        println!("{} {} not found.", "✗".red(), "cargo-watch".cyan());
        println!("  Install it with: {}", "cargo install cargo-watch".green());
        return Err("cargo-watch not installed".into());
    }

    println!("{}", "Starting R2E dev server...".blue().bold());
    println!();

    // Show routes before starting
    if let Ok(()) = super::routes::run() {
        println!();
    }

    // Build the cargo-watch command
    let mut cmd = Command::new("cargo");
    cmd.arg("watch")
        .arg("-w").arg("src")
        .arg("-w").arg("application.yaml")
        .arg("-w").arg("application-dev.yaml")
        .arg("-w").arg("migrations")
        .arg("--ignore").arg("target/")
        .arg("-x").arg("run");

    // Set R2E_PROFILE=dev
    cmd.env("R2E_PROFILE", "dev");

    println!("{} Watching src/, application*.yaml, migrations/", "→".blue());
    println!("{} Press {} to stop", "→".blue(), "Ctrl+C".yellow());
    println!();

    // Optionally open browser after a delay
    if open_browser {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let _ = open::that("http://localhost:8080");
        });
    }

    let status = cmd.status()?;
    if !status.success() {
        return Err("cargo watch exited with error".into());
    }

    Ok(())
}
```

### Mise à jour CLI
```rust
/// Start the dev server with hot-reload
Dev {
    /// Open browser automatically
    #[arg(long)]
    open: bool,
},
```

### Dépendances optionnelles
```toml
open = "5"  # Pour ouvrir le navigateur (optionnel)
```

### Validation
```bash
cargo build -p r2e-cli
cd example-app && ../target/debug/r2e dev
```

---

## Étape 7 — Commande `r2e generate middleware`

**Fichier** : `r2e-cli/src/commands/generate.rs` (ajouter)

### Templates
```rust
pub fn middleware(name: &str) -> String {
    format!(r#"use r2e::prelude::*;
use std::future::Future;

/// Custom interceptor: {name}
pub struct {name};

impl<R: Send> Interceptor<R> for {name} {{
    fn around<F, Fut>(&self, ctx: InterceptorContext, next: F) -> impl Future<Output = R> + Send
    where
        F: FnOnce() -> Fut + Send,
        Fut: Future<Output = R> + Send,
    {{
        async move {{
            tracing::info!(method = ctx.method_name, "{name}: before");
            let result = next().await;
            tracing::info!(method = ctx.method_name, "{name}: after");
            result
        }}
    }}
}}
"#)
}
```

---

## Mise à jour de main.rs du CLI

**Fichier** : `r2e-cli/src/main.rs`

### CLI final complet
```rust
#[derive(Subcommand)]
enum Commands {
    /// Create a new R2E project
    New {
        name: String,
        #[arg(long)] db: Option<String>,
        #[arg(long)] auth: bool,
        #[arg(long)] openapi: bool,
        #[arg(long)] metrics: bool,
        #[arg(long)] full: bool,
        #[arg(long)] no_interactive: bool,
    },
    /// Generate components
    Generate {
        #[command(subcommand)]
        kind: GenerateKind,
    },
    /// Add an extension
    Add { extension: String },
    /// Start dev server with hot-reload
    Dev {
        #[arg(long)] open: bool,
    },
    /// Check project health
    Doctor,
    /// List all declared routes
    Routes,
}

#[derive(Subcommand)]
enum GenerateKind {
    Controller { name: String },
    Service { name: String },
    Crud {
        name: String,
        #[arg(long, num_args = 1..)]
        fields: Vec<String>,
    },
    Entity {
        name: String,
        #[arg(long, num_args = 1..)]
        fields: Vec<String>,
    },
    Test { name: String },
    Middleware { name: String },
}
```

---

## Ordre d'implémentation

| # | Étape | Fichiers | Dépendance |
|---|-------|----------|------------|
| 1 | Système de templates | templates/mod.rs | Aucune |
| 2 | `r2e new` interactif | new_project.rs + templates/project.rs | Étape 1 |
| 3 | `r2e generate crud` | generate.rs + templates/crud.rs | Étape 1 |
| 4 | `r2e doctor` | doctor.rs | Aucune |
| 5 | `r2e routes` | routes.rs | Aucune |
| 6 | `r2e dev` enrichi | dev.rs | Étape 5 |
| 7 | `r2e generate middleware` | generate.rs | Étape 1 |

## Dépendances Cargo à ajouter

```toml
# r2e-cli/Cargo.toml
[dependencies]
# Existantes
clap = { version = "4", features = ["derive"] }
colored = "2"
toml_edit = "0.22"

# Nouvelles
dialoguer = "0.11"      # Prompts interactifs
chrono = "0.4"           # Timestamps pour les migrations
open = "5"               # Ouvrir le navigateur (optionnel)
```

## Critères de succès
- [ ] `cargo build -p r2e-cli` compile sans erreur
- [ ] `r2e new myapp` (interactif) crée un projet qui compile
- [ ] `r2e new myapp --full --no-interactive` crée un projet complet
- [ ] `r2e generate crud User --fields "name:String email:String"` génère 5 fichiers
- [ ] `r2e doctor` liste les checks et affiche un diagnostic
- [ ] `r2e routes` affiche les routes parsées depuis les fichiers source
- [ ] `r2e dev` lance cargo-watch avec les bons paramètres
- [ ] `r2e generate middleware AuditLog` génère un intercepteur template
- [ ] Tous les projets générés passent `cargo check`
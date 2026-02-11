# Plan d'implémentation — Configuration Profiles Riches

## Contexte

Le système de configuration actuel (`r2e-core/src/config.rs`) est fonctionnel mais basique :
- Chargement YAML + overlay env vars ✅
- Profils (`application-{profile}.yaml`) ✅
- Accès clé/valeur avec `get::<T>("key")` ✅

**Ce qui manque** : validation au démarrage, typage fort par section, support des secrets,
valeurs par défaut documentées, config-as-bean injectable, arrays/listes, includes,
et introspection (lister toutes les clés connues).

## Architecture cible

```
r2e-core/src/config.rs           ← Enrichir (ne pas casser l'existant)
r2e-core/src/config/
  ├── mod.rs                     ← Re-export public, R2eConfig enrichi
  ├── value.rs                   ← ConfigValue (existant, extrait)
  ├── loader.rs                  ← Chargement multi-sources (existant, extrait + enrichi)
  ├── typed.rs                   ← NEW: #[derive(ConfigProperties)] support
  ├── validation.rs              ← NEW: Validation au démarrage
  ├── secrets.rs                 ← NEW: Résolution de secrets
  └── registry.rs                ← NEW: Registre de propriétés connues
r2e-macros/src/config_derive.rs  ← NEW: Proc macro #[derive(ConfigProperties)]
```

---

## Étape 1 — Refactorer config.rs en module

**Fichier** : `r2e-core/src/config/mod.rs`

### Objectif
Extraire le fichier monolithique `config.rs` en sous-modules sans casser l'API publique.

### Actions
1. Créer le dossier `r2e-core/src/config/`
2. Déplacer `ConfigValue`, `FromConfigValue` et les impls dans `config/value.rs`
3. Déplacer `flatten_yaml`, `load_yaml_file` dans `config/loader.rs`
4. Garder `R2eConfig` dans `config/mod.rs` avec des re-exports publics
5. Vérifier que `use r2e_core::config::R2eConfig` continue de fonctionner
6. Vérifier que `#[config("key")]` dans les macros continue de fonctionner

### Validation
```bash
cargo check --workspace
cargo test --workspace
```

### Impact sur l'existant
- **AUCUNE cassure d'API** — tous les types publics restent aux mêmes chemins via re-exports

---

## Étape 2 — Support des listes et valeurs imbriquées

**Fichier** : `r2e-core/src/config/value.rs`

### Objectif
`ConfigValue` ne supporte actuellement que les scalaires. Il faut supporter les listes
et les maps pour des configs du type :

```yaml
app:
  cors:
    allowed-origins:
      - "http://localhost:3000"
      - "https://myapp.com"
  features:
    - "openapi"
    - "prometheus"
```

### Actions
1. Ajouter `ConfigValue::List(Vec<ConfigValue>)` et `ConfigValue::Map(HashMap<String, ConfigValue>)`
2. Modifier `flatten_yaml` pour stocker les listes comme `ConfigValue::List` au lieu de les ignorer
3. Implémenter `FromConfigValue for Vec<T>` (désérialise une liste en `Vec<T>`)
4. Implémenter `FromConfigValue for Vec<String>` (cas le plus courant)
5. Dans `R2eConfig`, ajouter `get_list<T: FromConfigValue>(&self, key: &str) -> Result<Vec<T>, ConfigError>`
6. Ajouter tests pour les listes YAML

### Code attendu (value.rs)
```rust
#[derive(Debug, Clone)]
pub enum ConfigValue {
    String(String),
    Integer(i64),
    Float(f64),
    Bool(bool),
    Null,
    List(Vec<ConfigValue>),    // NEW
    Map(HashMap<String, ConfigValue>), // NEW
}
```

### Code attendu (FromConfigValue for Vec<T>)
```rust
impl<T: FromConfigValue> FromConfigValue for Vec<T> {
    fn from_config_value(value: &ConfigValue, key: &str) -> Result<Self, ConfigError> {
        match value {
            ConfigValue::List(items) => items
                .iter()
                .enumerate()
                .map(|(i, v)| T::from_config_value(v, &format!("{key}[{i}]")))
                .collect(),
            // Fallback: single value → vec of one
            other => Ok(vec![T::from_config_value(other, key)?]),
        }
    }
}
```

### Gestion du flatten
Quand `flatten_yaml` rencontre un `serde_yaml::Value::Sequence`, il doit :
- Stocker la liste complète comme `ConfigValue::List` sous la clé parente
- AUSSI stocker chaque élément individuellement sous `key.0`, `key.1`, etc. (pour compat env vars)

### Tests
```rust
#[test]
fn test_list_config() {
    let yaml = r#"
app:
  origins:
    - "http://localhost"
    - "https://prod.com"
"#;
    let config = R2eConfig::from_yaml_str(yaml, "test").unwrap();
    let origins: Vec<String> = config.get("app.origins").unwrap();
    assert_eq!(origins, vec!["http://localhost", "https://prod.com"]);
}
```

### Validation
```bash
cargo test -p r2e-core -- config
```

---

## Étape 3 — Typage fort avec `#[derive(ConfigProperties)]`

**Fichiers** :
- `r2e-macros/src/config_derive.rs` (proc macro)
- `r2e-core/src/config/typed.rs` (trait `ConfigProperties`)

### Objectif
Permettre de déclarer des sections de configuration comme des structs typées,
à la manière de `@ConfigurationProperties` en Spring Boot.

### API utilisateur cible
```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app.database")]
pub struct DatabaseConfig {
    /// Database connection URL
    pub url: String,

    /// Connection pool size (default: 10)
    #[config(default = 10)]
    pub pool_size: i64,

    /// Enable query logging
    #[config(default = false)]
    pub log_queries: bool,

    /// Optional connection timeout in seconds
    pub timeout: Option<i64>,
}
```

```yaml
# application.yaml
app:
  database:
    url: "postgres://localhost/mydb"
    pool_size: 20
```

```rust
// Usage dans un controller
#[derive(Controller)]
#[controller(path = "/api", state = AppState)]
pub struct ApiController {
    #[config_section]   // Nouveau attribut, résolu comme #[config] mais pour une section entière
    db_config: DatabaseConfig,
}

// OU usage directe
let db_config = DatabaseConfig::from_config(&config)?;
```

### Trait ConfigProperties
```rust
// r2e-core/src/config/typed.rs
pub trait ConfigProperties: Sized {
    /// Le préfixe de configuration (ex: "app.database")
    fn prefix() -> &'static str;

    /// Les propriétés attendues avec leurs défauts et descriptions
    fn properties_metadata() -> Vec<PropertyMeta>;

    /// Construire depuis une R2eConfig
    fn from_config(config: &R2eConfig) -> Result<Self, ConfigError>;
}

#[derive(Debug, Clone)]
pub struct PropertyMeta {
    pub key: String,           // clé relative (ex: "pool_size")
    pub full_key: String,      // clé absolue (ex: "app.database.pool_size")
    pub type_name: &'static str,
    pub required: bool,
    pub default_value: Option<String>,
    pub description: Option<String>,
}
```

### Proc macro `#[derive(ConfigProperties)]`
**Fichier** : `r2e-macros/src/config_derive.rs`

La macro doit :
1. Lire `#[config(prefix = "...")]` sur la struct
2. Pour chaque champ, lire l'éventuel `#[config(default = ...)]` et le doc comment
3. Générer `impl ConfigProperties` :
   - `prefix()` → retourne le préfixe
   - `properties_metadata()` → retourne la liste des PropertyMeta
   - `from_config()` → appelle `config.get::<T>("prefix.field")` pour chaque champ, avec gestion des défauts et des `Option`
4. Les champs `Option<T>` sont automatiquement optionnels
5. Les champs sans `#[config(default = ...)]` et sans `Option` sont requis

### Code généré attendu (pour DatabaseConfig)
```rust
impl ConfigProperties for DatabaseConfig {
    fn prefix() -> &'static str { "app.database" }

    fn properties_metadata() -> Vec<PropertyMeta> {
        vec![
            PropertyMeta {
                key: "url".into(),
                full_key: "app.database.url".into(),
                type_name: "String",
                required: true,
                default_value: None,
                description: Some("Database connection URL".into()),
            },
            PropertyMeta {
                key: "pool_size".into(),
                full_key: "app.database.pool_size".into(),
                type_name: "i64",
                required: false,
                default_value: Some("10".into()),
                description: Some("Connection pool size (default: 10)".into()),
            },
            // ...
        ]
    }

    fn from_config(config: &R2eConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            url: config.get::<String>("app.database.url")?,
            pool_size: config.get_or::<i64>("app.database.pool_size", 10),
            log_queries: config.get_or::<bool>("app.database.log_queries", false),
            timeout: config.get::<Option<i64>>("app.database.timeout")
                .unwrap_or(None),
        })
    }
}
```

### Intégration avec les macros existantes
**Fichier** : `r2e-macros/src/codegen/handlers.rs`

Ajouter la reconnaissance de `#[config_section]` sur un champ de controller.
Si le champ est annoté `#[config_section]`, le code généré dans le handler fait :
```rust
let db_config = DatabaseConfig::from_config(&state.config)
    .expect("Failed to load DatabaseConfig from config");
```

### Enregistrer dans `lib.rs` des macros
**Fichier** : `r2e-macros/src/lib.rs`
- Ajouter `mod config_derive;`
- Ajouter le proc-macro derive `ConfigProperties`

### Tests
```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "test.section")]
struct TestConfig {
    pub name: String,
    #[config(default = 42)]
    pub count: i64,
    pub optional_val: Option<String>,
}

#[test]
fn test_typed_config() {
    let mut config = R2eConfig::empty();
    config.set("test.section.name", ConfigValue::String("hello".into()));

    let tc = TestConfig::from_config(&config).unwrap();
    assert_eq!(tc.name, "hello");
    assert_eq!(tc.count, 42); // default
    assert!(tc.optional_val.is_none());
}

#[test]
fn test_typed_config_missing_required() {
    let config = R2eConfig::empty();
    let result = TestConfig::from_config(&config);
    assert!(result.is_err()); // "name" is required
}
```

### Validation
```bash
cargo test -p r2e-core -- config
cargo test -p r2e-macros
cargo check --workspace
```

---

## Étape 4 — Validation de configuration au démarrage

**Fichier** : `r2e-core/src/config/validation.rs`

### Objectif
Quand l'application démarre, valider que toutes les clés de config requises sont présentes
et que les types sont corrects. Afficher un rapport clair en cas d'erreur au lieu d'un
panic obscur plus tard.

### API cible
```rust
// Dans AppBuilder, ajouter :
impl<T> AppBuilder<T> {
    /// Register a config section for validation at startup.
    pub fn validate_config<C: ConfigProperties>(mut self) -> Self {
        self.config_validators.push(Box::new(|| {
            C::validate(&self.config)
        }));
        self
    }
}

// Usage
AppBuilder::new()
    .validate_config::<DatabaseConfig>()
    .validate_config::<SecurityConfig>()
    .build_state::<AppState, _>()
    .await
    // ...
```

### ConfigValidator

```rust
// r2e-core/src/config/validation.rs

#[derive(Debug)]
pub struct ConfigValidationError {
    pub section: String,     // "app.database"
    pub errors: Vec<PropertyError>,
}

#[derive(Debug)]
pub enum PropertyError {
    Missing { key: String, type_name: &'static str, description: Option<String> },
    TypeMismatch { key: String, expected: &'static str, actual: String },
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Configuration validation failed for section '{}':", self.section)?;
        for err in &self.errors {
            match err {
                PropertyError::Missing { key, type_name, description } => {
                    write!(f, "  ✗ Missing required key '{}' ({})", key, type_name)?;
                    if let Some(desc) = description {
                        write!(f, " — {}", desc)?;
                    }
                    writeln!(f)?;
                }
                PropertyError::TypeMismatch { key, expected, actual } => {
                    writeln!(f, "  ✗ Key '{}': expected {}, got {}", key, expected, actual)?;
                }
            }
        }
        Ok(())
    }
}

/// Validate a config against its metadata.
pub fn validate_section<C: ConfigProperties>(config: &R2eConfig) -> Result<(), ConfigValidationError> {
    let meta = C::properties_metadata();
    let mut errors = Vec::new();

    for prop in &meta {
        if prop.required {
            match config.get::<String>(&prop.full_key) {
                Err(ConfigError::NotFound(_)) => {
                    errors.push(PropertyError::Missing {
                        key: prop.full_key.clone(),
                        type_name: prop.type_name,
                        description: prop.description.clone(),
                    });
                }
                _ => {}
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(ConfigValidationError {
            section: C::prefix().to_string(),
            errors,
        })
    }
}
```

### Intégration dans AppBuilder
**Fichier** : `r2e-core/src/builder.rs`

Ajouter dans `BuilderConfig` :
```rust
config_validators: Vec<Box<dyn FnOnce(&R2eConfig) -> Result<(), ConfigValidationError> + Send>>,
```

Dans la méthode `build_state` ou dans `serve`, avant de lancer le serveur, itérer
sur les validators et afficher les erreurs. Si au moins une validation échoue,
panic avec un message lisible.

### Comportement attendu au démarrage (en cas d'erreur)
```
╔══════════════════════════════════════════════════════════════╗
║                 CONFIGURATION ERRORS                        ║
╠══════════════════════════════════════════════════════════════╣
║                                                              ║
║  Section: app.database                                       ║
║    ✗ Missing required key 'app.database.url' (String)        ║
║      — Database connection URL                               ║
║                                                              ║
║  Section: app.security                                       ║
║    ✗ Missing required key 'app.security.jwt-issuer' (String) ║
║                                                              ║
╚══════════════════════════════════════════════════════════════╝
```

### Tests
```rust
#[test]
fn test_validation_reports_missing_keys() {
    let config = R2eConfig::empty();
    let result = validate_section::<DatabaseConfig>(&config);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(err.errors.iter().any(|e| matches!(e, PropertyError::Missing { key, .. } if key == "app.database.url")));
}
```

### Validation
```bash
cargo test -p r2e-core -- config::validation
```

---

## Étape 5 — Support des secrets

**Fichier** : `r2e-core/src/config/secrets.rs`

### Objectif
Permettre de référencer des secrets dans la configuration sans les hardcoder dans le YAML.

### Syntaxe de résolution
```yaml
app:
  database:
    url: "${DATABASE_URL}"                    # Résolution env var
    password: "${env:DB_PASSWORD}"            # Syntaxe explicite env
    api-key: "${file:/run/secrets/api_key}"   # Lecture depuis fichier (Docker secrets)
```

### Implémentation

```rust
// r2e-core/src/config/secrets.rs

/// Trait for secret resolution backends.
pub trait SecretResolver: Send + Sync {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError>;
}

/// Default resolver: env vars and file references.
pub struct DefaultSecretResolver;

impl SecretResolver for DefaultSecretResolver {
    fn resolve(&self, reference: &str) -> Result<String, ConfigError> {
        if let Some(path) = reference.strip_prefix("file:") {
            std::fs::read_to_string(path.trim())
                .map(|s| s.trim().to_string())
                .map_err(|e| ConfigError::Load(format!("Secret file '{}': {}", path, e)))
        } else if let Some(var) = reference.strip_prefix("env:") {
            std::env::var(var.trim())
                .map_err(|_| ConfigError::NotFound(format!("env:{}", var)))
        } else {
            // Default: env var
            std::env::var(reference.trim())
                .map_err(|_| ConfigError::NotFound(reference.to_string()))
        }
    }
}

/// Resolve `${...}` placeholders in a string value.
pub fn resolve_placeholders(value: &str, resolver: &dyn SecretResolver) -> Result<String, ConfigError> {
    let mut result = value.to_string();
    // Regex: ${...} — attention, ne pas être greedy
    // Pattern simple: trouver "${", puis tout jusqu'à "}"
    while let Some(start) = result.find("${") {
        let end = result[start..].find('}')
            .ok_or_else(|| ConfigError::Load(format!("Unclosed placeholder in: {}", value)))?;
        let reference = &result[start + 2..start + end];
        let resolved = resolver.resolve(reference)?;
        result = format!("{}{}{}", &result[..start], resolved, &result[start + end + 1..]);
    }
    Ok(result)
}
```

### Intégration dans R2eConfig::load

Modifier `loader.rs` pour appeler `resolve_placeholders` sur chaque `ConfigValue::String`
après le chargement YAML et avant l'overlay des env vars.

Ajouter dans `R2eConfig` :
```rust
/// Load config with a custom secret resolver.
pub fn load_with_resolver(profile: &str, resolver: &dyn SecretResolver) -> Result<Self, ConfigError>

/// Load config (default resolver: env + file).
pub fn load(profile: &str) -> Result<Self, ConfigError> {
    Self::load_with_resolver(profile, &DefaultSecretResolver)
}
```

### Tests
```rust
#[test]
fn test_env_var_resolution() {
    std::env::set_var("TEST_DB_URL", "postgres://localhost/test");
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("${TEST_DB_URL}", &resolver).unwrap();
    assert_eq!(result, "postgres://localhost/test");
    std::env::remove_var("TEST_DB_URL");
}

#[test]
fn test_mixed_resolution() {
    std::env::set_var("HOST", "localhost");
    let resolver = DefaultSecretResolver;
    let result = resolve_placeholders("http://${HOST}:8080/api", &resolver).unwrap();
    assert_eq!(result, "http://localhost:8080/api");
    std::env::remove_var("HOST");
}

#[test]
fn test_file_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let secret_file = dir.path().join("secret.txt");
    std::fs::write(&secret_file, "my-secret-value\n").unwrap();

    let resolver = DefaultSecretResolver;
    let ref_str = format!("file:{}", secret_file.display());
    let result = resolve_placeholders(&format!("${{{}}}", ref_str), &resolver).unwrap();
    assert_eq!(result, "my-secret-value");
}
```

### Validation
```bash
cargo test -p r2e-core -- config::secrets
```

---

## Étape 6 — Registre de propriétés et introspection

**Fichier** : `r2e-core/src/config/registry.rs`

### Objectif
Permettre de lister toutes les propriétés de configuration connues de l'application,
avec leurs types, défauts et descriptions. Utile pour le debugging, la documentation
automatique, et la commande CLI `r2e config:show`.

### Implémentation
```rust
// r2e-core/src/config/registry.rs

use std::sync::Mutex;
use once_cell::sync::Lazy;

static CONFIG_REGISTRY: Lazy<Mutex<Vec<RegisteredSection>>> = Lazy::new(|| Mutex::new(Vec::new()));

#[derive(Debug, Clone)]
pub struct RegisteredSection {
    pub prefix: String,
    pub properties: Vec<PropertyMeta>, // réutilise PropertyMeta de typed.rs
}

/// Register a config section's metadata in the global registry.
pub fn register_section<C: ConfigProperties>() {
    let section = RegisteredSection {
        prefix: C::prefix().to_string(),
        properties: C::properties_metadata(),
    };
    CONFIG_REGISTRY.lock().unwrap().push(section);
}

/// Get all registered config sections.
pub fn registered_sections() -> Vec<RegisteredSection> {
    CONFIG_REGISTRY.lock().unwrap().clone()
}
```

### Intégration avec AppBuilder
Quand `validate_config::<C>()` est appelé, appeler aussi `register_section::<C>()`.

### Endpoint de debug (optionnel, dev-mode uniquement)
Ajouter dans le dev-mode (`r2e-core/src/dev.rs`) un endpoint :
```
GET /__r2e_dev/config → Liste toutes les sections enregistrées avec leurs propriétés
GET /__r2e_dev/config/values → Liste les valeurs actuelles (sans les secrets)
```

### Tests
```rust
#[test]
fn test_registry() {
    register_section::<DatabaseConfig>();
    let sections = registered_sections();
    assert!(sections.iter().any(|s| s.prefix == "app.database"));
}
```

---

## Étape 7 — Inclure dans la façade et ajouter la feature

**Fichier** : `r2e/src/lib.rs`

### Actions
1. Dans la façade `r2e`, re-exporter les nouveaux types :
   - `ConfigProperties`, `PropertyMeta`, `validate_section`
   - `SecretResolver`, `DefaultSecretResolver`
2. S'assurer que `ConfigProperties` est dans le prelude
3. Ajouter `ConfigProperties` au derive disponible via `r2e-macros`

**Fichier** : `r2e-core/src/prelude.rs`
```rust
pub use crate::config::{R2eConfig, ConfigValue, ConfigError, FromConfigValue, ConfigProperties};
```

### Validation finale
```bash
cargo check --workspace
cargo test --workspace
cargo run -p example-app  # Vérifier que rien ne casse
```

---

## Étape 8 — Mettre à jour l'example-app

**Fichier** : `example-app/src/`

### Actions
1. Créer une `AppConfig` typée dans l'example-app :
```rust
#[derive(ConfigProperties, Clone, Debug)]
#[config(prefix = "app")]
pub struct AppConfig {
    /// Application name
    pub name: String,
    /// Welcome greeting
    #[config(default = "Hello!")]
    pub greeting: String,
    /// Application version
    pub version: Option<String>,
}
```

2. Utiliser `validate_config::<AppConfig>()` dans le builder
3. Injecter `AppConfig` via `#[config_section]` dans le `ConfigController`

### Validation
```bash
cargo run -p example-app
# Vérifier les logs de startup
# Tester GET /__r2e_dev/config
```

---

## Ordre d'implémentation

| # | Étape | Fichiers | Dépendance |
|---|-------|----------|------------|
| 1 | Refactorer config.rs en module | r2e-core/src/config/ | Aucune |
| 2 | Support listes/maps | config/value.rs, config/loader.rs | Étape 1 |
| 3 | #[derive(ConfigProperties)] | r2e-macros + config/typed.rs | Étape 2 |
| 4 | Validation au démarrage | config/validation.rs + builder.rs | Étape 3 |
| 5 | Support secrets | config/secrets.rs + loader.rs | Étape 1 |
| 6 | Registre + introspection | config/registry.rs + dev.rs | Étape 3 |
| 7 | Façade et prelude | r2e/src/lib.rs + prelude.rs | Étape 3-6 |
| 8 | Mettre à jour example-app | example-app/ | Étape 7 |

## Dépendances Cargo à ajouter
- `r2e-core` : `tempfile` (dev-dependency, pour tests secrets fichier)
- Aucune nouvelle dépendance runtime (les résolutions env/file utilisent la stdlib)

## Critères de succès
- [ ] `cargo check --workspace` passe
- [ ] `cargo test --workspace` passe (tous les tests existants + nouveaux)
- [ ] L'example-app démarre sans erreur
- [ ] La validation affiche un rapport clair quand des clés manquent
- [ ] Les secrets `${...}` sont résolus dans le YAML
- [ ] `#[derive(ConfigProperties)]` fonctionne sur une struct
- [ ] Le endpoint dev `/__r2e_dev/config` affiche les propriétés connues
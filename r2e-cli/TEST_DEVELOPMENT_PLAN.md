# r2e-cli — Test Development Plan

## Current State

- **~1 test** (basic field parsing in generate.rs)
- **Coverage**: ~5%
- **Gap**: Project scaffolding, code generation, doctor checks, routes parsing, dev server — all untested

---

## Phase 1: Template Helpers (Pure Logic, Quick Win)

**File**: `src/commands/templates/mod.rs` — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `to_snake_case_basic` | `"UserController"` → `"user_controller"` |
| `to_snake_case_already_snake` | `"user_service"` → `"user_service"` |
| `to_snake_case_single_word` | `"User"` → `"user"` |
| `to_snake_case_acronym` | `"HTTPClient"` → `"http_client"` or `"h_t_t_p_client"` (verify actual behavior) |
| `to_pascal_case_basic` | `"user_service"` → `"UserService"` |
| `to_pascal_case_already_pascal` | `"UserService"` → `"UserService"` |
| `to_pascal_case_single_word` | `"user"` → `"User"` |
| `pluralize_regular` | `"user"` → `"users"` |
| `pluralize_y_ending` | `"category"` → `"categories"` |
| `pluralize_s_ending` | `"status"` → `"statuses"` |
| `pluralize_already_plural` | `"users"` → `"userss"` or special handling (verify) |
| `render_basic` | `render("Hello {{name}}", &[("name", "World")])` → `"Hello World"` |
| `render_multiple_placeholders` | Multiple `{{key}}` replacements |
| `render_missing_placeholder` | `{{unknown}}` left as-is or stripped (verify) |

---

## Phase 2: Field Parsing (CRUD Generation)

**File**: `src/commands/generate.rs` — extend `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `parse_field_string` | `"name:String"` → Field { name: "name", rust_type: "String", is_optional: false } |
| `parse_field_i64` | `"age:i64"` → correct Field |
| `parse_field_bool` | `"active:bool"` → correct Field |
| `parse_field_optional` | `"email:Option<String>"` → is_optional = true |
| `parse_multiple_fields` | `"name:String age:i64"` → 2 fields |
| `parse_field_invalid_format` | `"name"` (no type) → error |
| `sql_type_string` | `String` → `TEXT` |
| `sql_type_i64` | `i64` → `INTEGER` |
| `sql_type_f64` | `f64` → `REAL` |
| `sql_type_bool` | `bool` → `BOOLEAN` |

---

## Phase 3: Code Generation Output

**File**: `tests/generate_test.rs` (new integration test)

Uses `tempdir` for isolated file system operations.

### 3.1 Controller Generation

| Test | Description |
|------|-------------|
| `generate_controller_creates_file` | `r2e generate controller User` → `src/controllers/user.rs` exists |
| `generate_controller_valid_rust` | Generated file contains valid `#[derive(Controller)]` syntax |
| `generate_controller_updates_mod` | `src/controllers/mod.rs` gets `pub mod user;` appended |
| `generate_controller_custom_path` | Controller has `#[controller(path = "/users")]` |

### 3.2 Service Generation

| Test | Description |
|------|-------------|
| `generate_service_creates_file` | `r2e generate service User` → `src/user_service.rs` exists |
| `generate_service_valid_rust` | Generated file contains struct + impl block |

### 3.3 CRUD Generation

| Test | Description |
|------|-------------|
| `generate_crud_creates_all_files` | Model, service, controller, test files created |
| `generate_crud_model_has_fields` | Model struct contains specified fields |
| `generate_crud_controller_has_endpoints` | Controller has GET/POST/PUT/DELETE endpoints |
| `generate_crud_migration_sql` | Migration file contains CREATE TABLE with correct columns |
| `generate_crud_migration_timestamp` | Migration filename has timestamp prefix |

### 3.4 Middleware Generation

| Test | Description |
|------|-------------|
| `generate_middleware_creates_file` | `r2e generate middleware Auth` → `src/middleware/auth.rs` exists |
| `generate_middleware_has_interceptor` | File contains `impl Interceptor` skeleton |

---

## Phase 4: Doctor Command

**File**: `tests/doctor_test.rs` (new integration test)

Uses `tempdir` with minimal project structures.

| Test | Description |
|------|-------------|
| `doctor_missing_cargo_toml` | No Cargo.toml → Error check |
| `doctor_missing_r2e_dep` | Cargo.toml without r2e → Error check |
| `doctor_valid_project` | Full valid project → all checks pass |
| `doctor_missing_config` | No application.yaml → Warning check |
| `doctor_missing_controllers_dir` | No src/controllers/ → Warning check |
| `doctor_missing_main_serve` | main.rs without `.serve()` → Warning check |

---

## Phase 5: Routes Command

**File**: `tests/routes_test.rs` (new integration test)

Uses temp directories with sample controller files.

| Test | Description |
|------|-------------|
| `routes_extracts_controller_path` | `#[controller(path = "/users")]` → path extracted |
| `routes_extracts_get` | `#[get("/")]` → GET method found |
| `routes_extracts_post` | `#[post("/")]` → POST method found |
| `routes_extracts_all_verbs` | GET/POST/PUT/DELETE/PATCH all found |
| `routes_combines_paths` | Controller path + method path → full route |
| `routes_extracts_roles` | `#[roles("admin")]` → roles listed |
| `routes_extracts_handler_name` | Next `fn` after attribute → name captured |
| `routes_empty_dir` | No controller files → empty output |

---

## Phase 6: Project Scaffolding (`r2e new`)

**File**: `tests/new_test.rs` (new integration test)

Each test creates a temp directory and runs `r2e new` logic.

| Test | Description |
|------|-------------|
| `new_creates_project_dir` | Project directory created |
| `new_creates_cargo_toml` | `Cargo.toml` with r2e dependency |
| `new_creates_main_rs` | `src/main.rs` with AppBuilder and `.serve()` |
| `new_creates_application_yaml` | `application.yaml` configuration file |
| `new_with_db_sqlite` | `--db sqlite` → SQLx + SQLite deps, migrations/ dir |
| `new_with_auth` | `--auth` → r2e-security dep, JwtClaimsValidator setup |
| `new_with_openapi` | `--openapi` → OpenApiPlugin in builder |
| `new_full` | `--full` → all features enabled |
| `new_no_interactive` | `--no-interactive` → uses defaults without prompting |

---

## Phase 7: Add Command

**File**: `tests/add_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `add_security` | `r2e add security` → adds r2e-security to Cargo.toml |
| `add_data` | `r2e add data` → adds r2e-data to Cargo.toml |
| `add_unknown_extension` | `r2e add unknown` → error message |
| `add_already_present` | Adding already-present dep → no duplication |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 14 | 1.5h | None |
| Phase 2 | 10 | 1h | None |
| Phase 3 | 12 | 4h | tempdir |
| Phase 4 | 6 | 2h | tempdir |
| Phase 5 | 8 | 2h | tempdir + sample files |
| Phase 6 | 9 | 3h | tempdir |
| Phase 7 | 4 | 1h | tempdir |
| **Total** | **63** | **~14.5h** | |

# r2e-openapi — Test Development Plan

## Current State

- **0 tests**
- **Coverage**: 0%
- **Gap**: Schema registration, spec generation, $ref rewriting, route metadata mapping, docs UI — all untested

---

## Phase 1: SchemaRegistry (Unit Tests)

**File**: `src/lib.rs` or relevant module — add `#[cfg(test)] mod tests`

| Test | Description |
|------|-------------|
| `registry_new_empty` | `SchemaRegistry::new()` → empty, no schemas |
| `register_single_schema` | `register::<User>()` → schema accessible |
| `register_object_schema` | `register_object(name, schema)` → stored by name |
| `register_duplicate` | Registering same type twice → no duplication |
| `contains_registered` | `contains("User")` → `true` after registration |
| `contains_unregistered` | `contains("Unknown")` → `false` |
| `into_schemas_output` | `into_schemas()` → correct JSON Schema map |

---

## Phase 2: SchemaProvider Trait

| Test | Description |
|------|-------------|
| `schema_name_returns_type_name` | `SchemaProvider::schema_name()` matches type |
| `json_schema_valid_structure` | Output is valid JSON Schema Draft 7 |
| `register_schema_populates_registry` | `register_schema(&mut registry)` adds schemas |

---

## Phase 3: Spec Generation (`build_spec`)

**File**: `tests/spec_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `empty_spec` | No routes → valid OpenAPI spec with info only |
| `spec_has_openapi_version` | `openapi` field is `"3.0.3"` |
| `spec_has_info` | Title and version from `OpenApiConfig` present |
| `spec_has_description` | Optional description included when set |
| `single_get_route` | One GET endpoint → correct path and operation |
| `route_with_path_param` | `/users/{id}` → path parameter with `in: "path"` |
| `route_with_query_param` | Query parameters → `in: "query"` parameters |
| `route_with_request_body` | POST with JSON body → requestBody schema |
| `route_with_response_schema` | Response type → 200 response with schema |
| `route_with_roles` | `#[roles("admin")]` → security requirement in operation |
| `multiple_routes_same_path` | GET + POST on `/users` → both operations under same path |
| `multiple_paths` | Multiple controllers → all paths in spec |

---

## Phase 4: Schema Sanitization

| Test | Description |
|------|-------------|
| `ref_rewrite_definitions_to_components` | `$ref: #/definitions/Foo` → `$ref: #/components/schemas/Foo` |
| `additional_properties_bool_sanitized` | `additionalProperties: false` handled correctly for OpenAPI 3.0 |
| `nested_ref_rewrite` | Deeply nested `$ref` paths all rewritten |
| `definitions_promoted_to_components` | Top-level `definitions` moved to `components.schemas` |

---

## Phase 5: OpenApiConfig

| Test | Description |
|------|-------------|
| `config_new` | `OpenApiConfig::new(title, version)` stores fields |
| `config_with_description` | `.with_description(desc)` sets description |
| `config_with_docs_ui_true` | `.with_docs_ui(true)` enables docs |
| `config_with_docs_ui_false` | `.with_docs_ui(false)` disables docs |

---

## Phase 6: Plugin & Routes Integration

**File**: `tests/plugin_test.rs` (new integration test)

| Test | Description |
|------|-------------|
| `openapi_json_endpoint` | `GET /openapi.json` → 200 with valid JSON |
| `openapi_json_content_type` | Response has `application/json` content-type |
| `docs_ui_when_enabled` | `GET /docs` → 200 with HTML |
| `docs_ui_when_disabled` | `GET /docs` → 404 when `docs_ui = false` |
| `docs_css_served` | `GET /docs/wti-element.css` → 200 |
| `docs_js_served` | `GET /docs/wti-element.js` → 200 |
| `spec_includes_registered_controllers` | After `register_controller()` → routes in spec |

---

## Phase 7: Validation Against OpenAPI Spec

| Test | Description |
|------|-------------|
| `generated_spec_is_valid_openapi` | Parse output with an OpenAPI validator → no errors |
| `generated_spec_paths_non_empty` | At least one path when controllers registered |
| `generated_spec_components_present` | Schemas referenced by routes exist in components |

---

## Estimated Effort

| Phase | Tests | Effort | Dependencies |
|-------|-------|--------|-------------|
| Phase 1 | 7 | 1.5h | None |
| Phase 2 | 3 | 1h | schemars |
| Phase 3 | 12 | 4h | Route metadata fixtures |
| Phase 4 | 4 | 1.5h | JSON fixtures |
| Phase 5 | 4 | 30m | None |
| Phase 6 | 7 | 2.5h | axum test utils |
| Phase 7 | 3 | 1h | OpenAPI validator crate |
| **Total** | **40** | **~12h** | |

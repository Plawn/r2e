use r2e_core::meta::{ParamInfo, ParamLocation, RouteInfo};
use r2e_openapi::{build_spec, OpenApiConfig};
use serde_json::{json, Value};

// ── Helpers ─────────────────────────────────────────────────────────────────

fn default_config() -> OpenApiConfig {
    OpenApiConfig::new("Test API", "0.1.0")
}

fn route(method: &str, path: &str, operation_id: &str) -> RouteInfo {
    RouteInfo {
        path: path.to_string(),
        method: method.to_string(),
        operation_id: operation_id.to_string(),
        summary: None,
        description: None,
        request_body_type: None,
        request_body_schema: None,
        request_body_required: true,
        response_type: None,
        response_schema: None,
        response_status: 200,
        params: vec![],
        roles: vec![],
        tag: None,
        deprecated: false,
        has_auth: false,
    }
}

// ── Phase 3: Spec Generation (build_spec) ───────────────────────────────────

#[test]
fn empty_spec() {
    let spec = build_spec(&default_config(), &[]);
    assert!(spec["paths"].as_object().unwrap().is_empty());
    assert_eq!(spec["openapi"], "3.1.0");
    assert_eq!(spec["info"]["title"], "Test API");
}

#[test]
fn spec_has_openapi_version() {
    let spec = build_spec(&default_config(), &[]);
    assert_eq!(spec["openapi"], "3.1.0");
}

#[test]
fn spec_has_info() {
    let config = OpenApiConfig::new("My Service", "2.0.0");
    let spec = build_spec(&config, &[]);
    assert_eq!(spec["info"]["title"], "My Service");
    assert_eq!(spec["info"]["version"], "2.0.0");
}

#[test]
fn spec_has_description() {
    let config = OpenApiConfig::new("API", "1.0.0").with_description("A test API");
    let spec = build_spec(&config, &[]);
    assert_eq!(spec["info"]["description"], "A test API");
}

#[test]
fn spec_without_description() {
    let spec = build_spec(&default_config(), &[]);
    assert!(spec["info"].get("description").is_none());
}

#[test]
fn single_get_route() {
    let routes = vec![route("GET", "/users", "list_users")];
    let spec = build_spec(&default_config(), &routes);

    let paths = spec["paths"].as_object().unwrap();
    assert!(paths.contains_key("/users"));

    let get_op = &spec["paths"]["/users"]["get"];
    assert_eq!(get_op["operationId"], "list_users");
}

#[test]
fn route_with_path_param() {
    let routes = vec![RouteInfo {
        params: vec![ParamInfo {
            name: "id".to_string(),
            location: ParamLocation::Path,
            param_type: "integer".to_string(),
            required: true,
        }],
        ..route("GET", "/users/{id}", "get_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let params = spec["paths"]["/users/{id}"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["name"], "id");
    assert_eq!(params[0]["in"], "path");
    assert_eq!(params[0]["required"], true);
    assert_eq!(params[0]["schema"]["type"], "integer");
}

#[test]
fn route_with_query_param() {
    let routes = vec![RouteInfo {
        params: vec![ParamInfo {
            name: "page".to_string(),
            location: ParamLocation::Query,
            param_type: "integer".to_string(),
            required: false,
        }],
        ..route("GET", "/users", "list_users")
    }];
    let spec = build_spec(&default_config(), &routes);

    let params = spec["paths"]["/users"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert_eq!(params[0]["name"], "page");
    assert_eq!(params[0]["in"], "query");
    assert_eq!(params[0]["required"], false);
}

#[test]
fn route_with_request_body() {
    let routes = vec![RouteInfo {
        request_body_type: Some("CreateUser".to_string()),
        ..route("POST", "/users", "create_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let req_body = &spec["paths"]["/users"]["post"]["requestBody"];
    assert_eq!(req_body["required"], true);
    assert_eq!(
        req_body["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/CreateUser"
    );

    // Schema should be in components
    assert!(spec["components"]["schemas"]["CreateUser"].is_object());
}

#[test]
fn route_with_request_body_schema() {
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("CreateUser".to_string()),
        request_body_schema: Some(schema.clone()),
        ..route("POST", "/users", "create_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let component_schema = &spec["components"]["schemas"]["CreateUser"];
    assert_eq!(component_schema["type"], "object");
    assert_eq!(component_schema["properties"]["name"]["type"], "string");
}

#[test]
fn route_with_roles() {
    let routes = vec![RouteInfo {
        roles: vec!["admin".to_string()],
        ..route("DELETE", "/users/{id}", "delete_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let security = spec["paths"]["/users/{id}"]["delete"]["security"]
        .as_array()
        .unwrap();
    assert_eq!(security.len(), 1);
    assert_eq!(security[0]["bearerAuth"], json!(["admin"]));
}

#[test]
fn multiple_routes_same_path() {
    let routes = vec![
        route("GET", "/users", "list_users"),
        route("POST", "/users", "create_user"),
    ];
    let spec = build_spec(&default_config(), &routes);

    let path = spec["paths"]["/users"].as_object().unwrap();
    assert!(path.contains_key("get"));
    assert!(path.contains_key("post"));
}

#[test]
fn multiple_paths() {
    let routes = vec![
        route("GET", "/users", "list_users"),
        route("GET", "/roles", "list_roles"),
        route("GET", "/health", "health"),
    ];
    let spec = build_spec(&default_config(), &routes);

    let paths = spec["paths"].as_object().unwrap();
    assert_eq!(paths.len(), 3);
    assert!(paths.contains_key("/users"));
    assert!(paths.contains_key("/roles"));
    assert!(paths.contains_key("/health"));
}

#[test]
fn route_with_tag() {
    let routes = vec![RouteInfo {
        tag: Some("Users".to_string()),
        ..route("GET", "/users", "list_users")
    }];
    let spec = build_spec(&default_config(), &routes);

    let tags = spec["paths"]["/users"]["get"]["tags"].as_array().unwrap();
    assert_eq!(tags, &[json!("Users")]);
}

#[test]
fn route_with_summary() {
    let routes = vec![RouteInfo {
        summary: Some("List all users".to_string()),
        ..route("GET", "/users", "list_users")
    }];
    let spec = build_spec(&default_config(), &routes);
    assert_eq!(
        spec["paths"]["/users"]["get"]["summary"],
        "List all users"
    );
}

#[test]
fn route_without_params_has_no_parameters_key() {
    let routes = vec![route("GET", "/users", "list_users")];
    let spec = build_spec(&default_config(), &routes);
    assert!(spec["paths"]["/users"]["get"].get("parameters").is_none());
}

#[test]
fn route_without_roles_has_no_security_key() {
    let routes = vec![route("GET", "/users", "list_users")];
    let spec = build_spec(&default_config(), &routes);
    assert!(spec["paths"]["/users"]["get"].get("security").is_none());
}

#[test]
fn spec_has_security_schemes() {
    let spec = build_spec(&default_config(), &[]);
    let bearer = &spec["components"]["securitySchemes"]["bearerAuth"];
    assert_eq!(bearer["type"], "http");
    assert_eq!(bearer["scheme"], "bearer");
    assert_eq!(bearer["bearerFormat"], "JWT");
}

#[test]
fn responses_present_without_auth() {
    let routes = vec![route("GET", "/users", "list_users")];
    let spec = build_spec(&default_config(), &routes);

    let responses = &spec["paths"]["/users"]["get"]["responses"];
    assert!(responses["200"].is_object());
    // No 401/403 when has_auth is false
    assert!(responses.get("401").is_none());
    assert!(responses.get("403").is_none());
}

// ── Phase 4: Schema Sanitization ────────────────────────────────────────────

#[test]
fn ref_rewrite_definitions_to_components() {
    let schema = json!({
        "type": "object",
        "properties": {
            "role": { "$ref": "#/$defs/Role" }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("User".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/users", "create_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let user_schema = &spec["components"]["schemas"]["User"];
    assert_eq!(
        user_schema["properties"]["role"]["$ref"],
        "#/components/schemas/Role"
    );
}

#[test]
fn additional_properties_true_removed() {
    // schemars 1.x no longer emits `additionalProperties: true`, but if a
    // schema happens to include it, it passes through unchanged. Verify the
    // builder doesn't strip arbitrary keys.
    let schema = json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {
            "name": { "type": "string" }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("Data".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/data", "create_data")
    }];
    let spec = build_spec(&default_config(), &routes);

    let data_schema = &spec["components"]["schemas"]["Data"];
    // additionalProperties is kept as-is (no special stripping in 3.1.0)
    assert_eq!(data_schema["additionalProperties"], json!(true));
}

#[test]
fn additional_properties_false_kept() {
    let schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "name": { "type": "string" }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("Strict".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/strict", "create_strict")
    }];
    let spec = build_spec(&default_config(), &routes);

    let strict_schema = &spec["components"]["schemas"]["Strict"];
    assert_eq!(strict_schema["additionalProperties"], json!(false));
}

#[test]
fn nested_ref_rewrite() {
    let schema = json!({
        "type": "object",
        "properties": {
            "items": {
                "type": "array",
                "items": { "$ref": "#/$defs/Item" }
            }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("Order".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/orders", "create_order")
    }];
    let spec = build_spec(&default_config(), &routes);

    let items_ref = &spec["components"]["schemas"]["Order"]["properties"]["items"]["items"]["$ref"];
    assert_eq!(items_ref, "#/components/schemas/Item");
}

#[test]
fn definitions_promoted_to_components() {
    let schema = json!({
        "type": "object",
        "properties": {
            "role": { "$ref": "#/$defs/Role" }
        },
        "$defs": {
            "Role": {
                "type": "string",
                "enum": ["admin", "user"]
            }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("User".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/users", "create_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    // $defs should be promoted to components/schemas
    let role = &spec["components"]["schemas"]["Role"];
    assert_eq!(role["type"], "string");
    assert_eq!(role["enum"], json!(["admin", "user"]));

    // User schema should not contain $defs key
    let user = &spec["components"]["schemas"]["User"];
    assert!(user.get("$defs").is_none());
}

#[test]
fn schema_key_stripped() {
    let schema = json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": {
            "name": { "type": "string" }
        }
    });
    let routes = vec![RouteInfo {
        request_body_type: Some("Data".to_string()),
        request_body_schema: Some(schema),
        ..route("POST", "/data", "create_data")
    }];
    let spec = build_spec(&default_config(), &routes);

    let data_schema = &spec["components"]["schemas"]["Data"];
    assert!(data_schema.get("$schema").is_none());
}

// ── Phase 7: Validation Against OpenAPI Spec ────────────────────────────────

#[test]
fn generated_spec_is_valid_openapi_structure() {
    let routes = vec![
        RouteInfo {
            tag: Some("Users".to_string()),
            params: vec![ParamInfo {
                name: "id".to_string(),
                location: ParamLocation::Path,
                param_type: "integer".to_string(),
                required: true,
            }],
            ..route("GET", "/users/{id}", "get_user")
        },
        RouteInfo {
            request_body_type: Some("CreateUser".to_string()),
            request_body_schema: Some(json!({
                "type": "object",
                "properties": { "name": { "type": "string" } }
            })),
            ..route("POST", "/users", "create_user")
        },
    ];
    let config = OpenApiConfig::new("Full API", "1.0.0").with_description("Complete test");
    let spec = build_spec(&config, &routes);

    // Top-level keys
    assert_eq!(spec["openapi"], "3.1.0");
    assert!(spec["info"].is_object());
    assert!(spec["paths"].is_object());
    assert!(spec["components"].is_object());

    // Serializes to valid JSON
    let json_str = serde_json::to_string_pretty(&spec).unwrap();
    let reparsed: Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(spec, reparsed);
}

#[test]
fn generated_spec_paths_non_empty() {
    let routes = vec![route("GET", "/health", "health_check")];
    let spec = build_spec(&default_config(), &routes);
    let paths = spec["paths"].as_object().unwrap();
    assert!(!paths.is_empty());
}

#[test]
fn generated_spec_components_present() {
    let routes = vec![RouteInfo {
        request_body_type: Some("Payload".to_string()),
        request_body_schema: Some(json!({"type": "object"})),
        ..route("POST", "/submit", "submit")
    }];
    let spec = build_spec(&default_config(), &routes);

    // The referenced schema exists in components
    assert!(spec["components"]["schemas"]["Payload"].is_object());

    // The $ref in the request body points to it
    let ref_path = spec["paths"]["/submit"]["post"]["requestBody"]["content"]["application/json"]
        ["schema"]["$ref"]
        .as_str()
        .unwrap();
    assert_eq!(ref_path, "#/components/schemas/Payload");
}

#[test]
fn duplicate_body_types_not_duplicated() {
    let schema = json!({"type": "object", "properties": {"name": {"type": "string"}}});
    let routes = vec![
        RouteInfo {
            request_body_type: Some("User".to_string()),
            request_body_schema: Some(schema.clone()),
            ..route("POST", "/users", "create_user")
        },
        RouteInfo {
            request_body_type: Some("User".to_string()),
            request_body_schema: Some(schema),
            ..route("PUT", "/users/{id}", "update_user")
        },
    ];
    let spec = build_spec(&default_config(), &routes);

    let schemas = spec["components"]["schemas"].as_object().unwrap();
    // Only one "User" schema even though two routes reference it
    assert_eq!(
        schemas.keys().filter(|k| *k == "User").count(),
        1
    );
}

#[test]
fn request_body_without_schema_gets_generic_object() {
    let routes = vec![RouteInfo {
        request_body_type: Some("Unknown".to_string()),
        request_body_schema: None,
        ..route("POST", "/submit", "submit")
    }];
    let spec = build_spec(&default_config(), &routes);

    let schema = &spec["components"]["schemas"]["Unknown"];
    assert_eq!(schema, &json!({"type": "object"}));
}

#[test]
fn header_param_location() {
    let routes = vec![RouteInfo {
        params: vec![ParamInfo {
            name: "X-Request-Id".to_string(),
            location: ParamLocation::Header,
            param_type: "string".to_string(),
            required: true,
        }],
        ..route("GET", "/data", "get_data")
    }];
    let spec = build_spec(&default_config(), &routes);

    let params = spec["paths"]["/data"]["get"]["parameters"]
        .as_array()
        .unwrap();
    assert_eq!(params[0]["in"], "header");
    assert_eq!(params[0]["name"], "X-Request-Id");
}

#[test]
fn multiple_roles_in_security() {
    let routes = vec![RouteInfo {
        roles: vec!["admin".to_string(), "manager".to_string()],
        ..route("DELETE", "/users/{id}", "delete_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let security = &spec["paths"]["/users/{id}"]["delete"]["security"][0]["bearerAuth"];
    assert_eq!(security, &json!(["admin", "manager"]));
}

// ── New feature tests ───────────────────────────────────────────────────────

#[test]
fn response_schema_in_spec() {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer" },
            "name": { "type": "string" }
        }
    });
    let routes = vec![RouteInfo {
        response_type: Some("User".to_string()),
        response_schema: Some(schema.clone()),
        ..route("GET", "/users/{id}", "get_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let resp = &spec["paths"]["/users/{id}"]["get"]["responses"]["200"];
    assert_eq!(
        resp["content"]["application/json"]["schema"]["$ref"],
        "#/components/schemas/User"
    );

    // Schema should be in components
    let user_schema = &spec["components"]["schemas"]["User"];
    assert_eq!(user_schema["type"], "object");
    assert_eq!(user_schema["properties"]["name"]["type"], "string");
}

#[test]
fn response_204_has_no_content_block() {
    let routes = vec![RouteInfo {
        response_status: 204,
        ..route("DELETE", "/users/{id}", "delete_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let resp = &spec["paths"]["/users/{id}"]["delete"]["responses"]["204"];
    assert_eq!(resp["description"], "No content");
    assert!(resp.get("content").is_none());
}

#[test]
fn post_defaults_to_201() {
    let routes = vec![RouteInfo {
        response_status: 201,
        response_type: Some("User".to_string()),
        response_schema: Some(json!({"type": "object"})),
        ..route("POST", "/users", "create_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let responses = &spec["paths"]["/users"]["post"]["responses"];
    assert!(responses.get("201").is_some());
    assert_eq!(responses["201"]["description"], "Created");
}

#[test]
fn status_override() {
    let routes = vec![RouteInfo {
        response_status: 202,
        ..route("POST", "/jobs", "create_job")
    }];
    let spec = build_spec(&default_config(), &routes);

    let responses = &spec["paths"]["/jobs"]["post"]["responses"];
    assert!(responses.get("202").is_some());
    assert_eq!(responses["202"]["description"], "Successful response");
}

#[test]
fn no_401_403_without_auth() {
    let routes = vec![RouteInfo {
        has_auth: false,
        ..route("GET", "/public", "public_endpoint")
    }];
    let spec = build_spec(&default_config(), &routes);

    let responses = &spec["paths"]["/public"]["get"]["responses"];
    assert!(responses.get("401").is_none());
    assert!(responses.get("403").is_none());
}

#[test]
fn auth_route_has_401_403() {
    let routes = vec![RouteInfo {
        has_auth: true,
        roles: vec!["admin".to_string()],
        ..route("GET", "/admin", "admin_endpoint")
    }];
    let spec = build_spec(&default_config(), &routes);

    let responses = &spec["paths"]["/admin"]["get"]["responses"];
    assert!(responses["401"].is_object());
    assert_eq!(responses["401"]["description"], "Unauthorized");
    assert!(responses["403"].is_object());
    assert_eq!(responses["403"]["description"], "Forbidden");
}

#[test]
fn deprecated_flag_in_spec() {
    let routes = vec![RouteInfo {
        deprecated: true,
        ..route("GET", "/v1/users", "list_users_v1")
    }];
    let spec = build_spec(&default_config(), &routes);

    assert_eq!(
        spec["paths"]["/v1/users"]["get"]["deprecated"],
        json!(true)
    );
}

#[test]
fn non_deprecated_has_no_deprecated_key() {
    let routes = vec![route("GET", "/users", "list_users")];
    let spec = build_spec(&default_config(), &routes);

    assert!(spec["paths"]["/users"]["get"].get("deprecated").is_none());
}

#[test]
fn optional_request_body() {
    let routes = vec![RouteInfo {
        request_body_type: Some("PatchUser".to_string()),
        request_body_required: false,
        ..route("PATCH", "/users/{id}", "patch_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    let req_body = &spec["paths"]["/users/{id}"]["patch"]["requestBody"];
    assert_eq!(req_body["required"], false);
}

#[test]
fn route_with_description() {
    let routes = vec![RouteInfo {
        summary: Some("List all users".to_string()),
        description: Some("Returns a paginated list of active users.".to_string()),
        ..route("GET", "/users", "list_users")
    }];
    let spec = build_spec(&default_config(), &routes);

    assert_eq!(
        spec["paths"]["/users"]["get"]["summary"],
        "List all users"
    );
    assert_eq!(
        spec["paths"]["/users"]["get"]["description"],
        "Returns a paginated list of active users."
    );
}

#[test]
fn response_schema_ref_rewrite() {
    let schema = json!({
        "type": "object",
        "properties": {
            "role": { "$ref": "#/$defs/Role" }
        },
        "$defs": {
            "Role": {
                "type": "string",
                "enum": ["admin", "user"]
            }
        }
    });
    let routes = vec![RouteInfo {
        response_type: Some("User".to_string()),
        response_schema: Some(schema),
        ..route("GET", "/users/{id}", "get_user")
    }];
    let spec = build_spec(&default_config(), &routes);

    // $ref should be rewritten
    let user_schema = &spec["components"]["schemas"]["User"];
    assert_eq!(
        user_schema["properties"]["role"]["$ref"],
        "#/components/schemas/Role"
    );

    // $defs should be promoted
    let role = &spec["components"]["schemas"]["Role"];
    assert_eq!(role["type"], "string");
}

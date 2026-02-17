use r2e_security::openid::{
    extract_string_array, Composite, Merge, RoleExtractor, StandardRoleExtractor,
};
use serde_json::json;

#[test]
fn test_standard_role_extractor() {
    let claims = json!({
        "sub": "user123",
        "roles": ["admin", "user"]
    });

    let extractor = StandardRoleExtractor;
    let roles = extractor.extract_roles(&claims);

    assert_eq!(roles, vec!["admin", "user"]);
}

#[test]
fn test_standard_role_extractor_empty() {
    let claims = json!({
        "sub": "user123"
    });

    let extractor = StandardRoleExtractor;
    let roles = extractor.extract_roles(&claims);

    assert!(roles.is_empty());
}

#[test]
fn test_composite_extractor_first_match() {
    #[derive(Clone, Copy)]
    struct FirstExtractor;
    impl RoleExtractor for FirstExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec!["first".to_string()]
        }
    }

    #[derive(Clone, Copy)]
    struct SecondExtractor;
    impl RoleExtractor for SecondExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec!["second".to_string()]
        }
    }

    let extractor = Composite(FirstExtractor, SecondExtractor);
    let roles = extractor.extract_roles(&json!({}));
    assert_eq!(roles, vec!["first"]);
}

#[test]
fn test_composite_extractor_fallback() {
    #[derive(Clone, Copy)]
    struct EmptyExtractor;
    impl RoleExtractor for EmptyExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec![]
        }
    }

    #[derive(Clone, Copy)]
    struct FallbackExtractor;
    impl RoleExtractor for FallbackExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec!["fallback".to_string()]
        }
    }

    let extractor = Composite(EmptyExtractor, FallbackExtractor);
    let roles = extractor.extract_roles(&json!({}));
    assert_eq!(roles, vec!["fallback"]);
}

#[test]
fn test_merge_extractor() {
    #[derive(Clone, Copy)]
    struct FirstExtractor;
    impl RoleExtractor for FirstExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec!["admin".to_string(), "user".to_string()]
        }
    }

    #[derive(Clone, Copy)]
    struct SecondExtractor;
    impl RoleExtractor for SecondExtractor {
        fn extract_roles(&self, _: &serde_json::Value) -> Vec<String> {
            vec!["user".to_string(), "guest".to_string()]
        }
    }

    let extractor = Merge(FirstExtractor, SecondExtractor);
    let roles = extractor.extract_roles(&json!({}));
    // "user" should be deduplicated
    assert_eq!(roles, vec!["admin", "user", "guest"]);
}

#[test]
fn test_extract_string_array_nested() {
    let value = json!({
        "a": {
            "b": {
                "c": ["x", "y", "z"]
            }
        }
    });

    let result = extract_string_array(&value, &["a", "b", "c"]);
    assert_eq!(result, vec!["x", "y", "z"]);
}

#[test]
fn test_extract_string_array_missing_path() {
    let value = json!({
        "a": {
            "b": {}
        }
    });

    let result = extract_string_array(&value, &["a", "b", "c"]);
    assert!(result.is_empty());
}

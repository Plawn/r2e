use r2e_core::interceptors::{Cacheable, InterceptorContext};
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct User {
    name: String,
    age: u32,
}

#[test]
fn json_cacheable_roundtrip() {
    let original = Json(User {
        name: "Alice".into(),
        age: 30,
    });
    let bytes = original.to_cache().expect("should serialize");
    let restored = Json::<User>::from_cache(&bytes).expect("should deserialize");
    assert_eq!(original.0, restored.0);
}

#[test]
fn json_cacheable_invalid_bytes() {
    let result = Json::<User>::from_cache(b"not valid json");
    assert!(result.is_none());
}

#[test]
fn result_ok_caches() {
    let val: Result<Json<User>, String> = Ok(Json(User {
        name: "Bob".into(),
        age: 25,
    }));
    let bytes = val.to_cache();
    assert!(bytes.is_some());
}

#[test]
fn result_err_skips_cache() {
    let val: Result<Json<User>, String> = Err("failure".into());
    let bytes = val.to_cache();
    assert!(bytes.is_none());
}

#[test]
fn result_from_cache_wraps_ok() {
    let original = Json(User {
        name: "Carol".into(),
        age: 40,
    });
    let bytes = original.to_cache().unwrap();
    let restored: Option<Result<Json<User>, String>> =
        Result::<Json<User>, String>::from_cache(&bytes);
    assert!(restored.is_some());
    let inner = restored.unwrap().unwrap();
    assert_eq!(inner.0, original.0);
}

#[test]
fn interceptor_context_accessors() {
    let state = 42u32;
    let ctx = InterceptorContext {
        method_name: "create",
        controller_name: "UserCtrl",
        state: &state,
    };
    assert_eq!(ctx.method_name, "create");
    assert_eq!(ctx.controller_name, "UserCtrl");
    assert_eq!(*ctx.state, 42u32);
}

#[test]
fn json_cacheable_empty_struct() {
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Empty {}
    let original = Json(Empty {});
    let bytes = original.to_cache().expect("should serialize");
    let restored = Json::<Empty>::from_cache(&bytes).expect("should deserialize");
    assert_eq!(original.0, restored.0);
}

//! `StandardClaims` — the typed claim set behind `Identity::claims()`.
//!
//! The type is defined in `r2e-core` (the `Identity` trait names it) and
//! re-exported by `r2e-security`, which is where every claim-consuming API
//! lives; the tests therefore sit with the security crate.

use r2e_security::keycloak::{ClientRoleExtractor, RealmRoleExtractor};
use r2e_security::openid::{Merge, RoleExtractor, StandardRoleExtractor};
use r2e_security::{Audience, StandardClaims};
use serde_json::json;

fn keycloak_payload() -> serde_json::Value {
    json!({
        "sub": "user-uuid",
        "iss": "https://kc.example.com/realms/demo",
        "aud": "my-api",
        "exp": 2_000_000_000u64,
        "iat": 1_999_996_400u64,
        "email": "alice@example.com",
        "preferred_username": "alice",
        "name": "Alice Example",
        "scope": "openid profile email",
        "realm_access": { "roles": ["realm-admin", "realm-user"] },
        "resource_access": {
            "my-api": { "roles": ["api-admin", "api-reader"] },
            "other": { "roles": ["other-role"] }
        },
        "tenant_id": "acme",
        "custom": { "nested": ["a", "b"] }
    })
}

fn parse(value: serde_json::Value) -> StandardClaims {
    serde_json::from_value(value).expect("payload deserializes into StandardClaims")
}

// ── Deserialization ──

#[test]
fn deserializes_a_keycloak_shaped_payload() {
    let claims = parse(keycloak_payload());

    assert_eq!(claims.sub, "user-uuid");
    assert_eq!(claims.email.as_deref(), Some("alice@example.com"));
    assert_eq!(claims.preferred_username.as_deref(), Some("alice"));
    assert_eq!(claims.name.as_deref(), Some("Alice Example"));
    assert_eq!(claims.exp, Some(2_000_000_000));
    assert_eq!(claims.iat, Some(1_999_996_400));
    assert_eq!(claims.nbf, None);
    assert_eq!(
        claims.iss.as_deref(),
        Some("https://kc.example.com/realms/demo")
    );
    assert_eq!(claims.realm_roles(), ["realm-admin", "realm-user"]);
    assert_eq!(claims.client_roles("my-api"), ["api-admin", "api-reader"]);
    assert_eq!(claims.client_roles("unknown"), [] as [String; 0]);
    assert_eq!(
        claims.scopes().collect::<Vec<_>>(),
        ["openid", "profile", "email"]
    );
}

#[test]
fn custom_claims_land_in_extra_and_are_read_with_get() {
    let claims = parse(keycloak_payload());

    assert_eq!(
        claims.get("tenant_id").and_then(|v| v.as_str()),
        Some("acme")
    );
    assert_eq!(
        claims.get("custom").and_then(|v| v.get("nested")),
        Some(&json!(["a", "b"]))
    );

    // Known claims are typed fields, deliberately *not* visible through `get`.
    for known in ["sub", "email", "exp", "realm_access", "scope"] {
        assert!(
            claims.get(known).is_none(),
            "`{known}` is a field, it must not be duplicated into `extra`"
        );
    }
}

#[test]
fn missing_sub_deserializes_to_the_empty_default() {
    // The validator turns this into a precise "no subject" rejection rather
    // than a serde error, so the field must not be mandatory.
    let claims = parse(json!({ "email": "nobody@example.com" }));
    assert_eq!(claims.sub, "");
    assert_eq!(
        claims,
        StandardClaims {
            email: Some("nobody@example.com".into()),
            ..Default::default()
        }
    );
}

#[test]
fn absent_optional_claims_are_none() {
    let claims = parse(json!({ "sub": "u" }));
    assert!(claims.email.is_none());
    assert!(claims.aud.is_none());
    assert!(claims.roles.is_none());
    assert!(claims.realm_access.is_none());
    assert!(claims.resource_access.is_none());
    assert!(claims.extra.is_empty());
    assert_eq!(claims.realm_roles(), [] as [String; 0]);
    assert_eq!(claims.scopes().count(), 0);
}

// ── `aud`: string or array ──

#[test]
fn aud_as_a_single_string() {
    let claims = parse(json!({ "sub": "u", "aud": "my-api" }));
    let aud = claims.aud.as_ref().unwrap();
    assert_eq!(aud, &Audience::Single("my-api".into()));
    assert_eq!(aud.as_str(), Some("my-api"));
    assert!(aud.contains("my-api"));
    assert!(!aud.contains("other"));
    assert_eq!(aud.iter().collect::<Vec<_>>(), ["my-api"]);
}

#[test]
fn aud_as_an_array() {
    let claims = parse(json!({ "sub": "u", "aud": ["my-api", "other-api"] }));
    let aud = claims.aud.as_ref().unwrap();
    assert_eq!(
        aud,
        &Audience::Multiple(vec!["my-api".into(), "other-api".into()])
    );
    assert_eq!(aud.as_str(), None);
    assert!(aud.contains("other-api"));
    assert_eq!(aud.iter().collect::<Vec<_>>(), ["my-api", "other-api"]);
}

// ── Role extractors over typed claims ──

#[test]
fn standard_role_extractor_reads_the_roles_field() {
    let claims = parse(json!({ "sub": "u", "roles": ["admin", "user"] }));
    assert_eq!(
        StandardRoleExtractor.extract_roles(&claims),
        ["admin", "user"]
    );
    assert!(StandardRoleExtractor
        .extract_roles(&parse(json!({ "sub": "u" })))
        .is_empty());
}

#[test]
fn keycloak_extractors_read_realm_and_resource_access() {
    let claims = parse(keycloak_payload());

    assert_eq!(
        RealmRoleExtractor.extract_roles(&claims),
        ["realm-admin", "realm-user"]
    );
    assert_eq!(
        ClientRoleExtractor::new("my-api").extract_roles(&claims),
        ["api-admin", "api-reader"]
    );
    assert!(ClientRoleExtractor::new("nope")
        .extract_roles(&claims)
        .is_empty());
}

#[test]
fn merge_combines_standard_and_realm_roles_and_dedups() {
    let claims = parse(json!({
        "sub": "u",
        "roles": ["admin", "user"],
        "realm_access": { "roles": ["admin", "realm-only"] }
    }));

    let extractor = Merge(StandardRoleExtractor, RealmRoleExtractor);
    assert_eq!(
        extractor.extract_roles(&claims),
        ["admin", "user", "realm-only"]
    );
}

// ── Serialization ──

#[test]
fn serialize_round_trips_the_payload() {
    let payload = keycloak_payload();
    let claims = parse(payload.clone());

    let round_tripped = serde_json::to_value(&claims).unwrap();
    assert_eq!(round_tripped, payload);

    // And back again through the façade's byte form.
    let bytes = r2e_core::json::to_vec(&claims).unwrap();
    let again: StandardClaims = r2e_core::json::from_slice(&bytes).unwrap();
    assert_eq!(again, claims);
}

#[test]
fn serialize_omits_absent_claims() {
    let claims = parse(json!({ "sub": "u" }));
    assert_eq!(
        serde_json::to_value(&claims).unwrap(),
        json!({ "sub": "u" })
    );
}

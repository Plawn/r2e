//! JWT validation (`r2e-security`).
//!
//! `JwtClaimsValidator::validate_as` is the single validation entry point for
//! all three transports (HTTP `AuthenticatedUser`, gRPC metadata, MCP), so it
//! runs on every authenticated request of the whole process. It used to clone a
//! `jsonwebtoken::Validation` — three `HashSet<String>` plus a `Vec<Algorithm>`
//! plus several `String`s — purely to overwrite one field; the validations are
//! now pre-built per allowed algorithm and borrowed.

use jsonwebtoken::{encode, DecodingKey, EncodingKey, Header};
use r2e::r2e_security::{Algorithm, JwtClaimsValidator, SecurityConfig};
use serde_json::json;

use crate::counter::{assert_config_size_invariant, runtime, steady_state, Alloc};

const ITERATIONS: u64 = 200;
const SECRET: &[u8] = b"hot-path-allocation-guard-secret";
const ISSUER: &str = "https://issuer.test";
const AUDIENCE: &str = "hot-path-guard";

fn token() -> String {
    let exp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_secs()
        + 3_600;
    encode(
        &Header::new(Algorithm::HS256),
        &json!({ "sub": "user-1", "iss": ISSUER, "aud": AUDIENCE, "exp": exp }),
        &EncodingKey::from_secret(SECRET),
    )
    .expect("token")
}

fn validator(extra_audiences: usize) -> JwtClaimsValidator {
    let mut audiences = vec![AUDIENCE.to_string()];
    audiences.extend((0..extra_audiences).map(|i| format!("spare-audience-{i:0>48}")));

    let mut config = SecurityConfig::new("https://unused.test/jwks.json", ISSUER, AUDIENCE);
    config.audiences = audiences;
    config.allowed_algorithms = vec![Algorithm::HS256];

    JwtClaimsValidator::new_with_static_key(DecodingKey::from_secret(SECRET), config)
}

fn drive(rt: &r2e::rt::Runtime, validator: &JwtClaimsValidator, token: &str) -> Alloc {
    steady_state(ITERATIONS, || {
        let claims = rt.block_on(validator.validate(token)).expect("valid token");
        assert_eq!(claims.sub, "user-1");
    })
}

/// Validation cost must not scale with the size of the immutable
/// `SecurityConfig` — an accepted-audience list is app-lifetime configuration,
/// not per-request data.
#[test]
fn validation_does_not_copy_the_security_config_per_token() {
    let rt = runtime();
    let token = token();

    let small = drive(&rt, &validator(0), &token);
    let large = drive(&rt, &validator(64), &token);

    // 64 spare audiences are ~3 KiB of `String`s in a `HashSet`; cloning the
    // `Validation` per call would cost that plus ~65 allocations.
    assert_config_size_invariant(
        "JwtClaimsValidator::validate_as",
        small,
        large,
        4,
        512,
    );
}

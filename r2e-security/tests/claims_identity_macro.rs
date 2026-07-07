//! Compile check for `impl_claims_identity_extractor!` against the
//! `FromRequestPartsVia`/`HasBean` extraction model.

use r2e_security::{impl_claims_identity_extractor, AuthenticatedUser, ClaimsIdentity};

#[derive(Clone)]
struct CustomUser {
    auth: AuthenticatedUser,
}

impl<S: Send + Sync> ClaimsIdentity<S> for CustomUser {
    async fn from_jwt_claims(
        claims: serde_json::Value,
        _state: &S,
    ) -> Result<Self, r2e_core::HttpError> {
        Ok(CustomUser {
            auth: AuthenticatedUser::from_claims(claims),
        })
    }
}

impl_claims_identity_extractor!(CustomUser);

#[test]
fn macro_expands() {
    // Nothing to run — the test is that the macro output compiles.
    let _ = CustomUser {
        auth: AuthenticatedUser::from_claims(serde_json::json!({"sub": "x"})),
    };
}

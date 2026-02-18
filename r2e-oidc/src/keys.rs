use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use jsonwebtoken::{DecodingKey, EncodingKey};
use rand::rngs::OsRng;
use rsa::pkcs8::EncodePrivateKey;
use rsa::traits::PublicKeyParts;
use rsa::{RsaPrivateKey, RsaPublicKey};
use serde::Serialize;

/// RSA key pair for JWT signing and JWKS publication.
pub struct OidcKeyPair {
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    /// Base64url-encoded RSA modulus (for JWKS).
    n: String,
    /// Base64url-encoded RSA public exponent (for JWKS).
    e: String,
    /// Key ID.
    kid: String,
}

impl OidcKeyPair {
    /// Generate a new RSA-2048 key pair.
    pub fn generate(kid: &str) -> Self {
        let private_key =
            RsaPrivateKey::new(&mut OsRng, 2048).expect("failed to generate RSA-2048 key");
        let public_key = RsaPublicKey::from(&private_key);

        // Export private key as PKCS8 PEM for jsonwebtoken EncodingKey.
        let pkcs8_pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .expect("failed to export RSA key as PKCS8 PEM");
        let encoding_key = EncodingKey::from_rsa_pem(pkcs8_pem.as_bytes())
            .expect("failed to create EncodingKey from RSA PEM");

        // Extract public key components (n, e) as base64url for JWKS and DecodingKey.
        let n = URL_SAFE_NO_PAD.encode(public_key.n().to_bytes_be());
        let e = URL_SAFE_NO_PAD.encode(public_key.e().to_bytes_be());

        let decoding_key = DecodingKey::from_rsa_components(&n, &e)
            .expect("failed to create DecodingKey from RSA components");

        Self {
            encoding_key,
            decoding_key,
            n,
            e,
            kid: kid.to_string(),
        }
    }

    /// Returns the encoding key for signing JWTs.
    pub fn encoding_key(&self) -> &EncodingKey {
        &self.encoding_key
    }

    /// Returns the decoding key for validating JWTs.
    pub fn decoding_key(&self) -> DecodingKey {
        self.decoding_key.clone()
    }

    /// Returns the JWKS JSON representation of the public key.
    pub fn jwks_json(&self) -> JwksResponse<'_> {
        JwksResponse {
            keys: vec![JwkEntry {
                kty: "RSA",
                alg: "RS256",
                r#use: "sig",
                kid: &self.kid,
                n: &self.n,
                e: &self.e,
            }],
        }
    }
}

/// JWKS response body.
#[derive(Serialize)]
pub struct JwksResponse<'a> {
    pub keys: Vec<JwkEntry<'a>>,
}

/// A single JWK entry in a JWKS response.
#[derive(Serialize)]
pub struct JwkEntry<'a> {
    pub kty: &'a str,
    pub alg: &'a str,
    #[serde(rename = "use")]
    pub r#use: &'a str,
    pub kid: &'a str,
    pub n: &'a str,
    pub e: &'a str,
}

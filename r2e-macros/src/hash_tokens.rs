use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use proc_macro2::TokenStream as TokenStream2;

/// Hash the string representation of a token stream to produce a stable `u64`.
///
/// Used to compute `BUILD_VERSION` for beans and producers at compile time.
/// When the source tokens of a constructor or producer function change,
/// the hash changes, which causes the dev-reload fingerprint to change,
/// triggering a state rebuild.
///
/// Note: This hashes the *textual* representation of tokens, not their spans.
/// Whitespace normalization is handled by `proc_macro2::TokenStream::to_string()`.
pub fn hash_token_stream(tokens: &TokenStream2) -> u64 {
    let mut hasher = DefaultHasher::new();
    tokens.to_string().hash(&mut hasher);
    hasher.finish()
}

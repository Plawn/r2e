use std::collections::hash_map::DefaultHasher;
use std::fmt::Write as _;
use std::hash::{Hash, Hasher};

use proc_macro2::{TokenStream as TokenStream2, TokenTree};

/// Hash the textual representation of a token stream to a stable `u64`.
///
/// Used to compute `BUILD_VERSION` for beans and producers; a changed hash
/// invalidates the dev-reload fingerprint and triggers a state rebuild.
/// Walks `TokenTree` leaves into one reusable buffer instead of allocating
/// the full `to_string()` up front.
pub fn hash_token_stream(tokens: &TokenStream2) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut scratch = String::with_capacity(64);
    feed(tokens, &mut hasher, &mut scratch);
    hasher.finish()
}

fn feed(tokens: &TokenStream2, hasher: &mut DefaultHasher, scratch: &mut String) {
    for tt in tokens.clone() {
        match tt {
            TokenTree::Group(g) => {
                // Include delimiter bytes so `(a b)` and `[a b]` hash differently.
                let (open, close) = match g.delimiter() {
                    proc_macro2::Delimiter::Parenthesis => ("(", ")"),
                    proc_macro2::Delimiter::Brace => ("{", "}"),
                    proc_macro2::Delimiter::Bracket => ("[", "]"),
                    proc_macro2::Delimiter::None => ("", ""),
                };
                open.hash(hasher);
                feed(&g.stream(), hasher, scratch);
                close.hash(hasher);
            }
            TokenTree::Ident(i) => {
                scratch.clear();
                let _ = write!(scratch, "{}", i);
                scratch.hash(hasher);
                // Separator byte so `ab` and `a b` don't collide.
                0u8.hash(hasher);
            }
            TokenTree::Punct(p) => {
                (p.as_char() as u32).hash(hasher);
            }
            TokenTree::Literal(l) => {
                scratch.clear();
                let _ = write!(scratch, "{}", l);
                scratch.hash(hasher);
                0u8.hash(hasher);
            }
        }
    }
}

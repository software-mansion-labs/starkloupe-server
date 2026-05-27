pub mod admin_token;

pub use admin_token::AdminAuth;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

/// Generates an opaque API token with a typed prefix.
///
/// Returns `(plaintext, sha256(plaintext), first 12 chars of plaintext)`.
/// The prefix is part of the plaintext and is included in the hash.
pub fn gen_token(prefix: &str) -> (String, [u8; 32], String) {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);

    let encoded = URL_SAFE_NO_PAD.encode(raw);
    let plaintext = format!("{prefix}{encoded}");

    let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
    let key_prefix: String = plaintext.chars().take(12).collect();

    (plaintext, hash, key_prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_token_produces_distinct_outputs() {
        let (p1, h1, _) = gen_token("wk_live_");
        let (p2, h2, _) = gen_token("wk_live_");
        assert_ne!(p1, p2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn gen_token_hash_matches_plaintext() {
        let (plaintext, hash, _) = gen_token("wk_live_");
        let expected: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
        assert_eq!(hash, expected);
    }

    #[test]
    fn gen_token_prefix_is_first_12_chars() {
        let (plaintext, _, key_prefix) = gen_token("wk_live_");
        assert_eq!(key_prefix.len(), 12);
        assert_eq!(key_prefix, &plaintext[..12]);
        assert!(plaintext.starts_with("wk_live_"));
    }

    #[test]
    fn gen_token_works_with_arbitrary_prefix() {
        // For M1 reuse: dt_ Devnet Tokens.
        let (plaintext, _, prefix) = gen_token("dt_");
        assert!(plaintext.starts_with("dt_"));
        assert!(prefix.starts_with("dt_"));
    }
}

pub mod admin_token;

pub use admin_token::AdminAuth;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};

/// An opaque API token together with its sha256 hash and a short lookup prefix.

pub struct GeneratedToken {
    pub plaintext: String,
    pub hash: [u8; 32],
    pub key_prefix: String,
}

pub fn gen_token(prefix: &str) -> GeneratedToken {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);

    let encoded = URL_SAFE_NO_PAD.encode(raw);
    let plaintext = format!("{prefix}{encoded}");

    let hash: [u8; 32] = Sha256::digest(plaintext.as_bytes()).into();
    let key_prefix: String = plaintext.chars().take(12).collect();

    GeneratedToken {
        plaintext,
        hash,
        key_prefix,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_token_produces_distinct_outputs() {
        let t1 = gen_token("wk_live_");
        let t2 = gen_token("wk_live_");
        assert_ne!(t1.plaintext, t2.plaintext);
        assert_ne!(t1.hash, t2.hash);
    }

    #[test]
    fn gen_token_hash_matches_plaintext() {
        let token = gen_token("wk_live_");
        let expected: [u8; 32] = Sha256::digest(token.plaintext.as_bytes()).into();
        assert_eq!(token.hash, expected);
    }

    #[test]
    fn gen_token_prefix_is_first_12_chars() {
        let token = gen_token("wk_live_");
        assert_eq!(token.key_prefix.len(), 12);
        assert_eq!(token.key_prefix, token.plaintext[..12]);
        assert!(token.plaintext.starts_with("wk_live_"));
    }

    #[test]
    fn gen_token_works_with_arbitrary_prefix() {
        // For M1 reuse: dt_ Devnet Tokens.
        let token = gen_token("dt_");
        assert!(token.plaintext.starts_with("dt_"));
        assert!(token.key_prefix.starts_with("dt_"));
    }
}

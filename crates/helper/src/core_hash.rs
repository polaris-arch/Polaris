//! Shared SHA256 encoding; file custody and installation live in their own modules.
use sha2::{Digest, Sha256};
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

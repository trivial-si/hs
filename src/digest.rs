//! SHA-256 fingerprints rendered as lowercase hexadecimal.
use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// Returns the SHA-256 digest of `bytes` as 64 lowercase hexadecimal characters.
pub(crate) fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

#[cfg(test)]
mod tests {
    use super::sha256_hex;

    #[test]
    fn renders_known_digest() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

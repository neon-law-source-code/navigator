//! GitHub's `X-Hub-Signature-256` verifier.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Verify GitHub's `sha256=<hex>` HMAC header against the exact raw body.
///
/// Malformed, missing-prefix, and mismatched values all return `false`. The
/// final comparison uses `hmac`'s constant-time verifier.
#[must_use]
pub fn verify_hmac_sha256_hex(key: &[u8], body: &[u8], provided: &str) -> bool {
    let Some(encoded) = provided.strip_prefix("sha256=") else {
        return false;
    };
    let Some(digest) = decode_hex(encoded) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(key) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&digest).is_ok()
}

/// Produce a GitHub-shaped header for tests and local delivery fixtures.
#[must_use]
pub fn sign_hmac_sha256_hex(key: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts every key length");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut output = String::from("sha256=");
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 || !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Some((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{sign_hmac_sha256_hex, verify_hmac_sha256_hex};

    #[test]
    fn rejects_missing_or_malformed_headers() {
        let body = b"signed bytes";
        assert!(!verify_hmac_sha256_hex(b"key", body, ""));
        assert!(!verify_hmac_sha256_hex(b"key", body, "sha1=deadbeef"));
        assert!(!verify_hmac_sha256_hex(b"key", body, "sha256=zz"));
        let signature = sign_hmac_sha256_hex(b"key", body);
        assert!(verify_hmac_sha256_hex(b"key", body, &signature));
    }
}

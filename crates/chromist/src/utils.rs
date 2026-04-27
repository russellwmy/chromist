/// Base64-decode a string. Used to decode screenshot / PDF payloads.
///
/// # Examples
///
/// ```
/// # use chromist::utils::{base64_decode, base64_encode};
/// let encoded = base64_encode(b"hi");
/// assert_eq!(base64_decode(&encoded).unwrap(), b"hi");
/// ```
pub fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
}

/// Base64-encode a byte slice.
pub fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_roundtrip() {
        let original = b"hello, world";
        let encoded = base64_encode(original);
        let decoded = base64_decode(&encoded).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn encode_empty_is_empty_string() {
        assert_eq!(base64_encode(&[]), "");
    }

    #[test]
    fn decode_empty_is_empty_vec() {
        assert_eq!(base64_decode("").expect("ok"), Vec::<u8>::new());
    }

    #[test]
    fn decode_invalid_returns_error() {
        assert!(base64_decode("!!!not-base64").is_err());
    }

    #[test]
    fn binary_roundtrip_preserves_arbitrary_bytes() {
        let original: Vec<u8> = (0u8..=255).collect();
        let encoded = base64_encode(&original);
        let decoded = base64_decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }
}

//! Stable, dependency-free content fingerprints (FNV-1a, 64-bit). Used for change detection and
//! de-duplication keys, never for security.

const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[must_use]
pub fn stable_fingerprint(value: &str) -> String {
    let mut hash = FNV_OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::stable_fingerprint;

    #[test]
    fn is_deterministic_and_sensitive_to_content() {
        assert_eq!(stable_fingerprint(""), "cbf29ce484222325");
        assert_eq!(stable_fingerprint("a"), "af63dc4c8601ec8c");
        assert_ne!(stable_fingerprint("abc"), stable_fingerprint("abd"));
    }
}

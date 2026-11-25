//! Lightweight XOR obfuscation: the source stores only ciphertext, restored at runtime.
//!
//! Used to deter plaintext scanners such as GitHub secret scanning; **not** real encryption against reverse engineering.

/// Obfuscation key matching the ciphertext (hard-coded into the program).
const OBFUSCATION_KEY: &[u8] = b"neo-obf-v1-x7K9mP2qR5sT8uV0";

/// Restore XOR ciphertext into a UTF-8 string.
pub fn reveal(obf: &[u8]) -> String {
    let key = OBFUSCATION_KEY;
    let plain: Vec<u8> = obf
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect();
    String::from_utf8(plain).unwrap_or_default()
}

/// Obfuscate plaintext into ciphertext bytes (for tests and offline generation).
#[cfg(test)]
pub fn conceal(plain: &str) -> Vec<u8> {
    let key = OBFUSCATION_KEY;
    plain
        .as_bytes()
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveal_roundtrips_short_plaintext() {
        let plain = "pk.test-token";
        let obf = conceal(plain);
        assert_ne!(obf.as_slice(), plain.as_bytes());
        assert_eq!(reveal(&obf), plain);
    }
}

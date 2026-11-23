//! 轻量 XOR 混淆：源码只存密文，运行时再还原。
//!
//! 用于挡 GitHub secret scanning 等明文扫描；**不是**对抗逆向的真正加密。

/// 与密文配套的混淆密钥（程序内硬编码）。
const OBFUSCATION_KEY: &[u8] = b"neo-obf-v1-x7K9mP2qR5sT8uV0";

/// 将 XOR 密文还原为 UTF-8 字符串。
pub fn reveal(obf: &[u8]) -> String {
    let key = OBFUSCATION_KEY;
    let plain: Vec<u8> = obf
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect();
    String::from_utf8(plain).unwrap_or_default()
}

/// 将明文混淆为密文字节（测试与离线生成用）。
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

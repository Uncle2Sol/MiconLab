//! 16 进制字符串解析与格式化工具。
//!
//! 统一 CLI / GUI / 协议层（CAN SLCAN 等）的 hex 处理逻辑，避免 3 处重复实现。
//!
//! 支持的输入格式：
//!   - `"48656C6C6F"`     (连续)
//!   - `"48 65 6C 6C 6F"` (空格分隔)
//!   - `"0x48 0x65"`      (0x 前缀)
//!   - `"48 65-6C 6C:6F"` (混合分隔符 : - 等)
//!
//! 行为约定：
//!   - 奇数长度直接报错（不静默截断，避免协议解析歧义）
//!   - 空字符串返回空 Vec
//!   - 大小写不敏感
//!   - 无效字符报错时携带偏移上下文

/// 解析 16 进制字符串为字节数组。
///
/// 详见模块文档。返回 `Result<Vec<u8>, String>` 以便不依赖 anyhow 的调用方（如 GUI）使用。
pub fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    // 移除所有空白与常见分隔符，以及 0x/0X 前缀
    let cleaned: String = input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ':' && *c != '-')
        .collect();
    let cleaned = cleaned.replace("0x", "").replace("0X", "");

    if cleaned.is_empty() {
        return Ok(Vec::new());
    }

    if cleaned.len() % 2 != 0 {
        return Err(format!(
            "hex string has odd length ({} chars): '{}'",
            cleaned.len(),
            cleaned
        ));
    }

    // 非 ASCII 输入（如中文、全角字符）按字节切片会劈开 UTF-8 字符边界导致 panic
    if !cleaned.is_ascii() {
        return Err("hex string contains non-ASCII characters".to_string());
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    let mut i = 0usize;
    while i < cleaned.len() {
        let byte = u8::from_str_radix(&cleaned[i..i + 2], 16)
            .map_err(|e| format!("invalid hex byte at offset {}: {}", i, e))?;
        bytes.push(byte);
        i += 2;
    }
    Ok(bytes)
}

/// 将字节数组格式化为连续小写 hex 字符串（如 `"48656c6c6f"`）。
pub fn format_hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

/// 将字节数组格式化为连续大写 hex 字符串（如 `"48656C6C6F"`）。
pub fn format_hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02X}", b));
    }
    s
}

/// 将字节数组格式化为空格分隔的大写 hex 字符串（如 `"48 65 6C 6C 6F"`）。
pub fn format_hex_spaced(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02X}", b)).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_continuous() {
        assert_eq!(parse_hex("48656C6C6F").unwrap(), b"Hello");
    }

    #[test]
    fn test_parse_hex_spaces() {
        assert_eq!(parse_hex("48 65 6C 6C 6F").unwrap(), b"Hello");
    }

    #[test]
    fn test_parse_hex_0x_prefix() {
        assert_eq!(parse_hex("0x48 0x65").unwrap(), b"He");
    }

    #[test]
    fn test_parse_hex_mixed_separators() {
        assert_eq!(parse_hex("48:65-6C 6C 6F").unwrap(), b"Hello");
    }

    #[test]
    fn test_parse_hex_empty() {
        assert!(parse_hex("").unwrap().is_empty());
    }

    #[test]
    fn test_parse_hex_odd_length_rejected() {
        let err = parse_hex("486").unwrap_err();
        assert!(err.contains("odd length"));
    }

    #[test]
    fn test_parse_hex_invalid_chars() {
        let err = parse_hex("ZZ").unwrap_err();
        assert!(err.contains("offset 0"));
    }

    #[test]
    fn test_parse_hex_mixed_case() {
        assert_eq!(parse_hex("4A 4b 4C").unwrap(), [0x4A, 0x4B, 0x4C]);
    }

    #[test]
    fn test_format_hex_lower() {
        assert_eq!(format_hex_lower(b"Hello"), "48656c6c6f");
    }

    #[test]
    fn test_format_hex_upper() {
        assert_eq!(format_hex_upper(b"Hello"), "48656C6C6F");
    }

    #[test]
    fn test_format_hex_spaced() {
        assert_eq!(format_hex_spaced(b"Hello"), "48 65 6C 6C 6F");
    }

    #[test]
    fn test_format_hex_empty() {
        assert_eq!(format_hex_lower(&[]), "");
        assert_eq!(format_hex_upper(&[]), "");
        assert_eq!(format_hex_spaced(&[]), "");
    }
}

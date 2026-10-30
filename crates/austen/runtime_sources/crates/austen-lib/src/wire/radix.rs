fn split(s: &str) -> (u32, &str) {
    match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => (16, hex),
        None => (10, s),
    }
}

pub fn parse_u64(s: &str) -> Option<u64> {
    let (radix, digits) = split(s.trim());
    u64::from_str_radix(digits, radix).ok()
}

pub fn validate_utf8(bytes: &[u8]) -> Result<&str, simdutf8::basic::Utf8Error> {
    simdutf8::basic::from_utf8(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_utf8_with_simd() {
        let value = validate_utf8("hello 🌍".as_bytes()).expect("utf8");
        assert_eq!(value, "hello 🌍");
    }
}

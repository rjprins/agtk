//! Text operations shared by paths and agent previews.

/// Decodes percent escapes without assuming the result is UTF-8. A plus is
/// literal here: these are URL paths, not form data.
pub(crate) fn percent_decode(value: &str) -> Option<Vec<u8>> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut input = value.as_bytes().iter().copied();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = hex(input.next()?)?;
            let low = hex(input.next()?)?;
            bytes.push(high * 16 + low);
        } else {
            bytes.push(byte);
        }
    }
    Some(bytes)
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn decoding_keeps_literal_pluses_and_non_utf8_path_bytes() {
        assert_eq!(
            percent_decode("a+b%20%C3%A9%ff"),
            Some(b"a+b \xc3\xa9\xff".to_vec())
        );
        for invalid in ["%", "%2", "%zz", "%+1"] {
            assert_eq!(percent_decode(invalid), None);
        }
    }
}

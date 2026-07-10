pub fn responses(bytes: &[u8], parser: &vt100::Parser) -> Vec<String> {
    let mut responses = Vec::new();

    if contains(bytes, b"\x1b[6n") {
        let (row, col) = parser.screen().cursor_position();
        responses.push(format!(
            "\x1b[{};{}R",
            row.saturating_add(1),
            col.saturating_add(1)
        ));
    }

    if contains(bytes, b"\x1b[5n") {
        responses.push("\x1b[0n".to_string());
    }

    if contains(bytes, b"\x1b[c") || contains(bytes, b"\x1b[0c") {
        responses.push("\x1b[?1;2c".to_string());
    }

    if contains(bytes, b"\x1b[>c") || contains(bytes, b"\x1b[>0c") {
        responses.push("\x1b[>0;0;0c".to_string());
    }

    responses
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responds_to_device_status_query() {
        let parser = vt100::Parser::new(24, 80, 0);
        assert_eq!(responses(b"\x1b[5n", &parser), vec!["\x1b[0n"]);
    }

    #[test]
    fn ignores_regular_output() {
        let parser = vt100::Parser::new(24, 80, 0);
        assert!(responses(b"hello", &parser).is_empty());
    }
}

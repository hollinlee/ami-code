use thiserror::Error;

const BRACKETED_PASTE_START: &[u8] = b"\x1b[200~";
const BRACKETED_PASTE_END: &[u8] = b"\x1b[201~";

#[derive(Debug, Error)]
pub enum PasteError {
    #[error("multi-line paste requires backend bracketed-paste support")]
    MultilineUnsupported,
    #[error("failed to write paste to PTY: {0}")]
    Write(#[source] anyhow::Error),
}

pub fn encode(contents: &str, bracketed: bool) -> Result<Vec<u8>, PasteError> {
    let sanitized = sanitize(contents);
    if !bracketed && sanitized.contains('\n') {
        return Err(PasteError::MultilineUnsupported);
    }

    if !bracketed {
        return Ok(sanitized.into_bytes());
    }

    let mut bytes = Vec::with_capacity(
        BRACKETED_PASTE_START.len() + sanitized.len() + BRACKETED_PASTE_END.len(),
    );
    bytes.extend_from_slice(BRACKETED_PASTE_START);
    bytes.extend_from_slice(sanitized.as_bytes());
    bytes.extend_from_slice(BRACKETED_PASTE_END);
    Ok(bytes)
}

fn sanitize(contents: &str) -> String {
    contents
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|character| *character == '\n' || *character == '\t' || !character.is_control())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_plain_utf8_and_cjk_text() {
        assert_eq!(
            encode("hello 中文", false).unwrap(),
            "hello 中文".as_bytes()
        );
    }

    #[test]
    fn frames_multiline_text_for_bracketed_paste() {
        assert_eq!(
            encode("one\ntwo", true).unwrap(),
            b"\x1b[200~one\ntwo\x1b[201~"
        );
    }

    #[test]
    fn rejects_multiline_text_without_bracketed_paste() {
        assert!(matches!(
            encode("one\ntwo", false),
            Err(PasteError::MultilineUnsupported)
        ));
    }

    #[test]
    fn filters_control_characters_and_injected_delimiters() {
        assert_eq!(
            encode("a\0\x1b[201~b\t\r\nc", true).unwrap(),
            b"\x1b[200~a[201~b\t\nc\x1b[201~"
        );
    }
}

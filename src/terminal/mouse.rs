use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use vt100::{MouseProtocolEncoding, MouseProtocolMode};

pub fn encode(
    mode: MouseProtocolMode,
    encoding: MouseProtocolEncoding,
    event: MouseEvent,
) -> Option<Vec<u8>> {
    if !reports(mode, event.kind) {
        return None;
    }

    let (button_code, release) = event_code(event.kind)?;
    let modifiers = modifier_code(event.modifiers);
    let code = button_code.saturating_add(modifiers);
    let legacy_code = if release { 3 + modifiers } else { code };
    let column = event.column.saturating_add(1);
    let row = event.row.saturating_add(1);

    match encoding {
        MouseProtocolEncoding::Sgr => Some(
            format!(
                "\x1b[<{code};{column};{row}{}",
                if release { 'm' } else { 'M' }
            )
            .into_bytes(),
        ),
        MouseProtocolEncoding::Default => encode_legacy(legacy_code, column, row, false),
        MouseProtocolEncoding::Utf8 => encode_legacy(legacy_code, column, row, true),
    }
}

fn reports(mode: MouseProtocolMode, kind: MouseEventKind) -> bool {
    match mode {
        MouseProtocolMode::None => false,
        MouseProtocolMode::Press => matches!(
            kind,
            MouseEventKind::Down(_)
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight
        ),
        MouseProtocolMode::PressRelease => {
            !matches!(kind, MouseEventKind::Drag(_) | MouseEventKind::Moved)
        }
        MouseProtocolMode::ButtonMotion => !matches!(kind, MouseEventKind::Moved),
        MouseProtocolMode::AnyMotion => true,
    }
}

fn event_code(kind: MouseEventKind) -> Option<(u16, bool)> {
    match kind {
        MouseEventKind::Down(button) => Some((button_code(button), false)),
        MouseEventKind::Up(button) => Some((button_code(button), true)),
        MouseEventKind::Drag(button) => Some((button_code(button) + 32, false)),
        MouseEventKind::Moved => Some((35, false)),
        MouseEventKind::ScrollUp => Some((64, false)),
        MouseEventKind::ScrollDown => Some((65, false)),
        MouseEventKind::ScrollLeft => Some((66, false)),
        MouseEventKind::ScrollRight => Some((67, false)),
    }
}

fn button_code(button: MouseButton) -> u16 {
    match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    }
}

fn modifier_code(modifiers: KeyModifiers) -> u16 {
    let mut code = 0;
    if modifiers.contains(KeyModifiers::SHIFT) {
        code += 4;
    }
    if modifiers.contains(KeyModifiers::ALT) {
        code += 8;
    }
    if modifiers.contains(KeyModifiers::CONTROL) {
        code += 16;
    }
    code
}

fn encode_legacy(code: u16, column: u16, row: u16, utf8: bool) -> Option<Vec<u8>> {
    let values = [
        code.saturating_add(32),
        column.saturating_add(32),
        row.saturating_add(32),
    ];
    let mut bytes = b"\x1b[M".to_vec();
    if utf8 {
        for value in values {
            if value > 0x07ff {
                return None;
            }
            let character = char::from_u32(u32::from(value))?;
            let mut buffer = [0; 4];
            bytes.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
        }
    } else {
        for value in values {
            bytes.push(u8::try_from(value).ok()?);
        }
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    #[test]
    fn encodes_sgr_press_release_and_wheel() {
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
                event(MouseEventKind::Down(MouseButton::Left), 4, 2),
            ),
            Some(b"\x1b[<0;5;3M".to_vec())
        );
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
                event(MouseEventKind::Up(MouseButton::Left), 4, 2),
            ),
            Some(b"\x1b[<0;5;3m".to_vec())
        );
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
                event(MouseEventKind::ScrollUp, 4, 2),
            ),
            Some(b"\x1b[<64;5;3M".to_vec())
        );
    }

    #[test]
    fn uses_legacy_release_button_code() {
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Default,
                event(MouseEventKind::Up(MouseButton::Left), 0, 0),
            ),
            Some(b"\x1b[M#!!".to_vec())
        );
    }

    #[test]
    fn honors_protocol_mode_and_modifiers() {
        let mut drag = event(MouseEventKind::Drag(MouseButton::Left), 0, 0);
        drag.modifiers = KeyModifiers::CONTROL;
        assert_eq!(
            encode(
                MouseProtocolMode::ButtonMotion,
                MouseProtocolEncoding::Sgr,
                drag,
            ),
            Some(b"\x1b[<48;1;1M".to_vec())
        );
        assert_eq!(
            encode(
                MouseProtocolMode::PressRelease,
                MouseProtocolEncoding::Sgr,
                drag,
            ),
            None
        );
    }

    #[test]
    fn enforces_utf8_extended_mouse_coordinate_limit() {
        assert!(
            encode(
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Utf8,
                event(MouseEventKind::Down(MouseButton::Left), 2014, 0),
            )
            .is_some()
        );
        assert_eq!(
            encode(
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Utf8,
                event(MouseEventKind::Down(MouseButton::Left), 2015, 0),
            ),
            None
        );
    }

    #[test]
    fn encodes_legacy_coordinates_and_rejects_overflow() {
        assert_eq!(
            encode(
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Default,
                event(MouseEventKind::Down(MouseButton::Left), 0, 0),
            ),
            Some(b"\x1b[M !!".to_vec())
        );
        assert_eq!(
            encode(
                MouseProtocolMode::Press,
                MouseProtocolEncoding::Default,
                event(MouseEventKind::Down(MouseButton::Left), 500, 0),
            ),
            None
        );
    }
}

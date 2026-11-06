//! Encoding of keyboard and mouse input into the byte sequences that
//! applications running in the terminal expect (xterm conventions).

use crate::{Modes, MouseMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

impl Mods {
    fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }

    /// The xterm modifier parameter (`1 + bitmask`).
    fn param(self) -> u8 {
        1 + self.shift as u8 + 2 * self.alt as u8 + 4 * self.ctrl as u8
    }
}

/// Encodes a key press. Returns `None` for keys that produce no input
/// (the caller should then let the platform handle the event).
pub fn encode_key(key: Key, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    let csi = |final_: char, m: Mods| -> Vec<u8> {
        if m.any() {
            format!("\x1b[1;{}{final_}", m.param()).into_bytes()
        } else if modes.app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8, m: Mods| -> Vec<u8> {
        if m.any() { format!("\x1b[{n};{}~", m.param()) } else { format!("\x1b[{n}~") }.into_bytes()
    };
    let alt_prefix = |mut bytes: Vec<u8>| -> Vec<u8> {
        if mods.alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    Some(match key {
        Key::Up => csi('A', mods),
        Key::Down => csi('B', mods),
        Key::Right => csi('C', mods),
        Key::Left => csi('D', mods),
        Key::Home => csi('H', mods),
        Key::End => csi('F', mods),
        Key::Insert => tilde(2, mods),
        Key::Delete => tilde(3, mods),
        Key::PageUp => tilde(5, mods),
        Key::PageDown => tilde(6, mods),
        Key::F(n @ 1..=4) => {
            let final_ = (b'P' + n - 1) as char;
            if mods.any() { format!("\x1b[1;{}{final_}", mods.param()) } else { format!("\x1bO{final_}") }.into_bytes()
        }
        Key::F(n @ 5..=12) => {
            const CODES: [u8; 8] = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(CODES[(n - 5) as usize], mods)
        }
        Key::F(_) => return None,
        Key::Enter => alt_prefix(if modes.newline { b"\r\n".to_vec() } else { b"\r".to_vec() }),
        Key::Backspace => alt_prefix(vec![if mods.ctrl { 0x08 } else { 0x7f }]),
        Key::Tab if mods.shift => b"\x1b[Z".to_vec(),
        Key::Tab => alt_prefix(b"\t".to_vec()),
        Key::Escape => alt_prefix(b"\x1b".to_vec()),
        Key::Char(c) if mods.ctrl => alt_prefix(vec![ctrl_byte(c)?]),
        Key::Char(c) => {
            let mut buf = [0u8; 4];
            alt_prefix(c.encode_utf8(&mut buf).as_bytes().to_vec())
        }
    })
}

/// The C0 control byte produced by Ctrl+`c`.
fn ctrl_byte(c: char) -> Option<u8> {
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '-' | '/' | '7' => 0x1f,
        '?' | '8' => 0x7f,
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    /// Motion with no button held.
    None,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

/// Encodes a mouse event for the application, honoring the active tracking
/// mode. Coordinates are 0-based cell positions.
pub fn encode_mouse(button: MouseButton, action: MouseAction, col: usize, row: usize, mods: Mods, modes: &Modes) -> Option<Vec<u8>> {
    match (modes.mouse, action, button) {
        (MouseMode::Off, ..) => return None,
        (MouseMode::Click, MouseAction::Motion, _) => return None,
        (MouseMode::Drag, MouseAction::Motion, MouseButton::None) => return None,
        _ => {}
    }

    let mut code: u32 = match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
        MouseButton::None => 3,
        MouseButton::WheelUp => 64,
        MouseButton::WheelDown => 65,
    };
    if action == MouseAction::Motion {
        code += 32;
    }
    code += 4 * mods.shift as u32 + 8 * mods.alt as u32 + 16 * mods.ctrl as u32;

    if modes.mouse_sgr {
        let final_ = if action == MouseAction::Release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{final_}", col + 1, row + 1).into_bytes());
    }

    // Legacy X10 encoding: releases don't say which button, coordinates are
    // limited to 223.
    if action == MouseAction::Release {
        code = (code & !0b11) | 3;
    }
    let enc = |v: usize| (v.min(222) + 1 + 32) as u8;
    Some(vec![0x1b, b'[', b'M', (code + 32) as u8, enc(col), enc(row)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys() {
        let mut modes = Modes::default();
        let none = Mods::default();
        let ctrl = Mods { ctrl: true, ..none };
        assert_eq!(encode_key(Key::Up, none, &modes).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(Key::Up, ctrl, &modes).unwrap(), b"\x1b[1;5A");
        modes.app_cursor = true;
        assert_eq!(encode_key(Key::Up, none, &modes).unwrap(), b"\x1bOA");
        assert_eq!(encode_key(Key::Char('c'), ctrl, &modes).unwrap(), [3]);
        assert_eq!(encode_key(Key::Char('b'), Mods { alt: true, ..none }, &modes).unwrap(), b"\x1bb");
        assert_eq!(encode_key(Key::F(5), none, &modes).unwrap(), b"\x1b[15~");
        assert_eq!(encode_key(Key::Tab, Mods { shift: true, ..none }, &modes).unwrap(), b"\x1b[Z");
    }

    #[test]
    fn mouse() {
        let mut modes = Modes::default();
        let none = Mods::default();
        assert!(encode_mouse(MouseButton::Left, MouseAction::Press, 0, 0, none, &modes).is_none());
        modes.mouse = MouseMode::Click;
        modes.mouse_sgr = true;
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, 4, 2, none, &modes).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Release, 4, 2, none, &modes).unwrap(), b"\x1b[<0;5;3m");
        assert!(encode_mouse(MouseButton::Left, MouseAction::Motion, 4, 2, none, &modes).is_none());
        modes.mouse_sgr = false;
        assert_eq!(encode_mouse(MouseButton::Left, MouseAction::Press, 0, 0, none, &modes).unwrap(), b"\x1b[M !!");
    }
}

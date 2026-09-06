//! Keystroke → escape-sequence encoding for the PTY (xterm-compatible
//! subset: what agent CLIs and a shell actually send).

use gpui_kit::Keystroke;

/// Encode a key-down into bytes to write to the PTY.
///
/// Returns `None` for keystrokes the terminal must not swallow —
/// notably anything with the platform (⌘) modifier, which is reserved
/// for app shortcuts.
pub fn encode(k: &Keystroke) -> Option<Vec<u8>> {
    let m = &k.modifiers;
    if m.platform {
        return None;
    }

    // Control/named keys first: their encoding ignores `key_char`.
    if let Some(seq) = named(&k.key, m) {
        return Some(seq.as_bytes().to_vec());
    }

    // Ctrl + printable key → C0 control byte ("space" included → NUL).
    if m.control {
        let key = if k.key == "space" { " " } else { k.key.as_str() };
        if key.len() == 1
            && let Some(byte) = control_byte(key) {
                return Some(vec![byte]);
            }
    }

    // Printable input: `key_char` carries the produced character
    // (respects shift and layouts). Alt prefixes ESC.
    if let Some(ch) = &k.key_char
        && !m.control {
            let mut bytes = ch.as_bytes().to_vec();
            if m.alt {
                bytes.insert(0, 0x1b);
            }
            return Some(bytes);
        }

    None
}

/// Xterm modifier parameter for CSI sequences: 1 + shift(1) + alt(2) + ctrl(4).
fn csi_mod(m: &gpui_kit::Modifiers) -> u8 {
    1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control)
}

fn arrow(code: char, m: &gpui_kit::Modifiers) -> String {
    if m.shift || m.alt || m.control {
        format!("\x1b[1;{}{code}", csi_mod(m))
    } else {
        format!("\x1b[{code}")
    }
}

fn tilde(n: u8, m: &gpui_kit::Modifiers) -> String {
    if m.shift || m.alt || m.control {
        format!("\x1b[{n};{}~", csi_mod(m))
    } else {
        format!("\x1b[{n}~")
    }
}

fn named(key: &str, m: &gpui_kit::Modifiers) -> Option<String> {
    Some(match key {
        "enter" => "\r".into(),
        "tab" if m.shift => "\x1b[Z".into(),
        "tab" => "\t".into(),
        "backspace" => "\x7f".into(),
        "escape" => "\x1b".into(),
        "up" => arrow('A', m),
        "down" => arrow('B', m),
        "right" => arrow('C', m),
        "left" => arrow('D', m),
        "home" => "\x1b[H".into(),
        "end" => "\x1b[F".into(),
        "insert" => tilde(2, m),
        "delete" => tilde(3, m),
        "pageup" => tilde(5, m),
        "pagedown" => tilde(6, m),
        "f1" => "\x1bOP".into(),
        "f2" => "\x1bOQ".into(),
        "f3" => "\x1bOR".into(),
        "f4" => "\x1bOS".into(),
        "f5" => tilde(15, m),
        "f6" => tilde(17, m),
        "f7" => tilde(18, m),
        "f8" => tilde(19, m),
        "f9" => tilde(20, m),
        "f10" => tilde(21, m),
        "f11" => tilde(23, m),
        "f12" => tilde(24, m),
        _ => return None,
    })
}

/// C0 control byte for Ctrl + ASCII key, or None when the combo has no
/// canonical mapping (Ctrl alone is not sent).
fn control_byte(key: &str) -> Option<u8> {
    let c = key.chars().next()?;
    let byte = match c {
        ' ' | '@' => 0x00,
        'a'..='z' => c as u8 - b'a' + 1,
        'A'..='Z' => c as u8 - b'A' + 1,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' | '/' => 0x1f,
        _ => return None,
    };
    Some(byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{Keystroke, Modifiers};

    fn ks(key: &str, key_char: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke { modifiers: m, key: key.into(), key_char: key_char.map(Into::into) }
    }

    fn mods(control: bool, alt: bool, shift: bool, platform: bool) -> Modifiers {
        Modifiers { control, alt, shift, platform, function: false }
    }

    #[test]
    fn printable_goes_through() {
        assert_eq!(encode(&ks("a", Some("a"), mods(false, false, false, false))).as_deref(), Some(b"a".as_slice()));
        assert_eq!(encode(&ks("a", Some("A"), mods(false, false, true, false))).as_deref(), Some(b"A".as_slice()));
        assert_eq!(encode(&ks("space", Some(" "), mods(false, false, false, false))).as_deref(), Some(b" ".as_slice()));
    }

    #[test]
    fn control_keys() {
        assert_eq!(encode(&ks("enter", None, mods(false, false, false, false))).as_deref(), Some(b"\r".as_slice()));
        assert_eq!(encode(&ks("backspace", None, mods(false, false, false, false))).as_deref(), Some(b"\x7f".as_slice()));
        assert_eq!(encode(&ks("tab", None, mods(false, false, true, false))).as_deref(), Some(b"\x1b[Z".as_slice()));
        assert_eq!(encode(&ks("escape", None, mods(false, false, false, false))).as_deref(), Some(b"\x1b".as_slice()));
    }

    #[test]
    fn arrows_and_navigation() {
        assert_eq!(encode(&ks("up", None, mods(false, false, false, false))).as_deref(), Some(b"\x1b[A".as_slice()));
        assert_eq!(encode(&ks("left", None, mods(true, false, false, false))).as_deref(), Some(b"\x1b[1;5D".as_slice()));
        assert_eq!(encode(&ks("delete", None, mods(false, false, false, false))).as_deref(), Some(b"\x1b[3~".as_slice()));
        assert_eq!(encode(&ks("pageup", None, mods(false, false, false, false))).as_deref(), Some(b"\x1b[5~".as_slice()));
    }

    #[test]
    fn control_combos() {
        // Ctrl-C interrupts, Ctrl-D EOFs, Ctrl-Space sends NUL.
        assert_eq!(encode(&ks("c", Some("c"), mods(true, false, false, false))).as_deref(), Some(&[0x03][..]));
        assert_eq!(encode(&ks("d", None, mods(true, false, false, false))).as_deref(), Some(&[0x04][..]));
        assert_eq!(encode(&ks("space", Some(" "), mods(true, false, false, false))).as_deref(), Some(&[0x00][..]));
    }

    #[test]
    fn alt_prefixes_escape() {
        assert_eq!(encode(&ks("x", Some("x"), mods(false, true, false, false))).as_deref(), Some(b"\x1bx".as_slice()));
    }

    #[test]
    fn platform_modifier_is_reserved() {
        assert_eq!(encode(&ks("t", Some("t"), mods(false, false, false, true))), None);
    }
}

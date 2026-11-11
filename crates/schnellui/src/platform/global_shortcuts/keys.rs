//! Pure key and chord tables for the global shortcut backends.
//!
//! Every backend translates a [`Shortcut`] into its system's own vocabulary:
//! XDG shortcut syntax on Linux, modifier flags and virtual-key codes on
//! Windows, Carbon key codes on macOS. The tables live here, free of system
//! calls, so tests on any host can check all three.

// Each target uses its own table; all of them compile everywhere so tests can run them.
#![allow(dead_code)]

use crate::{Shortcut, ShortcutKey};

/// A name for the key as people read it, as in `F5`, `Space` or `A`.
fn key_label(key: ShortcutKey) -> String {
    match key {
        ShortcutKey::Char(c) => c.to_uppercase().collect(),
        ShortcutKey::F(n) => format!("F{n}"),
        ShortcutKey::Space => "Space".into(),
        ShortcutKey::Enter => "Enter".into(),
        ShortcutKey::Tab => "Tab".into(),
        ShortcutKey::Escape => "Esc".into(),
        ShortcutKey::Backspace => "Backspace".into(),
        ShortcutKey::Delete => "Delete".into(),
        ShortcutKey::Insert => "Insert".into(),
        ShortcutKey::Home => "Home".into(),
        ShortcutKey::End => "End".into(),
        ShortcutKey::PageUp => "Page Up".into(),
        ShortcutKey::PageDown => "Page Down".into(),
        ShortcutKey::Left => "Left".into(),
        ShortcutKey::Right => "Right".into(),
        ShortcutKey::Up => "Up".into(),
        ShortcutKey::Down => "Down".into(),
        ShortcutKey::PrintScreen => "Print Screen".into(),
        ShortcutKey::MediaPlayPause => "Play/Pause".into(),
        ShortcutKey::MediaStop => "Stop".into(),
        ShortcutKey::MediaNext => "Next Track".into(),
        ShortcutKey::MediaPrevious => "Previous Track".into(),
        ShortcutKey::VolumeUp => "Volume Up".into(),
        ShortcutKey::VolumeDown => "Volume Down".into(),
        ShortcutKey::VolumeMute => "Volume Mute".into(),
    }
}

/// The chord as people read it, for example `Ctrl+Shift+A`. `command` is how
/// the platform spells its command modifier (`Ctrl`, `Cmd`).
pub(super) fn display_chord(shortcut: Shortcut, command: &str) -> String {
    let mut out = String::new();
    if shortcut.uses_command() {
        out.push_str(command);
        out.push('+');
    }
    if shortcut.uses_alt() {
        out.push_str("Alt+");
    }
    if shortcut.uses_shift() {
        out.push_str("Shift+");
    }
    out.push_str(&key_label(shortcut.logical_key()));
    out
}

/// The XKB keysym name for a punctuation character, as the XDG portal wants it.
fn xdg_punctuation(c: char) -> Option<&'static str> {
    Some(match c {
        ',' => "comma",
        '.' => "period",
        '-' => "minus",
        '=' => "equal",
        '/' => "slash",
        '\\' => "backslash",
        ';' => "semicolon",
        '\'' => "apostrophe",
        '`' => "grave",
        '[' => "bracketleft",
        ']' => "bracketright",
        _ => return None,
    })
}

/// The shortcut in the XDG portal's shortcut syntax, such as `CTRL+SHIFT+a`:
/// modifiers `CTRL`, `ALT` and `SHIFT` joined with `+` before an XKB keysym
/// name. The portal treats it as a hint; the desktop decides the real chord.
/// `None` for a key with no keysym name here (F keys above 35 and the like).
pub(super) fn xdg_trigger(shortcut: Shortcut) -> Option<String> {
    let keysym: String = match shortcut.logical_key() {
        ShortcutKey::Char(c) if c.is_ascii_alphanumeric() => c.to_ascii_lowercase().to_string(),
        ShortcutKey::Char(c) => match xdg_punctuation(c) {
            Some(name) => name.to_owned(),
            None if c.is_control() || c == ' ' => return None,
            None => format!("U{:04X}", c as u32),
        },
        ShortcutKey::F(n @ 1..=35) => format!("F{n}"),
        ShortcutKey::F(_) => return None,
        ShortcutKey::Space => "space".into(),
        ShortcutKey::Enter => "Return".into(),
        ShortcutKey::Tab => "Tab".into(),
        ShortcutKey::Escape => "Escape".into(),
        ShortcutKey::Backspace => "BackSpace".into(),
        ShortcutKey::Delete => "Delete".into(),
        ShortcutKey::Insert => "Insert".into(),
        ShortcutKey::Home => "Home".into(),
        ShortcutKey::End => "End".into(),
        ShortcutKey::PageUp => "Page_Up".into(),
        ShortcutKey::PageDown => "Page_Down".into(),
        ShortcutKey::Left => "Left".into(),
        ShortcutKey::Right => "Right".into(),
        ShortcutKey::Up => "Up".into(),
        ShortcutKey::Down => "Down".into(),
        ShortcutKey::PrintScreen => "Print".into(),
        ShortcutKey::MediaPlayPause => "XF86AudioPlay".into(),
        ShortcutKey::MediaStop => "XF86AudioStop".into(),
        ShortcutKey::MediaNext => "XF86AudioNext".into(),
        ShortcutKey::MediaPrevious => "XF86AudioPrev".into(),
        ShortcutKey::VolumeUp => "XF86AudioRaiseVolume".into(),
        ShortcutKey::VolumeDown => "XF86AudioLowerVolume".into(),
        ShortcutKey::VolumeMute => "XF86AudioMute".into(),
    };
    let mut out = String::new();
    if shortcut.uses_command() {
        out.push_str("CTRL+");
    }
    if shortcut.uses_alt() {
        out.push_str("ALT+");
    }
    if shortcut.uses_shift() {
        out.push_str("SHIFT+");
    }
    out.push_str(&keysym);
    Some(out)
}

/// Windows `RegisterHotKey` modifier flags (`MOD_ALT`, `MOD_CONTROL`,
/// `MOD_SHIFT`). The command modifier is Ctrl there.
pub(super) fn windows_modifiers(shortcut: Shortcut) -> u32 {
    const MOD_ALT: u32 = 0x1;
    const MOD_CONTROL: u32 = 0x2;
    const MOD_SHIFT: u32 = 0x4;
    let mut flags = 0;
    if shortcut.uses_alt() {
        flags |= MOD_ALT;
    }
    if shortcut.uses_command() {
        flags |= MOD_CONTROL;
    }
    if shortcut.uses_shift() {
        flags |= MOD_SHIFT;
    }
    flags
}

/// The Windows virtual-key code for the key. Punctuation uses the OEM codes of
/// the US layout, which is what the virtual-key names mean. `None` when the
/// key has no code.
pub(super) fn windows_virtual_key(key: ShortcutKey) -> Option<u32> {
    Some(match key {
        ShortcutKey::Char(c) => match c {
            'a'..='z' => c as u32 - 'a' as u32 + 0x41,
            'A'..='Z' => c as u32 - 'A' as u32 + 0x41,
            '0'..='9' => c as u32,
            ' ' => 0x20,
            ';' => 0xBA,
            '=' => 0xBB,
            ',' => 0xBC,
            '-' => 0xBD,
            '.' => 0xBE,
            '/' => 0xBF,
            '`' => 0xC0,
            '[' => 0xDB,
            '\\' => 0xDC,
            ']' => 0xDD,
            '\'' => 0xDE,
            _ => return None,
        },
        ShortcutKey::F(n @ 1..=24) => 0x70 + u32::from(n) - 1,
        ShortcutKey::F(_) => return None,
        ShortcutKey::Space => 0x20,
        ShortcutKey::Enter => 0x0D,
        ShortcutKey::Tab => 0x09,
        ShortcutKey::Escape => 0x1B,
        ShortcutKey::Backspace => 0x08,
        ShortcutKey::Delete => 0x2E,
        ShortcutKey::Insert => 0x2D,
        ShortcutKey::Home => 0x24,
        ShortcutKey::End => 0x23,
        ShortcutKey::PageUp => 0x21,
        ShortcutKey::PageDown => 0x22,
        ShortcutKey::Left => 0x25,
        ShortcutKey::Up => 0x26,
        ShortcutKey::Right => 0x27,
        ShortcutKey::Down => 0x28,
        ShortcutKey::PrintScreen => 0x2C,
        ShortcutKey::VolumeMute => 0xAD,
        ShortcutKey::VolumeDown => 0xAE,
        ShortcutKey::VolumeUp => 0xAF,
        ShortcutKey::MediaNext => 0xB0,
        ShortcutKey::MediaPrevious => 0xB1,
        ShortcutKey::MediaStop => 0xB2,
        ShortcutKey::MediaPlayPause => 0xB3,
    })
}

/// Carbon modifier bits (`cmdKey`, `shiftKey`, `optionKey`). The command
/// modifier is Cmd there.
pub(super) fn carbon_modifiers(shortcut: Shortcut) -> u32 {
    const CMD_KEY: u32 = 0x100;
    const SHIFT_KEY: u32 = 0x200;
    const OPTION_KEY: u32 = 0x800;
    let mut flags = 0;
    if shortcut.uses_command() {
        flags |= CMD_KEY;
    }
    if shortcut.uses_shift() {
        flags |= SHIFT_KEY;
    }
    if shortcut.uses_alt() {
        flags |= OPTION_KEY;
    }
    flags
}

/// The macOS virtual key code (`kVK_*`, ANSI layout positions) for the key.
/// Media keys are not hot keys on macOS, so they have none.
pub(super) fn carbon_key_code(key: ShortcutKey) -> Option<u32> {
    Some(match key {
        ShortcutKey::Char(c) => match c.to_ascii_lowercase() {
            'a' => 0x00,
            's' => 0x01,
            'd' => 0x02,
            'f' => 0x03,
            'h' => 0x04,
            'g' => 0x05,
            'z' => 0x06,
            'x' => 0x07,
            'c' => 0x08,
            'v' => 0x09,
            'b' => 0x0B,
            'q' => 0x0C,
            'w' => 0x0D,
            'e' => 0x0E,
            'r' => 0x0F,
            'y' => 0x10,
            't' => 0x11,
            '1' => 0x12,
            '2' => 0x13,
            '3' => 0x14,
            '4' => 0x15,
            '6' => 0x16,
            '5' => 0x17,
            '=' => 0x18,
            '9' => 0x19,
            '7' => 0x1A,
            '-' => 0x1B,
            '8' => 0x1C,
            '0' => 0x1D,
            ']' => 0x1E,
            'o' => 0x1F,
            'u' => 0x20,
            '[' => 0x21,
            'i' => 0x22,
            'p' => 0x23,
            'l' => 0x25,
            'j' => 0x26,
            '\'' => 0x27,
            'k' => 0x28,
            ';' => 0x29,
            '\\' => 0x2A,
            ',' => 0x2B,
            '/' => 0x2C,
            'n' => 0x2D,
            'm' => 0x2E,
            '.' => 0x2F,
            '`' => 0x32,
            ' ' => 0x31,
            _ => return None,
        },
        ShortcutKey::F(n) => match n {
            1 => 0x7A,
            2 => 0x78,
            3 => 0x63,
            4 => 0x76,
            5 => 0x60,
            6 => 0x61,
            7 => 0x62,
            8 => 0x64,
            9 => 0x65,
            10 => 0x6D,
            11 => 0x67,
            12 => 0x6F,
            13 => 0x69,
            14 => 0x6B,
            15 => 0x71,
            16 => 0x6A,
            17 => 0x40,
            18 => 0x4F,
            19 => 0x50,
            20 => 0x5A,
            _ => return None,
        },
        ShortcutKey::Space => 0x31,
        ShortcutKey::Enter => 0x24,
        ShortcutKey::Tab => 0x30,
        ShortcutKey::Escape => 0x35,
        ShortcutKey::Backspace => 0x33,
        ShortcutKey::Delete => 0x75,
        ShortcutKey::Insert => 0x72,
        ShortcutKey::Home => 0x73,
        ShortcutKey::End => 0x77,
        ShortcutKey::PageUp => 0x74,
        ShortcutKey::PageDown => 0x79,
        ShortcutKey::Left => 0x7B,
        ShortcutKey::Right => 0x7C,
        ShortcutKey::Down => 0x7D,
        ShortcutKey::Up => 0x7E,
        ShortcutKey::PrintScreen
        | ShortcutKey::MediaPlayPause
        | ShortcutKey::MediaStop
        | ShortcutKey::MediaNext
        | ShortcutKey::MediaPrevious
        | ShortcutKey::VolumeUp
        | ShortcutKey::VolumeDown
        | ShortcutKey::VolumeMute => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_syntax_orders_modifiers_before_the_keysym() {
        let chord = Shortcut::new('A', true, true, false);
        assert_eq!(xdg_trigger(chord).as_deref(), Some("CTRL+SHIFT+a"));
        let all = Shortcut::new('k', true, true, true);
        assert_eq!(xdg_trigger(all).as_deref(), Some("CTRL+ALT+SHIFT+k"));
        assert_eq!(
            xdg_trigger(Shortcut::new('7', false, false, false)).as_deref(),
            Some("7")
        );
    }

    #[test]
    fn xdg_names_non_character_keys() {
        let key = |k| Shortcut::key(k, false, false, false);
        assert_eq!(xdg_trigger(key(ShortcutKey::F(5))).as_deref(), Some("F5"));
        assert_eq!(xdg_trigger(key(ShortcutKey::F(40))), None);
        assert_eq!(
            xdg_trigger(Shortcut::key(ShortcutKey::Space, true, false, true)).as_deref(),
            Some("CTRL+ALT+space")
        );
        assert_eq!(
            xdg_trigger(key(ShortcutKey::MediaPlayPause)).as_deref(),
            Some("XF86AudioPlay")
        );
        assert_eq!(
            xdg_trigger(key(ShortcutKey::PageUp)).as_deref(),
            Some("Page_Up")
        );
    }

    #[test]
    fn xdg_names_punctuation_and_other_characters() {
        let chord = |c| Shortcut::new(c, true, false, false);
        assert_eq!(xdg_trigger(chord(',')).as_deref(), Some("CTRL+comma"));
        assert_eq!(xdg_trigger(chord('\\')).as_deref(), Some("CTRL+backslash"));
        assert_eq!(
            xdg_trigger(chord('\u{20ac}')).as_deref(),
            Some("CTRL+U20AC")
        );
        assert_eq!(xdg_trigger(chord('\n')), None);
    }

    #[test]
    fn windows_modifier_flags_match_register_hot_key() {
        assert_eq!(
            windows_modifiers(Shortcut::new('a', false, false, false)),
            0
        );
        assert_eq!(
            windows_modifiers(Shortcut::new('a', true, false, false)),
            0x2
        );
        assert_eq!(windows_modifiers(Shortcut::new('a', true, true, true)), 0x7);
        assert_eq!(
            windows_modifiers(Shortcut::new('a', false, true, false)),
            0x4
        );
    }

    #[test]
    fn windows_virtual_keys() {
        let vk = windows_virtual_key;
        assert_eq!(vk(ShortcutKey::Char('a')), Some(0x41));
        assert_eq!(vk(ShortcutKey::Char('Z')), Some(0x5A));
        assert_eq!(vk(ShortcutKey::Char('0')), Some(0x30));
        assert_eq!(vk(ShortcutKey::Char(',')), Some(0xBC));
        assert_eq!(vk(ShortcutKey::F(1)), Some(0x70));
        assert_eq!(vk(ShortcutKey::F(12)), Some(0x7B));
        assert_eq!(vk(ShortcutKey::F(24)), Some(0x87));
        assert_eq!(vk(ShortcutKey::F(25)), None);
        assert_eq!(vk(ShortcutKey::Space), Some(0x20));
        assert_eq!(vk(ShortcutKey::VolumeMute), Some(0xAD));
        assert_eq!(vk(ShortcutKey::MediaPlayPause), Some(0xB3));
        assert_eq!(vk(ShortcutKey::Char('\u{e9}')), None);
    }

    #[test]
    fn carbon_codes_and_modifiers() {
        assert_eq!(
            carbon_modifiers(Shortcut::new('a', true, true, true)),
            0xB00
        );
        assert_eq!(carbon_key_code(ShortcutKey::Char('A')), Some(0x00));
        assert_eq!(carbon_key_code(ShortcutKey::Char('z')), Some(0x06));
        assert_eq!(carbon_key_code(ShortcutKey::F(5)), Some(0x60));
        assert_eq!(carbon_key_code(ShortcutKey::F(21)), None);
        assert_eq!(carbon_key_code(ShortcutKey::Up), Some(0x7E));
        assert_eq!(carbon_key_code(ShortcutKey::VolumeUp), None);
    }

    #[test]
    fn chords_read_as_people_write_them() {
        assert_eq!(
            display_chord(Shortcut::new('a', true, true, false), "Ctrl"),
            "Ctrl+Shift+A"
        );
        assert_eq!(
            display_chord(Shortcut::key(ShortcutKey::F(5), true, false, true), "Cmd"),
            "Cmd+Alt+F5"
        );
    }
}

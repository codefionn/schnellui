//! Pure tables for the global menu backends: key equivalents and the
//! standard items. Free of system calls, so tests on any host can check what
//! the macOS backend will send.

// The macOS backend is the only user outside tests.
#![allow(dead_code)]

use std::ffi::CStr;

use super::StandardItem;
use crate::{Shortcut, ShortcutKey};

/// `NSEventModifierFlagShift`.
const MASK_SHIFT: usize = 1 << 17;
/// `NSEventModifierFlagOption`.
const MASK_OPTION: usize = 1 << 19;
/// `NSEventModifierFlagCommand`.
const MASK_COMMAND: usize = 1 << 20;

/// A key equivalent in AppKit's terms: the key string of
/// `NSMenuItem.keyEquivalent` and the modifier flags of
/// `keyEquivalentModifierMask`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEquivalent {
    /// What `keyEquivalent` is set to: a lowercase character, a control
    /// character or a private-use function key code.
    pub key: String,
    /// The `NSEventModifierFlags` bits. Zero means no modifier at all, which
    /// the item must set explicitly, since AppKit's default is Command.
    pub modifier_mask: usize,
}

/// The private-use character AppKit uses for a function key: `NSUpArrowFunctionKey`
/// is U+F700 and the rest follow the table in `NSEvent.h`.
fn function_key(code: u32) -> String {
    char::from_u32(code).map(String::from).unwrap_or_default()
}

/// The key equivalent for `shortcut`, or `None` for a key a menu cannot hold
/// (the media and volume keys, an F key past F35, a control character).
pub fn key_equivalent(shortcut: Shortcut) -> Option<KeyEquivalent> {
    let key = match shortcut.logical_key() {
        ShortcutKey::Char(c) if c == ' ' || c.is_control() => return None,
        // An uppercase letter would make AppKit add Shift by itself, so the
        // character is lowercased and Shift travels in the mask alone.
        ShortcutKey::Char(c) => c.to_lowercase().collect(),
        ShortcutKey::F(n @ 1..=35) => function_key(0xF703 + u32::from(n)),
        ShortcutKey::F(_) => return None,
        ShortcutKey::Space => " ".into(),
        ShortcutKey::Enter => "\r".into(),
        ShortcutKey::Tab => "\t".into(),
        ShortcutKey::Escape => "\u{1b}".into(),
        ShortcutKey::Backspace => "\u{8}".into(),
        ShortcutKey::Delete => function_key(0xF728),
        ShortcutKey::Insert => function_key(0xF727),
        ShortcutKey::Home => function_key(0xF729),
        ShortcutKey::End => function_key(0xF72B),
        ShortcutKey::PageUp => function_key(0xF72C),
        ShortcutKey::PageDown => function_key(0xF72D),
        ShortcutKey::Up => function_key(0xF700),
        ShortcutKey::Down => function_key(0xF701),
        ShortcutKey::Left => function_key(0xF702),
        ShortcutKey::Right => function_key(0xF703),
        ShortcutKey::PrintScreen => function_key(0xF738),
        ShortcutKey::MediaPlayPause
        | ShortcutKey::MediaStop
        | ShortcutKey::MediaNext
        | ShortcutKey::MediaPrevious
        | ShortcutKey::VolumeUp
        | ShortcutKey::VolumeDown
        | ShortcutKey::VolumeMute => return None,
    };
    let mut modifier_mask = 0;
    if shortcut.uses_command() {
        modifier_mask |= MASK_COMMAND;
    }
    if shortcut.uses_shift() {
        modifier_mask |= MASK_SHIFT;
    }
    if shortcut.uses_alt() {
        modifier_mask |= MASK_OPTION;
    }
    Some(KeyEquivalent { key, modifier_mask })
}

impl StandardItem {
    /// Whether the system cannot carry this item out in a schnellui window and
    /// reports it as [`GlobalMenuEvent::Standard`](super::GlobalMenuEvent::Standard)
    /// instead.
    pub fn is_edit(self) -> bool {
        matches!(
            self,
            Self::Undo | Self::Redo | Self::Cut | Self::Copy | Self::Paste | Self::SelectAll
        )
    }

    /// The text the system shows. `app_name` fills the items macOS names after
    /// the app.
    pub fn label(self, app_name: &str) -> String {
        match self {
            Self::About => format!("About {app_name}"),
            Self::Hide => format!("Hide {app_name}"),
            Self::HideOthers => "Hide Others".into(),
            Self::ShowAll => "Show All".into(),
            Self::Quit => format!("Quit {app_name}"),
            Self::Minimize => "Minimize".into(),
            Self::Zoom => "Zoom".into(),
            Self::Undo => "Undo".into(),
            Self::Redo => "Redo".into(),
            Self::Cut => "Cut".into(),
            Self::Copy => "Copy".into(),
            Self::Paste => "Paste".into(),
            Self::SelectAll => "Select All".into(),
        }
    }

    /// The fixed key equivalent macOS gives the item.
    pub fn shortcut(self) -> Option<Shortcut> {
        Some(match self {
            Self::Hide => Shortcut::command('h'),
            Self::HideOthers => Shortcut::new('h', true, false, true),
            Self::Quit => Shortcut::command('q'),
            Self::Minimize => Shortcut::command('m'),
            Self::Undo => Shortcut::command('z'),
            Self::Redo => Shortcut::new('z', true, true, false),
            Self::Cut => Shortcut::command('x'),
            Self::Copy => Shortcut::command('c'),
            Self::Paste => Shortcut::command('v'),
            Self::SelectAll => Shortcut::command('a'),
            Self::About | Self::ShowAll | Self::Zoom => return None,
        })
    }

    /// The Objective-C selector of the responder-chain action that carries the
    /// item out, sent with no target so it walks the chain up to the
    /// application or the key window. `None` for the edit items.
    pub(super) fn native_selector(self) -> Option<&'static CStr> {
        Some(match self {
            Self::About => c"orderFrontStandardAboutPanel:",
            Self::Hide => c"hide:",
            Self::HideOthers => c"hideOtherApplications:",
            Self::ShowAll => c"unhideAllApplications:",
            Self::Quit => c"terminate:",
            Self::Minimize => c"performMiniaturize:",
            Self::Zoom => c"performZoom:",
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eq(shortcut: Shortcut) -> (String, usize) {
        let k = key_equivalent(shortcut).unwrap();
        (k.key, k.modifier_mask)
    }

    #[test]
    fn characters_are_lowercase_and_modifiers_go_in_the_mask() {
        assert_eq!(eq(Shortcut::command('n')), ("n".into(), 1 << 20));
        assert_eq!(eq(Shortcut::command('N')), ("n".into(), 1 << 20));
        assert_eq!(
            eq(Shortcut::new('z', true, true, true)),
            ("z".into(), (1 << 20) | (1 << 17) | (1 << 19))
        );
        assert_eq!(
            eq(Shortcut::new(',', true, false, false)),
            (",".into(), 1 << 20)
        );
    }

    #[test]
    fn a_bare_key_has_an_empty_mask() {
        assert_eq!(eq(Shortcut::new('x', false, false, false)), ("x".into(), 0));
    }

    #[test]
    fn named_keys_use_appkits_codes() {
        let key = |k| eq(Shortcut::key(k, false, false, false)).0;
        assert_eq!(key(ShortcutKey::F(1)), "\u{F704}");
        assert_eq!(key(ShortcutKey::F(5)), "\u{F708}");
        assert_eq!(key(ShortcutKey::Up), "\u{F700}");
        assert_eq!(key(ShortcutKey::Right), "\u{F703}");
        assert_eq!(key(ShortcutKey::Delete), "\u{F728}");
        assert_eq!(key(ShortcutKey::Enter), "\r");
        assert_eq!(key(ShortcutKey::Escape), "\u{1b}");
        assert_eq!(key(ShortcutKey::Space), " ");
    }

    #[test]
    fn keys_a_menu_cannot_hold_have_no_equivalent() {
        let none = |k| key_equivalent(Shortcut::key(k, true, false, false)).is_none();
        assert!(none(ShortcutKey::MediaNext));
        assert!(none(ShortcutKey::VolumeMute));
        assert!(none(ShortcutKey::F(36)));
        assert!(none(ShortcutKey::Char('\n')));
    }

    #[test]
    fn standard_items_have_native_actions_except_the_edit_set() {
        use StandardItem::*;
        for item in [About, Hide, HideOthers, ShowAll, Quit, Minimize, Zoom] {
            assert!(!item.is_edit());
            let selector = item.native_selector().unwrap().to_str().unwrap();
            assert!(selector.ends_with(':'), "{selector}");
        }
        for item in [Undo, Redo, Cut, Copy, Paste, SelectAll] {
            assert!(item.is_edit());
            assert!(item.native_selector().is_none());
            assert!(item.shortcut().is_some());
        }
        assert_eq!(Quit.native_selector().unwrap().to_str(), Ok("terminate:"));
    }

    #[test]
    fn standard_labels_and_chords_follow_the_mac_convention() {
        assert_eq!(StandardItem::Quit.label("Demo"), "Quit Demo");
        assert_eq!(StandardItem::Hide.label("Demo"), "Hide Demo");
        assert_eq!(StandardItem::SelectAll.label("Demo"), "Select All");
        assert_eq!(
            StandardItem::Redo.shortcut(),
            Some(Shortcut::new('z', true, true, false))
        );
        assert_eq!(
            StandardItem::HideOthers.shortcut(),
            Some(Shortcut::new('h', true, false, true))
        );
        assert_eq!(StandardItem::About.shortcut(), None);
    }
}

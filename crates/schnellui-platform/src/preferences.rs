//! Platform preference snapshots and change notifications.

use std::collections::HashSet;
use std::env;

pub use schnellui_localization::Locale;

use crate::platform;

/// A platform-independent light or dark appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    #[default]
    Light,
    Dark,
}

/// A platform-independent contrast preference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contrast {
    /// The user requested more contrast than the normal appearance.
    High,
}

/// A point-in-time collection of UI-relevant platform preferences.
///
/// `None` means that the target did not report a value. In particular, it never
/// means `light`, `false`, `1.0`, or `16px`; callers choose their own fallback.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlatformPreferences {
    /// Preferred system appearance, when reported outside an individual window.
    pub color_scheme: Option<ColorScheme>,
    /// Ordered locale preferences, normalized as BCP-47-style identifiers.
    pub locales: Vec<Locale>,
    /// System UI font size in logical typographic points, when exposed by the platform.
    ///
    /// This is the unscaled base size. It is deliberately separate from
    /// [`text_scale`](Self::text_scale).
    pub base_font_size: Option<f32>,
    /// Accessibility text multiplier, where `1.0` is the platform's normal size.
    pub text_scale: Option<f32>,
    /// Requested contrast level, when exposed by the platform.
    pub contrast: Option<Contrast>,
    /// Whether non-essential motion should be reduced.
    pub reduced_motion: Option<bool>,
    /// Whether translucent UI surfaces should be made opaque.
    pub reduced_transparency: Option<bool>,
}

impl PlatformPreferences {
    /// Detects current preferences without panicking.
    ///
    /// Detection may briefly communicate with the desktop settings portal on
    /// Linux. Native errors simply leave the affected field unavailable.
    pub fn detect() -> Self {
        let mut preferences = Self {
            locales: locales_from(|name| env::var(name).ok()),
            ..Self::default()
        };
        platform::detect(&mut preferences);
        preferences
    }
}

/// A preference update emitted by [`watch_preferences`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceChange {
    ColorScheme(Option<ColorScheme>),
    Contrast(Option<Contrast>),
    ReducedMotion(bool),
    ReducedTransparency(bool),
}

/// Starts a detached platform preference watcher.
///
/// The callback returns `true` to keep watching and `false` to stop. The return
/// value indicates whether this target supports watching. A watcher also emits
/// the first successfully read value, so hosts need no separate initial probe.
pub fn watch_preferences(callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    platform::watch(callback)
}

fn locales_from(mut get: impl FnMut(&str) -> Option<String>) -> Vec<Locale> {
    let mut values = Vec::new();
    if let Some(language) = get("LANGUAGE") {
        values.extend(language.split(':').map(str::to_owned));
    }
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(value) = get(name) {
            values.push(value);
        }
    }

    let mut seen = HashSet::new();
    values
        .into_iter()
        .filter_map(Locale::parse)
        .filter(|locale| seen.insert(locale.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn locale_detection_preserves_priority_and_deduplicates() {
        let variables = HashMap::from([
            ("LANGUAGE", "de_DE:fr-FR:de-DE"),
            ("LC_ALL", "fr_FR.UTF-8"),
            ("LC_MESSAGES", "en_GB"),
            ("LANG", "C"),
        ]);
        let locales = locales_from(|name| variables.get(name).map(ToString::to_string));
        let tags: Vec<_> = locales.iter().map(Locale::as_str).collect();
        assert_eq!(tags, ["de-DE", "fr-FR", "en-GB"]);
    }

    #[test]
    fn locale_detection_ignores_invalid_and_empty_values() {
        let variables = HashMap::from([("LANGUAGE", ":::not a locale"), ("LANG", "POSIX")]);
        assert!(locales_from(|name| variables.get(name).map(ToString::to_string)).is_empty());
    }

    #[test]
    fn missing_values_do_not_gain_policy_defaults() {
        assert_eq!(
            PlatformPreferences::default(),
            PlatformPreferences {
                color_scheme: None,
                locales: Vec::new(),
                base_font_size: None,
                text_scale: None,
                contrast: None,
                reduced_motion: None,
                reduced_transparency: None,
            }
        );
    }
}

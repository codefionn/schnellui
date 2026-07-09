use std::time::Duration;

use objc2_app_kit::{NSFont, NSWorkspace};
use objc2_foundation::NSLocale;

use crate::{Contrast, Locale, PlatformPreferences, PreferenceChange};

pub(crate) fn detect(preferences: &mut PlatformPreferences) {
    // SAFETY: these are side-effect-free AppKit class/getter methods.
    unsafe {
        let workspace = NSWorkspace::sharedWorkspace();
        let locales = NSLocale::preferredLanguages();
        preferences.locales = locales
            .iter()
            .filter_map(|language| Locale::parse(language.to_string()))
            .collect();
        preferences.base_font_size = Some(NSFont::systemFontSize() as f32);
        preferences.reduced_motion = Some(workspace.accessibilityDisplayShouldReduceMotion());
        preferences.reduced_transparency =
            Some(workspace.accessibilityDisplayShouldReduceTransparency());
        if workspace.accessibilityDisplayShouldIncreaseContrast() {
            preferences.contrast = Some(Contrast::High);
        }
    }
}

pub(crate) fn watch(mut callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    std::thread::spawn(move || {
        let mut previous = None;
        loop {
            // SAFETY: NSWorkspace accessibility preferences are getter methods.
            let current = unsafe {
                let workspace = NSWorkspace::sharedWorkspace();
                (
                    workspace.accessibilityDisplayShouldReduceMotion(),
                    workspace.accessibilityDisplayShouldReduceTransparency(),
                    workspace.accessibilityDisplayShouldIncreaseContrast(),
                )
            };
            if previous.map(|value: (bool, bool, bool)| value.0) != Some(current.0)
                && !callback(PreferenceChange::ReducedMotion(current.0))
            {
                return;
            }
            if previous.map(|value| value.1) != Some(current.1)
                && !callback(PreferenceChange::ReducedTransparency(current.1))
            {
                return;
            }
            if previous.map(|value| value.2) != Some(current.2)
                && !callback(PreferenceChange::Contrast(
                    current.2.then_some(Contrast::High),
                ))
            {
                return;
            }
            previous = Some(current);
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    true
}

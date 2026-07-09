use std::time::Duration;

use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;
use windows_sys::Win32::System::SystemServices::LOCALE_NAME_MAX_LENGTH;
use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    SystemParametersInfoW, NONCLIENTMETRICSW, SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST,
    SPI_GETNONCLIENTMETRICS,
};

use crate::{Contrast, Locale, PlatformPreferences, PreferenceChange};

pub(crate) fn detect(preferences: &mut PlatformPreferences) {
    if let Some(locale) = user_locale() {
        preferences.locales.retain(|candidate| candidate != &locale);
        preferences.locales.insert(0, locale);
    }
    preferences.base_font_size = base_font_size();
    preferences.reduced_motion = reduced_motion();
    if high_contrast() == Some(true) {
        preferences.contrast = Some(Contrast::High);
    }
}

pub(crate) fn watch(mut callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    std::thread::spawn(move || {
        let mut previous_motion = None;
        let mut previous_contrast = None;
        loop {
            if let Some(current) = reduced_motion() {
                if previous_motion != Some(current)
                    && !callback(PreferenceChange::ReducedMotion(current))
                {
                    return;
                }
                previous_motion = Some(current);
            }
            if let Some(current) = high_contrast() {
                if previous_contrast != Some(current)
                    && !callback(PreferenceChange::Contrast(
                        current.then_some(Contrast::High),
                    ))
                {
                    return;
                }
                previous_contrast = Some(current);
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
    true
}

fn reduced_motion() -> Option<bool> {
    let mut animations_enabled = 1i32;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes a BOOL to the valid out pointer.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            (&raw mut animations_enabled).cast(),
            0,
        )
    };
    (ok != 0).then_some(animations_enabled == 0)
}

fn high_contrast() -> Option<bool> {
    let mut value = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..HIGHCONTRASTW::default()
    };
    // SAFETY: SPI_GETHIGHCONTRAST writes to the initialized structure.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            value.cbSize,
            (&raw mut value).cast(),
            0,
        )
    };
    (ok != 0).then_some(value.dwFlags & HCF_HIGHCONTRASTON != 0)
}

fn base_font_size() -> Option<f32> {
    let mut metrics = NONCLIENTMETRICSW {
        cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
        ..NONCLIENTMETRICSW::default()
    };
    // SAFETY: SPI_GETNONCLIENTMETRICS writes to the initialized structure.
    let ok = unsafe {
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            metrics.cbSize,
            (&raw mut metrics).cast(),
            0,
        )
    };
    if ok == 0 {
        return None;
    }
    // A negative LOGFONT height describes character height in device pixels.
    // Normalize it back to typographic points using the system DPI.
    let dpi = unsafe { GetDpiForSystem() };
    let height = metrics.lfMessageFont.lfHeight.unsigned_abs();
    (dpi > 0 && height > 0).then_some(height as f32 * 72.0 / dpi as f32)
}

fn user_locale() -> Option<Locale> {
    let mut buffer = [0u16; LOCALE_NAME_MAX_LENGTH as usize];
    // SAFETY: `buffer` is writable for the advertised number of UTF-16 units.
    let length =
        unsafe { GetUserDefaultLocaleName(buffer.as_mut_ptr(), LOCALE_NAME_MAX_LENGTH as i32) };
    if length <= 1 {
        return None;
    }
    String::from_utf16(&buffer[..length as usize - 1])
        .ok()
        .and_then(Locale::parse)
}

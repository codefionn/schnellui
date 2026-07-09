use crate::{ColorScheme, Contrast, PlatformPreferences, PreferenceChange};
use ashpd::desktop::settings::{
    ColorScheme as PortalColorScheme, Contrast as PortalContrast, ReducedMotion, Settings,
};
use futures_util::StreamExt;

pub(crate) fn detect(preferences: &mut PlatformPreferences) {
    let detected = async_io::block_on(async {
        let settings = Settings::new().await.ok()?;
        let color_scheme = settings.color_scheme().await.ok().and_then(color_scheme);
        let contrast = settings.contrast().await.ok().and_then(contrast);
        let reduced_motion = settings
            .reduced_motion()
            .await
            .ok()
            .map(|value| value == ReducedMotion::ReducedMotion);
        let text_scale = settings
            .read::<f64>("org.gnome.desktop.interface", "text-scaling-factor")
            .await
            .ok()
            .and_then(valid_scale);
        let base_font_size = settings
            .read::<String>("org.gnome.desktop.interface", "font-name")
            .await
            .ok()
            .and_then(|value| font_size_from_description(&value));
        Some((
            color_scheme,
            contrast,
            reduced_motion,
            text_scale,
            base_font_size,
        ))
    });
    if let Some((color_scheme, contrast, reduced_motion, text_scale, base_font_size)) = detected {
        preferences.color_scheme = color_scheme;
        preferences.contrast = contrast;
        preferences.reduced_motion = reduced_motion;
        preferences.text_scale = text_scale;
        preferences.base_font_size = base_font_size;
    }
}

pub(crate) fn watch(mut callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    std::thread::spawn(move || {
        async_io::block_on(async move {
            let Ok(settings) = Settings::new().await else {
                return;
            };
            let Ok(mut changes) = settings.receive_reduced_motion_changed().await else {
                return;
            };
            if let Ok(value) = settings.reduced_motion().await {
                if !callback(PreferenceChange::ReducedMotion(
                    value == ReducedMotion::ReducedMotion,
                )) {
                    return;
                }
            }
            while let Some(value) = changes.next().await {
                if !callback(PreferenceChange::ReducedMotion(
                    value == ReducedMotion::ReducedMotion,
                )) {
                    return;
                }
            }
        });
    });
    true
}

fn color_scheme(value: PortalColorScheme) -> Option<ColorScheme> {
    match value {
        PortalColorScheme::PreferDark => Some(ColorScheme::Dark),
        PortalColorScheme::PreferLight => Some(ColorScheme::Light),
        PortalColorScheme::NoPreference => None,
    }
}

fn contrast(value: PortalContrast) -> Option<Contrast> {
    match value {
        PortalContrast::High => Some(Contrast::High),
        PortalContrast::NoPreference => None,
    }
}

fn valid_scale(value: f64) -> Option<f32> {
    (value.is_finite() && (0.25..=8.0).contains(&value)).then_some(value as f32)
}

fn font_size_from_description(value: &str) -> Option<f32> {
    let size = value.split_whitespace().next_back()?.parse::<f32>().ok()?;
    (size.is_finite() && (4.0..=256.0).contains(&size)).then_some(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_font_size_at_end_of_font_description() {
        assert_eq!(font_size_from_description("Cantarell 11"), Some(11.0));
        assert_eq!(
            font_size_from_description("Noto Sans Display 10.5"),
            Some(10.5)
        );
        assert_eq!(font_size_from_description("Noto Sans"), None);
    }

    #[test]
    fn rejects_implausible_text_scales() {
        assert_eq!(valid_scale(1.25), Some(1.25));
        assert_eq!(valid_scale(f64::NAN), None);
        assert_eq!(valid_scale(12.0), None);
    }
}

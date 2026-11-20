use crate::platform::{ColorScheme, Contrast, PlatformPreferences, PreferenceChange};
use ashpd::desktop::settings::{
    ColorScheme as PortalColorScheme, Contrast as PortalContrast, ReducedMotion, Settings,
};
use futures_util::StreamExt;

/// Registers `id` with the portal's host registry and says whether it did. Inside a
/// sandbox the portal knows the sandbox's own id already, so nothing registers.
pub(crate) fn register_app_id(id: &str) -> Result<bool, String> {
    if ashpd::is_sandboxed() {
        return Ok(false);
    }
    let id = ashpd::AppID::try_from(id).map_err(|e| e.to_string())?;
    async_io::block_on(ashpd::register_host_app(id)).map_err(|e| e.to_string())?;
    Ok(true)
}

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
            // Current values first, so hosts need no separate probe.
            if let Ok(value) = settings.color_scheme().await {
                if !callback(PreferenceChange::ColorScheme(color_scheme(value))) {
                    return;
                }
            }
            if let Ok(value) = settings.reduced_motion().await {
                if !callback(PreferenceChange::ReducedMotion(
                    value == ReducedMotion::ReducedMotion,
                )) {
                    return;
                }
            }
            // A portal without one of the keys still reports the other.
            let schemes = match settings.receive_color_scheme_changed().await {
                Ok(stream) => stream
                    .map(|value| PreferenceChange::ColorScheme(color_scheme(value)))
                    .left_stream(),
                Err(_) => futures_util::stream::empty().right_stream(),
            };
            let motion = match settings.receive_reduced_motion_changed().await {
                Ok(stream) => stream
                    .map(|value| {
                        PreferenceChange::ReducedMotion(value == ReducedMotion::ReducedMotion)
                    })
                    .left_stream(),
                Err(_) => futures_util::stream::empty().right_stream(),
            };
            let mut changes = std::pin::pin!(futures_util::stream::select(schemes, motion));
            while let Some(change) = changes.next().await {
                if !callback(change) {
                    return;
                }
            }
        });
    });
    true
}

/// The xdg-decoration global through which a client asks the compositor to
/// draw its frame. Compositors that draw none, like Mutter, do not offer it.
const DECORATION_MANAGER: &str = "zxdg_decoration_manager_v1";

/// Asks the Wayland compositor, when the app will run on one, whether it offers
/// server-side decorations. X11 window managers draw frames.
pub(crate) fn windows_always_undecorated() -> bool {
    // winit picks Wayland whenever WAYLAND_DISPLAY is set. WAYLAND_SOCKET is an
    // inherited fd that only one connection can take, so leave it to winit.
    if std::env::var_os("WAYLAND_DISPLAY").is_none() || std::env::var_os("WAYLAND_SOCKET").is_some()
    {
        return false;
    }
    wayland_globals()
        .is_some_and(|globals| lacks_server_decorations(globals.iter().map(String::as_str)))
}

fn lacks_server_decorations<'a>(mut interfaces: impl Iterator<Item = &'a str>) -> bool {
    !interfaces.any(|interface| interface == DECORATION_MANAGER)
}

/// The interface names the compositor advertises, read on a connection of our own.
fn wayland_globals() -> Option<Vec<String>> {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    struct Registry;
    impl Dispatch<WlRegistry, GlobalListContents> for Registry {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    let connection = Connection::connect_to_env().ok()?;
    let (globals, _queue) = registry_queue_init::<Registry>(&connection).ok()?;
    Some(
        globals
            .contents()
            .with_list(|list| list.iter().map(|global| global.interface.clone()).collect()),
    )
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
    fn a_compositor_without_the_decoration_manager_leaves_windows_undecorated() {
        // Mutter offers neither xdg-decoration nor KDE's older protocol.
        assert!(lacks_server_decorations(
            [
                "wl_compositor",
                "xdg_wm_base",
                "org_kde_kwin_server_decoration_manager"
            ]
            .into_iter()
        ));
        assert!(!lacks_server_decorations(
            ["wl_compositor", "zxdg_decoration_manager_v1", "xdg_wm_base"].into_iter()
        ));
        assert!(lacks_server_decorations(std::iter::empty()));
    }

    #[test]
    fn rejects_implausible_text_scales() {
        assert_eq!(valid_scale(1.25), Some(1.25));
        assert_eq!(valid_scale(f64::NAN), None);
        assert_eq!(valid_scale(12.0), None);
    }
}

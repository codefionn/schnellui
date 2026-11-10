use crate::platform::{PlatformPreferences, PreferenceChange};

pub(crate) fn detect(_preferences: &mut PlatformPreferences) {}

pub(crate) fn watch(_callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    false
}

/// No desktop portal here, so nothing registers.
pub(crate) fn register_app_id(_id: &str) -> Result<bool, String> {
    Ok(false)
}

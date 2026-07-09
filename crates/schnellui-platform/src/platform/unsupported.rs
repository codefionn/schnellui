use crate::{PlatformPreferences, PreferenceChange};

pub(crate) fn detect(_preferences: &mut PlatformPreferences) {}

pub(crate) fn watch(_callback: impl FnMut(PreferenceChange) -> bool + Send + 'static) -> bool {
    false
}

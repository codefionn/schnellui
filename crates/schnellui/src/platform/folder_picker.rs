//! The platform's native folder chooser.

use std::path::PathBuf;

/// Asks the user to choose a folder with the platform's own dialog.
///
/// The dialog runs on its own thread, so the call returns at once. `done`
/// receives the chosen folder, or `None` when the user cancels or the dialog
/// fails. The return value indicates whether this target has a native chooser;
/// when it is `false`, `done` is never called.
///
/// Linux asks the XDG desktop portal. Other targets report no chooser yet.
pub fn pick_folder(title: &str, done: impl FnOnce(Option<PathBuf>) + Send + 'static) -> bool {
    imp::pick_folder(title, done)
}

#[cfg(target_os = "linux")]
mod imp {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::path::PathBuf;

    use ashpd::desktop::file_chooser::SelectedFiles;

    pub(super) fn pick_folder(
        title: &str,
        done: impl FnOnce(Option<PathBuf>) + Send + 'static,
    ) -> bool {
        let title = title.to_owned();
        std::thread::spawn(move || {
            let chosen = async_io::block_on(async {
                let files = SelectedFiles::open_file()
                    .title(title.as_str())
                    .directory(true)
                    .modal(true)
                    .send()
                    .await
                    .ok()?
                    .response()
                    .ok()?;
                files.uris().first().and_then(|uri| file_path(uri.as_str()))
            });
            done(chosen);
        });
        true
    }

    /// Decodes a `file://` URI into a local path.
    pub(super) fn file_path(uri: &str) -> Option<PathBuf> {
        let rest = uri.strip_prefix("file://")?;
        let path = &rest[rest.find('/')?..];
        let bytes = path.as_bytes();
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        Some(PathBuf::from(OsString::from_vec(out)))
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::path::PathBuf;

    pub(super) fn pick_folder(
        _title: &str,
        _done: impl FnOnce(Option<PathBuf>) + Send + 'static,
    ) -> bool {
        false
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::imp::file_path;
    use std::path::Path;

    #[test]
    fn decodes_file_uris() {
        assert_eq!(
            file_path("file:///home/me/my%20repo").as_deref(),
            Some(Path::new("/home/me/my repo"))
        );
        assert_eq!(
            file_path("file://localhost/srv/app").as_deref(),
            Some(Path::new("/srv/app"))
        );
        assert_eq!(file_path("https://example.com/x"), None);
        assert_eq!(file_path("file:///bad%2"), None);
    }
}

//! The platform's native folder chooser.

use std::path::PathBuf;

/// Asks the user to choose a folder with the platform's own dialog.
///
/// The dialog runs on its own thread, so the call returns at once. `done`
/// receives the chosen folder, or `None` when the user cancels or the dialog
/// fails. The return value indicates whether this target has a native chooser;
/// when it is `false`, `done` is never called.
///
/// Linux asks the XDG desktop portal and Windows shows the shell's folder
/// dialog (`IFileOpenDialog`). Other targets report no chooser yet.
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

#[cfg(target_os = "windows")]
mod imp {
    use std::ffi::{c_void, OsStr, OsString};
    use std::marker::PhantomData;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use std::ptr;

    use windows_sys::core::{GUID, HRESULT, PWSTR};
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    use windows_sys::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    };
    use windows_sys::Win32::UI::Shell::{
        FileOpenDialog, FILEOPENDIALOGOPTIONS, FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, SIGDN,
        SIGDN_FILESYSPATH,
    };

    const IID_IFILEOPENDIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);

    /// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`: what `Show` returns when the user
    /// closes the dialog without choosing a folder.
    const CANCELLED: HRESULT = (ERROR_CANCELLED | 0x8007_0000) as HRESULT;

    /// A vtable slot this module never calls.
    type Unused = usize;

    /// The `IUnknown` slots every COM vtable starts with.
    #[repr(C)]
    struct UnknownVtbl {
        query_interface: Unused,
        add_ref: Unused,
        release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    }

    /// The start of the `IFileOpenDialog` vtable: `IUnknown`, then
    /// `IModalWindow::Show`, then `IFileDialog` in the declaration order of
    /// `shobjidl_core.h`, up to `GetResult`.
    #[repr(C)]
    struct FileOpenDialogVtbl {
        unknown: UnknownVtbl,
        show: unsafe extern "system" fn(this: *mut c_void, owner: *mut c_void) -> HRESULT,
        set_file_types: Unused,
        set_file_type_index: Unused,
        get_file_type_index: Unused,
        advise: Unused,
        unadvise: Unused,
        set_options:
            unsafe extern "system" fn(this: *mut c_void, options: FILEOPENDIALOGOPTIONS) -> HRESULT,
        get_options: unsafe extern "system" fn(
            this: *mut c_void,
            options: *mut FILEOPENDIALOGOPTIONS,
        ) -> HRESULT,
        set_default_folder: Unused,
        set_folder: Unused,
        get_folder: Unused,
        get_current_selection: Unused,
        set_file_name: Unused,
        get_file_name: Unused,
        set_title: unsafe extern "system" fn(this: *mut c_void, title: *const u16) -> HRESULT,
        set_ok_button_label: Unused,
        set_file_name_label: Unused,
        get_result:
            unsafe extern "system" fn(this: *mut c_void, item: *mut *mut c_void) -> HRESULT,
    }

    /// The start of the `IShellItem` vtable, up to `GetDisplayName`.
    #[repr(C)]
    struct ShellItemVtbl {
        unknown: UnknownVtbl,
        bind_to_handler: Unused,
        get_parent: Unused,
        get_display_name:
            unsafe extern "system" fn(this: *mut c_void, kind: SIGDN, name: *mut PWSTR) -> HRESULT,
    }

    /// One owned reference to a COM object whose vtable starts with `V`,
    /// released on drop.
    struct Com<V> {
        this: *mut c_void,
        _vtbl: PhantomData<V>,
    }

    impl<V> Com<V> {
        /// Takes over one reference. `this` must be null or a live COM object
        /// whose vtable starts with `V`'s slots, which every caller gets from
        /// the IID it asked for.
        fn new(this: *mut c_void) -> Option<Self> {
            (!this.is_null()).then_some(Self {
                this,
                _vtbl: PhantomData,
            })
        }

        fn vtbl(&self) -> &V {
            // SAFETY: a COM object's first field points at its vtable, which
            // starts with `V`'s slots (see `new`) and lives as long as the object.
            unsafe { &**(self.this as *const *const V) }
        }
    }

    impl<V> Drop for Com<V> {
        fn drop(&mut self) {
            // SAFETY: every COM vtable starts with the `IUnknown` slots, and this
            // owns exactly one reference to give back.
            unsafe {
                let unknown = &**(self.this as *const *const UnknownVtbl);
                (unknown.release)(self.this);
            }
        }
    }

    pub(super) fn pick_folder(
        title: &str,
        done: impl FnOnce(Option<PathBuf>) + Send + 'static,
    ) -> bool {
        let title: Vec<u16> = OsStr::new(title)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        std::thread::Builder::new()
            .name("schnellui-folder-picker".into())
            .spawn(move || {
                // SAFETY: called once on this new thread, before any COM use.
                let initialized = unsafe {
                    CoInitializeEx(
                        ptr::null(),
                        (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
                    )
                } >= 0;
                let chosen = if initialized { show(&title) } else { None };
                if initialized {
                    // SAFETY: pairs with the successful `CoInitializeEx` above.
                    // `show` has released every interface it created by now.
                    unsafe { CoUninitialize() };
                }
                done(chosen);
            })
            .is_ok()
    }

    /// Shows the folder dialog and returns the chosen folder. `title` ends in
    /// a NUL.
    fn show(title: &[u16]) -> Option<PathBuf> {
        let mut raw = ptr::null_mut();
        // SAFETY: the CLSID and IID name the shell's file open dialog, and `raw`
        // receives an `IFileOpenDialog` pointer on success.
        let hr = unsafe {
            CoCreateInstance(
                &FileOpenDialog,
                ptr::null_mut(),
                CLSCTX_INPROC_SERVER,
                &IID_IFILEOPENDIALOG,
                &mut raw,
            )
        };
        if hr < 0 {
            return None;
        }
        let dialog = Com::<FileOpenDialogVtbl>::new(raw)?;
        let vtbl = dialog.vtbl();

        // SAFETY: each call passes the dialog's own pointer and arguments of the
        // types `shobjidl_core.h` declares. `title` is NUL-terminated and
        // outlives the call, and a null owner shows an unowned dialog.
        // `GetResult` hands back an `IShellItem` reference that `Com` releases.
        let item = unsafe {
            let mut options: FILEOPENDIALOGOPTIONS = 0;
            if (vtbl.get_options)(dialog.this, &mut options) < 0 {
                return None;
            }
            let options = options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM;
            if (vtbl.set_options)(dialog.this, options) < 0 {
                return None;
            }
            // A failed title only leaves the dialog's default one.
            (vtbl.set_title)(dialog.this, title.as_ptr());
            let hr = (vtbl.show)(dialog.this, ptr::null_mut());
            if hr == CANCELLED || hr < 0 {
                return None;
            }
            let mut item = ptr::null_mut();
            if (vtbl.get_result)(dialog.this, &mut item) < 0 {
                return None;
            }
            Com::<ShellItemVtbl>::new(item)?
        };

        let mut name: PWSTR = ptr::null_mut();
        // SAFETY: `item` is a live `IShellItem`. On success `name` is a
        // NUL-terminated string this side owns and frees with `CoTaskMemFree`
        // after copying it.
        unsafe {
            if (item.vtbl().get_display_name)(item.this, SIGDN_FILESYSPATH, &mut name) < 0
                || name.is_null()
            {
                return None;
            }
            let len = (0..).take_while(|&i| *name.add(i) != 0).count();
            let path = OsString::from_wide(std::slice::from_raw_parts(name, len));
            CoTaskMemFree(name as *const c_void);
            Some(PathBuf::from(path))
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
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

//! The console of an app built as a Windows GUI program.
//!
//! An app whose binary uses `#![windows_subsystem = "windows"]` opens no
//! console window when started from Explorer or the Start menu, but it also
//! has no console and no standard handles when started from a terminal, so
//! its help, errors and log lines go nowhere. [`attach_parent_console`]
//! borrows the starting terminal's console for them. [`own_console`] opens a
//! console window of the app's own, for commands the user types into: the
//! shell that started a GUI program does not wait for it, so both would read
//! the parent's console.
//!
//! Both do nothing on other targets and in processes that had a console at
//! start, such as console-subsystem debug builds.

/// Attaches a process that started without a console to the console of its
/// parent, when the parent has one, and points the standard handles that
/// the parent did not hand over at it; redirections such as `> file` stay.
///
/// Call it first thing in `main`, before anything prints. Returns whether
/// the process now uses its parent's console.
pub fn attach_parent_console() -> bool {
    imp::attach_parent_console()
}

/// Gives a process that started without a console one of its own, in a new
/// window, with all standard handles pointing at it. Leaves the parent's
/// console first if [`attach_parent_console`] attached to it.
///
/// Returns whether the process now has a console window of its own.
pub fn own_console() -> bool {
    imp::own_console()
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        AllocConsole, AttachConsole, FreeConsole, GetConsoleWindow, GetStdHandle, SetStdHandle,
        ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };

    /// Whether a console was missing at the first call, which is the start
    /// of the process when the app follows the docs.
    static STARTED_WITHOUT: AtomicBool = AtomicBool::new(false);
    static CHECKED: AtomicBool = AtomicBool::new(false);

    pub(super) fn started_without() -> bool {
        if !CHECKED.swap(true, Ordering::Relaxed) {
            // SAFETY: no arguments; returns null when there is no console.
            let without = unsafe { GetConsoleWindow() }.is_null();
            STARTED_WITHOUT.store(without, Ordering::Relaxed);
        }
        STARTED_WITHOUT.load(Ordering::Relaxed)
    }

    pub(super) fn attach_parent_console() -> bool {
        if !started_without() {
            return false;
        }
        // SAFETY: fails harmlessly when the parent has no console.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
            return false;
        }
        fill_std_handles(false);
        true
    }

    pub(super) fn own_console() -> bool {
        if !started_without() {
            return false;
        }
        // SAFETY: FreeConsole fails harmlessly without a console, and
        // AllocConsole needs none attached.
        let allocated = unsafe {
            FreeConsole();
            AllocConsole() != 0
        };
        if allocated {
            fill_std_handles(true);
        }
        allocated
    }

    /// Points the standard handles at the attached console: all of them
    /// when `all`, else only those that are missing.
    fn fill_std_handles(all: bool) {
        for (std, name) in [
            (STD_INPUT_HANDLE, "CONIN$"),
            (STD_OUTPUT_HANDLE, "CONOUT$"),
            (STD_ERROR_HANDLE, "CONOUT$"),
        ] {
            if all || !valid(std) {
                open_into(std, name);
            }
        }
    }

    fn valid(std: STD_HANDLE) -> bool {
        // SAFETY: reads one of the process's standard handles.
        let handle = unsafe { GetStdHandle(std) };
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }

    fn open_into(std: STD_HANDLE, name: &str) {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated path that outlives the call;
        // the security attributes and template may be null.
        let handle: HANDLE = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return;
        }
        // SAFETY: the handle stays open for the rest of the process, as
        // standard handles do; the standard library reads it on every use.
        unsafe { SetStdHandle(std, handle) };
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    pub(super) fn attach_parent_console() -> bool {
        false
    }

    pub(super) fn own_console() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test processes have a console or redirected handles, never a GUI
    /// start, so both calls leave them alone.
    #[test]
    fn processes_with_a_console_keep_it() {
        #[cfg(not(target_os = "windows"))]
        {
            assert!(!attach_parent_console());
            assert!(!own_console());
        }
        #[cfg(target_os = "windows")]
        {
            // A test runner may start without a console (CI services do),
            // so only the console-subsystem case is checked.
            if !imp::started_without() {
                assert!(!attach_parent_console());
                assert!(!own_console());
            }
        }
    }
}

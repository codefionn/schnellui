use std::error::Error;
use std::fmt;

/// A lazily initialized system text clipboard.
///
/// Creating or default-constructing this value never contacts the window system.
/// The first read or write opens the clipboard and later operations reuse it.
#[derive(Default)]
pub struct SystemClipboard {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    inner: Option<arboard::Clipboard>,
}

impl fmt::Debug for SystemClipboard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SystemClipboard")
            .field("initialized", &self.is_initialized())
            .finish()
    }
}

impl SystemClipboard {
    /// Creates a clipboard handle without opening the native clipboard.
    pub const fn new() -> Self {
        Self {
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
            inner: None,
        }
    }

    /// Returns whether a native clipboard was successfully opened already.
    pub fn is_initialized(&self) -> bool {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            self.inner.is_some()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            false
        }
    }

    /// Reads plain text from the native clipboard.
    pub fn read_text(&mut self) -> Result<String, ClipboardError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            self.open()?
                .get_text()
                .map_err(|error| ClipboardError::Read(error.to_string()))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Err(ClipboardError::Unsupported)
        }
    }

    /// Replaces the native clipboard contents with plain text.
    pub fn write_text(&mut self, text: impl Into<String>) -> Result<(), ClipboardError> {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            self.open()?
                .set_text(text.into())
                .map_err(|error| ClipboardError::Write(error.to_string()))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            let _ = text.into();
            Err(ClipboardError::Unsupported)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    fn open(&mut self) -> Result<&mut arboard::Clipboard, ClipboardError> {
        if self.inner.is_none() {
            self.inner = Some(
                arboard::Clipboard::new()
                    .map_err(|error| ClipboardError::Unavailable(error.to_string()))?,
            );
        }
        Ok(self.inner.as_mut().expect("clipboard initialized above"))
    }
}

/// A recoverable system clipboard failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardError {
    Unsupported,
    Unavailable(String),
    Read(String),
    Write(String),
}

impl fmt::Display for ClipboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => formatter.write_str("system clipboard is unsupported"),
            Self::Unavailable(message) => {
                write!(formatter, "system clipboard unavailable: {message}")
            }
            Self::Read(message) => write!(formatter, "could not read system clipboard: {message}"),
            Self::Write(message) => {
                write!(formatter, "could not write system clipboard: {message}")
            }
        }
    }
}

impl Error for ClipboardError {}

#[cfg(test)]
mod tests {
    use super::SystemClipboard;

    #[test]
    fn clipboard_construction_is_lazy() {
        let clipboard = SystemClipboard::new();
        assert!(!clipboard.is_initialized());
        assert!(format!("{clipboard:?}").contains("initialized: false"));
    }
}

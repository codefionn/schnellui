#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod unsupported;
#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "linux")]
pub(super) use linux::{detect, watch};
#[cfg(target_os = "macos")]
pub(super) use macos::{detect, watch};
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub(super) use unsupported::{detect, watch};
#[cfg(target_os = "windows")]
pub(super) use windows::{detect, watch};

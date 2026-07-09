use std::fmt::Write as _;
#[cfg(not(target_arch = "wasm32"))]
use std::path::{Path, PathBuf};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicU64, Ordering};

use schnellui_template::{
    Align, ButtonAppearance, ButtonProps, CheckboxProps, ComponentKind, ComponentProps,
    ComponentRef, ContainerKind, ContainerProps, EdgeInsets, FlexChild, Justify, Length,
    ResponsiveQuery, ResponsiveTarget, Role, SliderProps, TemplateRenderer, TextAlign, TextContent,
    TextInputProps, TextProps, WrapMode,
};
use schnellui_widgets::Theme;
#[cfg(not(target_arch = "wasm32"))]
use serde::Serialize;

mod renderer;
mod scripts;
#[cfg(feature = "ssr")]
mod ssr;
mod template;
#[cfg(test)]
mod tests;

pub use renderer::*;
#[cfg(all(feature = "ssr", target_arch = "wasm32"))]
pub use ssr::navigate;
#[cfg(feature = "ssr")]
pub use ssr::{
    Authorization, CsrRoute, HtmlRouter, HydrationError, HydrationKey, RouteMatch, RouteResponse,
    SsrAuthorize, SsrChain, SsrRoute,
};

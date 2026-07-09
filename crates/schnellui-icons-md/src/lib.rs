//! Material Design icons from [`md-icons`] as cached schnellui views.
//!
//! ```
//! use schnellui_icons_md::{outlined, MdIcon};
//!
//! let home = MdIcon::outlined("home", outlined::ICON_HOME).size(24.0);
//! ```

use std::borrow::Cow;

use schnellui_icons::{Icon, IconId, IconSource};
use schnellui_scene::{Color, WidgetId};
use schnellui_widgets::{BuildCtx, View};

pub use md_icons::{outlined, rounded, sharp};

/// The three style families and their filled variants exported by `md-icons`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MaterialStyle {
    Outlined,
    OutlinedFilled,
    Rounded,
    RoundedFilled,
    Sharp,
    SharpFilled,
}

impl MaterialStyle {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Outlined => "outlined",
            Self::OutlinedFilled => "outlined_filled",
            Self::Rounded => "rounded",
            Self::RoundedFilled => "rounded_filled",
            Self::Sharp => "sharp",
            Self::SharpFilled => "sharp_filled",
        }
    }
}

/// The lightweight `md-icons` implementation of [`IconSource`].
#[derive(Clone, Copy, Debug)]
pub struct MaterialIcon {
    name: &'static str,
    style: MaterialStyle,
    svg: &'static str,
}

impl MaterialIcon {
    pub const fn new(name: &'static str, style: MaterialStyle, svg: &'static str) -> Self {
        Self { name, style, svg }
    }

    pub const fn name(&self) -> &'static str {
        self.name
    }

    pub const fn style(&self) -> MaterialStyle {
        self.style
    }
}

impl IconSource for MaterialIcon {
    fn id(&self) -> IconId {
        IconId::new("md-icons/0.4.0", self.name, self.style.as_str())
    }

    fn svg(&self) -> &'static str {
        self.svg
    }
}

/// Ergonomic Material Design icon widget.
pub struct MdIcon {
    inner: Icon<MaterialIcon>,
}

impl MdIcon {
    pub fn new(name: &'static str, style: MaterialStyle, svg: &'static str) -> Self {
        Self {
            inner: Icon::new(MaterialIcon::new(name, style, svg)),
        }
    }

    pub fn outlined(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::Outlined, svg)
    }

    pub fn outlined_filled(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::OutlinedFilled, svg)
    }

    pub fn rounded(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::Rounded, svg)
    }

    pub fn rounded_filled(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::RoundedFilled, svg)
    }

    pub fn sharp(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::Sharp, svg)
    }

    pub fn sharp_filled(name: &'static str, svg: &'static str) -> Self {
        Self::new(name, MaterialStyle::SharpFilled, svg)
    }

    pub fn size(mut self, size: f32) -> Self {
        self.inner = self.inner.size(size);
        self
    }

    pub fn size_xy(mut self, width: f32, height: f32) -> Self {
        self.inner = self.inner.size_xy(width, height);
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.inner = self.inner.color(color);
        self
    }

    pub fn alt(mut self, alt: impl Into<Cow<'static, str>>) -> Self {
        self.inner = self.inner.alt(alt);
        self
    }

    pub fn source(&self) -> &MaterialIcon {
        self.inner.source()
    }
}

impl View for MdIcon {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        Box::new(self.inner).build(ctx, parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_constructors_produce_stable_distinct_ids() {
        let outlined = MdIcon::outlined("home", outlined::ICON_HOME);
        let outlined_filled = MdIcon::outlined_filled("home", outlined::filled::ICON_HOME);
        assert_eq!(outlined.source().id().name, "home");
        assert_eq!(outlined.source().id().variant, "outlined");
        assert_eq!(outlined_filled.source().id().variant, "outlined_filled");
        assert_ne!(outlined.source().id(), outlined_filled.source().id());
    }
}

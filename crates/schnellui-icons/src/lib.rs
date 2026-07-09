//! Library-neutral icons for schnellui.
//!
//! An icon pack implements [`IconSource`] and supplies a stable [`IconId`] plus
//! static SVG markup. [`Icon`] turns that source into a normal schnellui
//! [`View`](schnellui_widgets::View). Parsed SVG documents and size-specific CPU
//! rasters are cached process-wide; equal instances in one scene also share one
//! image-atlas allocation and therefore one resident GPU resource.

use std::borrow::Cow;

use schnellui_scene::{Color, WidgetId};
use schnellui_widgets::{BuildCtx, Svg, SvgCacheKey, View};

/// Stable identity of one icon in an icon library.
///
/// A library/version must never reuse the same `(library, name, variant)` for
/// different SVG content during one process. Keeping the variant separate makes
/// style families such as filled, outlined, sharp, and two-tone explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IconId {
    pub library: &'static str,
    pub name: &'static str,
    pub variant: &'static str,
}

impl IconId {
    pub const fn new(library: &'static str, name: &'static str, variant: &'static str) -> Self {
        Self {
            library,
            name,
            variant,
        }
    }
}

/// Adapter seam implemented by an icon-library crate.
pub trait IconSource: 'static {
    /// Stable cache identity for this asset.
    fn id(&self) -> IconId;

    /// Complete SVG markup for the asset.
    fn svg(&self) -> &'static str;
}

/// A library-neutral vector icon widget.
///
/// The icon is decorative by default. Call [`Icon::alt`] when it conveys meaning.
/// The source is cached as an alpha mask, allowing monochrome and two-tone packs
/// to be recolored without generating another raster or GPU atlas entry.
pub struct Icon<S: IconSource> {
    source: S,
    display: Option<(f32, f32)>,
    tint: Color,
    alt: Option<Cow<'static, str>>,
}

impl<S: IconSource> Icon<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            display: None,
            tint: Color::BLACK,
            alt: None,
        }
    }

    /// Sets a square logical size.
    pub fn size(mut self, size: f32) -> Self {
        self.display = Some((size, size));
        self
    }

    /// Sets independent logical width and height.
    pub fn size_xy(mut self, width: f32, height: f32) -> Self {
        self.display = Some((width, height));
        self
    }

    /// Colors the cached source mask at draw time.
    ///
    /// Tint is deliberately absent from the cache key: differently colored
    /// instances share the same CPU raster and GPU allocation.
    pub fn color(mut self, color: Color) -> Self {
        self.tint = color;
        self
    }

    /// Gives a meaningful icon an accessible name.
    pub fn alt(mut self, alt: impl Into<Cow<'static, str>>) -> Self {
        self.alt = Some(alt.into());
        self
    }

    pub fn source(&self) -> &S {
        &self.source
    }
}

impl<S: IconSource> View for Icon<S> {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = this.source.id();
        let mut svg = Svg::new(this.source.svg())
            .cache(SvgCacheKey::new(id.library, id.name, id.variant))
            .mask()
            .tint(this.tint);
        if let Some((width, height)) = this.display {
            svg = svg.size(width, height);
        }
        if let Some(alt) = this.alt {
            svg = svg.alt(alt);
        }
        Box::new(svg).build(ctx, parent)
    }
}

/// Returns the current process-wide parsed-document and raster cache counts.
pub use schnellui_widgets::{svg_cache_stats as cache_stats, SvgCacheStats as CacheStats};

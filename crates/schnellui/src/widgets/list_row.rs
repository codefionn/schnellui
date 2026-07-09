//! [`ListRow`]: a left-aligned, full-width clickable row for navigation rails
//! and command lists. It paints a background, an optional leading accent bar
//! and dot, a title, an optional right-aligned trailing text (plain or on a
//! pill) and an optional muted second line, all as one retained node (one
//! `Role::Button`).
//!
//! The row is chrome, not a button: the label is not centered and the resting
//! background is transparent. Hover, focus and press feedback come from the
//! theme's button interaction states like any other button.

use std::borrow::Cow;

use schnellui::a11y::{ActionFlags, Role, StateFlags};
use schnellui::scene::{Color, Point, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::text::ShapedText;

use crate::widgets::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers, BuildCtx,
    ClickHandler, View,
};

const DEFAULT_HEIGHT: f32 = 28.0;
const DEFAULT_PAD_H: f32 = 12.0;
const BAR_WIDTH: f32 = 3.0;
const DOT_SIZE: f32 = 8.0;
const DOT_GAP: f32 = 8.0;
/// Space kept between the title and the trailing text.
const TRAILING_GAP: f32 = 8.0;
const SUBTITLE_DROP: f32 = 4.0;
/// Horizontal padding inside a trailing pill.
const PILL_PAD_H: f32 = 6.0;
/// Vertical padding inside a trailing pill.
const PILL_PAD_V: f32 = 2.0;

/// A left-aligned clickable row. See the module docs.
pub struct ListRow {
    title: Cow<'static, str>,
    trailing: Option<Cow<'static, str>>,
    subtitle: Option<Cow<'static, str>>,
    accent: Option<Color>,
    dot: Option<Color>,
    selected: bool,
    selected_background: Option<Color>,
    width: f32,
    height: f32,
    pad_h: f32,
    title_size: f32,
    meta_size: f32,
    trailing_inline: bool,
    pill: Option<Color>,
    subtitle_color: Option<Color>,
    value: Option<Cow<'static, str>>,
    on_click: Option<ClickHandler>,
}

impl ListRow {
    /// A row whose accessible name is `title`.
    pub fn new(title: impl Into<Cow<'static, str>>) -> ListRow {
        ListRow {
            title: title.into(),
            trailing: None,
            subtitle: None,
            accent: None,
            dot: None,
            selected: false,
            selected_background: None,
            width: 240.0,
            height: DEFAULT_HEIGHT,
            pad_h: DEFAULT_PAD_H,
            title_size: 13.0,
            meta_size: 11.5,
            trailing_inline: false,
            pill: None,
            subtitle_color: None,
            value: None,
            on_click: None,
        }
    }

    /// Outer width in logical pixels.
    pub fn width(mut self, width: f32) -> ListRow {
        self.width = width;
        self
    }

    /// Height of the title line. A subtitle adds its own line below.
    pub fn height(mut self, height: f32) -> ListRow {
        self.height = height;
        self
    }

    /// Horizontal padding on both sides.
    pub fn padding(mut self, pad_h: f32) -> ListRow {
        self.pad_h = pad_h;
        self
    }

    /// Title font size; the trailing text and subtitle use [`Self::meta_size`].
    pub fn title_size(mut self, size: f32) -> ListRow {
        self.title_size = size;
        self
    }

    /// Font size of the trailing text and the subtitle.
    pub fn meta_size(mut self, size: f32) -> ListRow {
        self.meta_size = size;
        self
    }

    /// Right-aligned text in the theme's muted color.
    pub fn trailing(mut self, text: impl Into<Cow<'static, str>>) -> ListRow {
        let text = text.into();
        self.trailing = (!text.is_empty()).then_some(text);
        self
    }

    /// Puts the trailing text right after the title instead of at the right
    /// edge.
    pub fn trailing_after_title(mut self) -> ListRow {
        self.trailing_inline = true;
        self
    }

    /// Draws the trailing text on a rounded pill of this color, in the
    /// theme's `on_accent` color, like a badge inside the row.
    pub fn trailing_pill(mut self, color: Color) -> ListRow {
        self.pill = Some(color);
        self
    }

    /// Paints the subtitle in this color instead of the muted one.
    pub fn subtitle_color(mut self, color: Color) -> ListRow {
        self.subtitle_color = Some(color);
        self
    }

    /// The accessible value, for when the trailing text is too short to
    /// say what it means. Defaults to the trailing text.
    pub fn value(mut self, value: impl Into<Cow<'static, str>>) -> ListRow {
        self.value = Some(value.into());
        self
    }

    /// A second line in the theme's muted color.
    pub fn subtitle(mut self, text: impl Into<Cow<'static, str>>) -> ListRow {
        let text = text.into();
        self.subtitle = (!text.is_empty()).then_some(text);
        self
    }

    /// A bar of this color along the left edge, full row height.
    pub fn accent_bar(mut self, color: Color) -> ListRow {
        self.accent = Some(color);
        self
    }

    /// A filled circle of this color before the title.
    pub fn dot(mut self, color: Color) -> ListRow {
        self.dot = Some(color);
        self
    }

    /// Marks the row selected: it gets the selected background and the
    /// `SELECTED` state.
    pub fn selected(mut self, selected: bool) -> ListRow {
        self.selected = selected;
        self
    }

    /// Background of a selected row. Defaults to the theme's `selection`.
    pub fn selected_background(mut self, color: Color) -> ListRow {
        self.selected_background = Some(color);
        self
    }

    /// Click handler, shared with the accessibility `Click` action.
    pub fn on_click(mut self, f: impl FnMut() + 'static) -> ListRow {
        self.on_click = Some(Box::new(f));
        self
    }

    pub fn role(&self) -> Role {
        Role::Button
    }

    pub fn kind(&self) -> WidgetKind {
        WidgetKind::Button
    }

    fn total_height(&self, subtitle_height: f32) -> f32 {
        if self.subtitle.is_some() {
            self.height + subtitle_height + SUBTITLE_DROP * 0.5
        } else {
            self.height
        }
    }
}

/// Shapes `text` at `phys` pixels, cutting it with an ellipsis until it is at
/// most `max_width` logical pixels wide.
fn shape_fit(
    shaper: &mut schnellui::text::TextShaper,
    text: &str,
    phys: f32,
    max_width: f32,
    scale: f32,
) -> ShapedText {
    let inv = 1.0 / norm_scale(scale);
    let mut shaped = shaper.shape(text, phys, None);
    if shaped.width * inv <= max_width {
        return shaped;
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let mut cut: String = chars.iter().collect();
        cut.truncate(cut.trim_end().len());
        cut.push('…');
        shaped = shaper.shape(&cut, phys, None);
        if shaped.width * inv <= max_width {
            break;
        }
    }
    shaped
}

impl View for ListRow {
    fn build(self: Box<Self>, ctx: &mut BuildCtx, parent: Option<WidgetId>) -> WidgetId {
        let this = *self;
        let id = ctx.scene.insert(WidgetKind::Button, parent);
        {
            let a = ctx.scene.a11y_mut(id);
            a.role = Role::Button.as_u16();
            a.name = Some(this.title.to_string());
            a.value = this
                .value
                .as_ref()
                .or(this.trailing.as_ref())
                .map(|text| text.to_string());
            let mut actions = ActionFlags::default();
            actions.insert(ActionFlags::CLICK);
            actions.insert(ActionFlags::FOCUS);
            a.actions = actions.0;
            if this.selected {
                let mut state = StateFlags::default();
                state.insert(StateFlags::SELECTED);
                a.state = state.0;
            }
        }
        let theme = theme_for(&ctx.runtime, id);
        let inv = 1.0 / norm_scale(ctx.scale);
        let title_phys = phys_size_px(this.title_size, ctx.scale);
        let meta_phys = phys_size_px(this.meta_size, ctx.scale);

        let lead = this.pad_h
            + if this.dot.is_some() {
                DOT_SIZE + DOT_GAP
            } else {
                0.0
            };
        let trailing = this
            .trailing
            .as_ref()
            .map(|text| shape_fit(ctx.text, text, meta_phys, this.width * 0.5, ctx.scale));
        let pill_pad = if this.pill.is_some() { PILL_PAD_H } else { 0.0 };
        let trailing_width = trailing
            .as_ref()
            .map_or(0.0, |t| t.width * inv + 2.0 * pill_pad);
        let title_room = this.width
            - lead
            - this.pad_h
            - if trailing.is_some() {
                trailing_width + TRAILING_GAP
            } else {
                0.0
            };
        let title = shape_fit(
            ctx.text,
            &this.title,
            title_phys,
            title_room.max(8.0),
            ctx.scale,
        );
        let subtitle = this.subtitle.as_ref().map(|text| {
            shape_fit(
                ctx.text,
                text,
                meta_phys,
                (this.width - lead - this.pad_h).max(8.0),
                ctx.scale,
            )
        });
        let subtitle_height = subtitle.as_ref().map_or(0.0, |s| s.height * inv);
        let size = Size {
            width: this.width,
            height: this.total_height(subtitle_height),
        };

        let rect = node_rect(ctx.scene, id, size);
        let background = if this.selected {
            this.selected_background.unwrap_or(theme.selection)
        } else {
            Color::TRANSPARENT
        };
        let pd = ctx.scene.paint_mut(id);
        pd.primitives.clear();
        // [0] background, always present so the row has stable bounds.
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: background,
            corner_radius: 0.0,
        });
        if let Some(color) = this.accent {
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(rect.x, rect.y, BAR_WIDTH, rect.height),
                color,
                corner_radius: 0.0,
            });
        }
        if let Some(color) = this.dot {
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(
                    rect.x + this.pad_h,
                    rect.y + (this.height - DOT_SIZE) * 0.5,
                    DOT_SIZE,
                    DOT_SIZE,
                ),
                color,
                corner_radius: DOT_SIZE * 0.5,
            });
        }
        let title_y = rect.y + (this.height - title.height * inv) * 0.5;
        rasterize_and_push(
            pd,
            ctx.text,
            ctx.atlas,
            &title,
            title_phys as u32,
            theme.text,
            ctx.scale,
            Point {
                x: rect.x + lead,
                y: title_y,
            },
        );
        if let Some(trailing) = &trailing {
            let x = if this.trailing_inline {
                rect.x + lead + title.width * inv + TRAILING_GAP
            } else {
                rect.x + rect.width - this.pad_h - trailing_width
            };
            let text_height = trailing.height * inv;
            let y = rect.y + (this.height - text_height) * 0.5;
            let color = match this.pill {
                Some(pill) => {
                    let height = text_height + 2.0 * PILL_PAD_V;
                    pd.primitives.push(Primitive::SolidRect {
                        rect: Rect::new(x, y - PILL_PAD_V, trailing_width, height),
                        color: pill,
                        corner_radius: height * 0.5,
                    });
                    theme.on_accent
                }
                None => theme.text_muted,
            };
            rasterize_and_push(
                pd,
                ctx.text,
                ctx.atlas,
                trailing,
                meta_phys as u32,
                color,
                ctx.scale,
                Point { x: x + pill_pad, y },
            );
        }
        if let Some(subtitle) = &subtitle {
            rasterize_and_push(
                pd,
                ctx.text,
                ctx.atlas,
                subtitle,
                meta_phys as u32,
                this.subtitle_color.unwrap_or(theme.text_muted),
                ctx.scale,
                Point {
                    x: rect.x + lead,
                    y: rect.y + this.height - SUBTITLE_DROP - 2.0,
                },
            );
        }
        ctx.layout.set_measure(id, Box::new(move |_available| size));
        if let Some(handler) = this.on_click {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(handler));
        }
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui::layout::LayoutEngine;
    use schnellui::text::{GlyphAtlas, TextShaper};

    fn build(view: ListRow) -> (schnellui::scene::Scene, WidgetId) {
        let runtime = crate::widgets::Runtime::new();
        crate::widgets::reset(&runtime);
        let mut scene = schnellui::scene::Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(512, 512);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::widgets::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            Box::new(view).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, id)
    }

    fn rects(scene: &schnellui::scene::Scene, id: WidgetId) -> Vec<(Rect, Color, f32)> {
        scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match *p {
                Primitive::SolidRect {
                    rect,
                    color,
                    corner_radius,
                } => Some((rect, color, corner_radius)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn row_is_a_named_button_with_click_and_focus() {
        let (scene, id) = build(ListRow::new("main").trailing("~3").on_click(|| {}));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::Button);
        assert_eq!(a.name.as_deref(), Some("main"));
        assert_eq!(a.value.as_deref(), Some("~3"));
        let actions = ActionFlags(a.actions);
        assert!(actions.contains(ActionFlags::CLICK));
        assert!(actions.contains(ActionFlags::FOCUS));
        assert_eq!(ListRow::new("x").role(), Role::Button);
    }

    #[test]
    fn selected_rows_get_the_background_and_state() {
        let tint = Color::rgba(1, 2, 3, 40);
        let (scene, id) = build(ListRow::new("a").selected(true).selected_background(tint));
        assert_eq!(rects(&scene, id)[0].1, tint);
        assert!(StateFlags(scene.a11y(id).unwrap().state).contains(StateFlags::SELECTED));
        let (scene, id) = build(ListRow::new("a"));
        assert_eq!(rects(&scene, id)[0].1, Color::TRANSPARENT);
    }

    #[test]
    fn bar_and_dot_use_their_colors_and_the_dot_is_round() {
        let bar = Color::rgb(9, 8, 7);
        let dot = Color::rgb(1, 2, 3);
        let (scene, id) = build(ListRow::new("a").width(200.0).accent_bar(bar).dot(dot));
        let rects = rects(&scene, id);
        assert_eq!(rects[1].1, bar);
        assert_eq!(rects[1].0.width, 3.0);
        assert_eq!(rects[1].0.height, 28.0);
        assert_eq!(rects[2].1, dot);
        assert_eq!(rects[2].0.width, 8.0);
        assert_eq!(rects[2].2, 4.0);
    }

    #[test]
    fn a_trailing_pill_holds_its_text_and_ends_at_the_right_padding() {
        let pill = Color::rgb(200, 150, 50);
        let (scene, id) = build(
            ListRow::new("main")
                .trailing("input")
                .trailing_pill(pill)
                .width(200.0),
        );
        let (rect, _, radius) = *rects(&scene, id)
            .iter()
            .find(|(_, color, _)| *color == pill)
            .expect("the pill is painted");
        assert_eq!(rect.x + rect.width, 200.0 - 12.0);
        assert_eq!(radius, rect.height * 0.5);
        let glyphs: Vec<Rect> = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::GlyphQuad { rect, .. } if rect.x > 100.0 => Some(*rect),
                _ => None,
            })
            .collect();
        assert!(!glyphs.is_empty());
        for glyph in glyphs {
            assert!(
                glyph.x >= rect.x + PILL_PAD_H - 1.0,
                "{glyph:?} in {rect:?}"
            );
            assert!(glyph.x + glyph.width <= rect.x + rect.width - PILL_PAD_H + 1.0);
        }
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("input"));
        let (scene, id) = build(ListRow::new("main").trailing("input").value("Waiting"));
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("Waiting"));
    }

    #[test]
    fn trailing_text_ends_at_the_right_padding() {
        let (scene, id) = build(ListRow::new("branch").trailing("↑1").width(200.0));
        let right = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::GlyphQuad { rect, .. } => Some(rect.x + rect.width),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(right <= 200.0 - 12.0 + 1.0, "{right}");
        assert!(right > 200.0 - 12.0 - 12.0, "{right}");
    }

    #[test]
    fn long_titles_stay_inside_the_row() {
        let (scene, id) = build(ListRow::new("x".repeat(200)).width(120.0));
        let right = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::GlyphQuad { rect, .. } => Some(rect.x + rect.width),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(right <= 120.0 - 12.0 + 1.0, "{right}");
    }

    #[test]
    fn a_subtitle_makes_the_row_taller() {
        let (plain, a) = build(ListRow::new("a"));
        let (two, b) = build(ListRow::new("a").subtitle("~/src/a"));
        let height = |scene: &schnellui::scene::Scene, id| rects(scene, id)[0].0.height;
        assert!(height(&two, b) > height(&plain, a));
    }
}

#[cfg(test)]
mod inline_tests {
    use super::*;

    #[test]
    fn inline_trailing_text_follows_the_title() {
        let runtime = crate::widgets::Runtime::new();
        crate::widgets::reset(&runtime);
        let mut scene = schnellui::scene::Scene::new();
        let mut layout = schnellui::layout::LayoutEngine::new();
        let mut text = schnellui::text::TextShaper::new();
        let mut atlas = schnellui::text::GlyphAtlas::new(512, 512);
        let mut ctx = BuildCtx {
            context: crate::widgets::Context::new(),
            runtime: runtime.clone(),
            scene: &mut scene,
            layout: &mut layout,
            text: &mut text,
            atlas: &mut atlas,
            scale: 1.0,
        };
        let id = Box::new(
            ListRow::new("app")
                .trailing("local")
                .trailing_after_title()
                .width(260.0),
        )
        .build(&mut ctx, None);
        let right = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::GlyphQuad { rect, .. } => Some(rect.x + rect.width),
                _ => None,
            })
            .fold(0.0f32, f32::max);
        assert!(right < 120.0, "{right}");
    }
}

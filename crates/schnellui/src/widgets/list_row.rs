//! [`ListRow`]: a left-aligned, full-width clickable row for navigation rails
//! and command lists. It paints a background, an optional leading accent bar
//! and dot, a title, an optional right-aligned trailing text (plain or on a
//! pill) and an optional muted second line, all as one retained node (one
//! `Role::Button`). An optional [`ContextMenu`] opens on a secondary click or
//! the accessibility `ShowContextMenu` action.
//!
//! The row is chrome, not a button: the label is not centered and the resting
//! background is transparent. Hover, focus and press feedback come from the
//! theme's button interaction states like any other button.

use std::borrow::Cow;

use schnellui::a11y::{ActionFlags, Role, StateFlags};
use schnellui::scene::{Color, Point, Primitive, Rect, Size, WidgetId, WidgetKind};
use schnellui::text::{FontFace, ShapeOptions, ShapedText};

use crate::widgets::{
    node_rect, norm_scale, phys_size_px, rasterize_and_push, theme_for, with_handlers, BuildCtx,
    ClickHandler, ContextMenu, ContextMenuItem, View,
};

const DEFAULT_HEIGHT: f32 = 28.0;
const DEFAULT_PAD_H: f32 = 12.0;
const BAR_WIDTH: f32 = 3.0;
const DOT_SIZE: f32 = 8.0;
const DOT_GAP: f32 = 8.0;
/// The status dot on a leading mark's rim, and the ring of ground round it.
const MARK_DOT: f32 = 10.0;
const MARK_DOT_RING: f32 = 2.0;
/// The share of a dot's width a hollow dot keeps as its ring.
const HOLLOW_RING: f32 = 0.3;
/// The share of a dot's width the centre of a ringed dot covers.
const RINGED_HOLE: f32 = 0.55;
const RINGED_CORE: f32 = 0.25;

/// The shape of a row's status dot, so its meaning does not rest on its color alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DotShape {
    /// A full disc.
    #[default]
    Filled,
    /// A ring with the row's ground inside.
    Hollow,
    /// A disc with a ring of ground round a small core.
    Ringed,
}

/// The discs that draw a dot of `shape` and `size` in `color` on `ground`, largest first.
fn dot_discs(shape: DotShape, size: f32, color: Color, ground: Color) -> Vec<(f32, Color)> {
    match shape {
        DotShape::Filled => vec![(size, color)],
        DotShape::Hollow => vec![(size, color), (size * (1.0 - 2.0 * HOLLOW_RING), ground)],
        DotShape::Ringed => vec![
            (size, color),
            (size * RINGED_HOLE, ground),
            (size * RINGED_CORE, color),
        ],
    }
}
/// Space kept between the title and the trailing text.
const TRAILING_GAP: f32 = 8.0;
const SUBTITLE_DROP: f32 = 4.0;
/// Horizontal padding inside a trailing pill.
const PILL_PAD_H: f32 = 6.0;
/// Vertical padding inside a trailing pill.
const PILL_PAD_V: f32 = 2.0;
/// How far a selected row's muted texts move toward the text color, so
/// they keep their contrast on the selected background.
const SELECTED_META_LIFT: f32 = 0.35;

/// A round badge before the title: a letter or two on a filled disc.
struct LeadingMark {
    text: Cow<'static, str>,
    fill: Color,
    ink: Color,
    size: f32,
    /// Size of the letters, in the bold face. `None`: the meta size, regular.
    text_size: Option<f32>,
    /// RGBA pixels drawn over the disc instead of the letters: width, height, pixels.
    picture: Option<(u32, u32, Vec<u8>)>,
}

/// A left-aligned clickable row. See the module docs.
pub struct ListRow {
    title: Cow<'static, str>,
    trailing: Option<Cow<'static, str>>,
    meta: Option<Cow<'static, str>>,
    subtitle: Option<Cow<'static, str>>,
    accent: Option<Color>,
    dot: Option<Color>,
    dot_shape: DotShape,
    mark: Option<LeadingMark>,
    selected: bool,
    selected_background: Option<Color>,
    selected_radius: f32,
    width: f32,
    height: f32,
    pad_h: f32,
    end_room: f32,
    title_size: f32,
    title_bold: bool,
    title_wrap: bool,
    subtitle_wrap: bool,
    meta_size: f32,
    trailing_inline: bool,
    trailing_centered: bool,
    pill: Option<Color>,
    pill_ink: Option<Color>,
    subtitle_color: Option<Color>,
    value: Option<Cow<'static, str>>,
    tooltip: Option<Cow<'static, str>>,
    on_click: Option<ClickHandler>,
    context_menu: Option<ContextMenu>,
}

impl ListRow {
    /// A row whose accessible name is `title`.
    pub fn new(title: impl Into<Cow<'static, str>>) -> ListRow {
        ListRow {
            title: title.into(),
            trailing: None,
            meta: None,
            subtitle: None,
            accent: None,
            dot: None,
            dot_shape: DotShape::Filled,
            mark: None,
            selected: false,
            selected_background: None,
            selected_radius: 0.0,
            width: 240.0,
            height: DEFAULT_HEIGHT,
            pad_h: DEFAULT_PAD_H,
            end_room: 0.0,
            title_size: 13.0,
            title_bold: false,
            title_wrap: false,
            subtitle_wrap: false,
            meta_size: 11.5,
            trailing_inline: false,
            trailing_centered: false,
            pill: None,
            pill_ink: None,
            subtitle_color: None,
            value: None,
            tooltip: None,
            on_click: None,
            context_menu: None,
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

    /// Keeps `room` logical pixels free at the right edge, inside the row's
    /// background, for a control the caller places over it. The trailing
    /// text ends before it and the title and subtitle stop short of it.
    pub fn end_room(mut self, room: f32) -> ListRow {
        self.end_room = if room.is_finite() { room.max(0.0) } else { 0.0 };
        self
    }

    /// Title font size; the trailing text and subtitle use [`Self::meta_size`].
    pub fn title_size(mut self, size: f32) -> ListRow {
        self.title_size = size;
        self
    }

    /// Font size of the trailing text and the subtitle.
    /// Lets a title too long for its line go on to a second one, at a word break, before
    /// it is cut with an ellipsis. The row grows by that line; a title that fits keeps the
    /// row's height.
    pub fn title_wrap(mut self, wrap: bool) -> ListRow {
        self.title_wrap = wrap;
        self
    }

    /// Lets a subtitle too long for its line go on to a second one, at a word break, as
    /// [`Self::title_wrap`] does for the title.
    pub fn subtitle_wrap(mut self, wrap: bool) -> ListRow {
        self.subtitle_wrap = wrap;
        self
    }

    /// Draws the title in the bold face, for a row that has something new.
    pub fn title_bold(mut self, bold: bool) -> ListRow {
        self.title_bold = bold;
        self
    }

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

    /// Muted text at the right end of the title line, such as a time. With
    /// it and a subtitle the trailing text moves down to the subtitle's
    /// line, so a row can show a time above and a count below.
    pub fn meta(mut self, text: impl Into<Cow<'static, str>>) -> ListRow {
        let text = text.into();
        self.meta = (!text.is_empty()).then_some(text);
        self
    }

    /// Puts the trailing text right after the title instead of at the right
    /// edge.
    pub fn trailing_after_title(mut self) -> ListRow {
        self.trailing_inline = true;
        self
    }

    /// Centers the trailing text on the whole row, subtitle included,
    /// instead of on the title line: for a mark such as a chevron that
    /// belongs to the row rather than to its title.
    pub fn trailing_centered(mut self) -> ListRow {
        self.trailing_centered = true;
        self
    }

    /// Draws the trailing text on a rounded pill of this color, in the
    /// theme's `on_accent` color, like a badge inside the row.
    pub fn trailing_pill(mut self, color: Color) -> ListRow {
        self.pill = Some(color);
        self
    }

    /// Like [`ListRow::trailing_pill`], with the text in `ink` and in the bold
    /// face: for a pill whose fill is not the accent, where `on_accent` may not
    /// read, and for a count that must be legible at a small size.
    pub fn trailing_pill_ink(mut self, color: Color, ink: Color) -> ListRow {
        self.pill = Some(color);
        self.pill_ink = Some(ink);
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

    /// The shape of the dot: filled, hollow or ringed.
    pub fn dot_shape(mut self, shape: DotShape) -> ListRow {
        self.dot_shape = shape;
        self
    }

    /// Words shown on hover above the row, such as what its dot means.
    pub fn tooltip(mut self, label: impl Into<Cow<'static, str>>) -> ListRow {
        let label = label.into();
        self.tooltip = (!label.is_empty()).then_some(label);
        self
    }

    /// Marks the row selected: it gets the selected background and the
    /// `SELECTED` state.
    /// Draws a round badge `size` across before the title, with `text` (an
    /// initial or two) in `ink` on a disc of `fill`: an avatar stand-in. The
    /// title starts after it.
    pub fn leading_mark(
        mut self,
        text: impl Into<Cow<'static, str>>,
        fill: Color,
        ink: Color,
        size: f32,
    ) -> ListRow {
        self.mark = Some(LeadingMark {
            text: text.into(),
            fill,
            ink,
            size: if size.is_finite() { size.max(0.0) } else { 0.0 },
            text_size: None,
            picture: None,
        });
        self
    }

    /// Draws the letters of the leading mark `size` logical px high in the
    /// bold face, instead of at the meta size: for initials that must read
    /// at a glance. Without a leading mark it does nothing.
    pub fn leading_mark_text(mut self, size: f32) -> ListRow {
        if let Some(mark) = &mut self.mark {
            mark.text_size = (size.is_finite() && size > 0.0).then_some(size);
        }
        self
    }

    /// Draws `width` x `height` RGBA `pixels` in the leading mark, scaled to
    /// its size, instead of the letters: a person's or a room's picture. The
    /// disc still paints under it, so transparent corners show the fill.
    /// Without a leading mark, or with pixels that do not match the size, it
    /// does nothing.
    pub fn leading_mark_picture(
        mut self,
        width: u32,
        height: u32,
        pixels: impl Into<Vec<u8>>,
    ) -> ListRow {
        let pixels = pixels.into();
        let fits = width > 0
            && height > 0
            && (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(4))
                == Some(pixels.len());
        if let Some(mark) = self.mark.as_mut().filter(|_| fits) {
            mark.picture = Some((width, height, pixels));
        }
        self
    }

    pub fn selected(mut self, selected: bool) -> ListRow {
        self.selected = selected;
        self
    }

    /// Background of a selected row. Defaults to the theme's `selection`.
    pub fn selected_background(mut self, color: Color) -> ListRow {
        self.selected_background = Some(color);
        self
    }

    /// Rounds the corners of the selected row's background. `0` by default: a
    /// square fill that runs edge to edge.
    pub fn selected_radius(mut self, radius: f32) -> ListRow {
        self.selected_radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        self
    }

    /// Click handler, shared with the accessibility `Click` action.
    pub fn on_click(mut self, f: impl FnMut() + 'static) -> ListRow {
        self.on_click = Some(Box::new(f));
        self
    }

    /// Attaches a context menu, opened by a secondary click or the
    /// accessibility `ShowContextMenu` action.
    pub fn context_menu(mut self, menu: ContextMenu) -> ListRow {
        self.context_menu = Some(menu);
        self
    }

    /// Appends one command to this row's context menu.
    pub fn context_menu_item(mut self, item: ContextMenuItem) -> ListRow {
        self.context_menu
            .get_or_insert_with(ContextMenu::new)
            .push(item);
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
    shape_fit_face(shaper, text, phys, max_width, scale, FontFace::Sans)
}

/// [`shape_fit`] in `face`.
fn shape_fit_face(
    shaper: &mut schnellui::text::TextShaper,
    text: &str,
    phys: f32,
    max_width: f32,
    scale: f32,
    face: FontFace,
) -> ShapedText {
    let inv = 1.0 / norm_scale(scale);
    let opts = ShapeOptions {
        face,
        ..ShapeOptions::new(phys)
    };
    let mut shaped = shaper.shape_with(text, &opts);
    if shaped.width * inv <= max_width {
        return shaped;
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let mut cut: String = chars.iter().collect();
        cut.truncate(cut.trim_end().len());
        cut.push('…');
        shaped = shaper.shape_with(&cut, &opts);
        if shaped.width * inv <= max_width {
            break;
        }
    }
    shaped
}

/// Splits `text` at the last word break whose first part fits in `max_width`, for a title
/// on two lines. A text that fits, or has no break that helps, stays whole on the first.
fn split_title(
    shaper: &mut schnellui::text::TextShaper,
    text: &str,
    phys: f32,
    max_width: f32,
    scale: f32,
    face: FontFace,
) -> (String, Option<String>) {
    let inv = 1.0 / norm_scale(scale);
    let opts = ShapeOptions {
        face,
        ..ShapeOptions::new(phys)
    };
    if shaper.shape_with(text, &opts).width * inv <= max_width {
        return (text.to_owned(), None);
    }
    let breaks: Vec<usize> = text.match_indices(' ').map(|(at, _)| at).collect();
    for &at in breaks.iter().rev() {
        let head = text[..at].trim_end();
        if !head.is_empty() && shaper.shape_with(head, &opts).width * inv <= max_width {
            let tail = text[at..].trim_start();
            return (head.to_owned(), (!tail.is_empty()).then(|| tail.to_owned()));
        }
    }
    (text.to_owned(), None)
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
            if this
                .context_menu
                .as_ref()
                .is_some_and(|menu| !menu.is_empty())
            {
                actions.insert(ActionFlags::SHOW_CONTEXT_MENU);
            }
            a.actions = actions.0;
            if this.selected {
                let mut state = StateFlags::default();
                state.insert(StateFlags::SELECTED);
                a.state = state.0;
            }
        }
        with_handlers(&ctx.runtime, id, |h| h.flush_row = true);
        let theme = theme_for(&ctx.runtime, id);
        let inv = 1.0 / norm_scale(ctx.scale);
        let title_phys = phys_size_px(this.title_size, ctx.scale);
        let meta_phys = phys_size_px(this.meta_size, ctx.scale);

        // With a leading mark the dot sits on the mark's rim, as a status dot
        // on an avatar does, and takes no room of its own.
        let lead = this.pad_h
            + if this.dot.is_some() && this.mark.is_none() {
                DOT_SIZE + DOT_GAP
            } else {
                0.0
            }
            + this.mark.as_ref().map_or(0.0, |mark| mark.size + DOT_GAP);
        let trailing = this.trailing.as_ref().map(|text| {
            shape_fit_face(
                ctx.text,
                text,
                meta_phys,
                this.width * 0.5,
                ctx.scale,
                if this.pill_ink.is_some() {
                    FontFace::SansBold
                } else {
                    FontFace::Sans
                },
            )
        });
        let pill_pad = if this.pill.is_some() { PILL_PAD_H } else { 0.0 };
        let trailing_width = trailing
            .as_ref()
            .map_or(0.0, |t| t.width * inv + 2.0 * pill_pad);
        let meta_text = this
            .meta
            .as_ref()
            .map(|text| shape_fit(ctx.text, text, meta_phys, this.width * 0.5, ctx.scale));
        let meta_width = meta_text.as_ref().map_or(0.0, |t| t.width * inv);
        // A time on the title line sends the trailing text to the line under it.
        let trailing_below = meta_text.is_some() && this.subtitle.is_some();
        let trailing_room = if trailing.is_some() {
            trailing_width + TRAILING_GAP
        } else {
            0.0
        };
        let title_room = this.width
            - lead
            - this.pad_h
            - this.end_room
            - if trailing_below { 0.0 } else { trailing_room }
            - if meta_text.is_some() {
                meta_width + TRAILING_GAP
            } else {
                0.0
            };
        let title_face = if this.title_bold {
            FontFace::SansBold
        } else {
            FontFace::Sans
        };
        let (first, rest) = if this.title_wrap {
            split_title(
                ctx.text,
                &this.title,
                title_phys,
                title_room.max(8.0),
                ctx.scale,
                title_face,
            )
        } else {
            (this.title.to_string(), None)
        };
        let title = shape_fit_face(
            ctx.text,
            &first,
            title_phys,
            title_room.max(8.0),
            ctx.scale,
            title_face,
        );
        // The second line has the row's width: no time or meta stands at its end.
        let title_rest = rest.map(|rest| {
            shape_fit_face(
                ctx.text,
                &rest,
                title_phys,
                (this.width - lead - this.pad_h - this.end_room).max(8.0),
                ctx.scale,
                title_face,
            )
        });
        // What the second title line adds: every line under the first moves down by it.
        let extra = title_rest.as_ref().map_or(0.0, |rest| rest.height * inv);
        let subtitle_room = (this.width
            - lead
            - this.pad_h
            - this.end_room
            - if trailing_below { trailing_room } else { 0.0 })
        .max(8.0);
        let (subtitle_first, subtitle_rest) = match this.subtitle.as_ref() {
            Some(text) if this.subtitle_wrap => {
                let (first, rest) = split_title(
                    ctx.text,
                    text,
                    meta_phys,
                    subtitle_room,
                    ctx.scale,
                    FontFace::Sans,
                );
                (Some(first), rest)
            }
            Some(text) => (Some(text.to_string()), None),
            None => (None, None),
        };
        let subtitle = subtitle_first
            .as_ref()
            .map(|text| shape_fit(ctx.text, text, meta_phys, subtitle_room, ctx.scale));
        let subtitle_rest = subtitle_rest.map(|text| {
            shape_fit(
                ctx.text,
                &text,
                meta_phys,
                (this.width - lead - this.pad_h - this.end_room).max(8.0),
                ctx.scale,
            )
        });
        let subtitle_height = subtitle.as_ref().map_or(0.0, |s| s.height * inv);
        let subtitle_extra = subtitle_rest.as_ref().map_or(0.0, |s| s.height * inv);
        let size = Size {
            width: this.width,
            height: this.total_height(subtitle_height) + extra + subtitle_extra,
        };

        let rect = node_rect(ctx.scene, id, size);
        let background = if this.selected {
            this.selected_background.unwrap_or(theme.selection)
        } else {
            Color::TRANSPARENT
        };
        let meta = if this.selected {
            crate::widgets::theme::mix_color(theme.text_muted, theme.text, SELECTED_META_LIFT)
        } else {
            theme.text_muted
        };
        // The mark's picture goes to the image atlas; the runtime frees its region when the
        // row is removed. Pixels the atlas cannot fit fall back to the letters.
        let mark_texels = this
            .mark
            .as_ref()
            .and_then(|mark| mark.picture.as_ref())
            .and_then(|(width, height, pixels)| {
                ctx.scene.images_mut().insert(*width, *height, pixels)
            });
        if let Some(tex) = mark_texels {
            ctx.runtime.with(|runtime| {
                runtime.borrow_mut().image_regions.insert(id, tex);
            });
        }
        let pd = ctx.scene.paint_mut(id);
        pd.primitives.clear();
        // [0] background, always present so the row has stable bounds.
        pd.primitives.push(Primitive::SolidRect {
            rect,
            color: background,
            corner_radius: this.selected_radius.min(rect.height * 0.5),
        });
        if let Some(color) = this.accent {
            pd.primitives.push(Primitive::SolidRect {
                rect: Rect::new(rect.x, rect.y, BAR_WIDTH, rect.height),
                color,
                corner_radius: 0.0,
            });
        }
        if let Some(color) = this.dot.filter(|_| this.mark.is_none()) {
            let ground = if background.a > 0 {
                background
            } else {
                theme.surface
            };
            let centre = Point {
                x: rect.x + this.pad_h + DOT_SIZE * 0.5,
                y: rect.y + this.height * 0.5,
            };
            for (size, fill) in dot_discs(this.dot_shape, DOT_SIZE, color, ground) {
                pd.primitives.push(Primitive::SolidRect {
                    rect: Rect::new(centre.x - size * 0.5, centre.y - size * 0.5, size, size),
                    color: fill,
                    corner_radius: size * 0.5,
                });
            }
        }
        if let Some(mark) = &this.mark {
            let disc = Rect::new(
                rect.x + lead - DOT_GAP - mark.size,
                rect.y + (rect.height - mark.size) * 0.5,
                mark.size,
                mark.size,
            );
            pd.primitives.push(Primitive::SolidRect {
                rect: disc,
                color: mark.fill,
                corner_radius: mark.size * 0.5,
            });
            if let Some(tex) = mark_texels {
                pd.primitives.push(Primitive::ImageQuad {
                    rect: disc,
                    atlas_uv: Rect::new(
                        tex.x as f32,
                        tex.y as f32,
                        tex.width as f32,
                        tex.height as f32,
                    ),
                    tint: Color::WHITE,
                });
            } else {
                let (mark_phys, face) = match mark.text_size {
                    Some(size) => (phys_size_px(size, ctx.scale), FontFace::SansBold),
                    None => (meta_phys, FontFace::Sans),
                };
                let letters = ctx.text.shape_with(
                    &mark.text,
                    &ShapeOptions {
                        face,
                        ..ShapeOptions::new(mark_phys)
                    },
                );
                rasterize_and_push(
                    pd,
                    ctx.text,
                    ctx.atlas,
                    &letters,
                    mark_phys as u32,
                    mark.ink,
                    ctx.scale,
                    Point {
                        x: disc.x + (disc.width - letters.width * inv) * 0.5,
                        y: disc.y + (disc.height - letters.height * inv) * 0.5,
                    },
                );
            }
            if let Some(color) = this.dot.filter(|c| c.a > 0) {
                // A ring of the row's ground parts the dot from the disc.
                let ground = if background.a > 0 {
                    background
                } else {
                    theme.surface
                };
                let centre = Point {
                    x: disc.x + disc.width - MARK_DOT * 0.5,
                    y: disc.y + disc.height - MARK_DOT * 0.5,
                };
                let discs = std::iter::once((MARK_DOT + 2.0 * MARK_DOT_RING, ground))
                    .chain(dot_discs(this.dot_shape, MARK_DOT, color, ground));
                for (size, fill) in discs {
                    pd.primitives.push(Primitive::SolidRect {
                        rect: Rect::new(centre.x - size * 0.5, centre.y - size * 0.5, size, size),
                        color: fill,
                        corner_radius: size * 0.5,
                    });
                }
            }
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
        if let Some(rest) = &title_rest {
            rasterize_and_push(
                pd,
                ctx.text,
                ctx.atlas,
                rest,
                title_phys as u32,
                theme.text,
                ctx.scale,
                Point {
                    x: rect.x + lead,
                    y: title_y + title.height * inv,
                },
            );
        }
        if let Some(trailing) = &trailing {
            let x = if this.trailing_inline {
                rect.x + lead + title.width * inv + TRAILING_GAP
            } else {
                rect.x + rect.width - this.pad_h - this.end_room - trailing_width
            };
            let text_height = trailing.height * inv;
            let line = if this.trailing_centered {
                rect.height
            } else {
                this.height
            };
            let y = if trailing_below {
                // Centered on the subtitle's line.
                rect.y + this.height + extra - SUBTITLE_DROP - 2.0
                    + (subtitle_height - text_height) * 0.5
            } else {
                rect.y + (line - text_height) * 0.5
            };
            let color = match this.pill {
                Some(pill) => {
                    let height = text_height + 2.0 * PILL_PAD_V;
                    pd.primitives.push(Primitive::SolidRect {
                        rect: Rect::new(x, y - PILL_PAD_V, trailing_width, height),
                        color: pill,
                        corner_radius: height * 0.5,
                    });
                    this.pill_ink.unwrap_or(theme.on_accent)
                }
                None => meta,
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
        if let Some(meta_text) = &meta_text {
            let end = rect.x + rect.width - this.pad_h - this.end_room;
            let x = if trailing_below || trailing.is_none() || this.trailing_inline {
                end - meta_width
            } else {
                end - trailing_width - TRAILING_GAP - meta_width
            };
            rasterize_and_push(
                pd,
                ctx.text,
                ctx.atlas,
                meta_text,
                meta_phys as u32,
                meta,
                ctx.scale,
                Point {
                    x,
                    y: rect.y + (this.height - meta_text.height * inv) * 0.5,
                },
            );
        }
        if let Some(subtitle) = &subtitle {
            rasterize_and_push(
                pd,
                ctx.text,
                ctx.atlas,
                subtitle,
                meta_phys as u32,
                this.subtitle_color.unwrap_or(meta),
                ctx.scale,
                Point {
                    x: rect.x + lead,
                    y: rect.y + this.height + extra - SUBTITLE_DROP - 2.0,
                },
            );
            if let Some(rest) = &subtitle_rest {
                rasterize_and_push(
                    pd,
                    ctx.text,
                    ctx.atlas,
                    rest,
                    meta_phys as u32,
                    this.subtitle_color.unwrap_or(meta),
                    ctx.scale,
                    Point {
                        x: rect.x + lead,
                        y: rect.y + this.height + extra - SUBTITLE_DROP - 2.0
                            + subtitle.height * inv,
                    },
                );
            }
        }
        ctx.layout.set_measure(id, Box::new(move |_available| size));
        if let Some(tooltip) = &this.tooltip {
            crate::widgets::register_hover_tooltip(
                &ctx.runtime,
                ctx.scene,
                ctx.text,
                ctx.atlas,
                id,
                tooltip,
                size,
                ctx.scale,
            );
        }
        if let Some(handler) = this.on_click {
            with_handlers(&ctx.runtime, id, |h| h.click = Some(handler));
        }
        if let Some(menu) = this.context_menu {
            crate::widgets::context_menu::register_context_menu(&ctx.runtime, id, menu);
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
    fn a_leading_mark_picture_draws_one_image_over_the_disc() {
        let image_quads = |view: ListRow| {
            let (scene, id) = build(view);
            scene
                .paint(id)
                .unwrap()
                .primitives
                .iter()
                .filter_map(|p| match *p {
                    Primitive::ImageQuad { rect, .. } => Some(rect),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        let mark =
            || ListRow::new("room").leading_mark("RO", Color::rgb(1, 2, 3), Color::WHITE, 24.0);
        let quads = image_quads(mark().leading_mark_picture(2, 2, vec![0xff; 16]));
        assert_eq!(quads.len(), 1);
        assert_eq!((quads[0].width, quads[0].height), (24.0, 24.0));
        // Pixels that do not match the size, or no mark at all, draw no picture.
        assert!(image_quads(mark().leading_mark_picture(2, 2, vec![0xff; 15])).is_empty());
        assert!(
            image_quads(ListRow::new("room").leading_mark_picture(2, 2, vec![0xff; 16])).is_empty()
        );
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
    fn a_selected_rows_subtitle_is_lighter_than_the_muted_color() {
        let colors = |selected: bool| -> Vec<Color> {
            let (scene, id) = build(
                ListRow::new("a")
                    .subtitle("b")
                    .selected(selected)
                    .width(200.0),
            );
            scene
                .paint(id)
                .unwrap()
                .primitives
                .iter()
                .filter_map(|p| match p {
                    Primitive::GlyphQuad { color, .. } => Some(*color),
                    _ => None,
                })
                .collect()
        };
        // The title's glyphs come first, the subtitle's last.
        let (plain, selected) = (colors(false), colors(true));
        let (title, muted) = (plain[0], *plain.last().unwrap());
        assert_ne!(title, muted);
        assert_eq!(selected[0], title);
        let lifted = crate::widgets::theme::mix_color(muted, title, SELECTED_META_LIFT);
        assert_eq!(*selected.last().unwrap(), lifted);
        assert_ne!(lifted, muted);
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
    fn centered_trailing_text_sits_on_the_middle_of_a_two_line_row() {
        let (scene, id) = build(
            ListRow::new("title")
                .subtitle("second line")
                .trailing("›")
                .trailing_centered(),
        );
        // The first primitive is the row's background, over its whole box.
        let row = rects(&scene, id)[0].0;
        let glyphs: Vec<Rect> = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::GlyphQuad { rect, .. } => Some(*rect),
                _ => None,
            })
            .collect();
        let rightmost = glyphs.iter().max_by(|a, b| a.x.total_cmp(&b.x)).unwrap();
        let middle = rightmost.y + rightmost.height * 0.5;
        assert!(
            (middle - (row.y + row.height * 0.5)).abs() < row.height * 0.2,
            "{middle} in {row:?}"
        );
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
    fn end_room_moves_the_trailing_text_left_of_it() {
        let (scene, id) = build(
            ListRow::new("branch")
                .trailing("↑1")
                .width(200.0)
                .end_room(30.0),
        );
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
        assert!(right <= 200.0 - 12.0 - 30.0 + 1.0, "{right}");
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

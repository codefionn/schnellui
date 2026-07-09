//! The multi-line source editor [`TextArea`] (SOUL §6.3, §8.1) — the editing
//! counterpart of the [`RichText`](crate::RichText) viewer.
//!
//! A `TextArea` retains a multi-line **plain source** value plus a
//! caret/anchor byte pair, and renders it in the embedded mono face. Styling
//! is pluggable: an implementor-supplied [`HighlightFn`] maps the value to
//! per-line styled spans (the code-highlighting hook — the framework draws
//! what the hook returns, it never parses; per the project's current scope,
//! format parsing is application code).
//!
//! Editing follows the exact `text_edit` discipline (SOUL §6.3): windowed
//! keyboard/pointer events and inbound AccessKit `Focus`/`SetValue` requests
//! converge on the same dispatch functions, which mutate the edit state,
//! re-emit paint in place, update the accessible value, and fire `on_input`.
//! Every edit is user-initiated `text_edit`-budget work (SOUL §4.1) — a clean
//! frame does zero work here. Caret↔x mapping assumes LTR and relies on the
//! mono family's uniform advances (bold keywords keep column alignment).

use std::borrow::Cow;

use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_scene::{
    Color, DirtyFlags, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui_text::{FontFace, GlyphAtlas, ShapeOptions, SpanSpec, TextShaper, WrapMode};
use smallvec::SmallVec;

use crate::rich::RichSpan;
use crate::text_edit::{
    advance_before, apply_pointer_selection, byte_at_x, byte_under_x, map_selections,
    next_boundary, next_word, pointer_selection_index, prev_boundary, prev_word,
    replace_selections, selection_list, selection_text, toggle_additional_caret, Change, EditKey,
    PointerOrigin, ReplaceMode, TextPointerAction, TextSelection, CARET_W, INPUT_BORDER_W,
};
use crate::{
    node_rect, norm_scale, phys_size_px, with_handlers, BuildCtx, ContextMenu, ContextMenuItem,
    View,
};

/// Default editor font size, logical px (mono runs wide; slightly under body).
mod view;
pub(crate) use view::*;
const AREA_TEXT_SIZE: f32 = 14.0;
/// Minimum content width / rows an empty editor reserves (SOUL §8.1).
const MIN_AREA_W: f32 = 260.0;
const MIN_ROWS: u32 = 3;
/// Inner padding between the border and the text.
const AREA_PAD: f32 = 8.0;
/// Trailing wash width marking a selected newline.
const NEWLINE_SEL_W: f32 = 5.0;
/// Separation between the line-number gutter and source text.
const GUTTER_GAP: f32 = 8.0;

/// The implementor-supplied highlighter: the whole value in, one styled span
/// list **per line** out (line texts must concatenate back to each source
/// line — a mismatched line falls back to plain rendering rather than
/// corrupting the shot). Runs at edit/paint time, never on a clean frame.
pub type HighlightFn = Box<dyn FnMut(&str) -> Vec<Vec<RichSpan>> + 'static>;

/// The `on_input` handler shape (whole new value in — SOUL §6.3).
type InputHandler = Box<dyn FnMut(&str) + 'static>;

/// Visual-only configuration for a source line-number gutter.
#[derive(Clone, Copy)]
pub(crate) struct LineNumbers {
    pub(crate) start_line: usize,
    /// The highest line number the gutter may display. This lets a windowed
    /// editor reserve stable width before its visible source reaches it.
    pub(crate) last_line: usize,
    pub(crate) color: Color,
}

// ---------------------------------------------------------------------------
// the retained multi-line edit state (SOUL §3.3 registry)
// ---------------------------------------------------------------------------

/// One text area's retained editing state. `caret`/`anchor` are **byte**
/// indices into `value`, always on `char` boundaries.
pub(crate) struct AreaState {
    pub(crate) value: String,
    pub(crate) placeholder: String,
    pub(crate) caret: usize,
    pub(crate) anchor: usize,
    pub(crate) size_px: f32,
    pub(crate) scale: f32,
    pub(crate) min_rows: u32,
    pub(crate) wrap: WrapMode,
    /// Optional visual gutter. Its text never participates in edit state,
    /// accessibility, selection, or clipboard contents.
    pub(crate) line_numbers: Option<LineNumbers>,
    /// Read-only areas retain normal caret and selection behavior, but reject
    /// mutations from keys, paste, and accessibility SetValue requests.
    pub(crate) read_only: bool,
    /// The physical inline offset Up/Down runs steer toward (the "goal
    /// column"); cleared by any other edit.
    goal_x: Option<f32>,
    /// the pluggable syntax highlighter (taken out before running, §3.1).
    highlight: Option<HighlightFn>,
    /// the last emitted content size (logical) — read by the measure closure,
    /// written by paint emission; a change flags LAYOUT.
    pub(crate) content: Size,
    /// the rect the last emission painted into — the post-layout re-emit gate
    /// ([`reemit_moved_areas`]): when layout hands the node a new box (an edit
    /// grew it), paint is re-emitted at the laid-out rect.
    last_rect: Option<Rect>,
    /// Selection unit captured by the current pointer gesture.
    pointer_origin: Option<PointerOrigin>,
    /// Additional VS Code-style selections/carets.
    secondary: SmallVec<[TextSelection; 4]>,
}

impl AreaState {
    /// Byte index of the start of the line containing `i`.
    fn line_start(&self, i: usize) -> usize {
        self.value[..i].rfind('\n').map(|p| p + 1).unwrap_or(0)
    }

    /// Byte index of the end of the line containing `i` (before its `\n`).
    fn line_end(&self, i: usize) -> usize {
        self.value[i..]
            .find('\n')
            .map(|p| i + p)
            .unwrap_or(self.value.len())
    }

    /// Replaces the selection (or splices at the caret) with `t`.
    fn insert(&mut self, t: &str) -> Change {
        replace_selections(
            &mut self.value,
            &mut self.caret,
            &mut self.anchor,
            &mut self.secondary,
            t,
            ReplaceMode::Selection,
        )
    }

    /// Up/Down: land on the previous/next line at the goal column. Needs the
    /// shaper for x↔byte mapping on the target line.
    fn vertical(&mut self, down: bool, select: bool, shaper: &mut TextShaper) -> Change {
        let before = selection_list(self.caret, self.anchor, &self.secondary);
        let ls = self.line_start(self.caret);
        let le = self.line_end(self.caret);
        // Establish (or keep) the goal column in physical px.
        let phys = phys_size_px(self.size_px, self.scale);
        let goal = match self.goal_x {
            Some(g) => g,
            None => {
                let line = &self.value[ls..le];
                let shaped = shape_area_line(shaper, line, phys);
                let g = advance_before(&shaped, self.caret - ls);
                self.goal_x = Some(g);
                g
            }
        };
        let (ts, te) = if down {
            if le >= self.value.len() {
                // last line: Down parks at the very end (standard editor)
                (self.value.len(), self.value.len())
            } else {
                let ts = le + 1;
                (ts, self.line_end(ts))
            }
        } else {
            if ls == 0 {
                // first line: Up parks at the very start
                (0, 0)
            } else {
                let ts = self.line_start(ls - 1);
                (ts, ls - 1)
            }
        };
        let line = &self.value[ts..te];
        let shaped = shape_area_line(shaper, line, phys);
        let target = ts + byte_at_x(&shaped, line.len(), goal);
        self.caret = target;
        if !select {
            self.anchor = target;
        }

        for selection in &mut self.secondary {
            let ls = self.value[..selection.caret]
                .rfind('\n')
                .map(|p| p + 1)
                .unwrap_or(0);
            let le = self.value[selection.caret..]
                .find('\n')
                .map(|p| selection.caret + p)
                .unwrap_or(self.value.len());
            let current = shape_area_line(shaper, &self.value[ls..le], phys);
            let goal = advance_before(&current, selection.caret - ls);
            let (ts, te) = if down {
                if le >= self.value.len() {
                    (self.value.len(), self.value.len())
                } else {
                    let ts = le + 1;
                    let te = self.value[ts..]
                        .find('\n')
                        .map(|p| ts + p)
                        .unwrap_or(self.value.len());
                    (ts, te)
                }
            } else if ls == 0 {
                (0, 0)
            } else {
                let ts = self.value[..ls - 1].rfind('\n').map(|p| p + 1).unwrap_or(0);
                (ts, ls - 1)
            };
            let target_line = shape_area_line(shaper, &self.value[ts..te], phys);
            let target = ts + byte_at_x(&target_line, te - ts, goal);
            selection.caret = target;
            if !select {
                selection.anchor = target;
            }
        }
        let primary = TextSelection {
            caret: self.caret,
            anchor: self.anchor,
        };
        self.secondary.retain(|selection| *selection != primary);
        let after = selection_list(self.caret, self.anchor, &self.secondary);
        if after == before {
            Change::None
        } else {
            Change::Caret
        }
    }

    fn apply(&mut self, key: &EditKey, shaper: &mut TextShaper) -> Change {
        // Any non-vertical edit re-anchors the goal column.
        if !matches!(key, EditKey::Up { .. } | EditKey::Down { .. }) {
            self.goal_x = None;
        }
        match key {
            EditKey::Insert(t) => self.insert(t),
            EditKey::Enter => self.insert("\n"),
            EditKey::Backspace => replace_selections(
                &mut self.value,
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                "",
                ReplaceMode::Backspace,
            ),
            EditKey::Delete => replace_selections(
                &mut self.value,
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                "",
                ReplaceMode::Delete,
            ),
            EditKey::Left { select, word } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| {
                    let range = selection.range();
                    let target = if !*select && !*word && selection.caret != selection.anchor {
                        range.0
                    } else if *word {
                        prev_word(&self.value, selection.caret)
                    } else {
                        prev_boundary(&self.value, selection.caret)
                    };
                    TextSelection {
                        caret: target,
                        anchor: if *select { selection.anchor } else { target },
                    }
                },
            ),
            EditKey::Right { select, word } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| {
                    let range = selection.range();
                    let target = if !*select && !*word && selection.caret != selection.anchor {
                        range.1
                    } else if *word {
                        next_word(&self.value, selection.caret)
                    } else {
                        next_boundary(&self.value, selection.caret)
                    };
                    TextSelection {
                        caret: target,
                        anchor: if *select { selection.anchor } else { target },
                    }
                },
            ),
            // Line-scoped Home/End — the multi-line semantics (SOUL §8.1).
            EditKey::Home { select } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| {
                    let target = self.value[..selection.caret]
                        .rfind('\n')
                        .map(|p| p + 1)
                        .unwrap_or(0);
                    TextSelection {
                        caret: target,
                        anchor: if *select { selection.anchor } else { target },
                    }
                },
            ),
            EditKey::End { select } => map_selections(
                &mut self.caret,
                &mut self.anchor,
                &mut self.secondary,
                |selection| {
                    let target = self.value[selection.caret..]
                        .find('\n')
                        .map(|p| selection.caret + p)
                        .unwrap_or(self.value.len());
                    TextSelection {
                        caret: target,
                        anchor: if *select { selection.anchor } else { target },
                    }
                },
            ),
            EditKey::Up { select } => self.vertical(false, *select, shaper),
            EditKey::Down { select } => self.vertical(true, *select, shaper),
            EditKey::SelectAll => {
                let before = (self.caret, self.anchor);
                let had_secondary = !self.secondary.is_empty();
                self.secondary.clear();
                self.anchor = 0;
                self.caret = self.value.len();
                if (self.caret, self.anchor) == before && !had_secondary {
                    Change::None
                } else {
                    Change::Caret
                }
            }
        }
    }
}

/// Carries the user-controlled selection of a text area into its counterpart
/// in a freshly mounted runtime. Controlled value changes intentionally retain
/// the replacement area's default end selection.
pub(crate) fn inherit_area_selection(
    runtime: &crate::Runtime,
    id: WidgetId,
    previous_runtime: &crate::Runtime,
    previous_id: WidgetId,
) -> bool {
    let previous = previous_runtime.with(|rt| {
        rt.borrow().areas.get(previous_id).map(|state| {
            (
                state.value.clone(),
                state.caret,
                state.anchor,
                state.secondary.clone(),
                state.size_px,
                state.scale,
                state.goal_x,
            )
        })
    });
    let Some((value, caret, anchor, secondary, size_px, scale, goal_x)) = previous else {
        return false;
    };
    runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(state) = rt.areas.get_mut(id) else {
            return false;
        };
        if state.value != value {
            return false;
        }
        state.caret = caret;
        state.anchor = anchor;
        state.secondary = secondary;
        // The vertical goal is measured in physical pixels, so it remains
        // meaningful only when the controlled value and shaping metrics match.
        state.goal_x = (state.size_px == size_px && state.scale == scale)
            .then_some(goal_x)
            .flatten();
        state.pointer_origin = None;
        true
    })
}

/// Shapes one raw source line in the regular mono face — the caret/pointer
/// mapping shape (the mono family's uniform advances make it metric-identical
/// to the highlighted paint shape).
fn shape_area_line(shaper: &mut TextShaper, line: &str, phys: f32) -> schnellui_text::ShapedText {
    shaper.shape_with(
        line,
        &ShapeOptions::new(phys)
            .wrap(WrapMode::NoWrap)
            .face(FontFace::Mono),
    )
}

fn shape_area_line_wrapped(
    shaper: &mut TextShaper,
    line: &str,
    phys: f32,
    wrap: WrapMode,
    max_width: Option<f32>,
) -> schnellui_text::ShapedText {
    shaper.shape_with(
        line,
        &ShapeOptions::new(phys)
            .max_width(max_width)
            .wrap(wrap)
            .face(FontFace::Mono),
    )
}

/// The logical inline space reserved for a line-number gutter. The widest
/// number is supplied by the caller, so virtualized source windows retain a
/// stable gutter when they cross a digit boundary.
pub(crate) fn line_number_gutter_width(
    shaper: &mut TextShaper,
    line_numbers: Option<LineNumbers>,
    line_count: usize,
    phys: f32,
    inv: f32,
) -> f32 {
    let Some(line_numbers) = line_numbers else {
        return 0.0;
    };
    let visible_last = line_numbers
        .start_line
        .saturating_add(line_count.saturating_sub(1));
    let last = line_numbers.last_line.max(visible_last);
    shape_area_line(shaper, &last.to_string(), phys).width * inv + GUTTER_GAP
}

/// Returns (visual sub-row index, x in logical px) for byte offset `rel` inside a wrapped shaped line.
fn caret_x_in_wrapped(
    shaped: &schnellui_text::ShapedText,
    rel: usize,
    source_len: usize,
    inv: f32,
) -> (usize, f32) {
    // Walk glyphs to find which visual line the caret sits on.
    // Each glyph's cluster maps to a byte; the line's glyphs are contiguous but split across lines.
    // We use ShapedLine ranges to locate the caret.
    for (li, line) in shaped.lines.iter().enumerate() {
        let g0 = line.glyph_start as usize;
        let g1 = g0 + line.glyph_count as usize;
        let slice = &shaped.glyphs[g0..g1.min(shaped.glyphs.len())];
        if slice.is_empty() {
            // A soft wrap after trailing whitespace can produce an empty final
            // visual line whose glyph_start is exactly glyphs.len(). The
            // end-of-text caret belongs at the start of that line.
            if rel == 0 || (rel == source_len && li + 1 == shaped.lines.len()) {
                return (li, 0.0);
            }
            continue;
        }
        let first_cluster = slice[0].cluster as usize;
        // If caret is within this visual line's cluster range (or at its end), place it.
        let line_end_exclusive = if li + 1 < shaped.lines.len() {
            shaped.lines[li + 1..]
                .iter()
                .find_map(|next| {
                    shaped
                        .glyphs
                        .get(next.glyph_start as usize)
                        .map(|glyph| glyph.cluster as usize)
                })
                .unwrap_or(source_len)
        } else {
            usize::MAX
        };
        if rel >= first_cluster && rel < line_end_exclusive {
            // x relative to this visual line's origin
            let mut x_phys = 0.0f32;
            for g in slice {
                if (g.cluster as usize) < rel {
                    x_phys += g.x_advance;
                } else {
                    break;
                }
            }
            // Note: shaped.lines[li].x is the alignment offset (0 for Start). For wrapping with Start it is 0.
            // We return phys x divided to logical.
            return (li, (line.x + x_phys) * inv);
        }
    }
    // Fallback: end of last line.
    let last = shaped.lines.last();
    if let Some(line) = last {
        let g0 = line.glyph_start as usize;
        let g1 = g0 + line.glyph_count as usize;
        let mut x_phys = 0.0f32;
        for g in &shaped.glyphs[g0..g1.min(shaped.glyphs.len())] {
            x_phys += g.x_advance;
        }
        return (
            shaped.lines.len().saturating_sub(1),
            (line.x + x_phys) * inv,
        );
    }
    (0, 0.0)
}

// ---------------------------------------------------------------------------
// the widget builder (SOUL §3.3 typed builder chain)
// ---------------------------------------------------------------------------

/// A multi-line source editor (SOUL §8.1): caret/selection editing across
/// lines (arrows, line-scoped Home/End, Enter), pointer placement + drag
/// selection, and a pluggable per-line highlighter for code/markup coloring.
///
/// ```ignore
/// TextArea::new("fn main() {\n    println!(\"hi\");\n}")
///     .highlight(my_rust_highlighter)      // implementor-supplied styling
///     .on_input(move |v| source.set(v.to_string()))
/// ```
///
/// Carries the AccessKit `MultilineTextInput` role; `Focus` and `SetValue`
/// actions route through the same dispatch as keyboard input (SOUL §6.3).
/// Content-sized (grows with its text) — wrap it in a `Scroll` for a viewport.
pub struct TextArea {
    value: String,
    placeholder: Cow<'static, str>,
    size_px: f32,
    min_rows: u32,
    wrap: WrapMode,
    line_numbers: Option<LineNumbers>,
    read_only: bool,
    on_input: Option<InputHandler>,
    highlight: Option<HighlightFn>,
    context_menu: Option<ContextMenu>,
}

impl TextArea {
    pub fn new(value: impl Into<String>) -> TextArea {
        TextArea {
            value: value.into(),
            placeholder: Cow::Borrowed(""),
            size_px: AREA_TEXT_SIZE,
            min_rows: MIN_ROWS,
            wrap: WrapMode::NoWrap,
            line_numbers: None,
            read_only: false,
            on_input: None,
            highlight: None,
            context_menu: None,
        }
    }

    /// Gray hint shown while the value is empty (also the accessible name).
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'static, str>>) -> TextArea {
        self.placeholder = placeholder.into();
        self
    }

    /// Fired with the whole new value after every edit (SOUL §6.3).
    pub fn on_input(mut self, f: impl FnMut(&str) + 'static) -> TextArea {
        self.on_input = Some(Box::new(f));
        self
    }

    /// Replaces the standard Cut/Copy/Paste/Select All context menu.
    pub fn context_menu(mut self, menu: ContextMenu) -> TextArea {
        self.context_menu = Some(menu);
        self
    }

    /// Appends one command to the standard text-editing context menu.
    pub fn context_menu_item(mut self, item: ContextMenuItem) -> TextArea {
        self.context_menu
            .get_or_insert_with(ContextMenu::default_text)
            .push(item);
        self
    }

    /// Installs the implementor's syntax highlighter (see [`HighlightFn`]).
    pub fn highlight(mut self, f: impl FnMut(&str) -> Vec<Vec<RichSpan>> + 'static) -> TextArea {
        self.highlight = Some(Box::new(f));
        self
    }

    /// Editor font size (logical px).
    pub fn size(mut self, size_px: f32) -> TextArea {
        self.size_px = size_px;
        self
    }

    /// Minimum visible rows the empty editor reserves.
    pub fn rows(mut self, min_rows: u32) -> TextArea {
        self.min_rows = min_rows.max(1);
        self
    }

    /// Wrapping policy for long lines (`NoWrap` by default). Chat composers
    /// use `WrapMode::Anywhere` so long tokens never force horizontal scroll.
    pub fn wrap(mut self, wrap: WrapMode) -> TextArea {
        self.wrap = wrap;
        self
    }

    /// Draws a right-aligned mono line-number gutter beginning at `start_line`.
    /// `last_line` reserves enough width for the final line in a larger or
    /// virtualized source, so crossing a digit boundary does not shift text.
    /// Gutter text is visual-only: the source value, accessibility value,
    /// selection, and copied text remain unchanged.
    pub fn line_numbers(mut self, start_line: usize, last_line: usize, color: Color) -> TextArea {
        self.line_numbers = Some(LineNumbers {
            start_line,
            last_line,
            color,
        });
        self
    }

    /// Makes the area a selectable, copyable source projection. Navigation,
    /// pointer selection, and Select All continue to work; text insertion,
    /// deletion, paste, and accessibility value replacement are ignored.
    pub fn read_only(mut self) -> TextArea {
        self.read_only = true;
        self
    }

    pub fn role(&self) -> Role {
        Role::MultilineTextInput
    }
    pub fn kind(&self) -> WidgetKind {
        WidgetKind::TextArea
    }
}

fn is_text_area(scene: &Scene, id: WidgetId) -> bool {
    matches!(scene.node(id), Some(n) if n.kind == WidgetKind::TextArea)
}

fn is_disabled(scene: &Scene, id: WidgetId) -> bool {
    scene
        .a11y(id)
        .map(|a| StateFlags(a.state).contains(StateFlags::DISABLED))
        .unwrap_or(false)
}

pub(crate) fn selected_area_text(runtime: &crate::Runtime, id: WidgetId) -> Option<String> {
    runtime.with(|rt| {
        let rt = rt.borrow();
        let state = rt.areas.get(id)?;
        selection_text(&state.value, state.caret, state.anchor, &state.secondary)
    })
}

/// Re-emits paint + flags channels for a `Change`; `Text` also refreshes the
/// accessible value and fires `on_input` (identical to the TextInput commit).
fn commit_area_change(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    change: Change,
) -> bool {
    match change {
        Change::None => false,
        Change::Caret => {
            emit_text_area_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            true
        }
        Change::Text => {
            let before = runtime.with(|rt| rt.borrow().areas.get(id).map(|st| st.content));
            let value = runtime.with(|rt| rt.borrow().areas.get(id).map(|st| st.value.clone()));
            let Some(value) = value else { return false };
            scene.set_a11y_value(id, Some(value.clone()));
            emit_text_area_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            // An added/removed line or a new widest line changes the measured
            // size — flag LAYOUT so Taffy re-runs (SOUL §8.1 dirty channels).
            let after = runtime.with(|rt| rt.borrow().areas.get(id).map(|st| st.content));
            if before != after {
                scene.mark_dirty(id, DirtyFlags::LAYOUT);
            }
            crate::text_edit::fire_on_input(runtime, id, &value);
            true
        }
    }
}

/// Routes one editing key to a text area (the TextArea arm of
/// [`dispatch_edit_key`](crate::dispatch_edit_key), SOUL §6.3).
pub(crate) fn dispatch_area_key(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    key: EditKey,
) -> bool {
    if !is_text_area(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let taken = runtime.with(|rt| rt.borrow_mut().areas.remove(id));
    let Some(mut st) = taken else { return false };
    let change = if st.read_only
        && !matches!(
            key,
            EditKey::Left { .. }
                | EditKey::Right { .. }
                | EditKey::Up { .. }
                | EditKey::Down { .. }
                | EditKey::Home { .. }
                | EditKey::End { .. }
                | EditKey::SelectAll
        ) {
        Change::None
    } else {
        st.apply(&key, shaper)
    };
    runtime.with(|rt| {
        rt.borrow_mut().areas.insert(id, st);
    });
    commit_area_change(runtime, scene, shaper, atlas, id, change)
}

/// Places the caret from a pointer position (press), or extends the selection
/// (`extend` — drag / shift-click). `p` in logical window coordinates.
pub(crate) fn dispatch_area_pointer(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    p: Point,
    action: TextPointerAction,
) -> bool {
    if !is_text_area(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let Some(rect) = scene.layout(id).map(|b| b.rect).filter(|r| !r.is_empty()) else {
        return false;
    };
    let Some((value, size_px, scale, line_numbers)) = runtime.with(|rt| {
        let rt = rt.borrow();
        rt.areas
            .get(id)
            .map(|st| (st.value.clone(), st.size_px, st.scale, st.line_numbers))
    }) else {
        return false;
    };
    let inv = 1.0 / norm_scale(scale);
    let phys = phys_size_px(size_px, scale);
    let gutter_w =
        line_number_gutter_width(shaper, line_numbers, value.split('\n').count(), phys, inv);
    let line_h = shape_area_line(shaper, "Ag", phys).height * inv;
    // pointer → row (clamped), then inline offset within that row's line
    let row_f = ((p.y - rect.y - AREA_PAD) / line_h).floor();
    let rows = value.split('\n').count();
    let row = (row_f.max(0.0) as usize).min(rows - 1);
    let ls = value
        .split('\n')
        .take(row)
        .map(|l| l.len() + 1)
        .sum::<usize>();
    let line = value.split('\n').nth(row).unwrap_or("");
    let x_phys = (p.x - rect.x - AREA_PAD - gutter_w).max(0.0) * norm_scale(scale);
    let shaped = shape_area_line(shaper, line, phys);
    let caret_idx = ls + byte_at_x(&shaped, line.len(), x_phys);
    let hit_idx = ls + byte_under_x(&shaped, line.len(), x_phys);
    let change = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.areas
            .get_mut(id)
            .map(|st| {
                st.goal_x = None;
                if action == TextPointerAction::AddCaret {
                    st.pointer_origin = None;
                    return toggle_additional_caret(
                        &mut st.caret,
                        &mut st.anchor,
                        &mut st.secondary,
                        caret_idx,
                    );
                }
                let before = (st.caret, st.anchor);
                let cleared_secondary =
                    action != TextPointerAction::Drag && !st.secondary.is_empty();
                if action != TextPointerAction::Drag {
                    st.secondary.clear();
                }
                let idx = pointer_selection_index(action, st.pointer_origin, caret_idx, hit_idx);
                (st.caret, st.anchor) = apply_pointer_selection(
                    &st.value,
                    st.anchor,
                    &mut st.pointer_origin,
                    idx,
                    action,
                    true,
                );
                if (st.caret, st.anchor) == before && !cleared_secondary {
                    Change::None
                } else {
                    Change::Caret
                }
            })
            .unwrap_or(Change::None)
    });
    commit_area_change(runtime, scene, shaper, atlas, id, change)
}

/// Replaces the whole value — the inbound AccessKit `SetValue` path (SOUL
/// §6.3). The caret parks at the end.
pub(crate) fn dispatch_area_set_value(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    id: WidgetId,
    value: &str,
) -> bool {
    if !is_text_area(scene, id) || is_disabled(scene, id) {
        return false;
    }
    let change = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let Some(st) = rt.areas.get_mut(id) else {
            return Change::None;
        };
        if st.read_only {
            return Change::None;
        }
        if st.value == value {
            return Change::None;
        }
        st.value.clear();
        st.value.push_str(value);
        st.caret = st.value.len();
        st.anchor = st.caret;
        st.secondary.clear();
        st.goal_x = None;
        Change::Text
    });
    commit_area_change(runtime, scene, shaper, atlas, id, change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        dispatch_edit_key, dispatch_focus, dispatch_text_pointer, dispatch_text_pointer_action,
        set_text_value,
    };
    use schnellui_layout::LayoutEngine;
    use schnellui_scene::LayoutBox;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn build_area(
        runtime: &crate::Runtime,
        view: TextArea,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        crate::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
        let id = {
            let mut ctx = BuildCtx {
                context: crate::Context::new(),
                runtime: runtime.clone(),
                scene: &mut scene,
                layout: &mut layout,
                text: &mut text,
                atlas: &mut atlas,
                scale: 1.0,
            };
            (Box::new(view) as Box<dyn View>).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    fn caret_anchor(runtime: &crate::Runtime, id: WidgetId) -> (usize, usize) {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            (st.caret, st.anchor)
        })
    }

    fn all_selections(runtime: &crate::Runtime, id: WidgetId) -> Vec<(usize, usize)> {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            selection_list(st.caret, st.anchor, &st.secondary)
                .into_iter()
                .map(|selection| (selection.caret, selection.anchor))
                .collect()
        })
    }

    fn area_value(runtime: &crate::Runtime, id: WidgetId) -> String {
        runtime.with(|rt| rt.borrow().areas.get(id).map(|s| s.value.clone()).unwrap())
    }

    #[test]
    fn build_registers_multiline_semantics_and_paint() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (scene, _l, _t, _a, id) =
            build_area(runtime, TextArea::new("a\nb").placeholder("source"));
        let a = scene.a11y(id).unwrap();
        assert_eq!(Role::from_u16(a.role), Role::MultilineTextInput);
        assert_eq!(a.value.as_deref(), Some("a\nb"));
        assert_eq!(a.name.as_deref(), Some("source"));
        assert!(ActionFlags(a.actions).contains(ActionFlags::FOCUS));
        assert!(ActionFlags(a.actions).contains(ActionFlags::SET_VALUE));
        let prims = &scene.paint(id).unwrap().primitives;
        assert!(matches!(prims[0], Primitive::SolidRect { .. }));
        assert!(prims
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { .. })));
    }

    #[test]
    fn enter_splits_and_vertical_motion_keeps_goal_column() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("abcd\nx\nlong"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        // caret starts at the very end ("long"|) — Up lands on the short line
        // clamped to its end, Up again back on the long first line at col 4.
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Up { select: false }
        ));
        assert_eq!(
            caret_anchor(runtime, id).0,
            "abcd\n".len() + 1,
            "clamped to 'x' end"
        );
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Up { select: false }
        ));
        assert_eq!(
            caret_anchor(runtime, id).0,
            4,
            "goal column restored on 'abcd'"
        );
        // Down twice returns to the end of "long"
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Down { select: false },
        );
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Down { select: false },
        );
        assert_eq!(caret_anchor(runtime, id).0, "abcd\nx\nlong".len());
        // Enter splits at the caret
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Enter
        ));
        assert_eq!(area_value(runtime, id), "abcd\nx\nlong\n");
    }

    #[test]
    fn remount_preserves_vertical_goal_column_when_metrics_match() {
        let previous_runtime = crate::Runtime::new();
        let (mut previous_scene, _l, mut previous_text, mut previous_atlas, previous_id) =
            build_area(&previous_runtime, TextArea::new("abcd\nx\nlong"));
        dispatch_focus(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
            Some(previous_id),
        );
        assert!(dispatch_edit_key(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
            previous_id,
            EditKey::Up { select: false },
        ));
        assert_eq!(caret_anchor(&previous_runtime, previous_id), (6, 6));

        let replacement_runtime = crate::Runtime::new();
        let (
            mut replacement_scene,
            _l,
            mut replacement_text,
            mut replacement_atlas,
            replacement_id,
        ) = build_area(&replacement_runtime, TextArea::new("abcd\nx\nlong"));
        assert!(inherit_area_selection(
            &replacement_runtime,
            replacement_id,
            &previous_runtime,
            previous_id,
        ));
        assert!(dispatch_edit_key(
            &replacement_runtime,
            &mut replacement_scene,
            &mut replacement_text,
            &mut replacement_atlas,
            replacement_id,
            EditKey::Down { select: false },
        ));
        assert_eq!(
            caret_anchor(&replacement_runtime, replacement_id),
            ("abcd\nx\nlong".len(), "abcd\nx\nlong".len()),
            "Down retains the column established before the remount"
        );
    }

    #[test]
    fn remount_drops_vertical_goal_column_when_metrics_change() {
        let previous_runtime = crate::Runtime::new();
        let (mut previous_scene, _l, mut previous_text, mut previous_atlas, previous_id) =
            build_area(&previous_runtime, TextArea::new("abcd\nx\nlong"));
        dispatch_focus(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
            Some(previous_id),
        );
        assert!(dispatch_edit_key(
            &previous_runtime,
            &mut previous_scene,
            &mut previous_text,
            &mut previous_atlas,
            previous_id,
            EditKey::Up { select: false },
        ));

        let replacement_runtime = crate::Runtime::new();
        let (_scene, _l, _text, _atlas, replacement_id) = build_area(
            &replacement_runtime,
            TextArea::new("abcd\nx\nlong").size(20.0),
        );
        assert!(inherit_area_selection(
            &replacement_runtime,
            replacement_id,
            &previous_runtime,
            previous_id,
        ));
        replacement_runtime.with(|rt| {
            assert!(
                rt.borrow()
                    .areas
                    .get(replacement_id)
                    .unwrap()
                    .goal_x
                    .is_none(),
                "a physical goal from different text metrics must not survive"
            );
        });
    }

    #[test]
    fn home_end_are_line_scoped() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("one\ntwo"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        // caret at end (7). Home → start of "two" (4), End → back to 7.
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Home { select: false },
        );
        assert_eq!(caret_anchor(runtime, id), (4, 4));
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::End { select: true },
        );
        assert_eq!(
            caret_anchor(runtime, id),
            (7, 4),
            "shift+End selects the line tail"
        );
    }

    #[test]
    fn read_only_areas_select_and_copy_without_accepting_mutations() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("one\ntwo").read_only());
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));

        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::SelectAll,
        ));
        assert_eq!(selected_area_text(runtime, id).as_deref(), Some("one\ntwo"));
        assert!(!dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("changed"),
        ));
        assert!(!dispatch_area_set_value(
            runtime, &mut scene, &mut text, &mut atlas, id, "changed",
        ));
        assert_eq!(area_value(runtime, id), "one\ntwo");
    }

    #[test]
    fn line_number_gutter_is_visual_only_and_offsets_pointer_hit_testing() {
        use schnellui_scene::Color;

        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let gutter_color = Color::rgb(0x4a, 0x92, 0xc7);
        let source = "abcdefgh\nabcdefgh";
        let (mut scene, _l, mut text, mut atlas, id) = build_area(
            runtime,
            TextArea::new(source)
                .line_numbers(800, 1_000, gutter_color)
                .read_only(),
        );

        // The gutter is rasterized separately in its requested color; source
        // glyphs, a11y, and selections continue to contain only source text.
        assert!(scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == gutter_color)));
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some(source));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::SelectAll,
        ));
        assert_eq!(selected_area_text(runtime, id).as_deref(), Some(source));

        let rect = Rect::new(10.0, 10.0, 400.0, 80.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        let gutter_w = line_number_gutter_width(
            &mut text,
            Some(LineNumbers {
                start_line: 800,
                last_line: 1_000,
                color: gutter_color,
            }),
            2,
            AREA_TEXT_SIZE,
            1.0,
        );
        let visible_only_gutter_w = line_number_gutter_width(
            &mut text,
            Some(LineNumbers {
                start_line: 800,
                last_line: 801,
                color: gutter_color,
            }),
            2,
            AREA_TEXT_SIZE,
            1.0,
        );
        assert!(
            gutter_w > visible_only_gutter_w,
            "the explicit maximum reserves four-digit width before it is visible"
        );
        // This is the source column's left edge, after the reserved gutter.
        // Without the offset, it would land several bytes into "abcdefgh".
        assert!(dispatch_text_pointer(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + AREA_PAD + gutter_w + 0.1,
                y: rect.y + AREA_PAD + 25.0,
            },
            false,
        ));
        assert_eq!(
            caret_anchor(runtime, id),
            ("abcdefgh\n".len(), "abcdefgh\n".len())
        );
    }

    #[test]
    fn typing_updates_value_a11y_and_fires_on_input() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let seen: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let s2 = seen.clone();
        let (mut scene, _l, mut text, mut atlas, id) = build_area(
            runtime,
            TextArea::new("ab").on_input(move |v| s2.borrow_mut().push(v.into())),
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("c")
        ));
        assert_eq!(area_value(runtime, id), "abc");
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("abc"));
        assert_eq!(*seen.borrow(), vec!["abc".to_string()]);
    }

    #[test]
    fn multiline_growth_flags_layout() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) = build_area(runtime, TextArea::new("a"));
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        scene.clear_dirty();
        // 3 rows are reserved; splitting into a 4th row grows the box
        for _ in 0..3 {
            dispatch_edit_key(
                runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                id,
                EditKey::Enter,
            );
        }
        assert!(scene.dirty_flags(id).contains(DirtyFlags::LAYOUT));
    }

    #[test]
    fn selection_wash_spans_lines_and_pointer_places_caret() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("one\ntwo"));
        let rect = Rect::new(10.0, 10.0, 300.0, 80.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::SelectAll,
        );
        let washes = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(|p| matches!(p, Primitive::SolidRect { color, .. } if *color == crate::theme(runtime, ).text_selection))
            .count();
        assert_eq!(washes, 2, "one wash per selected line");
        // click on the second line's start places the caret at byte 4
        assert!(dispatch_text_pointer(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + AREA_PAD,
                y: rect.y + AREA_PAD + 25.0,
            },
            false,
        ));
        assert_eq!(caret_anchor(runtime, id), (4, 4));
    }

    #[test]
    fn triple_click_selects_a_whole_line_including_its_newline() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("one\ntwo\nthree"));
        let rect = Rect::new(10.0, 10.0, 300.0, 100.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + AREA_PAD,
                y: rect.y + AREA_PAD + 25.0,
            },
            TextPointerAction::SelectLine,
        ));
        assert_eq!(caret_anchor(runtime, id), (8, 4));
    }

    #[test]
    fn additional_caret_edits_and_paints_on_another_line() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(runtime, TextArea::new("one\ntwo"));
        let rect = Rect::new(10.0, 10.0, 300.0, 80.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(runtime, &mut scene, &mut text, &mut atlas, Some(id));
        assert!(dispatch_text_pointer_action(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            Point {
                x: rect.x + AREA_PAD,
                y: rect.y + AREA_PAD + 25.0,
            },
            TextPointerAction::AddCaret,
        ));
        assert_eq!(all_selections(runtime, id), vec![(7, 7), (4, 4)]);
        let carets = scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .filter(
                |primitive| matches!(primitive, Primitive::Line { width, .. } if *width == CARET_W),
            )
            .count();
        assert_eq!(carets, 2);

        assert!(dispatch_edit_key(
            runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("!"),
        ));
        assert_eq!(area_value(runtime, id), "one\n!two!");
        assert_eq!(all_selections(runtime, id), vec![(9, 9), (5, 5)]);
    }

    #[test]
    fn set_value_routes_and_highlight_colors_glyphs() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::rich::SpanStyle;
        use schnellui_scene::Color;
        let accent = Color::rgb(0x88, 0x33, 0xaa);
        let (mut scene, _l, mut text, mut atlas, id) = build_area(
            runtime,
            TextArea::new("fn x").highlight(move |src| {
                src.lines()
                    .map(|l| {
                        if let Some(rest) = l.strip_prefix("fn") {
                            vec![
                                RichSpan::new(
                                    "fn",
                                    SpanStyle {
                                        bold: true,
                                        ..SpanStyle::token(accent)
                                    },
                                ),
                                RichSpan::code(rest),
                            ]
                        } else {
                            vec![RichSpan::code(l)]
                        }
                    })
                    .collect()
            }),
        );
        // the keyword's glyphs carry the accent color
        assert!(scene
            .paint(id)
            .unwrap()
            .primitives
            .iter()
            .any(|p| matches!(p, Primitive::GlyphQuad { color, .. } if *color == accent)));
        // SetValue routes through the shared dispatch (SOUL §6.3)
        assert!(set_text_value(
            runtime, &mut scene, &mut text, &mut atlas, id, "let y"
        ));
        assert_eq!(area_value(runtime, id), "let y");
        assert_eq!(scene.a11y(id).unwrap().value.as_deref(), Some("let y"));
    }

    #[test]
    fn mono_bold_advances_match_regular_so_columns_align() {
        let runtime_handle = crate::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        let mut shaper = TextShaper::new();
        let a = shaper.shape_with("mmm", &ShapeOptions::new(28.0).face(FontFace::Mono));
        let b = shaper.shape_with("mmm", &ShapeOptions::new(28.0).face(FontFace::MonoBold));
        assert!(
            (a.width - b.width).abs() < 0.01,
            "Liberation Mono bold advances equal regular ({} vs {})",
            a.width,
            b.width
        );
    }

    #[test]
    fn wrapped_caret_handles_empty_trailing_visual_line() {
        let mut shaper = TextShaper::new();
        let value = "x ";
        let shaped =
            shape_area_line_wrapped(&mut shaper, value, 16.0, WrapMode::Anywhere, Some(1.0));
        assert_eq!(shaped.lines.last().unwrap().glyph_count, 0);
        assert_eq!(
            caret_x_in_wrapped(&shaped, value.len(), value.len(), 1.0),
            (shaped.lines.len() - 1, 0.0)
        );
    }
}

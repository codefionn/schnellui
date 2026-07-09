//! The multi-line source editor [`TextArea`] (SOUL §6.3, §8.1) — the editing
//! counterpart of the [`RichText`](crate::widgets::RichText) viewer.
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
//! frame only walks each area's ancestors to check its scroll band, with no
//! allocation and no emission. Caret↔x mapping assumes LTR and relies on the
//! mono family's uniform advances (bold keywords keep column alignment).

use std::borrow::Cow;

use schnellui::a11y::{ActionFlags, Role, StateFlags};
use schnellui::scene::{
    Color, DirtyFlags, PaintData, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui::text::{
    FontFace, GlyphAtlas, ShapeOptions, ShapedText, SpanSpec, TextShaper, WrapMode,
};
use smallvec::SmallVec;

use crate::widgets::rich::RichSpan;
use crate::widgets::text_edit::{
    advance_before, apply_pointer_selection, byte_at_x, byte_under_x, map_selections,
    next_boundary, next_word, pointer_selection_index, prev_boundary, prev_word,
    replace_selections, selection_list, selection_text, toggle_additional_caret, Change, EditKey,
    PointerOrigin, ReplaceMode, TextPointerAction, TextSelection, CARET_W, INPUT_BORDER_W,
};
use crate::widgets::{
    node_rect, norm_scale, phys_size_px, with_handlers, BuildCtx, ContextMenu, ContextMenuItem,
    View,
};

mod view;
pub(crate) use view::*;

/// Default editor font size, logical px (mono runs wide; slightly under body).
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
    /// Line starts, per-line shapes and the primitive layout of the last
    /// emission: what lets an edit or a caret move cost what changed.
    cache: AreaCache,
}

// ---------------------------------------------------------------------------
// the retained line table and paint layout (what makes an edit cost what changed)
// ---------------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    /// Counts every text shape this module requests, so tests can prove which
    /// edits reshape which lines.
    pub(crate) static SHAPES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Bumps the test shape counter; free outside tests.
#[inline]
fn note_shape() {
    #[cfg(test)]
    SHAPES.with(|c| c.set(c.get() + 1));
}

/// FNV-1a over `bytes`, folded with the length. Line hashes only need to tell
/// an edited line from its old self; a collision costs one stale line shape.
fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h ^ (bytes.len() as u64).rotate_left(32)
}

fn mix(h: u64, x: u64) -> u64 {
    (h ^ x).wrapping_mul(0x0100_0000_01b3).rotate_left(23)
}

/// What one shaped line depends on besides its text.
#[derive(Clone, Copy, PartialEq)]
struct ShapeKey {
    phys: f32,
    wrap: WrapMode,
    max_w: Option<f32>,
}

/// Everything the gutter and text block paint depends on besides line content.
/// A change rebuilds the block (cached shapes still apply); an unchanged key
/// lets an edit splice only the lines that changed.
#[derive(Clone, Copy, PartialEq)]
struct EmitKey {
    origin: Point,
    line_h: f32,
    shape: ShapeKey,
    scale: f32,
    text: Color,
    muted: Color,
    line_numbers: Option<(usize, usize, Color)>,
    placeholder: bool,
    /// the rows emitted when a Scroll ancestor clips the area (none: all)
    band: Option<(usize, usize)>,
}

/// Where each paint section of the node starts and ends:
/// `[surface 0..2][gutter][washes][text][carets]`, then any interaction
/// decoration the runtime appended after the carets.
#[derive(Clone, Copy)]
struct PrimRanges {
    gutter_end: usize,
    wash_end: usize,
    text_end: usize,
    caret_end: usize,
}

/// One source line's retained shape and paint facts.
struct LineInfo {
    /// hash of the line text
    hash: u64,
    /// hash of the line text and the highlight spans it was painted with
    sig: u64,
    /// the line as painted: wrapped at the cached width when wrapping
    shape: Option<Box<ShapedText>>,
    /// the shape epoch `shape` belongs to
    epoch: u32,
    /// the unwrapped shape caret/pointer mapping uses (only when wrapping)
    flat: Option<Box<ShapedText>>,
    /// visual rows the line occupies
    rows: u32,
    /// shaped width, logical px
    width: f32,
    /// glyph/decoration primitives this line owns in the text block
    text_prims: u32,
    /// whether any of them lies outside the surface rect (the focus ring
    /// hugs the painted bounds, so overflow changes where it goes)
    overflow: Option<[f32; 4]>,
}

impl LineInfo {
    fn new(hash: u64) -> LineInfo {
        LineInfo {
            hash,
            sig: hash,
            shape: None,
            epoch: 0,
            flat: None,
            rows: 1,
            width: 0.0,
            text_prims: 0,
            overflow: None,
        }
    }
}

/// Which lines of [`AreaCache::lines`] an edit left stale.
#[derive(Clone, Copy)]
enum Pending {
    /// Old lines `first..=old_last` became new lines `first..=new_last`.
    Window {
        first: usize,
        old_last: usize,
        new_last: usize,
    },
    /// Everything: drop all line state.
    All,
}

#[derive(Default)]
pub(crate) struct AreaCache {
    /// Byte offset of each line start; always matches `AreaState::value`.
    starts: Vec<usize>,
    lines: Vec<LineInfo>,
    pending: Option<Pending>,
    epoch: u32,
    shape_key: Option<ShapeKey>,
    shape_dirty: bool,
    /// the row prefix (or a line's row count) changed since it was rebuilt
    rows_stale: bool,
    key: Option<EmitKey>,
    ranges: Option<PrimRanges>,
    /// The visual rows `[lo, hi)` the text block was emitted for when a Scroll
    /// ancestor clips the area: the viewport plus an overscan each side. Lines
    /// outside own no primitives. `None`: every line is emitted.
    band: Option<(usize, usize)>,
    /// Visual row at which each line starts (+1 entry); only kept when wrapping.
    rows: Vec<u32>,
    /// Primitives each gutter label owns (no-wrap gutters grow/shrink at the tail).
    gutter_counts: Vec<u32>,
    labels: Vec<ShapedText>,
    labels_key: Option<(usize, f32)>,
    /// (digits of the widest number, phys size) -> gutter width, logical px
    gutter_w: Option<(usize, f32, f32)>,
    line_h: Option<(f32, f32)>,
    /// The highlighter's last output, one span list per line.
    hl: Vec<Vec<RichSpan>>,
    hl_stale: bool,
    /// union of what the last emission painted outside the surface rect
    painted_out: Option<[f32; 4]>,
    /// widest shaped line, logical px (valid until a line changes)
    widest: f32,
    /// union of what the lines paint outside the surface rect
    line_out: Option<[f32; 4]>,
    /// how many emissions rebuilt the whole paint (the rest spliced it)
    rebuilds: u64,
    /// how many emissions asked for the focus ring to be re-fitted
    restyles: u64,
    scratch: PaintData,
    washes: Vec<Primitive>,
}

impl AreaCache {
    pub(crate) fn new(value: &str) -> AreaCache {
        let mut cache = AreaCache {
            hl_stale: true,
            ..AreaCache::default()
        };
        cache.rebuild_starts(value);
        cache
    }

    fn rebuild_starts(&mut self, value: &str) {
        self.starts.clear();
        self.starts.push(0);
        self.starts
            .extend(value.match_indices('\n').map(|(i, _)| i + 1));
    }

    /// The whole value changed (SetValue): drop every line.
    fn reset(&mut self, value: &str) {
        self.rebuild_starts(value);
        self.pending = Some(Pending::All);
        self.hl_stale = true;
    }

    /// Index of the line containing byte `i` (O(log n)).
    fn line_of(&self, i: usize) -> usize {
        self.starts.partition_point(|&s| s <= i).saturating_sub(1)
    }

    /// Byte index of the end of line `i` (before its `\n`).
    fn line_end(&self, i: usize, value_len: usize) -> usize {
        match self.starts.get(i + 1) {
            Some(next) => next - 1,
            None => value_len,
        }
    }

    /// Updates the line starts for an edit whose changed bytes lay in the old
    /// range `lo..hi`, and records which lines went stale. One line of slack
    /// on each side covers a Backspace/Delete that joins a neighbouring line
    /// and a grapheme that starts on the previous line.
    fn note_edit(&mut self, value: &str, lo: usize, hi: usize, old_len: usize) {
        if self.pending.is_some() || self.starts.is_empty() {
            self.reset(value);
            return;
        }
        let first = self.line_of(lo).saturating_sub(1);
        let old_last = (self.line_of(hi) + 1).min(self.starts.len() - 1);
        let delta = value.len() as isize - old_len as isize;
        let win_start = self.starts[first];
        let old_end = self.line_end(old_last, old_len);
        let new_end = old_end as isize + delta;
        let sane = new_end >= win_start as isize
            && new_end as usize <= value.len()
            && (new_end as usize == value.len() || value.as_bytes()[new_end as usize] == b'\n');
        if !sane {
            self.reset(value);
            return;
        }
        let new_end = new_end as usize;
        let fresh: SmallVec<[usize; 8]> = value[win_start..new_end]
            .match_indices('\n')
            .map(|(i, _)| win_start + i + 1)
            .collect();
        let count = fresh.len() + 1;
        self.starts.splice(first + 1..=old_last, fresh);
        for s in &mut self.starts[first + count..] {
            *s = (*s as isize + delta) as usize;
        }
        self.pending = Some(Pending::Window {
            first,
            old_last,
            new_last: first + count - 1,
        });
        self.hl_stale = true;
    }

    fn usable(&self) -> bool {
        self.pending.is_none() && !self.lines.is_empty() && self.lines.len() == self.starts.len()
    }

    /// Selects the width the line shapes are valid for; a change makes every
    /// cached shape stale (reshaped lazily by [`shape_lines`](Self::shape_lines)).
    fn set_shape_key(&mut self, key: ShapeKey) {
        if self.shape_key != Some(key) {
            self.shape_key = Some(key);
            self.epoch = self.epoch.wrapping_add(1);
            self.shape_dirty = true;
        }
    }

    /// Shapes every line whose cached shape is missing or from another width.
    fn shape_lines(&mut self, value: &str, shaper: &mut TextShaper, inv: f32) {
        if !self.shape_dirty {
            return;
        }
        let Some(key) = self.shape_key else { return };
        let wrapping = key.wrap != WrapMode::NoWrap;
        for i in 0..self.lines.len() {
            let epoch = self.epoch;
            if self.lines[i].epoch == epoch && self.lines[i].shape.is_some() {
                continue;
            }
            let (s, e) = (self.starts[i], self.line_end(i, value.len()));
            let line = &value[s..e];
            let shaped = if wrapping {
                shape_area_line_wrapped(shaper, line, key.phys, key.wrap, key.max_w)
            } else {
                shape_area_line(shaper, line, key.phys)
            };
            let info = &mut self.lines[i];
            info.rows = if wrapping {
                shaped.lines.len().max(1) as u32
            } else {
                1
            };
            info.width = shaped.width * inv;
            info.shape = Some(Box::new(shaped));
            info.epoch = epoch;
        }
        self.shape_dirty = false;
        self.rows_stale = true;
    }

    /// Recomputes the cumulative visual row of every line (wrapping only).
    fn rebuild_rows(&mut self) {
        self.rows.clear();
        let mut acc = 0u32;
        for line in &self.lines {
            self.rows.push(acc);
            acc += line.rows;
        }
        self.rows.push(acc);
    }

    /// Visual row at which line `i` starts.
    fn row_of(&self, i: usize, wrapping: bool) -> usize {
        if wrapping {
            self.rows[i] as usize
        } else {
            i
        }
    }

    /// The lines that intersect the emitted band (all of them without one).
    /// Needs `rows` when wrapping.
    fn band_lines(
        &self,
        band: Option<(usize, usize)>,
        wrapping: bool,
        n: usize,
    ) -> std::ops::Range<usize> {
        let Some((lo, hi)) = band else { return 0..n };
        if wrapping {
            let rows = &self.rows[..n.min(self.rows.len())];
            let first = rows
                .partition_point(|&r| (r as usize) <= lo)
                .saturating_sub(1);
            let last = rows.partition_point(|&r| (r as usize) < hi);
            first..last.max(first)
        } else {
            lo.min(n)..hi.min(n)
        }
    }

    /// Whether line `i` has a visual row inside the band.
    fn in_band(&self, i: usize, wrapping: bool, band: Option<(usize, usize)>) -> bool {
        let Some((lo, hi)) = band else { return true };
        let row = self.row_of(i, wrapping);
        row < hi && row + self.lines[i].rows as usize > lo
    }

    /// Text-block primitives owned by `lines[..i]`.
    fn text_prims_before(&self, i: usize) -> usize {
        self.lines[..i].iter().map(|l| l.text_prims as usize).sum()
    }
}

/// The metrics every line of one emission shares.
#[derive(Clone, Copy)]
struct Metrics {
    phys: f32,
    scale: f32,
    inv: f32,
    wrap: WrapMode,
    wrapping: bool,
    max_w: Option<f32>,
    line_h: f32,
    origin: Point,
    text: Color,
    band: Option<(usize, usize)>,
}

impl AreaState {
    /// Runs a value-changing edit and records the lines it touched in the
    /// line table. The changed bytes lie inside the span of the pre-edit
    /// selections (one line of slack on each side is added by the table).
    fn edit_text(&mut self, f: impl FnOnce(&mut AreaState) -> Change) -> Change {
        let mut lo = usize::MAX;
        let mut hi = 0usize;
        for selection in selection_list(self.caret, self.anchor, &self.secondary) {
            let (a, b) = selection.range();
            lo = lo.min(a);
            hi = hi.max(b);
        }
        let old_len = self.value.len();
        let change = f(self);
        if change == Change::Text {
            self.cache.note_edit(&self.value, lo, hi, old_len);
        }
        change
    }

    /// Replaces the selection (or splices at the caret) with `t`.
    fn insert(&mut self, t: &str) -> Change {
        self.edit_text(|s| {
            replace_selections(
                &mut s.value,
                &mut s.caret,
                &mut s.anchor,
                &mut s.secondary,
                t,
                ReplaceMode::Selection,
            )
        })
    }

    /// Backspace/Delete over every selection.
    fn erase(&mut self, mode: ReplaceMode) -> Change {
        self.edit_text(|s| {
            replace_selections(
                &mut s.value,
                &mut s.caret,
                &mut s.anchor,
                &mut s.secondary,
                "",
                mode,
            )
        })
    }

    /// Runs `f` with the unwrapped (caret/pointer mapping) shape of line
    /// `row`: the cached one when the line table is current, a fresh shape
    /// otherwise.
    fn with_flat<R>(
        &mut self,
        row: usize,
        shaper: &mut TextShaper,
        f: impl FnOnce(&ShapedText) -> R,
    ) -> R {
        let phys = phys_size_px(self.size_px, self.scale);
        let cache = &mut self.cache;
        let (s, e) = (cache.starts[row], cache.line_end(row, self.value.len()));
        if cache.usable() {
            let epoch = cache.epoch;
            let info = &mut cache.lines[row];
            if self.wrap == WrapMode::NoWrap {
                if info.epoch == epoch {
                    if let Some(shaped) = info.shape.as_deref() {
                        return f(shaped);
                    }
                }
            } else {
                let flat = info.flat.get_or_insert_with(|| {
                    Box::new(shape_area_line(shaper, &self.value[s..e], phys))
                });
                return f(flat);
            }
        }
        f(&shape_area_line(shaper, &self.value[s..e], phys))
    }

    /// Physical pen advance before byte `rel` of line `row`.
    fn flat_advance(&mut self, row: usize, rel: usize, shaper: &mut TextShaper) -> f32 {
        self.with_flat(row, shaper, |shaped| advance_before(shaped, rel))
    }

    /// Caret byte for the physical offset `x` in line `row`.
    fn flat_byte_at_x(&mut self, row: usize, x: f32, shaper: &mut TextShaper) -> usize {
        let len = self.cache.line_end(row, self.value.len()) - self.cache.starts[row];
        self.with_flat(row, shaper, |shaped| byte_at_x(shaped, len, x))
    }

    /// Caret byte for the physical offset `x` in line `row`, plus the byte
    /// under that offset (pointer hit testing).
    fn flat_hit(&mut self, row: usize, x: f32, shaper: &mut TextShaper) -> (usize, usize) {
        let len = self.cache.line_end(row, self.value.len()) - self.cache.starts[row];
        self.with_flat(row, shaper, |shaped| {
            (byte_at_x(shaped, len, x), byte_under_x(shaped, len, x))
        })
    }

    /// Logical line height, shaped once per size.
    fn line_height(&mut self, shaper: &mut TextShaper) -> f32 {
        let phys = phys_size_px(self.size_px, self.scale);
        if let Some((p, h)) = self.cache.line_h {
            if p == phys {
                return h;
            }
        }
        let h = shape_area_line(shaper, "Ag", phys).height / norm_scale(self.scale);
        self.cache.line_h = Some((phys, h));
        h
    }

    /// The gutter's logical width. Digits share one advance in the mono face,
    /// so the width depends only on how many digits the widest number has.
    fn gutter_width(&mut self, shaper: &mut TextShaper, line_count: usize) -> f32 {
        let Some(line_numbers) = self.line_numbers else {
            return 0.0;
        };
        let phys = phys_size_px(self.size_px, self.scale);
        let visible_last = line_numbers
            .start_line
            .saturating_add(line_count.saturating_sub(1));
        let last = line_numbers.last_line.max(visible_last);
        let digits = last.checked_ilog10().map_or(1, |d| d as usize + 1);
        if let Some((d, p, w)) = self.cache.gutter_w {
            if d == digits && p == phys {
                return w;
            }
        }
        let w = line_number_gutter_width(
            shaper,
            self.line_numbers,
            line_count,
            phys,
            1.0 / norm_scale(self.scale),
        );
        self.cache.gutter_w = Some((digits, phys, w));
        w
    }

    /// The caret byte one row above/below `row` at physical offset `goal`.
    fn vertical_target(
        &mut self,
        row: usize,
        down: bool,
        goal: f32,
        shaper: &mut TextShaper,
    ) -> usize {
        let target_row = if down {
            if row + 1 >= self.cache.starts.len() {
                // last line: Down parks at the very end (standard editor)
                return self.value.len();
            }
            row + 1
        } else {
            if row == 0 {
                // first line: Up parks at the very start
                return 0;
            }
            row - 1
        };
        self.cache.starts[target_row] + self.flat_byte_at_x(target_row, goal, shaper)
    }

    /// Up/Down: land on the previous/next line at the goal column. Needs the
    /// shaper for x↔byte mapping on the target line.
    fn vertical(&mut self, down: bool, select: bool, shaper: &mut TextShaper) -> Change {
        let before = selection_list(self.caret, self.anchor, &self.secondary);
        let row = self.cache.line_of(self.caret);
        // Establish (or keep) the goal column in physical px.
        let goal = match self.goal_x {
            Some(g) => g,
            None => {
                let rel = self.caret - self.cache.starts[row];
                let g = self.flat_advance(row, rel, shaper);
                self.goal_x = Some(g);
                g
            }
        };
        let target = self.vertical_target(row, down, goal, shaper);
        self.caret = target;
        if !select {
            self.anchor = target;
        }

        for i in 0..self.secondary.len() {
            let caret = self.secondary[i].caret;
            let row = self.cache.line_of(caret);
            let goal = self.flat_advance(row, caret - self.cache.starts[row], shaper);
            let target = self.vertical_target(row, down, goal, shaper);
            self.secondary[i].caret = target;
            if !select {
                self.secondary[i].anchor = target;
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
            EditKey::Backspace => self.erase(ReplaceMode::Backspace),
            EditKey::Delete => self.erase(ReplaceMode::Delete),
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
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
    previous_runtime: &crate::widgets::Runtime,
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
fn shape_area_line(shaper: &mut TextShaper, line: &str, phys: f32) -> schnellui::text::ShapedText {
    note_shape();
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
) -> schnellui::text::ShapedText {
    note_shape();
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
    shaped: &schnellui::text::ShapedText,
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

pub(crate) fn selected_area_text(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
) -> Option<String> {
    runtime.with(|rt| {
        let rt = rt.borrow();
        let state = rt.areas.get(id)?;
        selection_text(&state.value, state.caret, state.anchor, &state.secondary)
    })
}

/// Re-emits paint + flags channels for a `Change`; `Text` also refreshes the
/// accessible value and fires `on_input` (identical to the TextInput commit).
fn commit_area_change(
    runtime: &crate::widgets::Runtime,
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
            // Retain the accessibility string's capacity across edits. Replacing
            // it with a fresh document clone makes allocation bytes grow with the
            // document and throws that capacity away on every backspace.
            let copied = runtime.with(|rt| {
                let rt = rt.borrow();
                let Some(state) = rt.areas.get(id) else {
                    return false;
                };
                let accessible = scene
                    .a11y_text_mut(id)
                    .value
                    .get_or_insert_with(String::new);
                accessible.clear();
                accessible.push_str(&state.value);
                true
            });
            if !copied {
                return false;
            }
            emit_text_area_paint(runtime, scene, shaper, atlas, id);
            scene.mark_dirty(id, DirtyFlags::PAINT);
            // An added/removed line or a new widest line changes the measured
            // size — flag LAYOUT so Taffy re-runs (SOUL §8.1 dirty channels).
            let after = runtime.with(|rt| rt.borrow().areas.get(id).map(|st| st.content));
            if before != after {
                scene.mark_dirty(id, DirtyFlags::LAYOUT);
            }
            if let Some(value) = scene.a11y(id).and_then(|a| a.value.as_deref()) {
                crate::widgets::text_edit::fire_on_input(runtime, id, value);
            }
            true
        }
    }
}

/// Routes one editing key to a text area (the TextArea arm of
/// [`dispatch_edit_key`](crate::widgets::dispatch_edit_key), SOUL §6.3).
pub(crate) fn dispatch_area_key(
    runtime: &crate::widgets::Runtime,
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
    runtime: &crate::widgets::Runtime,
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
    let change = runtime.with(|rt| {
        let mut rt = rt.borrow_mut();
        let st = rt.areas.get_mut(id)?;
        let scale = st.scale;
        let rows = st.cache.starts.len();
        let gutter_w = st.gutter_width(shaper, rows);
        let line_h = st.line_height(shaper);
        // pointer → row (clamped), then inline offset within that row's line
        let row_f = ((p.y - rect.y - AREA_PAD) / line_h).floor();
        let row = (row_f.max(0.0) as usize).min(rows - 1);
        let ls = st.cache.starts[row];
        let x_phys = (p.x - rect.x - AREA_PAD - gutter_w).max(0.0) * norm_scale(scale);
        let (rel_caret, rel_hit) = st.flat_hit(row, x_phys, shaper);
        let (caret_idx, hit_idx) = (ls + rel_caret, ls + rel_hit);
        st.goal_x = None;
        if action == TextPointerAction::AddCaret {
            st.pointer_origin = None;
            return Some(toggle_additional_caret(
                &mut st.caret,
                &mut st.anchor,
                &mut st.secondary,
                caret_idx,
            ));
        }
        let before = (st.caret, st.anchor);
        let cleared_secondary = action != TextPointerAction::Drag && !st.secondary.is_empty();
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
        Some(if (st.caret, st.anchor) == before && !cleared_secondary {
            Change::None
        } else {
            Change::Caret
        })
    });
    let Some(change) = change else { return false };
    commit_area_change(runtime, scene, shaper, atlas, id, change)
}

/// Replaces the whole value — the inbound AccessKit `SetValue` path (SOUL
/// §6.3). The caret parks at the end.
pub(crate) fn dispatch_area_set_value(
    runtime: &crate::widgets::Runtime,
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
        st.cache.reset(&st.value);
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
    use crate::widgets::{
        dispatch_edit_key, dispatch_focus, dispatch_text_pointer, dispatch_text_pointer_action,
        set_text_value,
    };
    use schnellui::layout::LayoutEngine;
    use schnellui::scene::LayoutBox;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn build_area(
        runtime: &crate::widgets::Runtime,
        view: impl View + 'static,
    ) -> (Scene, LayoutEngine, TextShaper, GlyphAtlas, WidgetId) {
        crate::widgets::reset(runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
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
            (Box::new(view) as Box<dyn View>).build(&mut ctx, None)
        };
        scene.set_root(id);
        (scene, layout, text, atlas, id)
    }

    fn caret_anchor(runtime: &crate::widgets::Runtime, id: WidgetId) -> (usize, usize) {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            (st.caret, st.anchor)
        })
    }

    fn all_selections(runtime: &crate::widgets::Runtime, id: WidgetId) -> Vec<(usize, usize)> {
        runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            selection_list(st.caret, st.anchor, &st.secondary)
                .into_iter()
                .map(|selection| (selection.caret, selection.anchor))
                .collect()
        })
    }

    fn area_value(runtime: &crate::widgets::Runtime, id: WidgetId) -> String {
        runtime.with(|rt| rt.borrow().areas.get(id).map(|s| s.value.clone()).unwrap())
    }

    #[test]
    fn build_registers_multiline_semantics_and_paint() {
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let previous_runtime = crate::widgets::Runtime::new();
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

        let replacement_runtime = crate::widgets::Runtime::new();
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
        let previous_runtime = crate::widgets::Runtime::new();
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

        let replacement_runtime = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        use schnellui::scene::Color;

        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
            .filter(|p| matches!(p, Primitive::SolidRect { color, .. } if *color == crate::widgets::theme(runtime, ).text_selection))
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
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
        let runtime_handle = crate::widgets::Runtime::new();
        #[allow(unused_variables)]
        let runtime = &runtime_handle;
        use crate::widgets::rich::SpanStyle;
        use schnellui::scene::Color;
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
        let runtime_handle = crate::widgets::Runtime::new();
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

    // ---- retained line table, shape cache and spliced paint ----

    fn shapes() -> u64 {
        SHAPES.with(|c| c.get())
    }

    fn key(
        runtime: &crate::widgets::Runtime,
        scene: &mut Scene,
        text: &mut TextShaper,
        atlas: &mut GlyphAtlas,
        id: WidgetId,
        key: EditKey<'static>,
    ) {
        dispatch_edit_key(runtime, scene, text, atlas, id, key);
    }

    fn numbered_source(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("line {i} of the document"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn caret_motion_reshapes_nothing_and_typing_reshapes_only_the_edited_line() {
        let runtime = crate::widgets::Runtime::new();
        let source = numbered_source(300);
        let (mut scene, _l, mut text, mut atlas, id) = build_area(
            &runtime,
            TextArea::new(source).line_numbers(1, 300, Color::BLACK),
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        // park the caret in the middle
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id).unwrap();
            st.caret = st.cache.starts[150] + 4;
            st.anchor = st.caret;
        });
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("x"),
        );

        let before = shapes();
        let rebuilds = |rt: &crate::widgets::Runtime| {
            rt.with(|r| r.borrow().areas.get(id).unwrap().cache.rebuilds)
        };
        let full = rebuilds(&runtime);
        for _ in 0..5 {
            key(
                &runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                id,
                EditKey::Down { select: false },
            );
        }
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Left {
                select: true,
                word: false,
            },
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Home { select: false },
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Up { select: true },
        );
        assert_eq!(
            shapes(),
            before,
            "caret and selection changes reshape no line"
        );

        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("y"),
        );
        assert_eq!(shapes() - before, 1, "typing reshapes only the edited line");
        assert_eq!(
            rebuilds(&runtime),
            full,
            "caret moves and typing splice the paint"
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Backspace,
        );
        assert_eq!(shapes() - before, 2);
        // a new line shapes itself and the line it split, not the document
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Enter,
        );
        assert!(
            shapes() - before <= 2 + 3,
            "Enter reshaped {}",
            shapes() - before - 2
        );
    }

    #[test]
    fn wrapped_typing_reshapes_only_the_edited_line() {
        let runtime = crate::widgets::Runtime::new();
        let source = numbered_source(120);
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(&runtime, TextArea::new(source).wrap(WrapMode::Anywhere));
        let rect = Rect::new(10.0, 10.0, 160.0, 400.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id).unwrap();
            st.caret = st.cache.starts[60] + 3;
            st.anchor = st.caret;
        });
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("x"),
        );
        let before = shapes();
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Right {
                select: false,
                word: false,
            },
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Right {
                select: true,
                word: false,
            },
        );
        assert_eq!(shapes(), before, "caret moves reshape nothing");
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("y"),
        );
        assert_eq!(shapes() - before, 1);
    }

    #[test]
    fn pointer_drag_reuses_cached_line_shapes() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(&runtime, TextArea::new(numbered_source(50)));
        let rect = Rect::new(10.0, 10.0, 400.0, 900.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        let at = |row: f32| Point {
            x: rect.x + AREA_PAD + 30.0,
            y: rect.y + AREA_PAD + row * 17.0,
        };
        assert!(dispatch_text_pointer(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            at(3.0),
            false
        ));
        let before = shapes();
        for row in 4..30 {
            dispatch_text_pointer(
                &runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                id,
                at(row as f32),
                true,
            );
        }
        assert_eq!(shapes(), before, "a drag shapes no line");
        let (caret, anchor) = caret_anchor(&runtime, id);
        assert!(caret > anchor);
    }

    fn prim_close(a: &Primitive, b: &Primitive) -> bool {
        let rc = |a: &Rect, b: &Rect| {
            (a.x - b.x).abs() < 1e-2
                && (a.y - b.y).abs() < 1e-2
                && (a.width - b.width).abs() < 1e-2
                && (a.height - b.height).abs() < 1e-2
        };
        match (a, b) {
            (
                Primitive::SolidRect {
                    rect: r1,
                    color: c1,
                    corner_radius: k1,
                },
                Primitive::SolidRect {
                    rect: r2,
                    color: c2,
                    corner_radius: k2,
                },
            ) => rc(r1, r2) && c1 == c2 && k1 == k2,
            (
                Primitive::GlyphQuad {
                    rect: r1,
                    atlas_uv: u1,
                    color: c1,
                },
                Primitive::GlyphQuad {
                    rect: r2,
                    atlas_uv: u2,
                    color: c2,
                },
            ) =>
            // the two areas pack their own atlases: compare the glyph bitmap size
            {
                rc(r1, r2) && u1.width == u2.width && u1.height == u2.height && c1 == c2
            }
            (
                Primitive::Line {
                    from: f1,
                    to: t1,
                    width: w1,
                    color: c1,
                },
                Primitive::Line {
                    from: f2,
                    to: t2,
                    width: w2,
                    color: c2,
                },
            ) => {
                (f1.x - f2.x).abs() < 1e-2
                    && (f1.y - f2.y).abs() < 1e-2
                    && (t1.x - t2.x).abs() < 1e-2
                    && (t1.y - t2.y).abs() < 1e-2
                    && w1 == w2
                    && c1 == c2
            }
            _ => false,
        }
    }

    /// Block-comment highlighter: carries state across lines.
    fn comment_highlighter(src: &str) -> Vec<Vec<RichSpan>> {
        use crate::widgets::rich::SpanStyle;
        let accent = Color::rgb(0x33, 0x99, 0x55);
        let mut open = false;
        src.split('\n')
            .map(|line| {
                let spans = if open || line.contains("/*") {
                    vec![RichSpan::new(line, SpanStyle::token(accent))]
                } else if let Some(rest) = line.strip_prefix("fn") {
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
                    vec![RichSpan::code(line)]
                };
                if line.contains("/*") && !line.contains("*/") {
                    open = true;
                }
                if line.contains("*/") {
                    open = false;
                }
                spans
            })
            .collect()
    }

    /// The edited area's line table and paint must equal those of an area
    /// built from scratch with the same value and selection.
    fn assert_matches_fresh(
        runtime: &crate::widgets::Runtime,
        scene: &Scene,
        id: WidgetId,
        make: &dyn Fn(String) -> TextArea,
        rect: Rect,
    ) {
        let (value, caret, anchor, secondary, starts) = runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            (
                st.value.clone(),
                st.caret,
                st.anchor,
                st.secondary.clone(),
                st.cache.starts.clone(),
            )
        });
        let mut expected = vec![0usize];
        expected.extend(value.match_indices('\n').map(|(i, _)| i + 1));
        assert_eq!(starts, expected, "line starts stay exact");

        let fresh_runtime = crate::widgets::Runtime::new();
        let (mut scene2, _l, mut text2, mut atlas2, id2) = build_area(&fresh_runtime, make(value));
        scene2.set_layout(
            id2,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(
            &fresh_runtime,
            &mut scene2,
            &mut text2,
            &mut atlas2,
            Some(id2),
        );
        fresh_runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id2).unwrap();
            st.caret = caret;
            st.anchor = anchor;
            st.secondary = secondary;
        });
        emit_text_area_paint(&fresh_runtime, &mut scene2, &mut text2, &mut atlas2, id2);
        let got = &scene.paint(id).unwrap().primitives;
        let want = &scene2.paint(id2).unwrap().primitives;
        assert_eq!(got.len(), want.len(), "primitive count");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!(
                prim_close(g, w),
                "primitive {i} differs:\n got  {g:?}\n want {w:?}"
            );
        }
    }

    #[test]
    fn spliced_paint_matches_a_fresh_build_through_edits() {
        type Config = (WrapMode, bool, bool);
        let configs: [Config; 4] = [
            (WrapMode::NoWrap, false, false),
            (WrapMode::NoWrap, true, true),
            (WrapMode::Anywhere, true, false),
            (WrapMode::Word, false, true),
        ];
        let source =
            "fn a\nlet b /* x\ny */ z\nthe quick brown fox jumps over the lazy dog again\n\nlast";
        for (wrap, gutter, highlight) in configs {
            let make = move |value: String| {
                let mut view = TextArea::new(value).wrap(wrap);
                if gutter {
                    view = view.line_numbers(1, 99, Color::rgb(1, 2, 3));
                }
                if highlight {
                    view = view.highlight(comment_highlighter);
                }
                view
            };
            let runtime = crate::widgets::Runtime::new();
            let (mut scene, _l, mut text, mut atlas, id) =
                build_area(&runtime, make(source.into()));
            let rect = Rect::new(10.0, 10.0, 170.0, 300.0);
            scene.set_layout(
                id,
                LayoutBox {
                    rect,
                    content: rect,
                },
            );
            dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
            let steps: Vec<EditKey<'static>> = vec![
                EditKey::Insert("x"),
                EditKey::Up { select: false },
                EditKey::Enter,
                EditKey::Insert("fn y /* open"),
                EditKey::Enter,
                EditKey::Insert("z"),
                EditKey::Up { select: true },
                EditKey::Backspace,
                EditKey::Home { select: false },
                EditKey::Backspace,
                EditKey::End { select: false },
                EditKey::Delete,
                EditKey::Down { select: true },
                EditKey::Right {
                    select: true,
                    word: true,
                },
                EditKey::Insert("*/ q\nw"),
                EditKey::Left {
                    select: false,
                    word: false,
                },
                EditKey::SelectAll,
                EditKey::Up { select: false },
                EditKey::Insert("a\n\n\nb"),
                EditKey::Backspace,
                EditKey::Backspace,
                EditKey::Backspace,
            ];
            for (n, step) in steps.into_iter().enumerate() {
                key(&runtime, &mut scene, &mut text, &mut atlas, id, step);
                eprintln!("config {wrap:?}/{gutter}/{highlight} step {n}");
                assert_matches_fresh(&runtime, &scene, id, &make, rect);
            }
            // additional carets exercise multi-selection edits
            for row in [0.0f32, 3.0] {
                let p = Point {
                    x: rect.x + AREA_PAD + 10.0,
                    y: rect.y + AREA_PAD + row * 17.0,
                };
                dispatch_text_pointer_action(
                    &runtime,
                    &mut scene,
                    &mut text,
                    &mut atlas,
                    id,
                    p,
                    TextPointerAction::AddCaret,
                );
                assert_matches_fresh(&runtime, &scene, id, &make, rect);
            }
            for step in [
                EditKey::Insert("!"),
                EditKey::Enter,
                EditKey::Backspace,
                EditKey::Backspace,
            ] {
                key(&runtime, &mut scene, &mut text, &mut atlas, id, step);
                assert_matches_fresh(&runtime, &scene, id, &make, rect);
            }
        }
    }

    /// Rows (relative to the area's first row) of every glyph in the text block.
    fn glyph_rows(scene: &Scene, id: WidgetId, rect: Rect, line_h: f32) -> (usize, usize, usize) {
        let origin_y = rect.y + AREA_PAD;
        let mut lo = usize::MAX;
        let mut hi = 0;
        let mut n = 0;
        for p in &scene.paint(id).unwrap().primitives {
            if let Primitive::GlyphQuad { rect: r, .. } = p {
                let row = ((r.y - origin_y) / line_h).floor().max(0.0) as usize;
                lo = lo.min(row);
                hi = hi.max(row);
                n += 1;
            }
        }
        (lo, hi, n)
    }

    #[test]
    fn scrolled_area_emits_only_the_rows_near_the_viewport() {
        let runtime = crate::widgets::Runtime::new();
        let lines = 2000;
        let (mut scene, _l, mut text, mut atlas, sv) = build_area(
            &runtime,
            crate::widgets::Scroll::new()
                .size(300.0, 200.0)
                .child(TextArea::new(numbered_source(lines))),
        );
        let id = runtime.with(|rt| rt.borrow().areas.keys().next().unwrap());
        let view = Rect::new(0.0, 0.0, 300.0, 200.0);
        scene.set_layout(
            sv,
            LayoutBox {
                rect: view,
                content: view,
            },
        );
        let line_h = runtime.with(|rt| rt.borrow().areas.get(id).unwrap().cache.line_h);
        // first emit without layout publishes the content size and line height
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
        let line_h = line_h
            .or_else(|| runtime.with(|rt| rt.borrow().areas.get(id).unwrap().cache.line_h))
            .unwrap()
            .1;
        let content_h = lines as f32 * line_h + 2.0 * AREA_PAD;
        let rect = Rect::new(0.0, 0.0, 300.0, content_h);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        let all = {
            emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
            let rebuilds = |rt: &crate::widgets::Runtime| {
                rt.with(|r| r.borrow().areas.get(id).unwrap().cache.rebuilds)
            };
            let first = rebuilds(&runtime);
            let (lo, hi, n) = glyph_rows(&scene, id, rect, line_h);
            let visible_rows = (200.0 / line_h).ceil() as usize;
            assert_eq!(lo, 0);
            assert!(hi <= 3 * visible_rows + 2, "top band reaches row {hi}");
            assert!(n < 40 * (3 * visible_rows + 2), "emitted {n} glyphs");

            // scrolling inside the band costs nothing
            scene.set_scroll_offset(sv, Point { x: 0.0, y: line_h });
            reemit_scrolled_areas(&runtime, &mut scene, &mut text, &mut atlas);
            assert_eq!(rebuilds(&runtime), first, "still inside the band");

            // a far jump moves the band with the viewport
            for target in [900usize, 1500, 40] {
                let y = target as f32 * line_h;
                scene.set_scroll_offset(sv, Point { x: 0.0, y });
                reemit_scrolled_areas(&runtime, &mut scene, &mut text, &mut atlas);
                let (lo, hi, n2) = glyph_rows(&scene, id, rect, line_h);
                assert!(
                    lo <= target && target + visible_rows <= hi + 1,
                    "{lo}..{hi} for {target}"
                );
                assert!(hi - lo <= 3 * visible_rows + 3, "band {lo}..{hi}");
                assert!(n2 <= n * 2, "glyph count grew with the document: {n2}");
                let seen = rebuilds(&runtime);
                reemit_scrolled_areas(&runtime, &mut scene, &mut text, &mut atlas);
                assert_eq!(rebuilds(&runtime), seen, "a settled frame emits nothing");
            }
            n
        };
        // typing in the scrolled window splices; the value stays complete
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id).unwrap();
            st.caret = st.cache.starts[45] + 2;
            st.anchor = st.caret;
        });
        let before = shapes();
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("x"),
        );
        assert_eq!(shapes() - before, 1);
        let (_, _, n3) = glyph_rows(&scene, id, rect, line_h);
        assert!(n3 <= all * 2);
        assert_eq!(area_value(&runtime, id).lines().count(), lines);
    }

    /// Gutter labels must follow the rows when an edit moves rows between
    /// lines without changing their total, inside a Scroll's band.
    #[test]
    fn scrolled_wrapped_gutter_follows_rows_redistributed_by_an_edit() {
        let runtime = crate::widgets::Runtime::new();
        let long = "word ".repeat(40);
        let source = format!("{long}\nb\n{}", numbered_source(30));
        let (mut scene, _l, mut text, mut atlas, sv) = build_area(
            &runtime,
            crate::widgets::Scroll::new().size(150.0, 200.0).child(
                TextArea::new(source)
                    .wrap(WrapMode::Anywhere)
                    .line_numbers(1, 99, Color::BLACK),
            ),
        );
        let id = runtime.with(|rt| rt.borrow().areas.keys().next().unwrap());
        let view = Rect::new(0.0, 0.0, 150.0, 200.0);
        scene.set_layout(
            sv,
            LayoutBox {
                rect: view,
                content: view,
            },
        );
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
        let rect = Rect::new(0.0, 0.0, 150.0, 4000.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
        // select from inside line 0 to the end of line 1 and paste over it
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id).unwrap();
            st.anchor = st.cache.starts[0] + 3;
            st.caret = st.cache.starts[1] + 1;
        });
        let paste: &'static str = Box::leak(format!("x\n{long}").into_boxed_str());
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert(paste),
        );
        let got = scene.paint(id).unwrap().primitives.clone();
        // force a full rebuild and compare
        runtime.with(|rt| rt.borrow_mut().areas.get_mut(id).unwrap().cache.key = None);
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
        let want = &scene.paint(id).unwrap().primitives;
        assert_eq!(got.len(), want.len(), "primitive count");
        for (i, (g, w)) in got.iter().zip(want).enumerate() {
            assert!(
                prim_close(g, w),
                "primitive {i} differs:\n got  {g:?}\n want {w:?}"
            );
        }
    }

    /// An unbreakable token spills out of the box for good; caret moves and
    /// typing elsewhere must not re-fit the focus ring each time.
    #[test]
    fn steady_overflow_does_not_refit_the_ring_per_edit() {
        let runtime = crate::widgets::Runtime::new();
        let source = format!("{}\n{}", "=".repeat(160), numbered_source(200));
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(&runtime, TextArea::new(source).wrap(WrapMode::Word));
        let rect = Rect::new(10.0, 10.0, 170.0, 4000.0);
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);
        assert!(runtime.with(|rt| rt
            .borrow()
            .areas
            .get(id)
            .unwrap()
            .cache
            .painted_out
            .is_some()));
        runtime.with(|rt| {
            let mut rt = rt.borrow_mut();
            let st = rt.areas.get_mut(id).unwrap();
            st.caret = st.cache.starts[100] + 2;
            st.anchor = st.caret;
        });
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("x"),
        );
        let restyles = |rt: &crate::widgets::Runtime| {
            rt.with(|r| r.borrow().areas.get(id).unwrap().cache.restyles)
        };
        let before = restyles(&runtime);
        for _ in 0..5 {
            key(
                &runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                id,
                EditKey::Down { select: false },
            );
            key(
                &runtime,
                &mut scene,
                &mut text,
                &mut atlas,
                id,
                EditKey::Insert("y"),
            );
        }
        assert_eq!(
            restyles(&runtime),
            before,
            "no ring re-fit while the spill is unchanged"
        );
    }

    #[test]
    fn ancestor_hover_decoration_does_not_leave_stale_colors_in_the_area() {
        let runtime = crate::widgets::Runtime::new();
        crate::widgets::reset(&runtime);
        let mut scene = Scene::new();
        let mut layout = LayoutEngine::new();
        let mut text = TextShaper::new();
        let mut atlas = GlyphAtlas::new(1024, 1024);
        let item = scene.insert(WidgetKind::ListItem, None);
        scene.set_root(item);
        let make = |value: String| TextArea::new(value);
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
            (Box::new(make("alpha\nbeta\ngamma".into())) as Box<dyn View>)
                .build(&mut ctx, Some(item))
        };
        let a = scene.a11y_mut(item);
        a.actions = ActionFlags::FOCUS.0;
        let item_rect = Rect::new(0.0, 0.0, 400.0, 120.0);
        let rect = Rect::new(10.0, 10.0, 200.0, 100.0);
        scene.set_layout(
            item,
            LayoutBox {
                rect: item_rect,
                content: item_rect,
            },
        );
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        emit_text_area_paint(&runtime, &mut scene, &mut text, &mut atlas, id);

        // hover the item outside the area: its decoration recolors the area's glyphs
        crate::widgets::update_pointer_proximity(&runtime, &mut scene, Point { x: 300.0, y: 60.0 });
        let hovered = runtime.with(|rt| rt.borrow().hover.as_ref().map(|h| h.target));
        assert_eq!(hovered, Some(item), "the list item is hovered");
        assert!(view::has_indexed_decoration(&runtime, id));

        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("zz"),
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Enter,
        );
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("q"),
        );
        crate::widgets::update_pointer_proximity(
            &runtime,
            &mut scene,
            Point { x: 900.0, y: 900.0 },
        );
        assert!(runtime.with(|rt| rt.borrow().hover.is_none()));
        assert_matches_fresh(&runtime, &scene, id, &make, rect);
    }

    #[test]
    fn line_of_agrees_with_counting_newlines() {
        let runtime = crate::widgets::Runtime::new();
        let (mut scene, _l, mut text, mut atlas, id) =
            build_area(&runtime, TextArea::new("a\n\nbcd\ne\n"));
        dispatch_focus(&runtime, &mut scene, &mut text, &mut atlas, Some(id));
        key(
            &runtime,
            &mut scene,
            &mut text,
            &mut atlas,
            id,
            EditKey::Insert("zz\nzz"),
        );
        runtime.with(|rt| {
            let rt = rt.borrow();
            let st = rt.areas.get(id).unwrap();
            for i in 0..=st.value.len() {
                assert_eq!(
                    st.cache.line_of(i),
                    st.value[..i].matches('\n').count(),
                    "byte {i}"
                );
            }
        });
    }
}

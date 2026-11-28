//! Transient context menus for editable text controls and other interactive
//! widgets that opt in.
//!
//! Menus are built on demand as manually positioned overlay nodes. Their source
//! configuration remains retained beside the source widget, so opening and
//! dismissing a menu never rebuilds the application tree.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use schnellui::a11y::{ActionFlags, Role, StateFlags};
use schnellui::scene::{
    Color, DirtyFlags, LayoutBox, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui::text::{GlyphAtlas, ShapedText, TextShaper};
use slotmap::SecondaryMap;

use crate::widgets::{
    emit_slider_paint, forget_node_theme, format_slider_value, norm_scale, phys_size_px,
    quantize_slider, rasterize_and_push, remember_node_theme, theme_for, ClickHandler, SliderState,
    Theme, BUTTON_TEXT_SIZE, PAD_H, SLIDER_STEP_FRACTION,
};

/// How much of the window a control may span and still be a row that a menu takes the
/// width of.
const ROW_ANCHOR_SHARE: f32 = 0.5;
const CONTEXT_MENU_MIN_WIDTH: f32 = 140.0;
/// Width of the frame around a menu's source while the menu is open.
/// How far the frame of the control a menu opened from goes from the selection colour
/// toward the accent.
const OPENED_FROM_MIX: f32 = 0.5;
const OPENED_FROM_FRAME: f32 = 2.0;
const CONTEXT_MENU_MARGIN: f32 = 4.0;
/// The least room between a menu and the window's left and right edges: wider than the
/// margin above and below, so a menu opened at the window's edge does not read as cut off.
const CONTEXT_MENU_SIDE_MARGIN: f32 = 16.0;
/// Vertical padding of a menu row, each side: menus stay denser than buttons.
const MENU_PAD_V: f32 = 5.0;
/// How far the words at a slider's ends reach up into the slider's row, toward its track.
const SLIDER_ENDS_RISE: f32 = 4.0;
/// Air between a menu's frame and its rows, so a row's wash and ring stand
/// clear of the frame as a popover's rows do.
const MENU_INSET: f32 = 4.0;

/// A built-in or user-defined context-menu command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Custom,
}

enum ItemAction {
    BuiltIn(ContextMenuAction),
    Custom(Option<ClickHandler>),
    /// What a slider row runs on each move.
    Slider(Option<Box<dyn FnMut(f32) + 'static>>),
}

/// Where a slider row stands and what it runs between.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SliderRange {
    value: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
}

impl SliderRange {
    /// The range as a slider keeps it: finite ends in order, a positive step and the value
    /// snapped onto it, as [`crate::widgets::Slider`] makes them.
    fn state(self) -> SliderState {
        let min = if self.min.is_finite() { self.min } else { 0.0 };
        let max = if self.max.is_finite() && self.max > min {
            self.max
        } else {
            min
        };
        let step = self
            .step
            .filter(|step| step.is_finite() && *step > 0.0)
            .unwrap_or(((max - min) * SLIDER_STEP_FRACTION).max(f32::EPSILON));
        let value = if self.value.is_finite() {
            self.value
        } else {
            min
        };
        SliderState {
            value: quantize_slider(value, min, max, step),
            min,
            max,
            step,
        }
    }
}

/// The layers of a menu's shadow: how far each reaches past the menu and
/// its alpha. Wider layers are fainter.
const MENU_SHADOW: [(f32, u8); 3] = [(8.0, 0x18), (4.0, 0x28), (1.0, 0x40)];
/// How far the shadow sits below the menu, as if lit from above.
const MENU_SHADOW_DROP: f32 = 3.0;
/// Size of the check mark at the end of a menu's chosen row.
const CHECK_SIZE: f32 = 12.0;
/// Stroke of the check mark.
const CHECK_STROKE: f32 = 2.0;
/// Room a menu with a chosen row keeps for the check mark.
const CHECK_ROOM: f32 = CHECK_SIZE + 12.0;
/// Size of the switch at the end of a setting's row.
const SWITCH_WIDTH: f32 = 28.0;
const SWITCH_HEIGHT: f32 = 16.0;
/// How far the switch's knob sits inside its track.
const SWITCH_KNOB_INSET: f32 = 2.0;
/// Room a menu with a switch keeps for it.
const SWITCH_ROOM: f32 = SWITCH_WIDTH + 12.0;
/// Height of a rule between two groups of a menu.
const SEPARATOR_HEIGHT: f32 = 9.0;

/// How much smaller a heading is than the commands under it.
const HEADING_SCALE: f32 = 0.85;
/// The smallest a heading gets: under it a caption is hard to read.
const HEADING_MIN: f32 = 12.0;

/// A heading's text size for commands of `size`: a step smaller, and never under
/// [`HEADING_MIN`].
fn heading_size(size: f32) -> f32 {
    (size * HEADING_SCALE).max(HEADING_MIN.min(size))
}

/// One row in a context menu.
pub struct ContextMenuItem {
    label: Cow<'static, str>,
    enabled: bool,
    selected: bool,
    /// Draws a switch at the row's end, on while `selected`; see [`ContextMenuItem::switch`].
    switch: bool,
    danger: bool,
    action: ItemAction,
    kind: ItemKind,
    range: SliderRange,
    /// Words under a slider's track at its two ends; see [`ContextMenuItem::ends`].
    ends: Option<(Cow<'static, str>, Cow<'static, str>)>,
}

/// What a menu row is: a command, a rule between groups, a group's caption, or a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemKind {
    Command,
    Separator,
    Heading,
    Slider,
}

impl ContextMenuItem {
    /// Creates a custom command. Attach its callback with
    /// [`ContextMenuItem::on_select`].
    pub fn new(label: impl Into<Cow<'static, str>>) -> ContextMenuItem {
        ContextMenuItem {
            label: label.into(),
            enabled: true,
            selected: false,
            switch: false,
            danger: false,
            action: ItemAction::Custom(None),
            kind: ItemKind::Command,
            range: SliderRange::default(),
            ends: None,
        }
    }

    /// A rule between two groups of commands. The keyboard and assistive
    /// technology pass over it.
    pub fn separator() -> ContextMenuItem {
        ContextMenuItem {
            label: Cow::Borrowed(""),
            enabled: false,
            selected: false,
            switch: false,
            danger: false,
            action: ItemAction::Custom(None),
            kind: ItemKind::Separator,
            range: SliderRange::default(),
            ends: None,
        }
    }

    /// A caption over the group of commands after it, in the muted text
    /// color. It is not a command: the keyboard passes over it.
    pub fn heading(label: impl Into<Cow<'static, str>>) -> ContextMenuItem {
        ContextMenuItem {
            label: label.into(),
            enabled: false,
            selected: false,
            switch: false,
            danger: false,
            action: ItemAction::Custom(None),
            kind: ItemKind::Heading,
            range: SliderRange::default(),
            ends: None,
        }
    }

    /// A slider across the menu that sets a value from `min` to `max`, such as how loud one
    /// sound plays. `label` is its accessible name; the row draws no words, so put a
    /// [`ContextMenuItem::heading`] above it that says what it sets. Dragging it or turning
    /// it with the arrow keys keeps the menu open, and every move reaches
    /// [`ContextMenuItem::on_change`]. Left and Right move it a step, Page Up and Page Down
    /// ten; Up and Down walk the rows as they do in any menu.
    pub fn slider(
        label: impl Into<Cow<'static, str>>,
        value: f32,
        min: f32,
        max: f32,
    ) -> ContextMenuItem {
        ContextMenuItem {
            label: label.into(),
            enabled: true,
            selected: false,
            switch: false,
            danger: false,
            action: ItemAction::Slider(None),
            kind: ItemKind::Slider,
            range: SliderRange {
                value,
                min,
                max,
                step: None,
            },
            ends: None,
        }
    }

    pub fn cut() -> ContextMenuItem {
        Self::built_in("Cut", ContextMenuAction::Cut)
    }

    pub fn copy() -> ContextMenuItem {
        Self::built_in("Copy", ContextMenuAction::Copy)
    }

    pub fn paste() -> ContextMenuItem {
        Self::built_in("Paste", ContextMenuAction::Paste)
    }

    pub fn select_all() -> ContextMenuItem {
        Self::built_in("Select All", ContextMenuAction::SelectAll)
    }

    fn built_in(label: &'static str, action: ContextMenuAction) -> ContextMenuItem {
        ContextMenuItem {
            label: Cow::Borrowed(label),
            enabled: true,
            selected: false,
            switch: false,
            danger: false,
            action: ItemAction::BuiltIn(action),
            kind: ItemKind::Command,
            range: SliderRange::default(),
            ends: None,
        }
    }

    /// Enables or disables this command independently of its dynamic clipboard
    /// state.
    pub fn enabled(mut self, enabled: bool) -> ContextMenuItem {
        self.enabled = enabled;
        self
    }

    /// Marks this item as the current one of a menu that picks among
    /// choices, such as the shown tab in a list of tabs: it gets a check mark
    /// in the accent at its end and the selected state.
    pub fn selected(mut self, selected: bool) -> ContextMenuItem {
        self.selected = selected;
        self
    }

    /// Makes this command a setting that is on or off, such as muting one sound: a switch at
    /// the row's end shows it, on in the accent while `on`, and the row has the selected state
    /// then. A check mark alone read as one choice of several, not as a setting to flip.
    pub fn switch(mut self, on: bool) -> ContextMenuItem {
        self.switch = true;
        self.selected = on;
        self
    }

    /// Marks a command that takes something away (leave, delete): its label
    /// takes the theme's attention color.
    pub fn danger(mut self, danger: bool) -> ContextMenuItem {
        self.danger = danger;
        self
    }

    /// Sets the callback for a custom command.
    pub fn on_select(mut self, callback: impl FnMut() + 'static) -> ContextMenuItem {
        self.action = ItemAction::Custom(Some(Box::new(callback)));
        self
    }

    /// Sets how far a slider row moves for one arrow key and where a drag snaps to. Without
    /// it a step is 1% of the range, as for [`crate::widgets::Slider`].
    pub fn step(mut self, step: f32) -> ContextMenuItem {
        self.range.step = Some(step);
        self
    }

    /// Puts `least` and `most` under a slider row's track, at its start and its end, in the
    /// headings' smaller size and muted ink, such as "10%" and "200%": with nothing at the
    /// ends, the range was a guess. The row grows by their line.
    pub fn ends(
        mut self,
        least: impl Into<Cow<'static, str>>,
        most: impl Into<Cow<'static, str>>,
    ) -> ContextMenuItem {
        self.ends = Some((least.into(), most.into()));
        self
    }

    /// Sets what a slider row runs with its new value on each move. The menu stays open.
    pub fn on_change(mut self, callback: impl FnMut(f32) + 'static) -> ContextMenuItem {
        self.action = ItemAction::Slider(Some(Box::new(callback)));
        self
    }
}

/// Context-menu contents for an interactive widget.
///
/// `ContextMenu::new()` is empty and is therefore suitable for replacing the
/// standard menu. [`ContextMenu::default_text`] starts with Cut, Copy, Paste,
/// and Select All.
#[derive(Default)]
pub struct ContextMenu {
    items: Vec<ContextMenuItem>,
    text_size: Option<f32>,
    row_height: Option<f32>,
    name: Option<Cow<'static, str>>,
    above: bool,
    spaced: bool,
    min_width: Option<f32>,
}

impl ContextMenu {
    pub fn new() -> ContextMenu {
        ContextMenu::default()
    }

    /// Sets the items' text size in logical px, for a menu that lists the
    /// smaller text of the control it opens from, such as tab titles.
    pub fn text_size(mut self, size: f32) -> ContextMenu {
        self.text_size = Some(size);
        self
    }

    /// Sets the least height of a row in logical px, so a menu in a small
    /// text size can keep rows as easy to hit as its neighbours' lists.
    pub fn row_height(mut self, height: f32) -> ContextMenu {
        self.row_height = Some(height);
        self
    }

    /// Opens the menu over the control it hangs from when it fits there, and
    /// under it only when it does not. For a control whose neighbours below
    /// matter more than those above, such as a button over a row of controls.
    pub fn above(mut self, above: bool) -> ContextMenu {
        self.above = above;
        self
    }

    /// Keeps the usual gap between the control and a menu that opens under it,
    /// as a menu over it has. For a small control at the foot of something the
    /// menu must leave in sight, such as a button under a video tile.
    pub fn spaced(mut self, spaced: bool) -> ContextMenu {
        self.spaced = spaced;
        self
    }

    /// Sets the least width of the menu in logical px, for a menu whose slider needs more
    /// room than its words give it.
    pub fn min_width(mut self, width: f32) -> ContextMenu {
        self.min_width = Some(width);
        self
    }

    /// Names the menu for assistive technology. By default it is named
    /// after the control it opens from, as "Save menu".
    pub fn name(mut self, name: impl Into<Cow<'static, str>>) -> ContextMenu {
        self.name = Some(name.into());
        self
    }

    pub fn default_text() -> ContextMenu {
        ContextMenu::new()
            .item(ContextMenuItem::cut())
            .item(ContextMenuItem::copy())
            .item(ContextMenuItem::paste())
            .item(ContextMenuItem::select_all())
    }

    pub fn item(mut self, item: ContextMenuItem) -> ContextMenu {
        self.items.push(item);
        self
    }

    pub(crate) fn push(&mut self, item: ContextMenuItem) {
        self.items.push(item);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Clone, Copy)]
struct OpenItem {
    source: WidgetId,
    action: ContextMenuAction,
    /// A slider row: picking it does nothing, and the menu stays open while it moves.
    slider: bool,
}

#[derive(Clone)]
struct OpenMenu {
    /// The widget whose menu this is.
    source: WidgetId,
    root: WidgetId,
    parent: WidgetId,
    /// The nodes of the rows that act (commands and sliders), top to bottom.
    items: Vec<WidgetId>,
    /// The kind of every row, the painted ones included: a rebuilt source whose menu has
    /// the same kinds shows it in these rows.
    kinds: Vec<ItemKind>,
    /// The source's box in window coordinates when the menu opened, framed while it is
    /// open. `None` for a text field.
    opened_from: Option<Rect>,
    /// Where it was opened and whether Paste was offered, so a rebuilt tree
    /// can open the same menu on the source's counterpart.
    anchor: Rect,
    can_paste: bool,
    /// The control that had focus when the menu opened from the keyboard.
    /// Closing the menu from the keyboard gives focus back to it.
    opener: Option<WidgetId>,
}

#[derive(Default)]
struct ContextRuntime {
    menus: SecondaryMap<WidgetId, ContextMenu>,
    actions: SecondaryMap<WidgetId, OpenItem>,
    triggers: SecondaryMap<WidgetId, WidgetId>,
    open: Option<OpenMenu>,
}

#[derive(Clone, Default)]
pub(crate) struct Runtime(Rc<RefCell<ContextRuntime>>);

impl Runtime {
    fn with<R>(&self, access: impl FnOnce(&RefCell<ContextRuntime>) -> R) -> R {
        access(&self.0)
    }
}

pub(crate) fn reset(runtime: &crate::widgets::Runtime) {
    runtime
        .context_menu
        .with(|state| *state.borrow_mut() = ContextRuntime::default());
}

pub(crate) fn purge_nodes(runtime: &crate::widgets::Runtime, nodes: &[WidgetId]) {
    runtime.context_menu.with(|state| {
        let mut state = state.borrow_mut();
        for &id in nodes {
            state.menus.remove(id);
            state.actions.remove(id);
            state.triggers.remove(id);
        }
        // Remove trigger entries pointing at a removed source.
        state.triggers.retain(|_, source| !nodes.contains(source));
        if state.open.as_ref().is_some_and(|open| {
            nodes.contains(&open.root)
                || nodes.contains(&open.parent)
                || open.items.iter().any(|id| nodes.contains(id))
        }) {
            state.open = None;
        }
    });
}

pub(crate) fn register_context_menu(
    runtime: &crate::widgets::Runtime,
    id: WidgetId,
    menu: ContextMenu,
) {
    runtime.context_menu.with(|state| {
        state.borrow_mut().menus.insert(id, menu);
    });
}

pub(crate) fn register_context_menu_trigger(
    runtime: &crate::widgets::Runtime,
    trigger: WidgetId,
    source: WidgetId,
) {
    runtime.context_menu.with(|state| {
        state.borrow_mut().triggers.insert(trigger, source);
    });
}

pub fn context_menu_trigger_source(
    runtime: &crate::widgets::Runtime,
    trigger: WidgetId,
) -> Option<WidgetId> {
    runtime
        .context_menu
        .with(|state| state.borrow().triggers.get(trigger).copied())
}

/// Resolves the nearest configured context-menu source at or above `id`.
///
/// Walking ancestors lets composed controls keep their menu on the semantic
/// owner while pointer hit-testing still resolves a nested visual leaf.
pub fn context_menu_source(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    id: WidgetId,
) -> Option<WidgetId> {
    let mut current = Some(id);
    while let Some(candidate) = current {
        if runtime.context_menu.with(|state| {
            state
                .borrow()
                .menus
                .get(candidate)
                .is_some_and(|menu| !menu.is_empty())
        }) {
            return Some(candidate);
        }
        current = scene.node(candidate).and_then(|node| node.parent);
    }
    None
}

/// Information returned when a transient menu item is activated. Native hosts
/// perform clipboard commands; custom callbacks have already fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextMenuActivation {
    pub source: WidgetId,
    pub action: ContextMenuAction,
}

struct RenderItem {
    label: String,
    enabled: bool,
    selected: bool,
    switch: bool,
    danger: bool,
    kind: ItemKind,
    action: ContextMenuAction,
    custom_index: Option<usize>,
    range: SliderRange,
    shaped: ShapedText,
    /// A slider's words at the ends of its track, shaped in the headings' size.
    ends: Option<(ShapedText, ShapedText)>,
}

fn action_enabled(
    action: ContextMenuAction,
    configured: bool,
    has_selection: bool,
    can_paste: bool,
    has_text: bool,
    has_callback: bool,
) -> bool {
    configured
        && match action {
            ContextMenuAction::Cut | ContextMenuAction::Copy => has_selection,
            ContextMenuAction::Paste => can_paste,
            ContextMenuAction::SelectAll => has_text,
            ContextMenuAction::Custom => has_callback,
        }
}

/// Where a menu of `size` goes for `anchor`: under or over it, see [`menu_top`]. A menu that
/// fits on neither side of a control and would cover it opens beside the control instead,
/// right of it or else left, so the control it hangs from stays in sight.
#[cfg(test)]
fn menu_origin(anchor: Rect, size: (f32, f32), viewport: Size, max_x: f32) -> (f32, f32) {
    menu_origin_toward(anchor, size, viewport, max_x, (false, false))
}

/// [`menu_origin`], over the anchor first when `above`, with a gap under it when `spaced`,
/// see [`menu_top_toward`].
fn menu_origin_toward(
    anchor: Rect,
    (width, height): (f32, f32),
    viewport: Size,
    max_x: f32,
    (above, spaced): (bool, bool),
) -> (f32, f32) {
    // A control in the window's right half lines the menu's right edge up with its own, so
    // the menu hangs over the column the control stands in and covers the same control of
    // the rows under it whole, not cut down the middle.
    let start = if anchor.width > 0.0 && anchor.x + anchor.width * 0.5 > viewport.width * 0.5 {
        anchor.right() - width
    } else {
        anchor.x
    };
    let x = start.clamp(CONTEXT_MENU_SIDE_MARGIN, max_x);
    let y = menu_top_toward(anchor, height, viewport.height, (above, spaced));
    let covers = anchor.height > 0.0 && y < anchor.bottom() && y + height > anchor.y;
    if !covers {
        return (x, y);
    }
    let right = anchor.right() + CONTEXT_MENU_MARGIN;
    let left = anchor.x - CONTEXT_MENU_MARGIN - width;
    let side = if right + width + CONTEXT_MENU_SIDE_MARGIN <= viewport.width {
        Some(right)
    } else if left >= CONTEXT_MENU_SIDE_MARGIN {
        Some(left)
    } else {
        None
    };
    match side {
        Some(side) => {
            // Its top beside the control's top, as far as the window lets it.
            let edge = CONTEXT_MENU_SIDE_MARGIN.min((viewport.height - height).max(0.0) / 2.0);
            let max_y = (viewport.height - height - edge).max(edge);
            (side, anchor.y.clamp(edge, max_y))
        }
        None => (x, y),
    }
}

/// The top edge of a menu `height` tall that hangs from `anchor` in a window
/// `window_height` tall. Below the anchor when it fits, else above it with the
/// usual gap (none for a pointer point), else on the side with more room,
/// clamped into the window and as near the anchor as it can be.
#[cfg(test)]
fn menu_top(anchor: Rect, height: f32, window_height: f32) -> f32 {
    menu_top_toward(anchor, height, window_height, (false, false))
}

/// [`menu_top`], but with `above` the menu hangs over its anchor whenever it fits there,
/// and goes under it only when it does not. With `spaced` a menu under its anchor keeps the
/// usual gap from it too.
fn menu_top_toward(
    anchor: Rect,
    height: f32,
    window_height: f32,
    (above, spaced): (bool, bool),
) -> f32 {
    if above && anchor.height > 0.0 {
        let edge = CONTEXT_MENU_SIDE_MARGIN.min((window_height - height).max(0.0) / 2.0);
        let over = anchor.y - CONTEXT_MENU_MARGIN - height;
        if over >= edge {
            return over;
        }
    }
    // The window's top and bottom keep the same air as its sides, so a menu never ends
    // against an edge with its shadow cut.
    let edge = CONTEXT_MENU_SIDE_MARGIN.min((window_height - height).max(0.0) / 2.0);
    let max_y = (window_height - height - edge).max(edge);
    let below = if spaced && anchor.height > 0.0 {
        anchor.bottom() + CONTEXT_MENU_MARGIN
    } else {
        anchor.bottom()
    };
    if below <= max_y {
        return below.max(edge);
    }
    let gap = if anchor.height > 0.0 {
        CONTEXT_MENU_MARGIN
    } else {
        0.0
    };
    let above = anchor.y - gap - height;
    if above >= edge {
        // Hanging over its anchor, even when that anchor sits in the edge's air itself.
        return above.min((window_height - height - CONTEXT_MENU_MARGIN).max(edge));
    }
    let room_below = window_height - edge - below;
    let room_above = anchor.y - gap - edge;
    let preferred = if room_above > room_below {
        above
    } else {
        below
    };
    preferred.clamp(edge, max_y)
}

/// What a menu shows, read from its source's configuration when it opens and again when the
/// source is rebuilt under it.
struct Contents {
    items: Vec<RenderItem>,
    /// Text size of the commands, in logical px.
    size: f32,
    /// The least height of a row, in logical px.
    least_row: f32,
    /// Over the anchor first, see [`ContextMenu::above`].
    above: bool,
    /// A gap under the anchor too, see [`ContextMenu::spaced`].
    spaced: bool,
    /// The least width, see [`ContextMenu::min_width`].
    least_width: f32,
    editable: bool,
}

/// The rows of `source`'s menu, shaped, with what each may do now. `None` when it has none.
fn contents(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
    shaper: &mut TextShaper,
    source: WidgetId,
    scale: f32,
    can_paste: bool,
) -> Option<Contents> {
    let kind = scene.node(source)?.kind;
    let editable = matches!(kind, WidgetKind::TextInput | WidgetKind::TextArea);
    let has_selection = editable && crate::widgets::selected_text(runtime, scene, source).is_some();
    let has_text = editable
        && scene
            .a11y(source)
            .and_then(|semantics| semantics.value.as_deref())
            .is_some_and(|value| !value.is_empty());
    let (size, least_row, above, spaced, least_width) = runtime.context_menu.with(|state| {
        let state = state.borrow();
        let menu = state.menus.get(source);
        (
            menu.and_then(|menu| menu.text_size)
                .unwrap_or(BUTTON_TEXT_SIZE),
            menu.and_then(|menu| menu.row_height).unwrap_or(0.0),
            menu.is_some_and(|menu| menu.above),
            menu.is_some_and(|menu| menu.spaced),
            menu.and_then(|menu| menu.min_width).unwrap_or(0.0),
        )
    });
    let phys = phys_size_px(size, scale);
    let items = runtime.context_menu.with(|state| {
        let state = state.borrow();
        let menu = state.menus.get(source)?;
        if menu.is_empty() {
            return None;
        }
        Some(
            menu.items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let (action, custom_index, has_callback) = match &item.action {
                        ItemAction::BuiltIn(action) => (*action, None, true),
                        ItemAction::Custom(callback) => {
                            (ContextMenuAction::Custom, Some(index), callback.is_some())
                        }
                        ItemAction::Slider(callback) => {
                            (ContextMenuAction::Custom, Some(index), callback.is_some())
                        }
                    };
                    let label = item.label.to_string();
                    // A heading is a caption over its group: a size smaller than the
                    // commands, so it does not read as one of them.
                    let item_phys = if item.kind == ItemKind::Heading {
                        phys_size_px(heading_size(size), scale)
                    } else {
                        phys
                    };
                    let small = phys_size_px(heading_size(size), scale);
                    let ends = item.ends.as_ref().map(|(least, most)| {
                        (
                            shaper.shape(least, small, None),
                            shaper.shape(most, small, None),
                        )
                    });
                    RenderItem {
                        ends,
                        shaped: shaper.shape(&label, item_phys, None),
                        label,
                        enabled: action_enabled(
                            action,
                            item.enabled,
                            has_selection,
                            editable && can_paste,
                            has_text,
                            has_callback,
                        ),
                        selected: item.selected,
                        switch: item.switch,
                        danger: item.danger,
                        kind: item.kind,
                        action,
                        custom_index,
                        range: item.range,
                    }
                })
                .collect::<Vec<_>>(),
        )
    })?;
    Some(Contents {
        items,
        size,
        least_row,
        above,
        spaced,
        least_width,
        editable,
    })
}

/// The measures of an open menu: its box in window coordinates and the room of its rows.
#[derive(Clone, Copy)]
struct Measures {
    rect: Rect,
    /// One over the scale's norm: shaped sizes are physical, rows logical.
    inv: f32,
    inset: f32,
    pad_h: f32,
    row_height: f32,
    separator_height: f32,
}

impl Measures {
    /// How tall `item`'s row is.
    fn height(&self, item: &RenderItem) -> f32 {
        match item.kind {
            ItemKind::Separator => self.separator_height,
            ItemKind::Command | ItemKind::Heading => self.row_height,
            ItemKind::Slider => self.row_height + self.ends_height(item),
        }
    }

    /// How tall the line of a slider's words at its ends is; none without them.
    fn ends_height(&self, item: &RenderItem) -> f32 {
        item.ends.as_ref().map_or(0.0, |(least, most)| {
            least.height.max(most.height) * self.inv
        })
    }
}

/// Where a menu with `contents` goes for `anchor` and how its rows are measured.
fn measure(
    theme: Theme,
    contents: &Contents,
    anchor: Rect,
    viewport: Size,
    scale: f32,
) -> Measures {
    let inv = 1.0 / norm_scale(scale);
    let items = &contents.items;
    let pad_h = theme.shape.pad(PAD_H + 4.0);
    let pad_v = theme.shape.pad(MENU_PAD_V);
    let row_height = items
        .iter()
        .map(|item| item.shaped.height * inv + 2.0 * pad_v)
        .fold(contents.least_row, f32::max);
    // A menu with a chosen row keeps room for the check mark at every row's end, and one
    // with a switch for the switch, on or off.
    let check_room = if items.iter().any(|item| item.switch) {
        SWITCH_ROOM
    } else if items.iter().any(|item| item.selected) {
        CHECK_ROOM
    } else {
        0.0
    };
    // A slider draws no words: it takes the width the others leave it.
    let desired_width = items
        .iter()
        .filter(|item| item.kind != ItemKind::Slider)
        .map(|item| item.shaped.width * inv + 2.0 * pad_h + check_room)
        .fold(CONTEXT_MENU_MIN_WIDTH * theme.shape.density, f32::max)
        .max(contents.least_width - 2.0 * theme.shape.pad(MENU_INSET));
    let inset = theme.shape.pad(MENU_INSET);
    // A menu that hangs under a row is as wide as the row: a narrower one left the ends of
    // the rows it covers showing beside it (a time, a badge), which read as the menu's own.
    let row_wide = anchor.height > 0.0 && anchor.width <= viewport.width * ROW_ANCHOR_SHARE;
    let width = (desired_width + 2.0 * inset)
        .max(if row_wide { anchor.width } else { 0.0 })
        .min((viewport.width - 2.0 * CONTEXT_MENU_SIDE_MARGIN).max(1.0));
    let separator_height = theme.shape.pad(SEPARATOR_HEIGHT);
    let mut measures = Measures {
        rect: Rect::default(),
        inv,
        inset,
        pad_h,
        row_height,
        separator_height,
    };
    let height = items.iter().map(|item| measures.height(item)).sum::<f32>() + 2.0 * inset;
    let max_x = (viewport.width - width - CONTEXT_MENU_SIDE_MARGIN).max(CONTEXT_MENU_SIDE_MARGIN);
    let (x, y) = menu_origin_toward(
        anchor,
        (width, height),
        viewport,
        max_x,
        (contents.above, contents.spaced),
    );
    measures.rect = Rect::new(x, y, width, height);
    measures
}

/// Opens `source`'s configured menu hanging from `anchor`, a logical window
/// rect: the field it belongs to, or a zero-size rect at the pointer. The menu
/// opens below the anchor when it fits, above it when it does not, and is
/// clamped into the window only when neither side has room.
#[allow(clippy::too_many_arguments)]
pub fn open_context_menu(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    source: WidgetId,
    anchor: Rect,
    viewport: Size,
    scale: f32,
    can_paste: bool,
) -> bool {
    let _ = dismiss_context_menu(runtime, scene);
    if scene.node(source).is_none() || context_menu_source(runtime, scene, source) != Some(source) {
        return false;
    }
    let Some(contents) = contents(runtime, scene, shaper, source, scale, can_paste) else {
        return false;
    };

    let theme = theme_for(runtime, source);
    // Inside a modal the menu hangs off the dialog panel: the panel is the
    // accessibility root and the focus boundary while it is open, so a menu
    // beside it would be invisible to screen readers and the keyboard.
    let parent = crate::widgets::dialog::active_modal_panel_in(runtime, scene)
        .filter(|panel| crate::widgets::dialog::is_in_subtree(scene, source, *panel))
        .or_else(|| scene.root());
    let Some(parent) = parent else {
        return false;
    };
    let root = scene.insert(WidgetKind::Column, Some(parent));
    remember_node_theme(runtime, root, theme);
    scene.set_overlay_level(root, crate::widgets::POPUP_OVERLAY_LEVEL);
    // The menu writes its own boxes below. Without this flag a later relayout
    // of `parent` would pull the menu into its flex flow, at the parent's
    // top-left, and slide the panel paint there while the items stay put.
    scene.set_placed_by_hand(root);
    let named = runtime.context_menu.with(|state| {
        state
            .borrow()
            .menus
            .get(source)
            .and_then(|menu| menu.name.as_ref().map(|name| name.to_string()))
    });
    let menu_name = if let Some(name) = named {
        name
    } else if contents.editable {
        "Text editing".to_string()
    } else {
        scene
            .a11y(source)
            .and_then(|source| source.name.as_deref())
            .map(|name| format!("{name} menu"))
            .unwrap_or_else(|| "Context menu".to_string())
    };
    {
        let semantics = scene.a11y_mut(root);
        semantics.role = Role::Menu.as_u16();
        semantics.name = Some(menu_name);
    }

    // The rows that act get nodes of their own. Their boxes, words and paint come with the
    // menu's below, and again when the source is rebuilt while the menu is open.
    let mut item_ids = Vec::new();
    for (index, item) in contents.items.iter().enumerate() {
        let slider = match item.kind {
            ItemKind::Separator | ItemKind::Heading => continue,
            ItemKind::Command => false,
            ItemKind::Slider => true,
        };
        let kind = if slider {
            WidgetKind::Slider
        } else {
            WidgetKind::Button
        };
        let id = scene.insert(kind, Some(root));
        remember_node_theme(runtime, id, theme);
        if slider {
            runtime.with(|rt| {
                rt.borrow_mut().sliders.insert(id, item.range.state());
            });
            let change_runtime = runtime.clone();
            crate::widgets::with_handlers(runtime, id, |handlers| {
                handlers.change = Some(Box::new(move |value| {
                    fire_slider(&change_runtime, index, value);
                }));
            });
        } else {
            // Rows sit flush against each other and the menu's frame, so the
            // keyboard ring stays inside the row instead of past the frame.
            crate::widgets::with_handlers(runtime, id, |handlers| handlers.flush_row = true);
            if item.custom_index.is_some() {
                let callback_runtime = runtime.clone();
                crate::widgets::with_handlers(runtime, id, |handlers| {
                    handlers.click = Some(Box::new(move || {
                        fire_custom(&callback_runtime, index);
                    }));
                });
            }
        }
        runtime.context_menu.with(|state| {
            state.borrow_mut().actions.insert(
                id,
                OpenItem {
                    source,
                    action: item.action,
                    slider,
                },
            );
        });
        item_ids.push(id);
    }

    // The control or row the menu belongs to keeps a frame while the menu is open, so the
    // menu reads as its own and not as the neighbour's it covers. A text field keeps its
    // look: its caret and selection already say where the menu acts.
    let opened_from = (!contents.editable).then(|| scene.composited_rect(source));
    let open = OpenMenu {
        source,
        root,
        parent,
        items: item_ids,
        kinds: contents.items.iter().map(|item| item.kind).collect(),
        opened_from,
        anchor,
        can_paste,
        opener: None,
    };
    lay_out_menu(
        runtime, scene, shaper, atlas, &open, &contents, viewport, scale,
    );
    scene.mark_dirty(parent, DirtyFlags::A11Y);
    // The menu answers the hover that opened it: the control's tooltip would
    // be drawn over the open menu.
    let _ = crate::widgets::hide_hover_tooltip(runtime, scene, source);
    runtime.context_menu.with(|state| {
        state.borrow_mut().open = Some(open);
    });
    true
}

/// Places the menu `open` and its rows and paints them for `contents`: on opening, and again
/// on the same nodes when its source is rebuilt.
#[allow(clippy::too_many_arguments)]
fn lay_out_menu(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    open: &OpenMenu,
    contents: &Contents,
    viewport: Size,
    scale: f32,
) {
    let root = open.root;
    let theme = theme_for(runtime, root);
    let size = contents.size;
    let phys = phys_size_px(size, scale);
    let inv = 1.0 / norm_scale(scale);
    let measures = measure(theme, contents, open.anchor, viewport, scale);
    // `measures.rect` is in window coordinates, but layout boxes are in the
    // parent's content space: a scroll view as the parent moves its children,
    // and the menu with them. Place the menu so it is drawn at `measures.rect`.
    let (inherited, _) = scene.overlay_inherited_context(root);
    let into = |at: Rect| Rect::new(at.x - inherited.x, at.y - inherited.y, at.width, at.height);
    let menu_rect = into(measures.rect);
    let (x, y, width) = (menu_rect.x, menu_rect.y, menu_rect.width);
    let inset = measures.inset;
    let pad_h = measures.pad_h;
    let row_height = measures.row_height;
    let separator_height = measures.separator_height;
    // A row taller than its text keeps the text in its middle.
    let text_top = |item: &RenderItem| (row_height - item.shaped.height * inv) * 0.5;
    scene.set_layout(
        root,
        LayoutBox {
            rect: menu_rect,
            content: menu_rect,
        },
    );
    {
        let radius = theme.shape.radius(4.0, menu_rect.height);
        let frame = theme.shape.frame.max(1.0);
        let paint = scene.paint_mut(root);
        paint.primitives.clear();
        // A soft shadow under the menu, in layers that widen and fade, so
        // it lifts off whatever it covers, a terminal's text included.
        for (grow, alpha) in MENU_SHADOW {
            paint.primitives.push(Primitive::SolidRect {
                rect: Rect::new(
                    menu_rect.x - grow,
                    menu_rect.y - grow + MENU_SHADOW_DROP,
                    menu_rect.width + 2.0 * grow,
                    menu_rect.height + 2.0 * grow,
                ),
                color: Color::rgba(0, 0, 0, alpha),
                corner_radius: radius + grow,
            });
        }
        if let Some(at) = open
            .opened_from
            .map(into)
            .filter(|at| at.width > 0.0 && at.height > 0.0)
        {
            // A frame between the selection colour and the accent: the menu paints above
            // the source, so a fill would pale the source's own words and picture. The
            // selection colour alone was too faint on a page to read as a state.
            let radius = theme.shape.radius(4.0, at.height);
            paint.primitives.push(Primitive::StrokeRect {
                rect: at,
                color: crate::widgets::theme::mix_color(
                    theme.selection,
                    theme.accent,
                    OPENED_FROM_MIX,
                ),
                corner_radius: radius,
                width: OPENED_FROM_FRAME,
            });
        }
        // The menu stands on the surface of a raised panel, with a frame
        // that follows its rounded corners.
        paint.primitives.push(Primitive::SolidRect {
            rect: menu_rect,
            color: theme.surface,
            corner_radius: radius,
        });
        paint.primitives.push(Primitive::StrokeRect {
            rect: menu_rect,
            color: crate::widgets::theme::mix_color(theme.separator, theme.outline, 0.5),
            corner_radius: radius,
            width: frame,
        });
    }
    scene.mark_dirty(root, DirtyFlags::PAINT);
    scene.mark_dirty(root, DirtyFlags::A11Y);

    let mut rows = open.items.iter().copied();
    let mut top = y + inset;
    for item in &contents.items {
        let row_top = top;
        top += measures.height(item);
        match item.kind {
            ItemKind::Command | ItemKind::Slider => {}
            ItemKind::Separator => {
                let line_y = (row_top + separator_height * 0.5).round();
                scene.paint_mut(root).primitives.push(Primitive::SolidRect {
                    rect: Rect::new(x + inset, line_y, width - 2.0 * inset, 1.0),
                    color: theme.separator,
                    corner_radius: 0.0,
                });
                continue;
            }
            ItemKind::Heading => {
                rasterize_and_push(
                    scene.paint_mut(root),
                    shaper,
                    atlas,
                    &item.shaped,
                    phys_size_px(heading_size(size), scale) as u32,
                    theme.text_muted,
                    scale,
                    Point {
                        x: x + inset + pad_h,
                        y: row_top + text_top(item),
                    },
                );
                continue;
            }
        }
        let Some(id) = rows.next() else {
            break;
        };
        let rect = Rect::new(x + inset, row_top, width - 2.0 * inset, row_height);
        if item.kind == ItemKind::Slider {
            // The track runs between the words' margins of the rows around it.
            let rect = Rect::new(
                rect.x + pad_h,
                rect.y,
                rect.width - 2.0 * pad_h,
                rect.height,
            );
            place_slider(runtime, scene, id, item, rect);
            // The words at the ends under the track, its start and its end on theirs.
            if let Some((least, most)) = &item.ends {
                let small = phys_size_px(heading_size(size), scale) as u32;
                let under = row_top + row_height - SLIDER_ENDS_RISE;
                for (shaped, left) in [
                    (least, rect.x),
                    (most, rect.x + rect.width - most.width * inv),
                ] {
                    rasterize_and_push(
                        scene.paint_mut(root),
                        shaper,
                        atlas,
                        shaped,
                        small,
                        theme.text_muted,
                        scale,
                        Point { x: left, y: under },
                    );
                }
                scene.mark_dirty(root, DirtyFlags::PAINT);
            }
            continue;
        }
        scene.set_layout(
            id,
            LayoutBox {
                rect,
                content: rect,
            },
        );
        {
            let semantics = scene.a11y_mut(id);
            semantics.role = Role::MenuItem.as_u16();
            semantics.name = Some(item.label.clone());
            semantics.state = 0;
            if item.enabled {
                // Focus lets the keyboard walk the rows.
                semantics.actions = ActionFlags::CLICK.0 | ActionFlags::FOCUS.0;
            } else {
                semantics.actions = 0;
                semantics.state = StateFlags::DISABLED.0;
            }
            if item.selected {
                semantics.state |= StateFlags::SELECTED.0;
            }
        }
        {
            let row_radius = theme.shape.radius(3.0, row_height);
            let paint = scene.paint_mut(id);
            paint.primitives.clear();
            // The row's ground: clear, so hover and the keyboard's row draw
            // their wash on it.
            paint.primitives.push(Primitive::SolidRect {
                rect,
                color: Color::TRANSPARENT,
                corner_radius: row_radius,
            });
            // The chosen row has a check mark in the accent at its end, as a
            // menu of choices marks the current one; a fill would read as the
            // keyboard's row.
            if item.switch {
                // A switch: a rounded track in the accent while on and in the muted text
                // while off, with a round knob at the end it is set to.
                let track = Rect::new(
                    rect.x + rect.width - pad_h - SWITCH_WIDTH,
                    rect.y + (rect.height - SWITCH_HEIGHT) * 0.5,
                    SWITCH_WIDTH,
                    SWITCH_HEIGHT,
                );
                paint.primitives.push(Primitive::SolidRect {
                    rect: track,
                    color: if item.selected {
                        theme.accent
                    } else {
                        theme.text_muted
                    },
                    corner_radius: SWITCH_HEIGHT * 0.5,
                });
                let knob = SWITCH_HEIGHT - 2.0 * SWITCH_KNOB_INSET;
                let knob_x = if item.selected {
                    track.x + track.width - SWITCH_KNOB_INSET - knob
                } else {
                    track.x + SWITCH_KNOB_INSET
                };
                paint.primitives.push(Primitive::SolidRect {
                    rect: Rect::new(knob_x, track.y + SWITCH_KNOB_INSET, knob, knob),
                    color: theme.surface,
                    corner_radius: knob * 0.5,
                });
            } else if item.selected {
                let cx = rect.x + rect.width - pad_h - CHECK_SIZE * 0.5;
                let cy = rect.y + rect.height * 0.5;
                let h = CHECK_SIZE * 0.5;
                let elbow = Point {
                    x: cx - h * 0.25,
                    y: cy + h * 0.55,
                };
                for (from, to) in [
                    (Point { x: cx - h, y: cy }, elbow),
                    (
                        elbow,
                        Point {
                            x: cx + h,
                            y: cy - h * 0.7,
                        },
                    ),
                ] {
                    paint.primitives.push(Primitive::Line {
                        from,
                        to,
                        width: CHECK_STROKE,
                        color: theme.accent,
                    });
                }
            }
            rasterize_and_push(
                paint,
                shaper,
                atlas,
                &item.shaped,
                phys as u32,
                if !item.enabled {
                    theme.text_muted
                } else if item.danger {
                    theme.attention
                } else {
                    theme.text
                },
                scale,
                Point {
                    x: rect.x + pad_h,
                    y: rect.y + text_top(item),
                },
            );
        }
        scene.mark_dirty(id, DirtyFlags::PAINT);
        scene.mark_dirty(id, DirtyFlags::A11Y);
    }
}

/// Lays the slider row `id` into `rect` at `item`'s value and range, and names it. Its
/// handler does not run: the program, not the user, set the value.
fn place_slider(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
    item: &RenderItem,
    rect: Rect,
) {
    scene.set_layout(
        id,
        LayoutBox {
            rect,
            content: rect,
        },
    );
    let state = item.range.state();
    let frac = if state.max > state.min {
        ((state.value - state.min) / (state.max - state.min)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let value = state.value;
    runtime.with(|rt| {
        rt.borrow_mut().sliders.insert(id, state);
    });
    {
        let semantics = scene.a11y_mut(id);
        semantics.role = Role::Slider.as_u16();
        semantics.name = Some(item.label.clone());
        semantics.value = Some(format_slider_value(value));
        if item.enabled {
            semantics.state &= !StateFlags::DISABLED.0;
            semantics.actions = ActionFlags::SET_VALUE.0
                | ActionFlags::INCREMENT.0
                | ActionFlags::DECREMENT.0
                | ActionFlags::FOCUS.0;
        } else {
            semantics.state |= StateFlags::DISABLED.0;
            semantics.actions = 0;
        }
    }
    emit_slider_paint(runtime, scene, id, frac, !item.enabled);
    scene.mark_dirty(id, DirtyFlags::PAINT);
    scene.mark_dirty(id, DirtyFlags::A11Y);
}

/// The open menu's source and its root node, if a menu is open.
pub fn open_context_menu_nodes(runtime: &crate::widgets::Runtime) -> Option<(WidgetId, WidgetId)> {
    runtime.context_menu.with(|state| {
        state
            .borrow()
            .open
            .as_ref()
            .map(|open| (open.source, open.root))
    })
}

/// Moves the open menu onto `source`, the counterpart of the control it opened from in a
/// rebuilt branch, and shows `source`'s menu in the rows already open. The rows keep their
/// nodes, so the keyboard's row, hover and a slider's drag stay where they are. `false`
/// when no menu is open or `source`'s menu has other kinds of rows; the open menu is then
/// left as it was, for the caller to close.
#[allow(clippy::too_many_arguments)]
pub fn retarget_context_menu(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    source: WidgetId,
    viewport: Size,
    scale: f32,
) -> bool {
    let Some(mut open) = runtime
        .context_menu
        .with(|state| state.borrow().open.clone())
    else {
        return false;
    };
    if context_menu_source(runtime, scene, source) != Some(source) {
        return false;
    }
    let Some(contents) = contents(runtime, scene, shaper, source, scale, open.can_paste) else {
        return false;
    };
    if !contents
        .items
        .iter()
        .map(|item| item.kind)
        .eq(open.kinds.iter().copied())
    {
        return false;
    }
    open.source = source;
    let lifted = crate::widgets::lift_decorations(runtime, scene, open.root);
    lay_out_menu(
        runtime, scene, shaper, atlas, &open, &contents, viewport, scale,
    );
    crate::widgets::restore_decorations(runtime, scene, lifted);
    runtime.context_menu.with(|state| {
        let mut state = state.borrow_mut();
        for &id in &open.items {
            if let Some(item) = state.actions.get_mut(id) {
                item.source = source;
            }
        }
        state.open = Some(open);
    });
    true
}

/// Runs the callback of the open menu's row `index`, a command.
fn fire_custom(runtime: &crate::widgets::Runtime, index: usize) {
    let Some(source) = open_context_menu_nodes(runtime).map(|(source, _)| source) else {
        return;
    };
    let callback = runtime.context_menu.with(|state| {
        let mut state = state.borrow_mut();
        let item = state.menus.get_mut(source)?.items.get_mut(index)?;
        match &mut item.action {
            ItemAction::Custom(callback) => callback.take(),
            ItemAction::BuiltIn(_) | ItemAction::Slider(_) => None,
        }
    });
    if let Some(mut callback) = callback {
        callback();
        runtime.context_menu.with(|state| {
            let mut state = state.borrow_mut();
            if let Some(ItemAction::Custom(slot)) = state
                .menus
                .get_mut(source)
                .and_then(|menu| menu.items.get_mut(index))
                .map(|item| &mut item.action)
            {
                *slot = Some(callback);
            }
        });
    }
}

/// Runs the callback of the open menu's row `index`, a slider, with its new value.
fn fire_slider(runtime: &crate::widgets::Runtime, index: usize, value: f32) {
    let Some(source) = open_context_menu_nodes(runtime).map(|(source, _)| source) else {
        return;
    };
    let callback = runtime.context_menu.with(|state| {
        let mut state = state.borrow_mut();
        let item = state.menus.get_mut(source)?.items.get_mut(index)?;
        match &mut item.action {
            ItemAction::Slider(callback) => callback.take(),
            ItemAction::BuiltIn(_) | ItemAction::Custom(_) => None,
        }
    });
    if let Some(mut callback) = callback {
        callback(value);
        runtime.context_menu.with(|state| {
            let mut state = state.borrow_mut();
            if let Some(ItemAction::Slider(slot)) = state
                .menus
                .get_mut(source)
                .and_then(|menu| menu.items.get_mut(index))
                .map(|item| &mut item.action)
            {
                *slot = Some(callback);
            }
        });
    }
}

/// Whether `id`'s own menu is the one open.
pub(crate) fn context_menu_open_for(runtime: &crate::widgets::Runtime, id: WidgetId) -> bool {
    runtime.context_menu.with(|state| {
        state
            .borrow()
            .open
            .as_ref()
            .is_some_and(|open| open.source == id)
    })
}

/// The open menu's source, the rect it hangs from and whether it offered
/// Paste: what a rebuild needs to open it again on the source's counterpart.
pub(crate) fn open_context_menu_anchor(
    runtime: &crate::widgets::Runtime,
) -> Option<(WidgetId, Rect, bool)> {
    runtime.context_menu.with(|state| {
        state
            .borrow()
            .open
            .as_ref()
            .map(|open| (open.source, open.anchor, open.can_paste))
    })
}

pub fn context_menu_is_open(runtime: &crate::widgets::Runtime) -> bool {
    runtime
        .context_menu
        .with(|state| state.borrow().open.is_some())
}

pub fn context_menu_item(runtime: &crate::widgets::Runtime, scene: &Scene, id: WidgetId) -> bool {
    scene.node(id).is_some()
        && runtime
            .context_menu
            .with(|state| state.borrow().actions.contains_key(id))
}

/// Records the control a keyboard-opened menu returns focus to.
pub(crate) fn set_context_menu_opener(runtime: &crate::widgets::Runtime, opener: WidgetId) {
    runtime.context_menu.with(|state| {
        if let Some(open) = state.borrow_mut().open.as_mut() {
            open.opener = Some(opener);
        }
    });
}

/// The control a keyboard-opened menu returns focus to, if any.
pub(crate) fn context_menu_opener(runtime: &crate::widgets::Runtime) -> Option<WidgetId> {
    runtime
        .context_menu
        .with(|state| state.borrow().open.as_ref().and_then(|open| open.opener))
}

/// The enabled rows of the open menu, top to bottom: the stops of the
/// keyboard walk through it.
pub(crate) fn context_menu_enabled_rows(
    runtime: &crate::widgets::Runtime,
    scene: &Scene,
) -> Vec<WidgetId> {
    let items = runtime.context_menu.with(|state| {
        state
            .borrow()
            .open
            .as_ref()
            .map(|open| open.items.clone())
            .unwrap_or_default()
    });
    items
        .into_iter()
        .filter(|&id| {
            scene
                .a11y(id)
                .is_some_and(|a| !StateFlags(a.state).contains(StateFlags::DISABLED))
        })
        .collect()
}

/// Activates one open menu row and dismisses the popup.
pub fn activate_context_menu_item(
    runtime: &crate::widgets::Runtime,
    scene: &mut Scene,
    id: WidgetId,
) -> Option<ContextMenuActivation> {
    let item = runtime
        .context_menu
        .with(|state| state.borrow().actions.get(id).copied())?;
    let disabled = scene
        .a11y(id)
        .map(|semantics| StateFlags(semantics.state).contains(StateFlags::DISABLED))
        .unwrap_or(true);
    // A slider is set by moving it, not picked: the menu stays open.
    if disabled || item.slider {
        return None;
    }
    if item.action == ContextMenuAction::Custom {
        let _ = crate::widgets::dispatch_click(runtime, scene, id);
    }
    let activation = ContextMenuActivation {
        source: item.source,
        action: item.action,
    };
    let _ = dismiss_context_menu(runtime, scene);
    Some(activation)
}

/// Dismisses the active menu, if any.
pub fn dismiss_context_menu(runtime: &crate::widgets::Runtime, scene: &mut Scene) -> bool {
    let open = runtime
        .context_menu
        .with(|state| state.borrow_mut().open.take());
    let Some(open) = open else {
        return false;
    };
    let _ = crate::widgets::strip_hover_decoration(runtime, scene);
    // A slider held by the pointer lets go with the menu.
    let active = runtime.with(|rt| rt.borrow().active.as_ref().map(|state| state.target));
    if active.is_some_and(|target| open.items.contains(&target)) {
        let _ = crate::widgets::strip_active_decoration(runtime, scene);
    }
    scene.mark_dirty(open.root, DirtyFlags::PAINT);
    for &item in &open.items {
        // A row that holds keyboard focus loses it with its ring before it
        // goes, or the ring's record would outlive the node.
        let state = scene.a11y(item).map_or(0, |a| a.state);
        if StateFlags(state).contains(StateFlags::FOCUSED) {
            scene.set_a11y_state(item, state & !StateFlags::FOCUSED.0);
            let _ = crate::widgets::remove_focus_ring(runtime, scene, item);
        }
    }
    for item in open.items {
        runtime.context_menu.with(|state| {
            state.borrow_mut().actions.remove(item);
        });
        runtime.with(|state| {
            let mut state = state.borrow_mut();
            state.handlers.remove(item);
            state.sliders.remove(item);
        });
        forget_node_theme(runtime, item);
        let _ = scene.remove_subtree(item);
    }
    forget_node_theme(runtime, open.root);
    let _ = scene.remove_subtree(open.root);
    if scene.node(open.parent).is_some() {
        scene.mark_dirty(open.parent, DirtyFlags::A11Y);
    }
    true
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    #[test]
    fn it_hangs_below_when_it_fits() {
        assert_eq!(
            menu_top(Rect::new(0.0, 10.0, 50.0, 30.0), 100.0, 300.0),
            40.0
        );
    }

    #[test]
    fn a_menu_that_prefers_above_hangs_over_its_anchor_when_it_fits() {
        let anchor = Rect::new(0.0, 200.0, 50.0, 30.0);
        let y = menu_top_toward(anchor, 100.0, 600.0, (true, false));
        assert_eq!(y + 100.0, 200.0 - CONTEXT_MENU_MARGIN);
        // Without room over it, it hangs under it as any menu does.
        let anchor = Rect::new(0.0, 40.0, 50.0, 30.0);
        assert_eq!(menu_top_toward(anchor, 100.0, 600.0, (true, false)), 70.0);
        // The default stays under the anchor.
        let anchor = Rect::new(0.0, 200.0, 50.0, 30.0);
        assert_eq!(menu_top_toward(anchor, 100.0, 600.0, (false, false)), 230.0);
        // A spaced menu under its anchor keeps the usual gap from it.
        assert_eq!(
            menu_top_toward(anchor, 100.0, 600.0, (false, true)),
            230.0 + CONTEXT_MENU_MARGIN
        );
    }

    #[test]
    fn a_switch_row_is_a_selected_setting_and_a_menu_keeps_its_least_width() {
        let on = ContextMenuItem::new("Mute").switch(true);
        assert!(on.switch && on.selected);
        let off = ContextMenuItem::new("Mute").switch(false);
        assert!(off.switch && !off.selected);
        assert!(!ContextMenuItem::new("Mute").selected(true).switch);
        assert_eq!(ContextMenu::new().min_width(200.0).min_width, Some(200.0));
        assert_eq!(ContextMenu::new().min_width, None);
    }

    #[test]
    fn a_slider_keeps_the_words_at_its_ends() {
        let plain = ContextMenuItem::slider("Volume", 100.0, 10.0, 200.0);
        assert!(plain.ends.is_none());
        let ended = ContextMenuItem::slider("Volume", 100.0, 10.0, 200.0).ends("10%", "200%");
        assert_eq!(
            ended.ends.as_ref().map(|(a, b)| (a.as_ref(), b.as_ref())),
            Some(("10%", "200%"))
        );
    }

    #[test]
    fn it_flips_above_with_a_gap_when_below_is_short() {
        let y = menu_top(Rect::new(0.0, 200.0, 50.0, 30.0), 100.0, 300.0);
        assert_eq!(y + 100.0, 200.0 - CONTEXT_MENU_MARGIN);
    }

    #[test]
    fn with_no_room_on_either_side_it_takes_the_roomier_one() {
        // A 300 tall window and a 200 tall menu: no side fits it. It keeps the window
        // edge's air.
        assert_eq!(
            menu_top(Rect::new(0.0, 150.0, 50.0, 40.0), 200.0, 300.0),
            CONTEXT_MENU_SIDE_MARGIN
        );
        assert_eq!(
            menu_top(Rect::new(0.0, 100.0, 50.0, 40.0), 200.0, 300.0),
            300.0 - 200.0 - CONTEXT_MENU_SIDE_MARGIN
        );
    }

    #[test]
    fn a_menu_under_a_control_on_the_right_ends_where_the_control_does() {
        let viewport = Size {
            width: 1000.0,
            height: 800.0,
        };
        let anchor = Rect::new(900.0, 100.0, 28.0, 28.0);
        let (x, y) = menu_origin(anchor, (150.0, 100.0), viewport, 1000.0 - 150.0 - 8.0);
        assert_eq!((x, y), (928.0 - 150.0, 128.0));
        // On the left half it starts where the control does.
        let anchor = Rect::new(40.0, 100.0, 28.0, 28.0);
        let (x, _) = menu_origin(anchor, (150.0, 100.0), viewport, 1000.0 - 150.0 - 8.0);
        assert_eq!(x, 40.0);
    }
}

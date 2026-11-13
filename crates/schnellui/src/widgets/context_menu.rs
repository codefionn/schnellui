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
    forget_node_theme, norm_scale, phys_size_px, rasterize_and_push, remember_node_theme,
    theme_for, ClickHandler, BUTTON_TEXT_SIZE, PAD_H,
};

const CONTEXT_MENU_MIN_WIDTH: f32 = 140.0;
const CONTEXT_MENU_MARGIN: f32 = 4.0;
/// Vertical padding of a menu row, each side: menus stay denser than buttons.
const MENU_PAD_V: f32 = 5.0;

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
}

/// The layers of a menu's shadow: how far each reaches past the menu and
/// its alpha. Wider layers are fainter.
const MENU_SHADOW: [(f32, u8); 3] = [(8.0, 0x18), (4.0, 0x28), (1.0, 0x40)];
/// How far the shadow sits below the menu, as if lit from above.
const MENU_SHADOW_DROP: f32 = 3.0;
/// Width of the accent bar that marks a menu's chosen row.
const SELECTED_BAR: f32 = 3.0;

/// One row in a context menu.
pub struct ContextMenuItem {
    label: Cow<'static, str>,
    enabled: bool,
    selected: bool,
    action: ItemAction,
}

impl ContextMenuItem {
    /// Creates a custom command. Attach its callback with
    /// [`ContextMenuItem::on_select`].
    pub fn new(label: impl Into<Cow<'static, str>>) -> ContextMenuItem {
        ContextMenuItem {
            label: label.into(),
            enabled: true,
            selected: false,
            action: ItemAction::Custom(None),
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
            action: ItemAction::BuiltIn(action),
        }
    }

    /// Enables or disables this command independently of its dynamic clipboard
    /// state.
    pub fn enabled(mut self, enabled: bool) -> ContextMenuItem {
        self.enabled = enabled;
        self
    }

    /// Marks this item as the current one of a menu that picks among
    /// choices, such as the shown tab in a list of tabs: it gets the theme's
    /// selection fill and the selected state.
    pub fn selected(mut self, selected: bool) -> ContextMenuItem {
        self.selected = selected;
        self
    }

    /// Sets the callback for a custom command.
    pub fn on_select(mut self, callback: impl FnMut() + 'static) -> ContextMenuItem {
        self.action = ItemAction::Custom(Some(Box::new(callback)));
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
}

#[derive(Clone)]
struct OpenMenu {
    /// The widget whose menu this is.
    source: WidgetId,
    root: WidgetId,
    parent: WidgetId,
    items: Vec<WidgetId>,
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
    action: ContextMenuAction,
    custom_index: Option<usize>,
    shaped: ShapedText,
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

/// The top edge of a menu `height` tall that hangs from `anchor` in a window
/// `window_height` tall. Below the anchor when it fits, else above it with the
/// usual gap (none for a pointer point), else on the side with more room,
/// clamped into the window and as near the anchor as it can be.
fn menu_top(anchor: Rect, height: f32, window_height: f32) -> f32 {
    let max_y = (window_height - height - CONTEXT_MENU_MARGIN).max(CONTEXT_MENU_MARGIN);
    let below = anchor.bottom();
    if below <= max_y {
        return below.max(CONTEXT_MENU_MARGIN);
    }
    let gap = if anchor.height > 0.0 {
        CONTEXT_MENU_MARGIN
    } else {
        0.0
    };
    let above = anchor.y - gap - height;
    if above >= CONTEXT_MENU_MARGIN {
        return above.min(max_y);
    }
    let room_below = window_height - CONTEXT_MENU_MARGIN - below;
    let room_above = anchor.y - gap - CONTEXT_MENU_MARGIN;
    let preferred = if room_above > room_below {
        above
    } else {
        below
    };
    preferred.clamp(CONTEXT_MENU_MARGIN, max_y)
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
    let Some(kind) = scene.node(source).map(|node| node.kind) else {
        return false;
    };
    if context_menu_source(runtime, scene, source) != Some(source) {
        return false;
    }

    let editable = matches!(kind, WidgetKind::TextInput | WidgetKind::TextArea);
    let has_selection = editable && crate::widgets::selected_text(runtime, scene, source).is_some();
    let has_text = editable
        && scene
            .a11y(source)
            .and_then(|semantics| semantics.value.as_deref())
            .is_some_and(|value| !value.is_empty());
    let (size, least_row) = runtime.context_menu.with(|state| {
        let state = state.borrow();
        let menu = state.menus.get(source);
        (
            menu.and_then(|menu| menu.text_size)
                .unwrap_or(BUTTON_TEXT_SIZE),
            menu.and_then(|menu| menu.row_height).unwrap_or(0.0),
        )
    });
    let phys = phys_size_px(size, scale);
    let inv = 1.0 / norm_scale(scale);
    let mut items = runtime.context_menu.with(|state| {
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
                    };
                    let label = item.label.to_string();
                    RenderItem {
                        shaped: shaper.shape(&label, phys, None),
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
                        action,
                        custom_index,
                    }
                })
                .collect::<Vec<_>>(),
        )
    });
    let Some(ref mut items) = items else {
        return false;
    };

    let theme = theme_for(runtime, source);
    let pad_h = theme.shape.pad(PAD_H + 4.0);
    let pad_v = theme.shape.pad(MENU_PAD_V);
    let row_height = items
        .iter()
        .map(|item| item.shaped.height * inv + 2.0 * pad_v)
        .fold(least_row, f32::max);
    // A row taller than its text keeps the text in its middle.
    let text_top = |item: &RenderItem| (row_height - item.shaped.height * inv) * 0.5;
    let desired_width = items
        .iter()
        .map(|item| item.shaped.width * inv + 2.0 * pad_h)
        .fold(CONTEXT_MENU_MIN_WIDTH * theme.shape.density, f32::max);
    let width = desired_width.min((viewport.width - 2.0 * CONTEXT_MENU_MARGIN).max(1.0));
    let height = row_height * items.len() as f32;
    let max_x = (viewport.width - width - CONTEXT_MENU_MARGIN).max(CONTEXT_MENU_MARGIN);
    let x = anchor.x.clamp(CONTEXT_MENU_MARGIN, max_x);
    let y = menu_top(anchor, height, viewport.height);
    let menu_rect = Rect::new(x, y, width, height);

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
    // `menu_rect` is in window coordinates, but layout boxes are in the
    // parent's content space: a scroll view as the parent moves its children,
    // and the menu with them. Place the menu so it is drawn at `menu_rect`.
    let (inherited, _) = scene.overlay_inherited_context(root);
    let menu_rect = Rect::new(
        menu_rect.x - inherited.x,
        menu_rect.y - inherited.y,
        menu_rect.width,
        menu_rect.height,
    );
    let (x, y) = (menu_rect.x, menu_rect.y);
    scene.set_layout(
        root,
        LayoutBox {
            rect: menu_rect,
            content: menu_rect,
        },
    );
    let named = runtime.context_menu.with(|state| {
        state
            .borrow()
            .menus
            .get(source)
            .and_then(|menu| menu.name.as_ref().map(|name| name.to_string()))
    });
    let menu_name = if let Some(name) = named {
        name
    } else if editable {
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
    {
        let radius = theme.shape.radius(4.0, menu_rect.height);
        let frame = theme.shape.frame.max(1.0);
        let paint = scene.paint_mut(root);
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
        paint.primitives.push(Primitive::SolidRect {
            rect: menu_rect,
            color: theme.surface_muted,
            corner_radius: radius,
        });
        for rect in [
            Rect::new(menu_rect.x, menu_rect.y, menu_rect.width, frame),
            Rect::new(
                menu_rect.x,
                menu_rect.y + menu_rect.height - frame,
                menu_rect.width,
                frame,
            ),
            Rect::new(menu_rect.x, menu_rect.y, frame, menu_rect.height),
            Rect::new(
                menu_rect.x + menu_rect.width - frame,
                menu_rect.y,
                frame,
                menu_rect.height,
            ),
        ] {
            paint.primitives.push(Primitive::SolidRect {
                rect,
                color: theme.outline,
                corner_radius: 0.0,
            });
        }
    }
    scene.mark_dirty(root, DirtyFlags::PAINT);
    scene.mark_dirty(root, DirtyFlags::A11Y);

    let mut item_ids = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let id = scene.insert(WidgetKind::Button, Some(root));
        remember_node_theme(runtime, id, theme);
        let rect = Rect::new(x, y + index as f32 * row_height, width, row_height);
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
            if item.enabled {
                // Focus lets the keyboard walk the rows.
                semantics.actions = ActionFlags::CLICK.0 | ActionFlags::FOCUS.0;
            } else {
                semantics.state = StateFlags::DISABLED.0;
            }
            if item.selected {
                semantics.state |= StateFlags::SELECTED.0;
            }
        }
        {
            let paint = scene.paint_mut(id);
            // The chosen row takes a lighter fill than the keyboard's row
            // and a bar in the accent at its start, so the two never read
            // alike while the keys walk the menu.
            paint.primitives.push(Primitive::SolidRect {
                rect,
                color: if item.selected {
                    crate::widgets::theme::mix_color(theme.surface_muted, theme.selection, 0.5)
                } else {
                    Color::TRANSPARENT
                },
                corner_radius: 0.0,
            });
            if item.selected {
                paint.primitives.push(Primitive::SolidRect {
                    rect: Rect::new(rect.x, rect.y + 4.0, SELECTED_BAR, rect.height - 8.0),
                    color: theme.accent,
                    corner_radius: SELECTED_BAR / 2.0,
                });
            }
            rasterize_and_push(
                paint,
                shaper,
                atlas,
                &item.shaped,
                phys as u32,
                if item.enabled {
                    theme.text
                } else {
                    theme.text_muted
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
        // Rows sit flush against each other and the menu's frame, so the
        // keyboard ring stays inside the row instead of past the frame.
        crate::widgets::with_handlers(runtime, id, |handlers| handlers.flush_row = true);
        runtime.context_menu.with(|state| {
            state.borrow_mut().actions.insert(
                id,
                OpenItem {
                    source,
                    action: item.action,
                },
            );
        });
        if let Some(custom_index) = item.custom_index {
            let callback_runtime = runtime.clone();
            crate::widgets::with_handlers(runtime, id, |handlers| {
                handlers.click = Some(Box::new(move || {
                    fire_custom(&callback_runtime, source, custom_index);
                }));
            });
        }
        item_ids.push(id);
    }
    scene.mark_dirty(parent, DirtyFlags::A11Y);
    // The menu answers the hover that opened it: the control's tooltip would
    // be drawn over the open menu.
    let _ = crate::widgets::hide_hover_tooltip(runtime, scene, source);
    runtime.context_menu.with(|state| {
        state.borrow_mut().open = Some(OpenMenu {
            source,
            root,
            parent,
            items: item_ids,
            anchor,
            can_paste,
            opener: None,
        });
    });
    true
}

fn fire_custom(runtime: &crate::widgets::Runtime, source: WidgetId, index: usize) {
    let callback = runtime.context_menu.with(|state| {
        let mut state = state.borrow_mut();
        let item = state.menus.get_mut(source)?.items.get_mut(index)?;
        match &mut item.action {
            ItemAction::Custom(callback) => callback.take(),
            ItemAction::BuiltIn(_) => None,
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
    if disabled {
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
            state.borrow_mut().handlers.remove(item);
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
    fn it_flips_above_with_a_gap_when_below_is_short() {
        let y = menu_top(Rect::new(0.0, 200.0, 50.0, 30.0), 100.0, 300.0);
        assert_eq!(y + 100.0, 200.0 - CONTEXT_MENU_MARGIN);
    }

    #[test]
    fn with_no_room_on_either_side_it_takes_the_roomier_one() {
        // A 300 tall window and a 200 tall menu: no side fits it.
        assert_eq!(
            menu_top(Rect::new(0.0, 150.0, 50.0, 40.0), 200.0, 300.0),
            4.0
        );
        assert_eq!(
            menu_top(Rect::new(0.0, 100.0, 50.0, 40.0), 200.0, 300.0),
            96.0
        );
    }
}

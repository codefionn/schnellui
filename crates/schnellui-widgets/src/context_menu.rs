//! Transient context menus for editable text controls and other interactive
//! widgets that opt in.
//!
//! Menus are built on demand as manually positioned overlay nodes. Their source
//! configuration remains retained beside the source widget, so opening and
//! dismissing a menu never rebuilds the application tree.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use schnellui_a11y::{ActionFlags, Role, StateFlags};
use schnellui_scene::{
    Color, DirtyFlags, LayoutBox, Point, Primitive, Rect, Scene, Size, WidgetId, WidgetKind,
};
use schnellui_text::{GlyphAtlas, ShapedText, TextShaper};
use slotmap::SecondaryMap;

use crate::{
    forget_node_theme, norm_scale, phys_size_px, rasterize_and_push, remember_node_theme,
    theme_for, ClickHandler, BUTTON_TEXT_SIZE, PAD_H, PAD_V,
};

const CONTEXT_MENU_OVERLAY_LEVEL: u8 = 30;
const CONTEXT_MENU_MIN_WIDTH: f32 = 140.0;
const CONTEXT_MENU_MARGIN: f32 = 4.0;

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

/// One row in a context menu.
pub struct ContextMenuItem {
    label: Cow<'static, str>,
    enabled: bool,
    action: ItemAction,
}

impl ContextMenuItem {
    /// Creates a custom command. Attach its callback with
    /// [`ContextMenuItem::on_select`].
    pub fn new(label: impl Into<Cow<'static, str>>) -> ContextMenuItem {
        ContextMenuItem {
            label: label.into(),
            enabled: true,
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
            action: ItemAction::BuiltIn(action),
        }
    }

    /// Enables or disables this command independently of its dynamic clipboard
    /// state.
    pub fn enabled(mut self, enabled: bool) -> ContextMenuItem {
        self.enabled = enabled;
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
}

impl ContextMenu {
    pub fn new() -> ContextMenu {
        ContextMenu { items: Vec::new() }
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
    root: WidgetId,
    parent: WidgetId,
    items: Vec<WidgetId>,
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

pub(crate) fn reset(runtime: &crate::Runtime) {
    runtime
        .context_menu
        .with(|state| *state.borrow_mut() = ContextRuntime::default());
}

pub(crate) fn purge_nodes(runtime: &crate::Runtime, nodes: &[WidgetId]) {
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

pub(crate) fn register_context_menu(runtime: &crate::Runtime, id: WidgetId, menu: ContextMenu) {
    runtime.context_menu.with(|state| {
        state.borrow_mut().menus.insert(id, menu);
    });
}

pub(crate) fn register_context_menu_trigger(
    runtime: &crate::Runtime,
    trigger: WidgetId,
    source: WidgetId,
) {
    runtime.context_menu.with(|state| {
        state.borrow_mut().triggers.insert(trigger, source);
    });
}

pub fn context_menu_trigger_source(
    runtime: &crate::Runtime,
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
    runtime: &crate::Runtime,
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

/// Opens `source`'s configured menu at a logical window position.
#[allow(clippy::too_many_arguments)]
pub fn open_context_menu(
    runtime: &crate::Runtime,
    scene: &mut Scene,
    shaper: &mut TextShaper,
    atlas: &mut GlyphAtlas,
    source: WidgetId,
    position: Point,
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
    let has_selection = editable && crate::selected_text(runtime, scene, source).is_some();
    let has_text = editable
        && scene
            .a11y(source)
            .and_then(|semantics| semantics.value.as_deref())
            .is_some_and(|value| !value.is_empty());
    let phys = phys_size_px(BUTTON_TEXT_SIZE, scale);
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
    let pad_v = theme.shape.pad(PAD_V + 1.0);
    let row_height = items
        .iter()
        .map(|item| item.shaped.height * inv + 2.0 * pad_v)
        .fold(0.0f32, f32::max);
    let desired_width = items
        .iter()
        .map(|item| item.shaped.width * inv + 2.0 * pad_h)
        .fold(CONTEXT_MENU_MIN_WIDTH * theme.shape.density, f32::max);
    let width = desired_width.min((viewport.width - 2.0 * CONTEXT_MENU_MARGIN).max(1.0));
    let height = row_height * items.len() as f32;
    let max_x = (viewport.width - width - CONTEXT_MENU_MARGIN).max(CONTEXT_MENU_MARGIN);
    let max_y = (viewport.height - height - CONTEXT_MENU_MARGIN).max(CONTEXT_MENU_MARGIN);
    let x = position.x.clamp(CONTEXT_MENU_MARGIN, max_x);
    let y = position.y.clamp(CONTEXT_MENU_MARGIN, max_y);
    let menu_rect = Rect::new(x, y, width, height);

    let parent = crate::dialog::active_modal_layer(runtime, scene)
        .filter(|layer| crate::dialog::is_in_subtree(scene, source, *layer))
        .or_else(|| scene.root());
    let Some(parent) = parent else {
        return false;
    };
    let root = scene.insert(WidgetKind::Column, Some(parent));
    remember_node_theme(runtime, root, theme);
    scene.set_overlay_level(root, CONTEXT_MENU_OVERLAY_LEVEL);
    scene.set_layout(
        root,
        LayoutBox {
            rect: menu_rect,
            content: menu_rect,
        },
    );
    let menu_name = if editable {
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
                semantics.actions = ActionFlags::CLICK.0;
            } else {
                semantics.state = StateFlags::DISABLED.0;
            }
        }
        {
            let paint = scene.paint_mut(id);
            paint.primitives.push(Primitive::SolidRect {
                rect,
                color: Color::TRANSPARENT,
                corner_radius: 0.0,
            });
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
                    y: rect.y + pad_v,
                },
            );
        }
        scene.mark_dirty(id, DirtyFlags::PAINT);
        scene.mark_dirty(id, DirtyFlags::A11Y);
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
            crate::with_handlers(runtime, id, |handlers| {
                handlers.click = Some(Box::new(move || {
                    fire_custom(&callback_runtime, source, custom_index);
                }));
            });
        }
        item_ids.push(id);
    }
    scene.mark_dirty(parent, DirtyFlags::A11Y);
    runtime.context_menu.with(|state| {
        state.borrow_mut().open = Some(OpenMenu {
            root,
            parent,
            items: item_ids,
        });
    });
    true
}

fn fire_custom(runtime: &crate::Runtime, source: WidgetId, index: usize) {
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

pub fn context_menu_is_open(runtime: &crate::Runtime) -> bool {
    runtime
        .context_menu
        .with(|state| state.borrow().open.is_some())
}

pub fn context_menu_item(runtime: &crate::Runtime, scene: &Scene, id: WidgetId) -> bool {
    scene.node(id).is_some()
        && runtime
            .context_menu
            .with(|state| state.borrow().actions.contains_key(id))
}

/// Activates one open menu row and dismisses the popup.
pub fn activate_context_menu_item(
    runtime: &crate::Runtime,
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
        let _ = crate::dispatch_click(runtime, scene, id);
    }
    let activation = ContextMenuActivation {
        source: item.source,
        action: item.action,
    };
    let _ = dismiss_context_menu(runtime, scene);
    Some(activation)
}

/// Dismisses the active menu, if any.
pub fn dismiss_context_menu(runtime: &crate::Runtime, scene: &mut Scene) -> bool {
    let open = runtime
        .context_menu
        .with(|state| state.borrow_mut().open.take());
    let Some(open) = open else {
        return false;
    };
    let _ = crate::strip_hover_decoration(runtime, scene);
    scene.mark_dirty(open.root, DirtyFlags::PAINT);
    for item in open.items {
        runtime.context_menu.with(|state| {
            state.borrow_mut().actions.remove(item);
        });
        runtime.with(|state| {
            state.borrow_mut().handlers.remove(item);
        });
        forget_node_theme(runtime, item);
        scene.remove(item);
    }
    forget_node_theme(runtime, open.root);
    scene.remove(open.root);
    if scene.node(open.parent).is_some() {
        scene.mark_dirty(open.parent, DirtyFlags::A11Y);
    }
    true
}

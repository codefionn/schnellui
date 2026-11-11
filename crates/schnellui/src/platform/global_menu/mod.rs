//! The system's global menu bar: the one outside the window, as on macOS.
//!
//! schnellui never draws a menu bar inside a window. This module only asks the
//! system to show one. [`GlobalMenu::supported`] says whether the system has a
//! global menu at all. Where it does, the app builds a [`MenuBar`] and hands it
//! to [`GlobalMenu::show`] (or [`App::set_global_menu`](crate::App::set_global_menu),
//! which runs the handlers on the UI thread). Where it does not, the query
//! returns `false`, nothing is shown, and the app decides what to offer
//! instead, such as a toolbar of its own.
//!
//! Each system differs:
//!
//! - **macOS** builds an `NSMenu` tree and installs it as the application's
//!   main menu. The first menu of the bar becomes the application menu, which
//!   the system titles with the app's name whatever title it has here, so put
//!   About, Hide and Quit there. Key equivalents belong to the menu: AppKit
//!   consumes the chord before the window sees it, so an in-app
//!   [`Shortcut`] on the same chord does not fire while the item is enabled.
//!   Dropping the [`GlobalMenu`] puts back a minimal menu with a Quit item.
//! - **Windows** has no global menu: `supported()` is `false`.
//! - **Linux** returns `false` for now. GNOME has no global menu. KDE has one,
//!   but using it needs a `com.canonical.dbusmenu` object exported on the
//!   session bus and the `org_kde_kwin_appmenu` Wayland protocol (or the
//!   matching X11 window properties) to tell the shell where it is. winit does
//!   not expose either. A backend would live in a `linux.rs` beside
//!   `macos.rs`, selected in the `imp` list below, and flip `SUPPORTED` when
//!   the session has an appmenu registrar.
//! - Other targets, wasm included, are not supported.
//!
//! Where `supported()` is `false`, [`GlobalMenu::show`] does nothing and the
//! key equivalents of the items never fire. The [`App`](crate::App) still keeps
//! the model and can run an item by id with
//! [`App::dispatch_global_menu_item`](crate::App::dispatch_global_menu_item),
//! which is how tests and the debug server drive it.
//!
//! # Standard items
//!
//! [`StandardItem`] names items the system implements. The window-level ones
//! (About, Hide, Hide Others, Show All, Quit, Minimize, Zoom) are native
//! responder-chain actions. The edit ones (Undo, Redo, Cut, Copy, Paste,
//! Select All) are different: a schnellui window has no `NSTextView`, so the
//! native selectors would find no responder and the items would stay dead. The
//! backend sends them back as [`GlobalMenuEvent::Standard`] and the window host
//! runs its own clipboard, undo and select-all paths on the focused control.

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::Shortcut;

mod keys;

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod imp;
#[cfg(not(target_os = "macos"))]
#[path = "unsupported.rs"]
mod imp;

pub use keys::{key_equivalent, KeyEquivalent};

/// Whether a menu id is usable: not empty, ASCII letters, digits, `_`, `-` or
/// `.`, and at most 128 bytes.
fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// An item the system implements itself. About, Hide, Hide Others, Show All,
/// Quit, Minimize and Zoom are native actions. Undo, Redo, Cut, Copy, Paste
/// and Select All are not: a schnellui window has no `NSTextView` to answer
/// the native selectors, so the backend reports them as
/// [`GlobalMenuEvent::Standard`] and the window host runs its own clipboard,
/// undo and select-all paths on the focused control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StandardItem {
    /// Shows the system's About panel.
    About,
    /// Hides the app.
    Hide,
    /// Hides every other app.
    HideOthers,
    /// Shows every hidden app again.
    ShowAll,
    /// Quits the app.
    Quit,
    /// Minimizes the key window.
    Minimize,
    /// Zooms the key window.
    Zoom,
    /// Undoes the last edit in the focused control.
    Undo,
    /// Redoes the last undone edit.
    Redo,
    /// Cuts the selection to the clipboard.
    Cut,
    /// Copies the selection to the clipboard.
    Copy,
    /// Pastes the clipboard at the cursor.
    Paste,
    /// Selects all text in the focused control.
    SelectAll,
}

/// One entry of a [`Menu`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// An item that runs the app's handler for its id.
    Action(MenuAction),
    /// A dividing line.
    Separator,
    /// A nested menu.
    Submenu(Menu),
    /// An item the system implements.
    Standard(StandardItem),
}

impl From<MenuAction> for MenuItem {
    fn from(action: MenuAction) -> Self {
        Self::Action(action)
    }
}

impl From<Menu> for MenuItem {
    fn from(menu: Menu) -> Self {
        Self::Submenu(menu)
    }
}

impl From<StandardItem> for MenuItem {
    fn from(item: StandardItem) -> Self {
        Self::Standard(item)
    }
}

/// A menu item the app handles, named by a stable id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MenuAction {
    id: String,
    label: String,
    shortcut: Option<Shortcut>,
    enabled: bool,
    checked: Option<bool>,
}

impl MenuAction {
    /// An enabled item with no key equivalent and no check mark.
    ///
    /// The id is a stable ASCII name such as `file.open`. It may hold letters,
    /// digits, `_`, `-` and `.`, and must be unique in the whole menu bar.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            checked: None,
        }
    }

    /// Sets the key equivalent the system shows and fires. Keys with no menu
    /// form, such as the media keys, are shown without one.
    #[must_use]
    pub fn shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Sets whether the item can be chosen. A disabled item is greyed out and
    /// its handler never runs.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Gives the item a check mark state: `Some(true)` shows the mark,
    /// `Some(false)` hides it, `None` makes the item a plain one.
    #[must_use]
    pub fn checked(mut self, checked: Option<bool>) -> Self {
        self.checked = checked;
        self
    }

    /// The stable id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The text of the item.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// The key equivalent, if any.
    pub fn key_equivalent(&self) -> Option<Shortcut> {
        self.shortcut
    }

    /// Whether the item can be chosen.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The check mark state, `None` for an item that has none.
    pub fn checked_state(&self) -> Option<bool> {
        self.checked
    }

    /// Replaces the text of the item.
    pub fn set_label(&mut self, label: impl Into<String>) {
        self.label = label.into();
    }

    /// Enables or disables the item.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Sets the check mark state.
    pub fn set_checked(&mut self, checked: Option<bool>) {
        self.checked = checked;
    }
}

/// A titled list of items: a top-level menu or a submenu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Menu {
    title: String,
    items: Vec<MenuItem>,
}

impl Menu {
    /// An empty menu.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            items: Vec::new(),
        }
    }

    /// Appends an item: a [`MenuAction`], a [`Menu`] (as a submenu) or a
    /// [`StandardItem`].
    #[must_use]
    pub fn item(mut self, item: impl Into<MenuItem>) -> Self {
        self.items.push(item.into());
        self
    }

    /// Appends a separator.
    #[must_use]
    pub fn separator(mut self) -> Self {
        self.items.push(MenuItem::Separator);
        self
    }

    /// The title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The items in order.
    pub fn items(&self) -> &[MenuItem] {
        &self.items
    }

    fn find_action(&self, id: &str) -> Option<&MenuAction> {
        self.items.iter().find_map(|item| match item {
            MenuItem::Action(action) if action.id == id => Some(action),
            MenuItem::Submenu(menu) => menu.find_action(id),
            _ => None,
        })
    }

    fn find_action_mut(&mut self, id: &str) -> Option<&mut MenuAction> {
        self.items.iter_mut().find_map(|item| match item {
            MenuItem::Action(action) if action.id == id => Some(action),
            MenuItem::Submenu(menu) => menu.find_action_mut(id),
            _ => None,
        })
    }

    fn collect_actions<'a>(&'a self, out: &mut Vec<&'a MenuAction>) {
        for item in &self.items {
            match item {
                MenuItem::Action(action) => out.push(action),
                MenuItem::Submenu(menu) => menu.collect_actions(out),
                _ => {}
            }
        }
    }
}

/// A menu bar: a list of top-level menus.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MenuBar {
    menus: Vec<Menu>,
}

impl MenuBar {
    /// A menu bar with no menus.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a top-level menu. On macOS the first one is the application
    /// menu.
    #[must_use]
    pub fn menu(mut self, menu: Menu) -> Self {
        self.menus.push(menu);
        self
    }

    /// The top-level menus in order.
    pub fn menus(&self) -> &[Menu] {
        &self.menus
    }

    /// Whether the bar has no menus.
    pub fn is_empty(&self) -> bool {
        self.menus.is_empty()
    }

    /// The action with this id, searching submenus too.
    pub fn find_action(&self, id: &str) -> Option<&MenuAction> {
        self.menus.iter().find_map(|menu| menu.find_action(id))
    }

    /// The action with this id for changing its label, enabled or check state.
    pub fn find_action_mut(&mut self, id: &str) -> Option<&mut MenuAction> {
        self.menus
            .iter_mut()
            .find_map(|menu| menu.find_action_mut(id))
    }

    /// Every action in menu order, submenus included.
    pub fn actions(&self) -> Vec<&MenuAction> {
        let mut out = Vec::new();
        for menu in &self.menus {
            menu.collect_actions(&mut out);
        }
        out
    }

    /// Checks that every action id is valid and used once.
    pub fn validate(&self) -> Result<(), GlobalMenuError> {
        let actions = self.actions();
        for (index, action) in actions.iter().enumerate() {
            if !is_valid_id(&action.id) {
                return Err(GlobalMenuError::InvalidId(action.id.clone()));
            }
            if actions[..index].iter().any(|seen| seen.id == action.id) {
                return Err(GlobalMenuError::DuplicateId(action.id.clone()));
            }
        }
        Ok(())
    }
}

/// Why a menu bar was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalMenuError {
    /// An action id is empty, too long or has characters outside ASCII
    /// letters, digits, `_`, `-` and `.`.
    InvalidId(String),
    /// Two actions share an id.
    DuplicateId(String),
}

impl fmt::Display for GlobalMenuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "menu action id {id:?} is not valid"),
            Self::DuplicateId(id) => write!(f, "menu action id {id:?} is used twice"),
        }
    }
}

impl std::error::Error for GlobalMenuError {}

/// What a backend reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalMenuEvent {
    /// The user chose the action with this id.
    Action(String),
    /// The user chose an edit item the system cannot carry out in a schnellui
    /// window (Undo, Redo, Cut, Copy, Paste, Select All), so the app host must.
    Standard(StandardItem),
}

/// The channel from a backend to the app: forwards events to the callback.
pub(crate) struct Sink {
    on_event: Mutex<Box<dyn FnMut(GlobalMenuEvent) + Send>>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl Sink {
    fn emit(&self, event: GlobalMenuEvent) {
        if let Ok(mut on_event) = self.on_event.lock() {
            on_event(event);
        }
    }

    pub(crate) fn action(&self, id: &str) {
        self.emit(GlobalMenuEvent::Action(id.to_owned()));
    }

    pub(crate) fn standard(&self, item: StandardItem) {
        self.emit(GlobalMenuEvent::Standard(item));
    }
}

/// A menu bar the system is showing. Dropping it takes the menu down; on
/// macOS a minimal menu with a Quit item takes its place.
///
/// schnellui never draws a menu bar in the window. Ask
/// [`GlobalMenu::supported`] first. Only macOS has a backend today: Windows
/// has no global menu, GNOME has none, and KDE's needs a
/// `com.canonical.dbusmenu` export plus the `org_kde_kwin_appmenu` protocol,
/// which winit does not expose. Where it is `false`, nothing is shown and the
/// key equivalents never fire. On macOS the first menu is the application menu,
/// which the system titles with the app's name, and AppKit consumes an item's
/// key equivalent before the window sees the key.
pub struct GlobalMenu {
    _session: imp::Session,
}

impl GlobalMenu {
    /// Whether this system has a global menu this backend can fill. When this
    /// is `false`, [`GlobalMenu::show`] does nothing and the app should offer
    /// its commands some other way.
    pub fn supported() -> bool {
        imp::SUPPORTED
    }

    /// Shows `menu` as the system menu bar and returns at once. Chosen items
    /// arrive through `on_event`. The menu is not checked here; call
    /// [`MenuBar::validate`] first, since duplicate ids make the handlers
    /// ambiguous.
    ///
    /// On macOS the call and the drop must happen on the main thread, and
    /// `on_event` runs there. Where [`GlobalMenu::supported`] is `false` this
    /// shows nothing and `on_event` never runs.
    pub fn show(menu: &MenuBar, on_event: impl FnMut(GlobalMenuEvent) + Send + 'static) -> Self {
        let sink = Arc::new(Sink {
            on_event: Mutex::new(Box::new(on_event)),
        });
        Self {
            _session: imp::start(menu, sink),
        }
    }
}

impl fmt::Debug for GlobalMenu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlobalMenu").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MenuBar {
        MenuBar::new()
            .menu(
                Menu::new("App")
                    .item(StandardItem::About)
                    .separator()
                    .item(StandardItem::Quit),
            )
            .menu(
                Menu::new("File")
                    .item(MenuAction::new("file.new", "New").shortcut(Shortcut::command('n')))
                    .item(
                        Menu::new("Recent")
                            .item(MenuAction::new("file.recent.1", "One").enabled(false)),
                    ),
            )
    }

    #[test]
    fn the_builders_keep_order_and_nesting() {
        let bar = sample();
        assert_eq!(bar.menus().len(), 2);
        assert_eq!(bar.menus()[1].title(), "File");
        let ids: Vec<_> = bar.actions().iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["file.new", "file.recent.1"]);
        assert_eq!(bar.menus()[0].items()[1], MenuItem::Separator);
        assert!(MenuBar::new().is_empty());
        assert!(!bar.is_empty());
    }

    #[test]
    fn find_reaches_into_submenus_and_edits_in_place() {
        let mut bar = sample();
        assert!(bar
            .find_action("file.recent.1")
            .is_some_and(|a| !a.is_enabled()));
        bar.find_action_mut("file.recent.1")
            .unwrap()
            .set_enabled(true);
        let action = bar.find_action_mut("file.new").unwrap();
        action.set_label("New file");
        action.set_checked(Some(true));
        assert_eq!(bar.find_action("file.new").unwrap().label(), "New file");
        assert_eq!(
            bar.find_action("file.new").unwrap().checked_state(),
            Some(true)
        );
        assert!(bar.find_action("file.recent.1").unwrap().is_enabled());
        assert!(bar.find_action("nope").is_none());
    }

    #[test]
    fn validate_refuses_bad_and_repeated_ids() {
        assert_eq!(sample().validate(), Ok(()));
        let bad = MenuBar::new().menu(Menu::new("M").item(MenuAction::new("has space", "x")));
        assert_eq!(
            bad.validate(),
            Err(GlobalMenuError::InvalidId("has space".into()))
        );
        let empty = MenuBar::new().menu(Menu::new("M").item(MenuAction::new("", "x")));
        assert!(empty.validate().is_err());
        let twice = MenuBar::new().menu(
            Menu::new("M")
                .item(MenuAction::new("a", "x"))
                .item(Menu::new("S").item(MenuAction::new("a", "y"))),
        );
        assert_eq!(
            twice.validate(),
            Err(GlobalMenuError::DuplicateId("a".into()))
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn systems_without_a_backend_show_nothing_and_say_so() {
        assert!(!GlobalMenu::supported());
        let _menu = GlobalMenu::show(&sample(), |_| panic!("no event can arrive"));
    }
}

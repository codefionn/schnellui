//! The system's global menu bar on the app: the model, the handlers and
//! dispatch.
//!
//! The app keeps the [`MenuBar`] it asked for on every target, plus a handler
//! per action id; handlers run on the UI thread. The window host owns the
//! platform side: it notices a new [`revision`](GlobalMenuRegistry::revision),
//! hands the model to the backend and feeds chosen items back through
//! the window host's `apply_global_menu_event`. Where the system has
//! no global menu the model is kept and nothing is shown. schnellui never draws
//! a menu bar in the window. Nothing here runs per frame.

use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::platform::{GlobalMenu, GlobalMenuError, MenuAction, MenuBar};

type GlobalMenuHandler = Box<dyn FnMut() + 'static>;

/// Hands out a new value for every change to any registry, so a registry
/// built after a remount never matches the revision the host last synced.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub(crate) struct GlobalMenuRegistry {
    bar: MenuBar,
    handlers: Vec<(String, GlobalMenuHandler)>,
    /// Changes whenever the model does. 0 for a registry never used.
    revision: u64,
    /// The revision the platform menu was last built for.
    synced: u64,
    /// The platform menu, once the window host showed one.
    session: Option<GlobalMenu>,
}

impl GlobalMenuRegistry {
    fn touch(&mut self) {
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }
}

impl App {
    /// Whether the system has a global menu bar this build can fill: macOS
    /// today. When `false`, [`App::set_global_menu`] still records the menu but
    /// nothing is shown and the items' key equivalents never fire, so the app
    /// has to offer its commands another way. schnellui draws no menu bar of
    /// its own.
    pub fn global_menu_supported() -> bool {
        GlobalMenu::supported()
    }

    /// Sets the menu bar the system shows, replacing the earlier one. Handlers
    /// registered with [`App::on_global_menu_item`] stay and apply to whichever
    /// ids the new menu has. Returns an error and changes nothing when an action
    /// id is not valid or is used twice (see [`MenuBar::validate`]).
    ///
    /// Only the native window host passes the menu to the system, and only
    /// where [`App::global_menu_supported`] is `true`. Headless runs and tests
    /// never touch the platform; drive an item with
    /// [`App::dispatch_global_menu_item`].
    pub fn set_global_menu(&mut self, menu: MenuBar) -> Result<(), GlobalMenuError> {
        menu.validate()?;
        let registry = &mut self.global_menu;
        registry.bar = menu;
        registry.touch();
        Ok(())
    }

    /// Registers the handler for the action with this id, replacing an earlier
    /// one. It runs on the UI thread when the user chooses the item, or when
    /// [`App::dispatch_global_menu_item`] does. The id need not be in the menu
    /// yet.
    pub fn on_global_menu_item(&mut self, id: impl Into<String>, handler: impl FnMut() + 'static) {
        let id = id.into();
        let handler: GlobalMenuHandler = Box::new(handler);
        let handlers = &mut self.global_menu.handlers;
        match handlers.iter_mut().find(|(known, _)| *known == id) {
            Some(entry) => entry.1 = handler,
            None => handlers.push((id, handler)),
        }
    }

    /// Removes the menu and every handler. On macOS the system falls back to a
    /// minimal menu with a Quit item.
    pub fn clear_global_menu(&mut self) {
        let registry = &mut self.global_menu;
        registry.handlers.clear();
        if !registry.bar.is_empty() {
            registry.bar = MenuBar::new();
            registry.touch();
        }
    }

    /// The menu bar the app set, empty if none. The same on every target.
    pub fn global_menu(&self) -> &MenuBar {
        &self.global_menu.bar
    }

    /// Changes one action in the current menu: its label, enabled state or
    /// check mark. Returns whether an action with this id exists. The system
    /// menu is rebuilt only when `change` altered something.
    pub fn update_global_menu_item(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut MenuAction),
    ) -> bool {
        let registry = &mut self.global_menu;
        let Some(action) = registry.bar.find_action_mut(id) else {
            return false;
        };
        let before = action.clone();
        change(action);
        if *action != before {
            registry.touch();
        }
        true
    }

    /// Runs the handler of the action `id`, as choosing it in the system menu
    /// would. Returns `true` when a handler ran. Returns `false` for an unknown
    /// id, a disabled item (its handler must not run) and an item with no
    /// handler. Headless tests and the debug server use this to drive a menu
    /// without a desktop. A check mark is not toggled here: the handler changes
    /// it with [`App::update_global_menu_item`].
    pub fn dispatch_global_menu_item(&mut self, id: &str) -> bool {
        let registry = &mut self.global_menu;
        if !registry.bar.find_action(id).is_some_and(|a| a.is_enabled()) {
            return false;
        }
        let Some((_, handler)) = registry.handlers.iter_mut().find(|(known, _)| known == id) else {
            return false;
        };
        handler();
        true
    }

    /// Whether the model changed since the host last gave the platform a menu.
    /// One integer comparison.
    pub(crate) fn global_menu_stale(&self) -> bool {
        self.global_menu.revision != self.global_menu.synced
    }

    /// Replaces the platform menu with one for the current model, or takes it
    /// down when the model is empty. `show` gets the model and returns the
    /// new menu.
    pub(crate) fn sync_global_menu_session(&mut self, show: impl FnOnce(&MenuBar) -> GlobalMenu) {
        let registry = &mut self.global_menu;
        registry.synced = registry.revision;
        registry.session = None;
        if !registry.bar.is_empty() {
            registry.session = Some(show(&registry.bar));
        }
    }

    /// Moves the model, handlers and platform menu into a rebuilt app.
    pub(crate) fn inherit_global_menu(&mut self, previous: &mut App) {
        self.global_menu = std::mem::take(&mut previous.global_menu);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Menu, StandardItem};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn app() -> App {
        App::new(64, 64)
    }

    fn bar() -> MenuBar {
        MenuBar::new()
            .menu(Menu::new("App").item(StandardItem::Quit))
            .menu(
                Menu::new("File")
                    .item(MenuAction::new("new", "New"))
                    .item(MenuAction::new("off", "Off").enabled(false))
                    .item(Menu::new("Recent").item(MenuAction::new("recent.1", "One"))),
            )
    }

    fn counting(app: &mut App, id: &str) -> Rc<RefCell<u32>> {
        let count = Rc::new(RefCell::new(0));
        app.on_global_menu_item(id, {
            let count = count.clone();
            move || *count.borrow_mut() += 1
        });
        count
    }

    #[test]
    fn set_list_and_clear() {
        let mut app = app();
        assert!(app.global_menu().is_empty());
        assert!(!app.global_menu_stale());
        app.set_global_menu(bar()).unwrap();
        assert!(app.global_menu_stale());
        assert_eq!(app.global_menu().menus().len(), 2);
        assert_eq!(app.global_menu().menus()[1].title(), "File");
        counting(&mut app, "new");
        app.clear_global_menu();
        assert!(app.global_menu().is_empty());
        assert!(!app.dispatch_global_menu_item("new"));
    }

    #[test]
    fn an_invalid_menu_changes_nothing() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        let twice = MenuBar::new().menu(
            Menu::new("M")
                .item(MenuAction::new("a", "x"))
                .item(MenuAction::new("a", "y")),
        );
        assert_eq!(
            app.set_global_menu(twice),
            Err(GlobalMenuError::DuplicateId("a".into()))
        );
        assert!(app
            .set_global_menu(
                MenuBar::new().menu(Menu::new("M").item(MenuAction::new("bad id", "x")))
            )
            .is_err());
        assert_eq!(app.global_menu(), &bar());
    }

    #[test]
    fn dispatch_runs_enabled_items_including_nested_ones() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        let new = counting(&mut app, "new");
        let nested = counting(&mut app, "recent.1");
        assert!(app.dispatch_global_menu_item("new"));
        assert!(app.dispatch_global_menu_item("recent.1"));
        assert!(app.dispatch_global_menu_item("recent.1"));
        assert_eq!((*new.borrow(), *nested.borrow()), (1, 2));
    }

    #[test]
    fn dispatch_skips_disabled_unknown_and_handlerless_items() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        let off = counting(&mut app, "off");
        let ghost = counting(&mut app, "ghost");
        assert!(!app.dispatch_global_menu_item("off"));
        assert!(!app.dispatch_global_menu_item("ghost"), "not in the menu");
        assert!(!app.dispatch_global_menu_item("missing"));
        assert!(!app.dispatch_global_menu_item("new"), "no handler");
        assert_eq!((*off.borrow(), *ghost.borrow()), (0, 0));
        // Enabling it makes the same dispatch run.
        assert!(app.update_global_menu_item("off", |a| a.set_enabled(true)));
        assert!(app.dispatch_global_menu_item("off"));
        assert_eq!(*off.borrow(), 1);
    }

    #[test]
    fn a_later_handler_replaces_the_earlier_one() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        let first = counting(&mut app, "new");
        let second = counting(&mut app, "new");
        app.dispatch_global_menu_item("new");
        assert_eq!((*first.borrow(), *second.borrow()), (0, 1));
    }

    #[test]
    fn update_by_id_changes_one_item_and_only_rebuilds_on_change() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        app.sync_global_menu_session(|_| unreachable_show());
        assert!(!app.global_menu_stale());
        assert!(app.update_global_menu_item("recent.1", |a| {
            a.set_label("Uno");
            a.set_checked(Some(true));
        }));
        assert!(app.global_menu_stale());
        let action = app.global_menu().find_action("recent.1").unwrap();
        assert_eq!(
            (action.label(), action.checked_state()),
            ("Uno", Some(true))
        );
        assert_eq!(app.global_menu().find_action("new").unwrap().label(), "New");
        app.sync_global_menu_session(|_| unreachable_show());
        assert!(app.update_global_menu_item("new", |_| {}));
        assert!(!app.global_menu_stale(), "a no-op change is not a change");
        assert!(!app.update_global_menu_item("nope", |a| a.set_enabled(false)));
    }

    /// A platform menu for tests: shows nothing on this target.
    fn unreachable_show() -> GlobalMenu {
        GlobalMenu::show(&MenuBar::new(), |_| {})
    }

    #[test]
    fn the_model_going_stale_is_what_rebuilds_the_platform_menu() {
        let mut app = app();
        app.set_global_menu(bar()).unwrap();
        assert!(app.global_menu_stale());
        let mut shown = 0;
        app.sync_global_menu_session(|menu| {
            shown += 1;
            assert_eq!(menu.menus().len(), 2);
            unreachable_show()
        });
        assert_eq!(shown, 1);
        assert!(!app.global_menu_stale());
        // A dispatch is not a change to the model.
        counting(&mut app, "new");
        app.dispatch_global_menu_item("new");
        assert!(!app.global_menu_stale());
        app.clear_global_menu();
        assert!(app.global_menu_stale());
        app.sync_global_menu_session(|_| panic!("an empty menu is not shown"));
        assert!(!app.global_menu_stale());
    }

    #[test]
    fn a_rebuilt_app_inherits_the_menu_and_handlers() {
        let mut old = app();
        old.set_global_menu(bar()).unwrap();
        let hits = counting(&mut old, "new");
        let revision = old.global_menu.revision;
        let mut new = app();
        new.inherit_global_menu(&mut old);
        assert!(new.dispatch_global_menu_item("new"));
        assert_eq!(*hits.borrow(), 1);
        assert_eq!(new.global_menu(), &bar());
        assert_eq!(new.global_menu.revision, revision);
        assert!(old.global_menu().is_empty());
        assert!(!old.dispatch_global_menu_item("new"));
    }

    #[test]
    fn support_follows_the_platform_query() {
        assert_eq!(App::global_menu_supported(), GlobalMenu::supported());
    }
}

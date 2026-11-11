//! AppKit backend: builds an `NSMenu` tree and installs it as the main menu.
//!
//! Everything runs on the main thread, which is where the window host runs.
//! One Objective-C class, `SchnellUiMenuTarget`, is the target of every
//! action and edit item. It has one action selector and finds the item's entry
//! through the item's tag. The window-level standard items (Hide, Quit and the
//! like) have no target, so the responder chain finds `NSApplication` or the
//! key window. This file is type-checked from other hosts but has not run on
//! a Mac.

use std::cell::RefCell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::Sel;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
use objc2_foundation::{
    MainThreadMarker, NSInteger, NSObject, NSObjectProtocol, NSProcessInfo, NSString,
};

use super::keys::key_equivalent;
use super::{Menu, MenuAction, MenuBar, MenuItem, Sink, StandardItem};

pub(super) const SUPPORTED: bool = true;

/// What a menu item stands for, found again from its tag.
enum Entry {
    Action(String),
    Standard(StandardItem),
}

struct TargetIvars {
    sink: Arc<Sink>,
    entries: RefCell<Vec<Entry>>,
}

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and the class does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SchnellUiMenuTarget"]
    #[ivars = TargetIvars]
    struct MenuTarget;

    // SAFETY: `NSObjectProtocol` has no safety requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        // SAFETY: the signature matches the `-(void)action:(id)sender` shape
        // AppKit calls; the sender is always the `NSMenuItem` that was chosen.
        #[unsafe(method(schnellUiMenuItemChosen:))]
        fn chosen(&self, sender: &NSMenuItem) {
            let ivars = self.ivars();
            let tag = sender.tag();
            // Copy the entry out so the callback may rebuild the menu.
            let event = usize::try_from(tag)
                .ok()
                .and_then(|index| match ivars.entries.borrow().get(index)? {
                    Entry::Action(id) => Some(Ok(id.clone())),
                    Entry::Standard(item) => Some(Err(*item)),
                });
            match event {
                Some(Ok(id)) => ivars.sink.action(&id),
                Some(Err(item)) => ivars.sink.standard(item),
                None => {}
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, sink: Arc<Sink>) -> Retained<Self> {
        let this = mtm.alloc::<Self>().set_ivars(TargetIvars {
            sink,
            entries: RefCell::new(Vec::new()),
        });
        // SAFETY: `NSObject`'s `init` has the signature `-(instancetype)init`.
        unsafe { msg_send![super(this), init] }
    }
}

/// The live menu. Holds the target alive, since menu items only reference
/// their target weakly.
pub(super) struct Session {
    target: Option<Retained<MenuTarget>>,
}

fn app_name() -> String {
    NSProcessInfo::processInfo().processName().to_string()
}

/// A top-level or nested menu item, with `title` and the submenu attached.
fn menu_item(mtm: MainThreadMarker, title: &NSString) -> Retained<NSMenuItem> {
    // SAFETY: no action selector is given, so there is nothing to validate.
    unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            mtm.alloc::<NSMenuItem>(),
            title,
            None,
            &NSString::from_str(""),
        )
    }
}

fn new_menu(mtm: MainThreadMarker, title: &str) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(mtm.alloc::<NSMenu>(), &NSString::from_str(title));
    // Items carry their own enabled flag; without this AppKit would enable
    // every item whose target responds and ignore ours.
    menu.setAutoenablesItems(false);
    menu
}

fn apply_key_equivalent(item: &NSMenuItem, shortcut: Option<crate::Shortcut>) {
    let Some(equivalent) = shortcut.and_then(key_equivalent) else {
        return;
    };
    item.setKeyEquivalent(&NSString::from_str(&equivalent.key));
    item.setKeyEquivalentModifierMask(NSEventModifierFlags(equivalent.modifier_mask));
}

/// Builds items onto a menu and records what each tag means.
struct Builder<'a> {
    mtm: MainThreadMarker,
    app_name: &'a str,
    /// The target for action and edit items; `None` for the default menu,
    /// which has only responder-chain items.
    target: Option<&'a MenuTarget>,
}

impl Builder<'_> {
    fn push_entry(&self, entry: Entry) -> NSInteger {
        let Some(target) = self.target else {
            return 0;
        };
        let mut entries = target.ivars().entries.borrow_mut();
        entries.push(entry);
        (entries.len() - 1) as NSInteger
    }

    fn standard(&self, standard: StandardItem) -> Retained<NSMenuItem> {
        let title = NSString::from_str(&standard.label(self.app_name));
        let item = menu_item(self.mtm, &title);
        match (standard.native_selector(), self.target) {
            (Some(name), _) => {
                // SAFETY: the selector names an `NSApplication` or `NSWindow`
                // action; with no target AppKit walks the responder chain and
                // greys the item out where nothing answers.
                unsafe { item.setAction(Some(Sel::register(name))) };
                // Native items validate through the chain, so let AppKit
                // decide their state.
                item.setEnabled(true);
            }
            (None, Some(target)) => {
                item.setTag(self.push_entry(Entry::Standard(standard)));
                // SAFETY: the selector is the target class's one action, and
                // the session keeps the target alive as long as the menu.
                unsafe {
                    item.setTarget(Some(target));
                    item.setAction(Some(sel!(schnellUiMenuItemChosen:)));
                }
            }
            (None, None) => {}
        }
        apply_key_equivalent(&item, standard.shortcut());
        item
    }

    fn action(&self, action: &MenuAction) -> Retained<NSMenuItem> {
        let item = menu_item(self.mtm, &NSString::from_str(action.label()));
        if let Some(target) = self.target {
            item.setTag(self.push_entry(Entry::Action(action.id().to_owned())));
            // SAFETY: as in `standard`.
            unsafe {
                item.setTarget(Some(target));
                item.setAction(Some(sel!(schnellUiMenuItemChosen:)));
            }
        }
        item.setEnabled(action.is_enabled());
        // `NSControlStateValueOn` is 1 and `Off` is 0.
        item.setState(NSInteger::from(action.checked_state() == Some(true)));
        apply_key_equivalent(&item, action.key_equivalent());
        item
    }

    fn menu(&self, menu: &Menu) -> Retained<NSMenu> {
        let ns_menu = new_menu(self.mtm, menu.title());
        for entry in menu.items() {
            let item = match entry {
                MenuItem::Action(action) => self.action(action),
                MenuItem::Standard(standard) => self.standard(*standard),
                MenuItem::Separator => NSMenuItem::separatorItem(self.mtm),
                MenuItem::Submenu(sub) => {
                    let item = menu_item(self.mtm, &NSString::from_str(sub.title()));
                    item.setSubmenu(Some(&self.menu(sub)));
                    item
                }
            };
            ns_menu.addItem(&item);
        }
        ns_menu
    }

    fn bar(&self, bar: &MenuBar) -> Retained<NSMenu> {
        let main = new_menu(self.mtm, "");
        for menu in bar.menus() {
            let item = menu_item(self.mtm, &NSString::from_str(menu.title()));
            item.setSubmenu(Some(&self.menu(menu)));
            main.addItem(&item);
        }
        main
    }
}

/// The menu shown when no app menu is set: an application menu with Quit.
fn install_default(mtm: MainThreadMarker) {
    let name = app_name();
    let bar = MenuBar::new().menu(Menu::new(&name).item(StandardItem::Quit));
    let builder = Builder {
        mtm,
        app_name: &name,
        target: None,
    };
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&builder.bar(&bar)));
}

pub(super) fn start(menu: &MenuBar, sink: Arc<Sink>) -> Session {
    let Some(mtm) = MainThreadMarker::new() else {
        // Nothing can be shown from another thread. The caller asked for a
        // menu and gets none; the docs say the call belongs on the main thread.
        return Session { target: None };
    };
    let name = app_name();
    let target = MenuTarget::new(mtm, sink);
    let builder = Builder {
        mtm,
        app_name: &name,
        target: Some(&target),
    };
    let main = builder.bar(menu);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&main));
    Session {
        target: Some(target),
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(target) = self.target.take() else {
            return;
        };
        match MainThreadMarker::new() {
            // Replace the menu first: its items point at `target`, which is
            // released when this function returns.
            Some(mtm) => install_default(mtm),
            // Off the main thread the menu cannot be swapped safely and an
            // item could still send to the target. Leak it.
            None => std::mem::forget(target),
        }
    }
}

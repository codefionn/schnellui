//! The widget families under test. Each component mounts a small realistic
//! view, knows how to drive itself into one canonical state from any state,
//! and states what must stay true about its accessibility tree after every
//! input.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use schnellui::a11y::{A11yNodeDump, Role};
use schnellui::accesskit_action::{Action, ActionRequest};
use schnellui::accesskit_reexport::TreeId;
use schnellui::layout::Align;
use schnellui::scene::{Size, WidgetId};
use schnellui::signal::create_signal;
use schnellui::widgets::{
    Button, Checkbox, Column, ContextMenu, ContextMenuItem, Dialog, Dropdown, DropdownOption,
    GroupedTabList, Link, List, ListItem, Pad, Radio, Row, Scroll, Slider, Stack, Switch, Tab,
    TabBar, TabGroup, TabNode, Table, TableRow, Text, TextArea, TextInput, Theme, View, WrapMode,
};
use schnellui::{App, Remount, ThemeMode, WindowUpdate};

use crate::{HeadlessHost, HostKey};

/// How a component is mounted.
#[derive(Clone, Copy)]
pub struct Mount {
    pub theme: Theme,
    pub size: (u32, u32),
    pub scale: f32,
    /// Whether animations run. Off, the app behaves as under the platform's
    /// reduced-motion preference and every animation jumps to its end.
    pub animate: bool,
}

impl std::fmt::Debug for Mount {
    // A theme's derived Debug runs to hundreds of lines, which buries the
    // failing input. Name it by its place in THEMES instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let theme = crate::THEMES.iter().position(|t| *t == self.theme);
        f.debug_struct("Mount")
            .field(
                "theme",
                &theme.map_or("other".to_string(), |i| format!("THEMES[{i}]")),
            )
            .field("size", &self.size)
            .field("scale", &self.scale)
            .field("animate", &self.animate)
            .finish()
    }
}

/// A widget family under test.
pub struct Component {
    pub name: &'static str,
    /// The window size the component is designed for.
    pub size: (u32, u32),
    pub mount: fn(&Mount) -> HeadlessHost,
    /// Drives any state of the component to one fixed state, through
    /// accessibility actions and keys. `None` when the component keeps
    /// state no input can undo, such as a dialog's dragged position.
    pub canonical: Option<fn(&mut HeadlessHost)>,
    /// What must hold of the accessibility tree after every input.
    pub check: fn(&App) -> Result<(), String>,
}

/// Every component, for properties that run across all of them.
pub fn all() -> Vec<Component> {
    vec![
        BUTTONS,
        TOGGLES,
        SLIDERS,
        TABS,
        LIST,
        TABLE,
        TEXT_INPUT,
        TEXT_AREA,
        SCROLL,
        DROPDOWN,
        CONTEXT_MENU,
        DIALOGS,
        GROUPED_TABS,
        TODO,
    ]
}

/// Looks a component up by name.
pub fn named(name: &str) -> Component {
    all()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no component {name:?}"))
}

/// Mounts `view` as a themed app, so a theme switch rebuilds it from the
/// factory as an application's own theme switcher does.
fn host<V: View + 'static>(view: impl FnMut() -> V + 'static, mount: &Mount) -> HeadlessHost {
    let mut app = App::mount_themed_with_size_scaled(
        ThemeMode::Fixed(mount.theme),
        view,
        mount.size.0,
        mount.size.1,
        mount.scale,
    );
    app.apply_reduced_motion(!mount.animate);
    app.frame();
    HeadlessHost::new(app)
}

// --- accessibility helpers -------------------------------------------------

fn walk<'a>(node: &'a A11yNodeDump, out: &mut Vec<&'a A11yNodeDump>) {
    out.push(node);
    for child in &node.children {
        walk(child, out);
    }
}

/// Every node with `role`, in reading order.
pub fn nodes(app: &App, role: Role) -> Vec<A11yNodeDump> {
    let tree = schnellui::a11y::dump_tree(app.scene());
    let mut all = Vec::new();
    if let Some(root) = &tree.root {
        walk(root, &mut all);
    }
    all.into_iter()
        .filter(|n| n.role == role.label())
        .cloned()
        .collect()
}

fn has(node: &A11yNodeDump, state: &str) -> bool {
    node.state.iter().any(|s| s == state)
}

fn widget(app: &App, node: &A11yNodeDump) -> Option<WidgetId> {
    let scene = app.scene();
    let mut stack: Vec<WidgetId> = scene.root().into_iter().collect();
    while let Some(id) = stack.pop() {
        if schnellui::a11y::to_access_id(id).0 == node.id {
            return Some(id);
        }
        if let Some(n) = scene.node(id) {
            stack.extend(n.children.iter().copied());
        }
    }
    None
}

/// Sends an accessibility action to the first node with `role` and `name`.
pub fn act(host: &mut HeadlessHost, action: Action, role: Role, name: Option<&str>) -> bool {
    let Some(id) = host.app().find_widget(role, name) else {
        return false;
    };
    let done = host.app_mut().dispatch_action(&ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: schnellui::a11y::to_access_id(id),
        data: None,
    });
    host.frame();
    done
}

fn click(host: &mut HeadlessHost, role: Role, name: &str) {
    act(host, Action::Click, role, Some(name));
}

fn focus(host: &mut HeadlessHost, role: Role, name: Option<&str>) {
    if let Some(id) = host.app().find_widget(role, name) {
        host.app_mut().focus(Some(id));
        host.frame();
    }
}

fn at_most_one(app: &App, role: Role, state: &str) -> Result<(), String> {
    let on: Vec<_> = nodes(app, role)
        .into_iter()
        .filter(|n| has(n, state))
        .map(|n| n.name.unwrap_or_default())
        .collect();
    if on.len() > 1 {
        return Err(format!("{} {role:?} nodes are {state}: {on:?}", on.len()));
    }
    Ok(())
}

fn exactly_one(app: &App, role: Role, state: &str) -> Result<(), String> {
    at_most_one(app, role, state)?;
    if !nodes(app, role).iter().any(|n| has(n, state)) {
        return Err(format!("no {role:?} is {state}"));
    }
    Ok(())
}

fn fine(_: &App) -> Result<(), String> {
    Ok(())
}

/// Every text field's value is free of control characters other than the
/// line breaks a text area keeps.
fn clean_text(app: &App) -> Result<(), String> {
    for role in [Role::TextInput, Role::MultilineTextInput] {
        for node in nodes(app, role) {
            let value = node.value.unwrap_or_default();
            let bad = value
                .chars()
                .any(|c| c.is_control() && !(role == Role::MultilineTextInput && c == '\n'));
            if bad {
                return Err(format!("{role:?} {:?} holds {value:?}", node.name));
            }
        }
    }
    Ok(())
}

// --- buttons and links -----------------------------------------------------

fn buttons(mount: &Mount) -> HeadlessHost {
    let last = create_signal("none");
    host(
        move || {
            Stack::new().fill().child(
                Pad::all(24.0).child(
                    Column::new()
                        .gap(12.0)
                        .child(
                            Row::new()
                                .gap(10.0)
                                .child(Button::new("Count").on_click(move || last.set("Count")))
                                .child(Button::new("Off").disabled(true))
                                .child(Link::new("Docs").on_click(move || last.set("Docs")))
                                .child(Text::dynamic(move || format!("last {}", last.get()))),
                        )
                        .child(
                            Row::new()
                                .fill()
                                .child(Text::new("Edge"))
                                .child(Column::new().fill())
                                .child(
                                    Button::new("Save")
                                        .tooltip("Save every open document to disk")
                                        .on_click(move || last.set("Save")),
                                ),
                        ),
                ),
            )
        },
        mount,
    )
}

pub const BUTTONS: Component = Component {
    name: "buttons",
    size: (520, 240),
    mount: buttons,
    canonical: Some(|host| click(host, Role::Button, "Count")),
    check: fine,
};

// --- checkbox, switch, radio -----------------------------------------------

fn toggles(mount: &Mount) -> HeadlessHost {
    // Controlled, as an application keeps them, so a theme rebuild keeps
    // what the person chose.
    let email = create_signal(false);
    let sync = create_signal(true);
    let often = create_signal("Daily");
    host(
        move || {
            let labelled = |control: Box<dyn View>, label: &'static str| {
                Row::new()
                    .gap(8.0)
                    .align(Align::Center)
                    .child(control)
                    .child(Text::new(label))
            };
            let mut radios = Row::new().gap(8.0).align(Align::Center);
            for label in ["Daily", "Weekly", "Monthly"] {
                // One row of siblings is one exclusive radio group.
                radios = radios
                    .child(
                        Radio::new(often.get() == label)
                            .name(label)
                            .on_select(move || often.set(label)),
                    )
                    .child(Text::new(label));
            }
            Pad::all(20.0).child(
                Column::new()
                    .gap(10.0)
                    .child(labelled(
                        Box::new(
                            Checkbox::new(email.get())
                                .name("Email")
                                .on_toggle(move |on| email.set(on)),
                        ),
                        "Email",
                    ))
                    .child(labelled(
                        Box::new(
                            Switch::new(sync.get())
                                .name("Sync")
                                .on_toggle(move |on| sync.set(on)),
                        ),
                        "Sync",
                    ))
                    .child(radios),
            )
        },
        mount,
    )
}

fn toggles_canonical(host: &mut HeadlessHost) {
    let checked = |host: &HeadlessHost, role, name: &str| {
        nodes(host.app(), role)
            .iter()
            .any(|n| n.name.as_deref() == Some(name) && has(n, "checked"))
    };
    if checked(host, Role::CheckBox, "Email") {
        click(host, Role::CheckBox, "Email");
    }
    if !checked(host, Role::Switch, "Sync") {
        click(host, Role::Switch, "Sync");
    }
    click(host, Role::Radio, "Daily");
}

pub const TOGGLES: Component = Component {
    name: "toggles",
    size: (420, 200),
    mount: toggles,
    canonical: Some(toggles_canonical),
    check: |app| exactly_one(app, Role::Radio, "checked"),
};

// --- sliders ---------------------------------------------------------------

fn sliders(mount: &Mount) -> HeadlessHost {
    let volume = create_signal(40.0f32);
    let balance = create_signal(0.3f32);
    let offset = create_signal(7.0f32);
    host(
        move || {
            Pad::all(20.0).child(
                Column::new()
                    .gap(14.0)
                    .child(
                        Slider::new(volume.get(), 0.0, 100.0)
                            .step(5.0)
                            .name("Volume")
                            .on_change(move |v| volume.set(v)),
                    )
                    .child(
                        Slider::new(balance.get(), 0.0, 1.0)
                            .step(0.1)
                            .name("Balance")
                            .on_change(move |v| balance.set(v)),
                    )
                    .child(
                        Slider::new(offset.get(), -10.0, 10.0)
                            .name("Offset")
                            .on_change(move |v| offset.set(v)),
                    ),
            )
        },
        mount,
    )
}

fn sliders_canonical(host: &mut HeadlessHost) {
    for name in ["Volume", "Balance", "Offset"] {
        focus(host, Role::Slider, Some(name));
        host.key(&HostKey::Home, false, false);
    }
}

fn sliders_check(app: &App) -> Result<(), String> {
    for (name, min, max, step) in [
        ("Volume", 0.0, 100.0, 5.0),
        ("Balance", 0.0, 1.0, 0.1),
        ("Offset", -10.0, 10.0, 0.0),
    ] {
        let node = nodes(app, Role::Slider)
            .into_iter()
            .find(|n| n.name.as_deref() == Some(name))
            .ok_or_else(|| format!("slider {name} is gone"))?;
        let raw = node.value.unwrap_or_default();
        let value: f64 = raw
            .trim()
            .parse()
            .map_err(|_| format!("slider {name} reads {raw:?}"))?;
        if !(min - 1e-6..=max + 1e-6).contains(&value) {
            return Err(format!("slider {name} reads {value}, outside {min}..{max}"));
        }
        if step > 0.0 {
            let k = (value - min) / step;
            if (k - k.round()).abs() > 1e-3 {
                return Err(format!("slider {name} reads {value}, off its step {step}"));
            }
        }
    }
    Ok(())
}

pub const SLIDERS: Component = Component {
    name: "sliders",
    size: (360, 200),
    mount: sliders,
    canonical: Some(sliders_canonical),
    check: sliders_check,
};

// --- tabs ------------------------------------------------------------------

fn tabs(mount: &Mount) -> HeadlessHost {
    let chosen = create_signal("General");
    host(
        move || {
            let tab = |label: &'static str| {
                Tab::new(label)
                    .selected(chosen.get() == label)
                    .on_select(move || chosen.set(label))
            };
            Pad::all(16.0).child(
                Column::new()
                    .gap(10.0)
                    .child(
                        TabBar::new()
                            .gap(2.0)
                            .child(tab("General"))
                            .child(tab("Privacy"))
                            .child(tab("About")),
                    )
                    .child(Text::dynamic(move || format!("showing {}", chosen.get()))),
            )
        },
        mount,
    )
}

pub const TABS: Component = Component {
    name: "tabs",
    size: (380, 160),
    mount: tabs,
    canonical: Some(|host| click(host, Role::Tab, "General")),
    check: |app| exactly_one(app, Role::Tab, "selected"),
};

// --- list ------------------------------------------------------------------

fn list(mount: &Mount) -> HeadlessHost {
    let chosen = create_signal("Inbox");
    host(
        move || {
            let mut list = List::new();
            for label in ["Inbox", "Archive", "Drafts", "Spam"] {
                list = list.child(
                    ListItem::new(label)
                        .selected(chosen.get() == label)
                        .on_select(move || chosen.set(label)),
                );
            }
            Column::new()
                .fill()
                .align(Align::Stretch)
                .child(Pad::all(12.0).child(list))
        },
        mount,
    )
}

pub const LIST: Component = Component {
    name: "list",
    size: (300, 220),
    mount: list,
    canonical: Some(|host| click(host, Role::ListItem, "Inbox")),
    check: |app| exactly_one(app, Role::ListItem, "selected"),
};

// --- table -----------------------------------------------------------------

fn table(mount: &Mount) -> HeadlessHost {
    let row = create_signal(0usize);
    host(
        move || {
            Pad::all(12.0).child(
                Table::new()
                    .push_row(TableRow::new().header().cell("Name").cell("Age"))
                    .row(["Ada Lovelace", "36"])
                    .row(["Grace Hopper", "85"])
                    .row(["Edsger Dijkstra", "72"])
                    .selected_row(row.get())
                    .on_select_row(move |i| row.set(i)),
            )
        },
        mount,
    )
}

fn table_canonical(host: &mut HeadlessHost) {
    let first = nodes(host.app(), Role::TableRow).into_iter().find(|n| {
        !n.children
            .iter()
            .any(|c| c.role == Role::ColumnHeader.label())
    });
    if let Some(id) = first.and_then(|n| widget(host.app(), &n)) {
        host.app_mut().dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: schnellui::a11y::to_access_id(id),
            data: None,
        });
        host.frame();
    }
}

pub const TABLE: Component = Component {
    name: "table",
    size: (420, 220),
    mount: table,
    canonical: Some(table_canonical),
    check: |app| at_most_one(app, Role::TableRow, "selected"),
};

// --- text fields -----------------------------------------------------------

fn retype(host: &mut HeadlessHost, role: Role, name: Option<&str>, text: &str) {
    focus(host, role, name);
    host.key(&HostKey::SelectAll, false, false);
    host.key(&HostKey::Backspace, false, false);
    for c in text.chars() {
        host.key(&HostKey::Text(c.to_string()), false, false);
    }
    host.blur();
}

fn text_input(mount: &Mount) -> HeadlessHost {
    // Controlled, as an application keeps them: a theme switch rebuilds the
    // view from these signals and the typed text survives.
    let project = create_signal(String::from("Apollo"));
    let owner = create_signal(String::new());
    host(
        move || {
            Pad::all(16.0).child(
                Column::new()
                    .gap(10.0)
                    .child(
                        TextInput::new(project.get())
                            .label("Project name")
                            .on_input(move |v: &str| project.set(v.to_owned())),
                    )
                    .child(
                        TextInput::new(owner.get())
                            .placeholder("Owner")
                            .label("Owner")
                            .on_input(move |v: &str| owner.set(v.to_owned())),
                    ),
            )
        },
        mount,
    )
}

fn text_input_canonical(host: &mut HeadlessHost) {
    for name in ["Project name", "Owner"] {
        retype(host, Role::TextInput, Some(name), "Zed");
    }
}

pub const TEXT_INPUT: Component = Component {
    name: "text_input",
    size: (360, 200),
    mount: text_input,
    canonical: Some(text_input_canonical),
    check: clean_text,
};

fn text_area(mount: &Mount) -> HeadlessHost {
    let text = create_signal(String::from(
        "first line\na second line long enough to wrap at a narrow window width\nthird",
    ));
    host(
        move || {
            Pad::all(16.0).child(
                TextArea::new(text.get())
                    .rows(4)
                    .wrap(WrapMode::Word)
                    .on_input(move |v: &str| text.set(v.to_owned())),
            )
        },
        mount,
    )
}

pub const TEXT_AREA: Component = Component {
    name: "text_area",
    size: (360, 240),
    mount: text_area,
    canonical: Some(|host| retype(host, Role::MultilineTextInput, None, "one\ntwo")),
    check: clean_text,
};

// --- scroll ----------------------------------------------------------------

fn scroll(mount: &Mount) -> HeadlessHost {
    let chosen = create_signal(0usize);
    host(
        move || {
            let mut rows = List::new();
            for i in 0..30 {
                rows = rows.child(
                    ListItem::new(format!("Row {i}"))
                        .selected(chosen.get() == i)
                        .on_select(move || chosen.set(i)),
                );
            }
            Pad::all(12.0).child(Scroll::new().size(240.0, 180.0).child(rows))
        },
        mount,
    )
}

fn scroll_canonical(host: &mut HeadlessHost) {
    if let Some(r) = host
        .app()
        .find_widget(Role::ScrollView, None)
        .and_then(|id| host.app().scene().layout(id))
        .map(|l| l.rect)
    {
        host.pointer_move(schnellui::scene::Point {
            x: r.x + r.width / 2.0,
            y: r.y + r.height / 2.0,
        });
        for _ in 0..40 {
            host.wheel(-400.0);
        }
        host.pointer_leave();
    }
    click(host, Role::ListItem, "Row 0");
}

pub const SCROLL: Component = Component {
    name: "scroll",
    size: (300, 240),
    mount: scroll,
    canonical: Some(scroll_canonical),
    check: fine,
};

// --- dropdown, with the remount an app does to open it ----------------------

#[derive(Clone, Copy)]
struct Choice {
    open: bool,
    chosen: usize,
}

const DENSITIES: [&str; 3] = ["Compact", "Comfortable", "Spacious"];

fn dropdown_view(state: Rc<Cell<Choice>>, dirty: Rc<Cell<bool>>) -> impl View {
    let now = state.get();
    let mut dropdown = Dropdown::new("Density").open(now.open);
    for (i, label) in DENSITIES.into_iter().enumerate() {
        let (state, dirty) = (state.clone(), dirty.clone());
        dropdown = dropdown.option(
            DropdownOption::new(label)
                .selected(i == now.chosen)
                .on_select(move || {
                    state.set(Choice {
                        open: false,
                        chosen: i,
                    });
                    dirty.set(true);
                }),
        );
    }
    let (toggle_state, toggle_dirty) = (state.clone(), dirty.clone());
    dropdown = dropdown.on_toggle(move || {
        let now = toggle_state.get();
        toggle_state.set(Choice {
            open: !now.open,
            ..now
        });
        toggle_dirty.set(true);
    });
    Pad::all(16.0).child(
        Column::new()
            .gap(10.0)
            .child(
                Row::new()
                    .gap(10.0)
                    .align(Align::Center)
                    .child(Text::new("Density"))
                    .child(dropdown),
            )
            .child(Button::new("Apply")),
    )
}

fn dropdown(mount: &Mount) -> HeadlessHost {
    let state = Rc::new(Cell::new(Choice {
        open: false,
        chosen: 0,
    }));
    let dirty = Rc::new(Cell::new(false));
    let mut host = {
        let (state, dirty) = (state.clone(), dirty.clone());
        host(move || dropdown_view(state.clone(), dirty.clone()), mount)
    };
    let (scale, animate) = (mount.scale, mount.animate);
    host.set_remount(move |size: Size, theme: Theme| {
        if !dirty.replace(false) {
            return None;
        }
        let (state, dirty) = (state.clone(), dirty.clone());
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme),
            move || dropdown_view(state.clone(), dirty.clone()),
            size.width as u32,
            size.height as u32,
            scale,
        );
        app.apply_reduced_motion(!animate);
        Some(WindowUpdate::Remount(Remount::new(app, "dropdown")))
    });
    host
}

fn dropdown_canonical(host: &mut HeadlessHost) {
    let open = |host: &HeadlessHost| {
        nodes(host.app(), Role::ComboBox)
            .iter()
            .any(|n| has(n, "expanded"))
    };
    if open(host) {
        click(host, Role::ComboBox, "Density");
    }
    click(host, Role::ComboBox, "Density");
    click(host, Role::ListBoxOption, "Compact");
    if open(host) {
        click(host, Role::ComboBox, "Density");
    }
}

fn dropdown_check(app: &App) -> Result<(), String> {
    let expanded = nodes(app, Role::ComboBox)
        .iter()
        .any(|n| has(n, "expanded"));
    let options = nodes(app, Role::ListBoxOption).len();
    match (expanded, options) {
        (true, 0) => Err("the dropdown reads expanded but shows no options".into()),
        (false, n) if n > 0 => Err(format!(
            "the dropdown reads collapsed but shows {n} options"
        )),
        _ => at_most_one(app, Role::ListBoxOption, "selected"),
    }
}

pub const DROPDOWN: Component = Component {
    name: "dropdown",
    size: (360, 260),
    mount: dropdown,
    canonical: Some(dropdown_canonical),
    check: dropdown_check,
};

// --- context menu ----------------------------------------------------------

fn context_menu(mount: &Mount) -> HeadlessHost {
    let said = create_signal("nothing");
    let text = create_signal(String::from("right-click me"));
    host(
        move || {
            Pad::all(16.0).child(
                Column::new()
                    .gap(10.0)
                    .child(
                        TextInput::new(text.get())
                            .on_input(move |v: &str| text.set(v.to_owned()))
                            .context_menu(
                                ContextMenu::new()
                                    .item(
                                        ContextMenuItem::new("Shout")
                                            .on_select(move || said.set("Shout")),
                                    )
                                    .item(
                                        ContextMenuItem::new("Whisper")
                                            .on_select(move || said.set("Whisper")),
                                    ),
                            ),
                    )
                    .child(Text::dynamic(move || format!("said {}", said.get()))),
            )
        },
        mount,
    )
}

pub const CONTEXT_MENU: Component = Component {
    name: "context_menu",
    size: (360, 220),
    mount: context_menu,
    canonical: None,
    check: clean_text,
};

// --- dialogs ---------------------------------------------------------------

fn dialogs(mount: &Mount) -> HeadlessHost {
    let note = create_signal(String::from("note"));
    host(
        move || {
            Stack::new()
                .fill()
                .child(Pad::all(16.0).child(Text::new("Desk")))
                .child(
                    Pad::all(24.0).child(
                        Column::new()
                            .fill()
                            .child(
                                Dialog::new("Issue board")
                                    .modeless()
                                    .non_fixed()
                                    .movable()
                                    .resizable()
                                    .at(28.0, 60.0)
                                    .size(300.0, 180.0)
                                    .child(
                                        Text::new("Margin note 12, confirm the caption.")
                                            .wrap(WrapMode::Word),
                                    )
                                    .child(Button::new("Resolve").tooltip("Resolve the note"))
                                    .child(
                                        TextInput::new(note.get())
                                            .on_input(move |v: &str| note.set(v.to_owned())),
                                    ),
                            )
                            .child(
                                Dialog::new("Proof preview")
                                    .modeless()
                                    .non_fixed()
                                    .movable()
                                    .resizable()
                                    .at(200.0, 120.0)
                                    .size(320.0, 200.0)
                                    .child(Text::new("FIELD GUIDE").size(25.0))
                                    .child(Button::new("Approve")),
                            ),
                    ),
                )
        },
        mount,
    )
}

pub const DIALOGS: Component = Component {
    name: "dialogs",
    size: (720, 520),
    mount: dialogs,
    canonical: None,
    check: clean_text,
};

// --- grouped tabs ----------------------------------------------------------

fn grouped_tabs(mount: &Mount) -> HeadlessHost {
    let chosen = create_signal("Overview");
    let last = create_signal(String::new());
    let tab = move |label: &'static str| {
        TabNode::new(label)
            .selected(chosen.get() == label)
            .on_select(move || chosen.set(label))
            .action(
                Button::new(format!("Archive {label}"))
                    .tooltip(format!("Archive {label}"))
                    .on_click(move || last.set(format!("archived {label}"))),
            )
    };
    host(
        move || {
            Row::new()
                .gap(16.0)
                .child(
                    GroupedTabList::new().groups([
                        TabGroup::new("PROJECT")
                            .tab(tab("Overview"))
                            .tab(tab("Planning"))
                            .tab(tab("Notes")),
                        TabGroup::new("TEAM").tab(tab("People")).tab(tab("Rota")),
                    ]),
                )
                .child(
                    Column::new()
                        .gap(8.0)
                        .child(Text::dynamic(move || format!("on {}", chosen.get())))
                        .child(Text::dynamic(move || last.get())),
                )
        },
        mount,
    )
}

pub const GROUPED_TABS: Component = Component {
    name: "grouped_tabs",
    size: (520, 320),
    mount: grouped_tabs,
    canonical: None,
    check: fine,
};

// --- todo list, changed by remounts ----------------------------------------

#[derive(Default)]
struct Todos {
    items: Vec<(String, bool)>,
    added: usize,
}

fn todo_view(state: Rc<RefCell<Todos>>, dirty: Rc<Cell<bool>>) -> impl View {
    let mut list = Column::new().gap(4.0);
    for (i, (label, done)) in state.borrow().items.iter().enumerate() {
        let (toggle_state, toggle_dirty) = (state.clone(), dirty.clone());
        let (remove_state, remove_dirty) = (state.clone(), dirty.clone());
        list = list.child(
            Row::new()
                .gap(8.0)
                .align(Align::Center)
                .child(
                    Checkbox::new(*done)
                        .name(label.clone())
                        .on_toggle(move |on| {
                            if let Some(item) = toggle_state.borrow_mut().items.get_mut(i) {
                                item.1 = on;
                            }
                            toggle_dirty.set(true);
                        }),
                )
                .child(Text::new(label.clone()))
                .child(Button::new(format!("Remove {label}")).on_click(move || {
                    let mut todos = remove_state.borrow_mut();
                    if i < todos.items.len() {
                        todos.items.remove(i);
                    }
                    remove_dirty.set(true);
                })),
        );
    }
    let (add_state, add_dirty) = (state.clone(), dirty.clone());
    Pad::all(16.0).child(
        Column::new()
            .gap(10.0)
            .child(Button::new("Add").on_click(move || {
                let mut todos = add_state.borrow_mut();
                todos.added += 1;
                let label = format!("task {}", todos.added);
                todos.items.push((label, false));
                add_dirty.set(true);
            }))
            .child(list),
    )
}

fn todo(mount: &Mount) -> HeadlessHost {
    let state = Rc::new(RefCell::new(Todos {
        items: vec![("milk".into(), false), ("bread".into(), true)],
        added: 0,
    }));
    let dirty = Rc::new(Cell::new(false));
    let mut host = {
        let (state, dirty) = (state.clone(), dirty.clone());
        host(move || todo_view(state.clone(), dirty.clone()), mount)
    };
    let (scale, animate) = (mount.scale, mount.animate);
    host.set_remount(move |size: Size, theme: Theme| {
        if !dirty.replace(false) {
            return None;
        }
        let (state, dirty) = (state.clone(), dirty.clone());
        let mut app = App::mount_themed_with_size_scaled(
            ThemeMode::Fixed(theme),
            move || todo_view(state.clone(), dirty.clone()),
            size.width as u32,
            size.height as u32,
            scale,
        );
        app.apply_reduced_motion(!animate);
        Some(WindowUpdate::Remount(Remount::new(app, "todo")))
    });
    host
}

/// Removes every task. Whatever happened before, the list ends empty.
fn todo_canonical(host: &mut HeadlessHost) {
    for _ in 0..64 {
        let Some(name) = nodes(host.app(), Role::Button)
            .into_iter()
            .filter_map(|n| n.name)
            .find(|n| n.starts_with("Remove "))
        else {
            break;
        };
        click(host, Role::Button, &name);
    }
}

fn todo_check(app: &App) -> Result<(), String> {
    let boxes = nodes(app, Role::CheckBox).len();
    let removes = nodes(app, Role::Button)
        .iter()
        .filter(|n| n.name.as_deref().is_some_and(|n| n.starts_with("Remove ")))
        .count();
    if boxes != removes {
        return Err(format!("{boxes} checkboxes but {removes} remove buttons"));
    }
    Ok(())
}

pub const TODO: Component = Component {
    name: "todo",
    size: (360, 320),
    mount: todo,
    canonical: Some(todo_canonical),
    check: todo_check,
};

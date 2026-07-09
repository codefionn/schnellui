//! # schnellui::a11y
//!
//! Accessibility as a **first-class second rendering target** (SOUL §6). This crate
//! turns the retained [`Scene`](schnellui::scene::Scene) into an **AccessKit**
//! `TreeUpdate` — full at mount, **incremental** from the scene's a11y-dirty set on
//! every later frame (SOUL §6.2) — routes inbound `ActionRequest`s to the same
//! handlers as pointer input (SOUL §6.3), and emits an owned-serde **JSON tree
//! dump** for the agent loop and snapshot tests (SOUL §6.5, §7.1 `--dump-a11y`).
//!
//! AccessKit's own types are not `serde`, so the JSON dump uses local structs.

use schnellui::scene::{A11yData, Point, Scene, WidgetId, WidgetKind};
use serde::{Deserialize, Serialize};
use slotmap::SecondaryMap;
use smallvec::SmallVec;

/// Re-export of the AccessKit types on schnellui's public boundary, so downstream
/// crates route actions without depending on `accesskit` directly (SOUL §6.3).
pub mod accesskit_reexport {
    pub use accesskit::TreeInfo as Tree;
    pub use accesskit::{
        Action, ActionData, ActionRequest, Node, NodeId, Role, TreeId, TreeInfo, TreeUpdate,
    };
}

/// Declares [`Role`] and every per-role mapping from one table, so the stored
/// tag, the AccessKit role, the snake_case dump label and the kebab-case DOM
/// name cannot drift apart. Each row reads
/// `Variant = tag => accesskit::Role, "dump_label", "dom-name";`.
macro_rules! roles {
    ($($(#[$meta:meta])* $variant:ident = $tag:literal => $accesskit:ident, $label:literal, $dom:literal;)+) => {
        /// AccessKit roles schnellui uses (SOUL §6.1). Stored in the scene a11y column as a
        /// `u16` tag ([`Role::as_u16`]) so `schnellui::scene` needs no accesskit dependency;
        /// mapped to [`accesskit::Role`] at tree-build time.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        #[repr(u16)]
        pub enum Role {
            $($(#[$meta])* $variant = $tag,)+
        }

        impl Role {
            /// Every role, in tag order.
            pub const ALL: &'static [Role] = &[$(Role::$variant,)+];

            /// Reconstructs a role from its stored tag (defaults to `Group`).
            pub fn from_u16(v: u16) -> Role {
                match v {
                    $($tag => Role::$variant,)+
                    _ => Role::Group,
                }
            }

            /// Maps to the AccessKit role used in the real `TreeUpdate` (SOUL §6.1).
            pub fn to_accesskit(self) -> accesskit::Role {
                match self {
                    $(Role::$variant => accesskit::Role::$accesskit,)+
                }
            }

            /// A stable snake_case name for the JSON dump (SOUL §6.5).
            pub const fn label(self) -> &'static str {
                match self {
                    $(Role::$variant => $label,)+
                }
            }

            /// The kebab-case name native HTML emits as `data-sui-role` and
            /// scenario drivers match against.
            pub const fn dom_name(self) -> &'static str {
                match self {
                    $(Role::$variant => $dom,)+
                }
            }
        }
    };
}

roles! {
    /// a layout container with no semantics of its own (§8.1).
    Group = 0 => Group, "group", "group";
    Label = 1 => Label, "label", "label";
    Button = 2 => Button, "button", "button";
    CheckBox = 3 => CheckBox, "checkbox", "checkbox";
    Slider = 4 => Slider, "slider", "slider";
    TextInput = 5 => TextInput, "text_input", "text-input";
    Image = 6 => Image, "image", "image";
    List = 7 => List, "list", "list";
    /// a live status region (e.g. the counter value, §7.5).
    Status = 8 => Status, "status", "status";
    // Roles for the widgets/charts groundwork (SOUL §6.1). Stable u16 tags appended
    // after `Status = 8` so the scene a11y column stays wire-stable.
    /// a determinate/indeterminate progress bar.
    ProgressIndicator = 9 => ProgressIndicator, "progress_indicator", "progress-indicator";
    /// an on/off switch (distinct from a checkbox's tri-state).
    Switch = 10 => Switch, "switch", "switch";
    /// one option in a radio group.
    Radio = 11 => RadioButton, "radio", "radio";
    /// a scrollable viewport (SOUL §3.2 scroll).
    ScrollView = 12 => ScrollView, "scroll_view", "scroll-view";
    /// a data chart / figure (bar / line / sparkline). accesskit 0.24 has no
    /// dedicated "chart" role; `Figure` is the closest (a self-contained
    /// graphical unit with an accessible summary).
    Chart = 13 => Figure, "chart", "chart";
    // Roles for the navigation/selection components (SOUL §6.1). Appended after
    // `Chart = 13` so the scene a11y column stays wire-stable.
    /// an inline navigation link.
    Link = 14 => Link, "link", "link";
    /// one tab of a tab bar.
    Tab = 15 => Tab, "tab", "tab";
    /// the tab bar itself (the container of [`Role::Tab`]s).
    TabList = 16 => TabList, "tab_list", "tab-list";
    /// one entry of a [`Role::List`].
    ListItem = 17 => ListItem, "list_item", "list-item";
    // Table roles (SOUL §6.1). Appended after `ListItem = 17` so the scene a11y
    // column stays wire-stable. Row/column counts and per-cell indices are
    // *derived from the retained tree* at tree-build time (see `table_facts`), so
    // the scene column carries no duplicate table geometry.
    /// a data table (the container of [`Role::TableRow`]s).
    Table = 18 => Table, "table", "table";
    /// one row of a table.
    TableRow = 19 => Row, "row", "table-row";
    /// one data cell of a table row.
    Cell = 20 => Cell, "cell", "cell";
    /// one header cell of a table's header row.
    ColumnHeader = 21 => ColumnHeader, "column_header", "column-header";
    // Rich text roles (SOUL §6.1). Appended after `ColumnHeader = 21` so the
    // scene a11y column stays wire-stable.
    /// a formatted read-only document (the rich text viewer).
    Document = 22 => Document, "document", "document";
    /// a multi-line editable text area (the rich text source editor).
    MultilineTextInput = 23 => MultilineTextInput, "multiline_text_input", "multiline-text-input";
    // Dropdown roles (SOUL §6.1). Appended after `MultilineTextInput = 23` so the
    // scene a11y column stays wire-stable.
    /// a dropdown's trigger: name = its label, value = the chosen option,
    /// `EXPANDED` while the option list is showing.
    ComboBox = 24 => ComboBox, "combo_box", "combo-box";
    /// one option of an open dropdown's list.
    ListBoxOption = 25 => ListBoxOption, "list_box_option", "list-box-option";
    /// A dialog that can be left open while the rest of the application remains
    /// interactive.
    Dialog = 26 => Dialog, "dialog", "dialog";
    /// A modal alert dialog that requires the user's attention.
    AlertDialog = 27 => AlertDialog, "alert_dialog", "alert-dialog";
    /// A transient context menu.
    Menu = 28 => Menu, "menu", "menu";
    /// One command in a context menu.
    MenuItem = 29 => MenuItem, "menu_item", "menu-item";
    /// A single-line editable whose value is visually and semantically protected.
    PasswordInput = 30 => PasswordInput, "password_input", "password-input";
}

impl Role {
    /// The `u16` tag stored in the scene a11y column.
    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// Packed semantic-state bits stored in the scene a11y column (SOUL §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StateFlags(pub u32);

impl StateFlags {
    pub const CHECKED: StateFlags = StateFlags(1 << 0);
    pub const DISABLED: StateFlags = StateFlags(1 << 1);
    pub const EXPANDED: StateFlags = StateFlags(1 << 2);
    pub const SELECTED: StateFlags = StateFlags(1 << 3);
    pub const FOCUSED: StateFlags = StateFlags(1 << 4);
    /// The dialog blocks interaction with content outside its subtree.
    pub const MODAL: StateFlags = StateFlags(1 << 5);
    /// The node exposes a binary expanded/collapsed state. `EXPANDED` distinguishes
    /// the open state; without it, the node is explicitly collapsed.
    pub const COLLAPSIBLE: StateFlags = StateFlags(1 << 6);

    #[inline]
    pub fn contains(self, f: StateFlags) -> bool {
        (self.0 & f.0) == f.0 && f.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, f: StateFlags) {
        self.0 |= f.0;
    }
    /// Decodes set bits to stable snake_case names for the JSON dump.
    pub fn names(self) -> Vec<String> {
        let mut v = Vec::new();
        for (bit, name) in [
            (Self::CHECKED, "checked"),
            (Self::DISABLED, "disabled"),
            (Self::EXPANDED, "expanded"),
            (Self::SELECTED, "selected"),
            (Self::FOCUSED, "focused"),
            (Self::MODAL, "modal"),
        ] {
            if self.contains(bit) {
                v.push(name.to_string());
            }
        }
        if self.contains(Self::COLLAPSIBLE) && !self.contains(Self::EXPANDED) {
            v.push("collapsed".to_string());
        }
        v
    }
}

/// The active ordering advertised by a sortable table column header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDirection {
    Ascending = 1,
    Descending = 2,
}

impl SortDirection {
    /// The compact tag stored in [`schnellui::scene::A11yData`].
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    /// Decodes the retained scene tag. Unknown tags remain safely unset.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Ascending),
            2 => Some(Self::Descending),
            _ => None,
        }
    }

    /// The direction requested by activating this header.
    pub const fn toggled(self) -> Self {
        match self {
            Self::Ascending => Self::Descending,
            Self::Descending => Self::Ascending,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Ascending => "ascending",
            Self::Descending => "descending",
        }
    }
}

/// Packed supported-action bits (SOUL §6.1, §6.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ActionFlags(pub u32);

impl ActionFlags {
    pub const CLICK: ActionFlags = ActionFlags(1 << 0);
    pub const FOCUS: ActionFlags = ActionFlags(1 << 1);
    pub const SET_VALUE: ActionFlags = ActionFlags(1 << 2);
    pub const INCREMENT: ActionFlags = ActionFlags(1 << 3);
    pub const DECREMENT: ActionFlags = ActionFlags(1 << 4);
    pub const SCROLL_INTO_VIEW: ActionFlags = ActionFlags(1 << 5);
    /// scroll the viewport toward its start (SOUL §3.2 scroll; `accesskit::Action::ScrollUp`).
    pub const SCROLL_UP: ActionFlags = ActionFlags(1 << 6);
    /// scroll the viewport toward its end (SOUL §3.2 scroll; `accesskit::Action::ScrollDown`).
    pub const SCROLL_DOWN: ActionFlags = ActionFlags(1 << 7);
    /// Opens the widget's context menu.
    pub const SHOW_CONTEXT_MENU: ActionFlags = ActionFlags(1 << 8);

    #[inline]
    pub fn contains(self, f: ActionFlags) -> bool {
        (self.0 & f.0) == f.0 && f.0 != 0
    }
    #[inline]
    pub fn insert(&mut self, f: ActionFlags) {
        self.0 |= f.0;
    }
    /// Decodes set bits to stable names for the JSON dump.
    pub fn names(self) -> Vec<String> {
        let mut v = Vec::new();
        for (bit, name) in [
            (Self::CLICK, "click"),
            (Self::FOCUS, "focus"),
            (Self::SET_VALUE, "set_value"),
            (Self::INCREMENT, "increment"),
            (Self::DECREMENT, "decrement"),
            (Self::SCROLL_INTO_VIEW, "scroll_into_view"),
            (Self::SCROLL_UP, "scroll_up"),
            (Self::SCROLL_DOWN, "scroll_down"),
            (Self::SHOW_CONTEXT_MENU, "show_context_menu"),
        ] {
            if self.contains(bit) {
                v.push(name.to_string());
            }
        }
        v
    }
}

/// Maps a retained [`WidgetId`] to its AccessKit node id — the **same id** (SOUL
/// §6.2: "each retained node's `NodeId` *is* its AccessKit `NodeId`").
pub fn to_access_id(id: WidgetId) -> accesskit::NodeId {
    use slotmap::Key;
    accesskit::NodeId(id.data().as_ffi())
}

/// One node in the owned-serde JSON dump (SOUL §6.5). Mirrors the AccessKit node
/// but is serializable and stable for snapshot diffs. `Default` so hand-built
/// fixtures fill only the fields they assert (`..Default::default()`), staying
/// source-compatible as fields are appended.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct A11yNodeDump {
    pub id: u64,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<String>,
    // Derived table facts (SOUL §6.1): counts on a `table`, indices on a
    // `row`/`cell`/`column_header`. Absent (and unserialized) on other roles, so
    // non-table dumps are byte-identical to before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub row_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column_index: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<A11yNodeDump>,
}

/// The root of the JSON dump (SOUL §6.5, §7.1 `--dump-a11y`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct A11yTreeDump {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<A11yNodeDump>,
}

/// Returns the highest focus-grabbing dialog in visual/tree stacking order.
///
/// A modal dialog is an accessibility boundary: while it exists, content outside
/// this subtree is inert and must not remain in the screen-reader reading order.
/// Modeless dialogs never create this boundary, so several of them may coexist
/// beside the application and one another.
pub fn active_modal_root(scene: &Scene) -> Option<WidgetId> {
    scope(scene).0
}

// Test-only work counters: nodes visited by the tree walks and sibling role
// reads done for table facts. Thread-local so parallel tests do not interfere.
#[cfg(test)]
thread_local! {
    static WALK_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static ROLE_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline]
fn count_visit() {
    #[cfg(test)]
    WALK_VISITS.with(|c| c.set(c.get() + 1));
}

/// `(active modal root, focused widget)`, answered from the scene's cache and
/// recomputed with a tree walk only after a mutation that could change either.
fn scope(scene: &Scene) -> (Option<WidgetId>, Option<WidgetId>) {
    if let Some(cached) = scene.a11y_scope_cache() {
        return cached;
    }
    let modal = find_modal_root(scene);
    let root = modal.or_else(|| scene.root().filter(|root| scene.is_visible(*root)));
    let focus = root.and_then(|root| find_focused(scene, root));
    scene.set_a11y_scope_cache((modal, focus));
    (modal, focus)
}

fn find_modal_root(scene: &Scene) -> Option<WidgetId> {
    fn walk(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
        if !scene.is_visible(id) {
            return None;
        }
        count_visit();
        let node = scene.node(id)?;
        // Later siblings are higher in the dialog stack.
        for &child in node.children.iter().rev() {
            if let Some(found) = walk(scene, child) {
                return Some(found);
            }
        }
        let a = scene.a11y(id)?;
        let role = Role::from_u16(a.role);
        (matches!(role, Role::Dialog | Role::AlertDialog)
            && StateFlags(a.state).contains(StateFlags::MODAL))
        .then_some(id)
    }
    walk(scene, scene.root()?)
}

/// The root exposed to assistive technology. A focus-grabbing dialog temporarily
/// becomes the accessibility tree root, which removes the page, modeless peers,
/// and lower modal layers from both reading and action order.
fn accessibility_root(scene: &Scene) -> Option<WidgetId> {
    scope(scene)
        .0
        .or_else(|| scene.root().filter(|root| scene.is_visible(*root)))
}

fn is_in_subtree(scene: &Scene, id: WidgetId, ancestor: WidgetId) -> bool {
    let mut current = Some(id);
    while let Some(node) = current {
        if node == ancestor {
            return true;
        }
        current = scene.node(node).and_then(|entry| entry.parent);
    }
    false
}

/// Builds the serializable dump by walking the retained tree and reading the scene
/// a11y column (SOUL §6.5). Real, allocation-honest: it owns its `String`s.
pub fn dump_tree(scene: &Scene) -> A11yTreeDump {
    fn walk(scene: &Scene, id: WidgetId, facts: TableFacts) -> A11yNodeDump {
        use slotmap::Key;
        count_visit();
        let default_data;
        let a = match scene.a11y(id) {
            Some(a) => a,
            None => {
                default_data = A11yData::default();
                &default_data
            }
        };
        let mut children = Vec::new();
        for_each_child_facts(scene, id, facts.row_index, |child, child_facts| {
            if scene.is_visible(child) {
                children.push(walk(scene, child, child_facts));
            }
        });
        A11yNodeDump {
            id: id.data().as_ffi(),
            role: Role::from_u16(a.role).label().to_string(),
            name: a.name.clone(),
            value: a.value.clone(),
            state: StateFlags(a.state).names(),
            actions: ActionFlags(a.actions).names(),
            row_count: facts.row_count,
            column_count: facts.column_count,
            row_index: facts.row_index,
            column_index: facts.column_index,
            sort_direction: SortDirection::from_u8(a.sort_direction)
                .map(|direction| direction.label().to_string()),
            children,
        }
    }
    A11yTreeDump {
        focus: focused(scene)
            .or_else(|| active_modal_root(scene))
            .map(|id| to_access_id(id).0),
        root: accessibility_root(scene).map(|r| walk(scene, r, table_facts(scene, r))),
    }
}

/// Serializes the a11y tree to pretty JSON (SOUL §7.1 `--dump-a11y <path.json>`).
pub fn dump_json(scene: &Scene) -> String {
    serde_json::to_string_pretty(&dump_tree(scene)).expect("a11y dump serialization")
}

/// Derived table facts for a node (SOUL §6.1): row/column **counts** on a
/// [`Role::Table`], the **row index** on a [`Role::TableRow`], and **row + column
/// indices** on a [`Role::Cell`] / [`Role::ColumnHeader`]. All fields are `None`
/// on every other role.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TableFacts {
    pub row_count: Option<usize>,
    pub column_count: Option<usize>,
    pub row_index: Option<usize>,
    pub column_index: Option<usize>,
}

/// The role a node's a11y column carries (defaults to the transparent `Group`).
#[inline]
fn role_of(scene: &Scene, id: WidgetId) -> Role {
    #[cfg(test)]
    ROLE_READS.with(|c| c.set(c.get() + 1));
    scene
        .a11y(id)
        .map(|a| Role::from_u16(a.role))
        .unwrap_or(Role::Group)
}

/// A row's zero-based index among the row-role children of its parent table.
fn row_index_of(scene: &Scene, row: WidgetId) -> Option<usize> {
    let parent = scene.node(row)?.parent?;
    scene
        .node(parent)?
        .children
        .iter()
        .filter(|c| role_of(scene, **c) == Role::TableRow)
        .position(|c| *c == row)
}

/// Derives a node's [`TableFacts`] from the retained tree (SOUL §6.1). The tree is
/// the single source of truth — counts and indices are *positions in the retained
/// structure*, never duplicated state the scene column would have to keep in sync.
/// Because the a11y pass already re-sends a node whenever its structure changes
/// (SOUL §6.2), derived facts ride the same incremental `TreeUpdate` for free.
pub fn table_facts(scene: &Scene, id: WidgetId) -> TableFacts {
    match role_of(scene, id) {
        Role::Table => {
            let Some(node) = scene.node(id) else {
                return TableFacts::default();
            };
            let mut rows = 0usize;
            let mut cols = 0usize;
            for &c in &node.children {
                if role_of(scene, c) != Role::TableRow {
                    continue;
                }
                rows += 1;
                let row_cols = scene
                    .node(c)
                    .map(|r| {
                        r.children
                            .iter()
                            .filter(|cc| {
                                matches!(role_of(scene, **cc), Role::Cell | Role::ColumnHeader)
                            })
                            .count()
                    })
                    .unwrap_or(0);
                cols = cols.max(row_cols);
            }
            TableFacts {
                row_count: Some(rows),
                column_count: Some(cols),
                ..TableFacts::default()
            }
        }
        Role::TableRow => TableFacts {
            row_index: row_index_of(scene, id),
            ..TableFacts::default()
        },
        Role::Cell | Role::ColumnHeader => {
            let row = scene.node(id).and_then(|n| n.parent);
            let column_index = row.and_then(|r| {
                scene.node(r).and_then(|rn| {
                    rn.children
                        .iter()
                        .filter(|c| matches!(role_of(scene, **c), Role::Cell | Role::ColumnHeader))
                        .position(|c| *c == id)
                })
            });
            TableFacts {
                row_index: row.and_then(|r| row_index_of(scene, r)),
                column_index,
                ..TableFacts::default()
            }
        }
        _ => TableFacts::default(),
    }
}

/// Hands each child of `id` its [`TableFacts`] in one pass over the siblings.
///
/// [`table_facts`] answers one node by scanning its siblings (and its parent's),
/// so asking it for every child of a wide table is quadratic. A whole-tree walk
/// calls this once per parent instead: rows are counted among the `TableRow`
/// children, cells among the `Cell`/`ColumnHeader` children, and `row_index` is
/// this node's own row index, which its cells inherit. Results equal
/// [`table_facts`] for every child.
fn for_each_child_facts(
    scene: &Scene,
    id: WidgetId,
    row_index: Option<usize>,
    mut f: impl FnMut(WidgetId, TableFacts),
) {
    let Some(node) = scene.node(id) else { return };
    let cell_row = row_index.filter(|_| role_of(scene, id) == Role::TableRow);
    let (mut rows, mut cols) = (0usize, 0usize);
    for &child in &node.children {
        let facts = match role_of(scene, child) {
            Role::Table => table_facts(scene, child),
            Role::TableRow => {
                rows += 1;
                TableFacts {
                    row_index: Some(rows - 1),
                    ..TableFacts::default()
                }
            }
            Role::Cell | Role::ColumnHeader => {
                cols += 1;
                TableFacts {
                    row_index: cell_row,
                    column_index: Some(cols - 1),
                    ..TableFacts::default()
                }
            }
            _ => TableFacts::default(),
        };
        f(child, facts);
    }
}

/// Table facts for a batch of dirty nodes. [`table_facts`] answers a lone node
/// by scanning its siblings, so a table that moved as a whole (every cell dirty)
/// would cost O(R*C*(R+C)). Layout writes siblings next to each other, so this
/// keeps the facts of the last-seen parent: one `for_each_child_facts` pass per
/// row (for its cells) and one for the table (for its rows' indices).
#[derive(Default)]
struct TableMemo {
    /// Facts of each child of `table`, row indices included.
    table: Option<WidgetId>,
    rows: std::collections::HashMap<WidgetId, TableFacts>,
    /// Facts of each child of `row`, with the row's own index folded in.
    row: Option<WidgetId>,
    cells: std::collections::HashMap<WidgetId, TableFacts>,
}

impl TableMemo {
    fn facts(&mut self, scene: &Scene, id: WidgetId) -> TableFacts {
        let role = role_of(scene, id);
        if !matches!(role, Role::TableRow | Role::Cell | Role::ColumnHeader) {
            return table_facts(scene, id);
        }
        let Some(parent) = scene.node(id).and_then(|n| n.parent) else {
            return table_facts(scene, id);
        };
        if role == Role::TableRow {
            return self.row_facts(scene, parent, id);
        }
        // A cell inherits its row's index, which the row's own parent knows.
        let row_index = match scene.node(parent).and_then(|n| n.parent) {
            Some(table) => self.row_facts(scene, table, parent).row_index,
            None => None,
        };
        if self.row != Some(parent) {
            self.cells.clear();
            for_each_child_facts(scene, parent, row_index, |c, f| {
                self.cells.insert(c, f);
            });
            self.row = Some(parent);
        }
        self.cells.get(&id).copied().unwrap_or_default()
    }

    fn row_facts(&mut self, scene: &Scene, table: WidgetId, row: WidgetId) -> TableFacts {
        if self.table != Some(table) {
            self.rows.clear();
            for_each_child_facts(scene, table, None, |c, f| {
                self.rows.insert(c, f);
            });
            self.table = Some(table);
        }
        self.rows.get(&row).copied().unwrap_or_default()
    }
}

/// Assembles a single [`accesskit::Node`] from a widget's scene a11y column plus
/// its retained children (SOUL §6.1). Shared by the full and incremental builders so
/// a node is described identically whether it is (re)sent at mount or on a change —
/// AccessKit overwrites a node wholesale, so every field must be present each time.
#[derive(Clone, Copy)]
struct WindowTreeContext<'a> {
    root: WidgetId,
    scale: f32,
    title: &'a str,
}

fn build_node(
    scene: &Scene,
    id: WidgetId,
    window: Option<WindowTreeContext<'_>>,
    facts: TableFacts,
) -> accesskit::Node {
    let default_data;
    let a = match scene.a11y(id) {
        Some(a) => a,
        None => {
            default_data = A11yData::default();
            &default_data
        }
    };
    let role = Role::from_u16(a.role);
    // A schnellui layout root is normally a semantically transparent group. In a
    // native window tree that same node is the platform's top-level Window object;
    // preserving Group here would leave AT-SPI without a proper application window.
    let platform_role = if window.is_some_and(|context| context.root == id) && role == Role::Group {
        accesskit::Role::Window
    } else {
        role.to_accesskit()
    };
    let mut node = accesskit::Node::new(platform_role);
    if !scene.is_visible(id) {
        node.set_hidden();
    }

    if let Some(name) = a.name.as_deref() {
        node.set_label(name);
    } else if role == Role::Status {
        // Status text is retained in `value` so JSON snapshots can distinguish it
        // from an authored label. AT-SPI's StatusBar does not expose arbitrary text,
        // though, so mirror the value into AccessKit's label for native readers.
        if let Some(value) = a.value.as_deref() {
            node.set_label(value);
        }
    } else if let Some(context) = window.filter(|context| context.root == id) {
        node.set_label(context.title);
    }
    if let Some(value) = a.value.as_deref() {
        node.set_value(value);
    }

    // Bounds resolve property translations and clips in logical window
    // coordinates, without scrolling. A viewport publishes its scroll offset as
    // its own transform and clips its children, so scrolling changes one node's
    // transform instead of every descendant's bounds. The transform also applies
    // to the viewport's own bounds, which are therefore shifted back by the
    // offset. The root supplies logical to physical scaling.
    let is_viewport = scene.node(id).is_some_and(|n| n.kind == WidgetKind::Scroll);
    let scroll = if is_viewport {
        scene.scroll_offset(id)
    } else {
        Point::default()
    };
    if scene.layout(id).is_some() {
        let mut rect = scene.accessibility_rect(id);
        rect.x += scroll.x;
        rect.y += scroll.y;
        node.set_bounds(accesskit::Rect {
            x0: rect.x as f64,
            y0: rect.y as f64,
            x1: (rect.x + rect.width) as f64,
            y1: (rect.y + rect.height) as f64,
        });
    }
    let root_scale = window.filter(|context| context.root == id).map(|context| {
        if context.scale.is_finite() && context.scale > 0.0 {
            context.scale as f64
        } else {
            1.0
        }
    });
    let scrolled = scroll.x != 0.0 || scroll.y != 0.0;
    if root_scale.is_some() || scrolled {
        let scale = accesskit::Affine::scale(root_scale.unwrap_or(1.0));
        let offset = accesskit::Affine::translate((-scroll.x as f64, -scroll.y as f64));
        node.set_transform(scale * offset);
    }
    if is_viewport {
        node.set_clips_children();
    }

    let state = StateFlags(a.state);
    if role == Role::Status {
        node.set_live(accesskit::Live::Polite);
    }
    if state.contains(StateFlags::DISABLED) {
        node.set_disabled();
    }
    if state.contains(StateFlags::COLLAPSIBLE) {
        node.set_expanded(state.contains(StateFlags::EXPANDED));
    } else if state.contains(StateFlags::EXPANDED) {
        node.set_expanded(true);
    }
    if state.contains(StateFlags::MODAL) {
        node.set_modal();
    }
    if state.contains(StateFlags::SELECTED) {
        node.set_selected(true);
    }
    if let Some(direction) = SortDirection::from_u8(a.sort_direction) {
        node.set_sort_direction(match direction {
            SortDirection::Ascending => accesskit::SortDirection::Ascending,
            SortDirection::Descending => accesskit::SortDirection::Descending,
        });
    }
    // Checked → AccessKit's tri-state `Toggled`. A checkbox announces both states
    // ("checked"/"not checked"), so it always carries a value; other roles only
    // carry `Toggled` when actually checked.
    let checked = state.contains(StateFlags::CHECKED);
    if role == Role::CheckBox {
        node.set_toggled(if checked {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        });
    } else if checked {
        node.set_toggled(accesskit::Toggled::True);
    }

    let actions = ActionFlags(a.actions);
    for (flag, action) in [
        (ActionFlags::CLICK, accesskit::Action::Click),
        (ActionFlags::FOCUS, accesskit::Action::Focus),
        (ActionFlags::SET_VALUE, accesskit::Action::SetValue),
        (ActionFlags::INCREMENT, accesskit::Action::Increment),
        (ActionFlags::DECREMENT, accesskit::Action::Decrement),
        (
            ActionFlags::SCROLL_INTO_VIEW,
            accesskit::Action::ScrollIntoView,
        ),
        (ActionFlags::SCROLL_UP, accesskit::Action::ScrollUp),
        (ActionFlags::SCROLL_DOWN, accesskit::Action::ScrollDown),
        (
            ActionFlags::SHOW_CONTEXT_MENU,
            accesskit::Action::ShowContextMenu,
        ),
    ] {
        if actions.contains(flag) {
            node.add_action(action);
        }
    }

    // Table facts, derived from the retained tree (SOUL §6.1): counts on a table,
    // indices on rows and cells — what a screen reader needs to announce
    // "row 2 of 3, Name column" and offer table navigation.
    if let Some(rc) = facts.row_count {
        node.set_row_count(rc);
    }
    if let Some(cc) = facts.column_count {
        node.set_column_count(cc);
    }
    if let Some(ri) = facts.row_index {
        node.set_row_index(ri);
    }
    if let Some(ci) = facts.column_index {
        node.set_column_index(ci);
    }

    if let Some(n) = scene.node(id) {
        for c in &n.children {
            node.push_child(to_access_id(*c));
        }
    }
    node
}

/// The AccessKit focus target for a `TreeUpdate`: the focused node if one carries the
/// [`StateFlags::FOCUSED`] bit, else the root (AccessKit requires focus to name a real
/// node — the root when nothing specific is focused, SOUL §6.2/§6.3).
pub fn focus_node_id(scene: &Scene) -> accesskit::NodeId {
    focused(scene)
        .or_else(|| accessibility_root(scene))
        .map(to_access_id)
        .unwrap_or(accesskit::NodeId(0))
}

/// Builds a **full** AccessKit `TreeUpdate` from the whole retained tree — used at
/// mount (SOUL §6.2). Every reachable node is emitted once in pre-order, the `Tree`
/// root and `focus` are set, and `tree_id` is the root tree.
fn build_full_tree_update_with_window(
    scene: &Scene,
    window: Option<(f32, &str)>,
) -> accesskit::TreeUpdate {
    fn walk(
        scene: &Scene,
        id: WidgetId,
        facts: TableFacts,
        window: Option<WindowTreeContext<'_>>,
        nodes: &mut Vec<(accesskit::NodeId, accesskit::Node)>,
    ) {
        count_visit();
        nodes.push((to_access_id(id), build_node(scene, id, window, facts)));
        // Children get their table facts from one sibling pass, keeping a wide
        // table linear instead of rescanning the row for every cell.
        for_each_child_facts(scene, id, facts.row_index, |child, child_facts| {
            walk(scene, child, child_facts, window, nodes);
        });
    }

    let root = accessibility_root(scene);
    let window_context =
        root.and_then(|root| window.map(|(scale, title)| WindowTreeContext { root, scale, title }));
    let mut nodes = Vec::new();
    if let Some(root) = root {
        walk(
            scene,
            root,
            table_facts(scene, root),
            window_context,
            &mut nodes,
        );
    }
    let tree = root.map(|root| {
        let mut tree = accesskit::TreeInfo::new(to_access_id(root));
        if window.is_some() {
            tree.toolkit_name = Some("schnellui".to_string());
            tree.toolkit_version = Some(env!("CARGO_PKG_VERSION").to_string());
        }
        tree
    });
    accesskit::TreeUpdate {
        nodes,
        tree,
        tree_id: accesskit::TreeId::ROOT,
        focus: focus_node_id(scene),
    }
}

pub fn build_full_tree_update(scene: &Scene) -> accesskit::TreeUpdate {
    build_full_tree_update_with_window(scene, None)
}

/// Builds a full tree for a native window adapter. Unlike the backend-neutral
/// update, this decorates a transparent root as a Window, labels it with the native
/// title, supplies toolkit metadata, and transforms logical bounds to physical px.
pub fn build_full_window_tree_update(
    scene: &Scene,
    scale: f32,
    title: &str,
) -> accesskit::TreeUpdate {
    build_full_tree_update_with_window(scene, Some((scale, title)))
}

/// Builds an **incremental** AccessKit `TreeUpdate` containing only the nodes in the
/// scene's a11y-dirty set (SOUL §6.2) — proportional to changed nodes, never tree
/// size. `tree` is `None` (the shape is unchanged: only semantic properties moved),
/// but `focus` is always supplied as AccessKit requires. Skips any dirty id whose
/// node was removed the same frame.
pub fn build_incremental_tree_update(scene: &Scene) -> accesskit::TreeUpdate {
    build_incremental_tree_update_with_window(scene, None)
}

/// Whether an incremental update would tell the platform anything: some node is
/// a11y-dirty, or the focus target differs from `last_focus` (the target of the
/// previous update sent). Answered from the scene's cached focus, with no tree walk.
pub fn incremental_update_needed(scene: &Scene, last_focus: accesskit::NodeId) -> bool {
    !scene.a11y_dirty().is_empty() || focus_node_id(scene) != last_focus
}

fn build_incremental_tree_update_with_window(
    scene: &Scene,
    window: Option<(f32, &str)>,
) -> accesskit::TreeUpdate {
    let dirty = scene.a11y_dirty();
    let scope = active_modal_root(scene);
    let root = accessibility_root(scene);
    let window_context =
        root.and_then(|root| window.map(|(scale, title)| WindowTreeContext { root, scale, title }));
    let mut nodes = Vec::with_capacity(dirty.len());
    let mut memo = TableMemo::default();
    for &id in dirty {
        if scene.node(id).is_some()
            && scope
                .map(|root| is_in_subtree(scene, id, root))
                .unwrap_or(true)
        {
            count_visit();
            nodes.push((
                to_access_id(id),
                build_node(scene, id, window_context, memo.facts(scene, id)),
            ));
        }
    }
    accesskit::TreeUpdate {
        nodes,
        tree: None,
        tree_id: accesskit::TreeId::ROOT,
        focus: focus_node_id(scene),
    }
}

/// Builds the incremental counterpart of [`build_full_window_tree_update`]. Root
/// decoration is repeated whenever the root itself is dirty because AccessKit node
/// updates replace the whole prior node.
pub fn build_incremental_window_tree_update(
    scene: &Scene,
    scale: f32,
    title: &str,
) -> accesskit::TreeUpdate {
    build_incremental_tree_update_with_window(scene, Some((scale, title)))
}

/// The widget currently holding focus, i.e. the first node (pre-order) whose a11y
/// column carries [`StateFlags::FOCUSED`] (SOUL §6.3). `None` if nothing is focused.
pub fn focused(scene: &Scene) -> Option<WidgetId> {
    scope(scene).1
}

fn find_focused(scene: &Scene, id: WidgetId) -> Option<WidgetId> {
    if !scene.is_visible(id) {
        return None;
    }
    count_visit();
    if let Some(a) = scene.a11y(id) {
        if StateFlags(a.state).contains(StateFlags::FOCUSED) {
            return Some(id);
        }
    }
    if let Some(n) = scene.node(id) {
        for c in &n.children {
            if let Some(f) = find_focused(scene, *c) {
                return Some(f);
            }
        }
    }
    None
}

/// The keyboard tab order, derived purely from **tree order** (SOUL §6.3): the
/// focusable nodes — those whose a11y column advertises the `Focus` action
/// ([`ActionFlags::FOCUS`]) and are not [`StateFlags::DISABLED`] — in depth-first
/// pre-order. Disabled widgets are skipped like a browser skips a disabled
/// `<button>`: they can never take focus, so listing them would strand Tab.
/// This is the reading/focus order the JSON dump and screen readers walk.
pub fn tab_order(scene: &Scene) -> Vec<WidgetId> {
    with_tab_order(scene, <[WidgetId]>::to_vec)
}

/// Runs `f` over the tab order without allocating. The order is cached in the
/// scene and rebuilt only after a structural, visibility, focusability or modal
/// change, so a Tab press costs a lookup rather than a tree walk.
pub fn with_tab_order<R>(scene: &Scene, f: impl FnOnce(&[WidgetId]) -> R) -> R {
    fn walk(scene: &Scene, id: WidgetId, out: &mut Vec<WidgetId>) {
        if !scene.is_visible(id) {
            return;
        }
        count_visit();
        if let Some(a) = scene.a11y(id) {
            if ActionFlags(a.actions).contains(ActionFlags::FOCUS)
                && !StateFlags(a.state).contains(StateFlags::DISABLED)
            {
                out.push(id);
            }
        }
        if let Some(n) = scene.node(id) {
            for c in &n.children {
                walk(scene, *c, out);
            }
        }
    }
    scene.with_tab_order_cache(
        || {
            let mut out = Vec::new();
            if let Some(root) = accessibility_root(scene) {
                walk(scene, root, &mut out);
            }
            out
        },
        f,
    )
}

/// The next focusable widget after `current` in [`tab_order`], wrapping to the first
/// (SOUL §6.3 Tab navigation). `None` if `current` is not focusable or the tree has
/// no focusable nodes.
pub fn next_in_tab_order(scene: &Scene, current: WidgetId) -> Option<WidgetId> {
    with_tab_order(scene, |order| {
        let pos = order.iter().position(|&id| id == current)?;
        order
            .get(pos + 1)
            .copied()
            .or_else(|| order.first().copied())
    })
}

/// The previous focusable widget before `current` in [`tab_order`], wrapping to the
/// last (SOUL §6.3 Shift-Tab navigation).
pub fn prev_in_tab_order(scene: &Scene, current: WidgetId) -> Option<WidgetId> {
    with_tab_order(scene, |order| {
        let pos = order.iter().position(|&id| id == current)?;
        if pos == 0 {
            order.last().copied()
        } else {
            order.get(pos - 1).copied()
        }
    })
}

/// Reverse-maps an [`accesskit::NodeId`] back to its [`WidgetId`] (the identity is
/// bijective by SOUL §6.2 — [`to_access_id`] is its inverse), returning `None` if the
/// id names no live node in `scene`.
pub fn resolve_target(scene: &Scene, node_id: accesskit::NodeId) -> Option<WidgetId> {
    let id = WidgetId::from(slotmap::KeyData::from_ffi(node_id.0));
    scene.node(id).map(|_| id)
}

/// Routes an inbound AccessKit `ActionRequest` to the target widget — the *same*
/// path as the equivalent pointer/keyboard event (SOUL §6.3). Returns the resolved
/// [`WidgetId`] the umbrella should dispatch to, or `None` if the target is
/// unknown.
pub fn route_action(scene: &Scene, request: &accesskit::ActionRequest) -> Option<WidgetId> {
    let target = resolve_target(scene, request.target_node)?;
    if !scene.is_effectively_visible(target) {
        return None;
    }
    if let Some(root) = active_modal_root(scene) {
        is_in_subtree(scene, target, root).then_some(target)
    } else {
        Some(target)
    }
}

/// Context handed to an inbound-action handler: the resolved target, the AccessKit
/// [`accesskit::Action`], and any attached [`accesskit::ActionData`] (e.g. the string
/// for `SetValue`) — everything the handler needs to react exactly as it would to the
/// equivalent pointer/keyboard event (SOUL §6.3).
pub struct ActionContext<'a> {
    pub target: WidgetId,
    pub action: accesskit::Action,
    pub data: Option<&'a accesskit::ActionData>,
}

/// A registered inbound-action handler — **the same closure a pointer or key press
/// fires** (SOUL §6.3). Boxed once at mount, invoked (never reallocated) per action.
pub type ActionHandlerFn = Box<dyn FnMut(&ActionContext<'_>)>;

/// Routes inbound AccessKit `ActionRequest`s to per-widget handlers — the concrete
/// realization of SOUL §6.3's "assistive path and pointer path converge on one code
/// path". The umbrella registers, at mount, the *same* closures pointer/keyboard
/// input fires; a screen reader `Click`/`Focus`/`SetValue` then runs the identical
/// handler.
///
/// Storage is an ECS-style [`SecondaryMap`] column keyed by [`WidgetId`], each holding
/// a small inline [`SmallVec`] of `(action tag, handler)` pairs — no per-dispatch
/// allocation once registered (SOUL §4).
#[derive(Default)]
pub struct ActionRouter {
    handlers: SecondaryMap<WidgetId, SmallVec<[(u8, ActionHandlerFn); 2]>>,
}

impl ActionRouter {
    /// An empty router.
    pub fn new() -> ActionRouter {
        ActionRouter {
            handlers: SecondaryMap::new(),
        }
    }

    /// Registers (or replaces) the handler for `action` on `id` — the closure pointer
    /// input would fire for the same interaction (SOUL §6.3).
    pub fn on(
        &mut self,
        id: WidgetId,
        action: accesskit::Action,
        handler: impl FnMut(&ActionContext<'_>) + 'static,
    ) {
        let tag = action as u8;
        let list = self.handlers.entry(id).unwrap().or_default();
        if let Some(slot) = list.iter_mut().find(|(t, _)| *t == tag) {
            slot.1 = Box::new(handler);
        } else {
            list.push((tag, Box::new(handler)));
        }
    }

    /// Drops all handlers registered for `id` (e.g. when the widget is removed).
    pub fn clear(&mut self, id: WidgetId) {
        self.handlers.remove(id);
    }

    /// `true` if `id` has a handler for `action`.
    pub fn has_handler(&self, id: WidgetId, action: accesskit::Action) -> bool {
        let tag = action as u8;
        self.handlers
            .get(id)
            .is_some_and(|l| l.iter().any(|(t, _)| *t == tag))
    }

    /// Resolves `request`'s target through [`route_action`] and fires the matching
    /// handler (SOUL §6.3). Returns `true` iff a handler ran.
    pub fn dispatch(&mut self, scene: &Scene, request: &accesskit::ActionRequest) -> bool {
        let Some(target) = route_action(scene, request) else {
            return false;
        };
        self.fire(target, request.action, request.data.as_ref())
    }

    /// Fires the handler for `(target, action)` directly (the path a resolved pointer
    /// event takes), passing `data` through to the closure. Returns `true` iff a
    /// handler ran.
    pub fn fire(
        &mut self,
        target: WidgetId,
        action: accesskit::Action,
        data: Option<&accesskit::ActionData>,
    ) -> bool {
        let tag = action as u8;
        if let Some(list) = self.handlers.get_mut(target) {
            if let Some((_, handler)) = list.iter_mut().find(|(t, _)| *t == tag) {
                handler(&ActionContext {
                    target,
                    action,
                    data,
                });
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests;

// A configurable personal workspace with dockable tabs.
//
// Run the live example with:
// `cargo run -p dockable_workspace -- --scenario starter --windowed`
//
// Tabs retain click-to-select behavior, but a pointer movement beyond the drag
// threshold captures the tab. Drop on another tab to merge/reorder; drop on any
// pane edge to split left/right/top/bottom with a live half-pane preview.

use schnellui::scene::Color;
use schnellui::widgets::{DockPosition, Shape, Theme};
use schnellui::State;

pub(crate) const INK: Color = Color::rgb(0x18, 0x1b, 0x1f);
pub(crate) const PAPER: Color = Color::rgb(0xf2, 0xee, 0xe6);
pub(crate) const PANEL: Color = Color::rgb(0xff, 0xfc, 0xf5);
pub(crate) const PANEL_ALT: Color = Color::rgb(0xe5, 0xdf, 0xd3);
pub(crate) const ORANGE: Color = Color::rgb(0xe6, 0x63, 0x2e);
pub(crate) const CYAN: Color = Color::rgb(0x13, 0x91, 0x9d);
pub(crate) const LIME: Color = Color::rgb(0x8d, 0xa8, 0x29);
pub(crate) const MUTED: Color = Color::rgb(0x6c, 0x69, 0x62);

pub(crate) const WORKSPACE_THEME: Theme = Theme {
    text: INK,
    text_muted: MUTED,
    surface: PANEL,
    surface_muted: PANEL_ALT,
    separator: Color::rgb(0xc9, 0xc0, 0xb2),
    outline: Color::rgb(0x78, 0x73, 0x69),
    accent: INK,
    on_accent: PAPER,
    selection: Color::rgb(0xff, 0xd7, 0xbd),
    interactions: schnellui::widgets::InteractionStates {
        hover: schnellui::widgets::InteractionStyle::all(
            Color::rgba(0x13, 0x91, 0x9d, 0x1c),
            INK,
            CYAN,
        ),
        focus: schnellui::widgets::InteractionStyle::border(CYAN),
        active: schnellui::widgets::InteractionStyle::background(Color::rgb(0xff, 0xd7, 0xbd)),
    },
    component_interactions: schnellui::widgets::ComponentInteractions::NONE,
    text_selection: Color::rgb(0xff, 0xb8, 0x8f),
    disabled: Color::rgb(0x9e, 0x98, 0x8e),
    positive: LIME,
    attention: ORANGE,
    media: PANEL_ALT,
    page: PAPER,
    shape: Shape {
        roundness: 0.35,
        density: 1.05,
        frame: 1.0,
        shadow: 2.0,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WidgetType {
    Pulse,
    Focus,
    Notes,
    Weather,
    Agent,
    Terminal,
}

impl WidgetType {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Pulse => "Pulse",
            Self::Focus => "Focus",
            Self::Notes => "Notes",
            Self::Weather => "Weather",
            Self::Agent => "Agent",
            Self::Terminal => "Terminal",
        }
    }

    pub(crate) fn marker(self) -> &'static str {
        match self {
            Self::Pulse => "01",
            Self::Focus => "02",
            Self::Notes => "03",
            Self::Weather => "04",
            Self::Agent => "AG",
            Self::Terminal => "TR",
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceTab {
    pub(crate) id: u64,
    pub(crate) title: String,
    pub(crate) widget: WidgetType,
}

#[derive(Clone)]
pub(crate) struct PaneState {
    pub(crate) id: u64,
    pub(crate) tabs: Vec<u64>,
    pub(crate) active: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SplitAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LayoutNode {
    Pane(u64),
    Split {
        axis: SplitAxis,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

#[derive(Clone)]
pub(crate) struct WorkspaceState {
    pub(crate) tabs: Vec<WorkspaceTab>,
    pub(crate) panes: Vec<PaneState>,
    pub(crate) layout: LayoutNode,
    pub(crate) dragging: Option<u64>,
    pub(crate) compact: bool,
    pub(crate) hide_single_tab_bars: bool,
    pub(crate) next_tab: u64,
    pub(crate) next_pane: u64,
}

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            tabs: vec![
                WorkspaceTab {
                    id: 1,
                    title: "Pulse".into(),
                    widget: WidgetType::Pulse,
                },
                WorkspaceTab {
                    id: 2,
                    title: "Focus".into(),
                    widget: WidgetType::Focus,
                },
                WorkspaceTab {
                    id: 3,
                    title: "Notes".into(),
                    widget: WidgetType::Notes,
                },
                WorkspaceTab {
                    id: 4,
                    title: "Weather".into(),
                    widget: WidgetType::Weather,
                },
            ],
            panes: vec![
                PaneState {
                    id: 1,
                    tabs: vec![1, 2],
                    active: 1,
                },
                PaneState {
                    id: 2,
                    tabs: vec![3, 4],
                    active: 3,
                },
            ],
            layout: LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(LayoutNode::Pane(1)),
                second: Box::new(LayoutNode::Pane(2)),
            },
            dragging: None,
            compact: false,
            hide_single_tab_bars: false,
            next_tab: 5,
            next_pane: 3,
        }
    }
}

#[derive(Clone)]
pub(crate) struct WorkspaceRuntime(State<WorkspaceRuntimeState>);

pub(crate) struct WorkspaceRuntimeState {
    pub(crate) workspace: WorkspaceState,
    pub(crate) pending_remount: bool,
}

impl Default for WorkspaceRuntime {
    fn default() -> Self {
        Self(State::new(WorkspaceRuntimeState {
            workspace: WorkspaceState::default(),
            pending_remount: false,
        }))
    }
}

impl WorkspaceRuntime {
    pub(crate) fn read<R>(&self, read: impl FnOnce(&WorkspaceState) -> R) -> R {
        self.0.read(|state| read(&state.workspace))
    }

    pub(crate) fn update<R>(&self, update: impl FnOnce(&mut WorkspaceState) -> R) -> R {
        self.0.update(|state| update(&mut state.workspace))
    }

    pub(crate) fn request_remount(&self) {
        self.0.update(|state| state.pending_remount = true);
    }

    pub(crate) fn take_remount(&self) -> bool {
        self.0
            .update(|state| std::mem::take(&mut state.pending_remount))
    }
}

pub(crate) fn tab_by_id(state: &WorkspaceState, id: u64) -> WorkspaceTab {
    state
        .tabs
        .iter()
        .find(|tab| tab.id == id)
        .cloned()
        .expect("pane tab must exist")
}

pub(crate) fn move_dragged_to_pane(
    runtime: &WorkspaceRuntime,
    target_pane: u64,
    before: Option<u64>,
) -> bool {
    runtime.update(|state| {
        let Some(tab_id) = state.dragging else {
            return false;
        };
        let Some((source_pane, source_tab_count)) = state
            .panes
            .iter()
            .find(|pane| pane.tabs.contains(&tab_id))
            .map(|pane| (pane.id, pane.tabs.len()))
        else {
            return false;
        };
        if !state.panes.iter().any(|pane| pane.id == target_pane) {
            return false;
        }
        let pruned_layout = if source_pane != target_pane && source_tab_count == 1 {
            let Some(layout) = remove_pane(state.layout.clone(), source_pane) else {
                return false;
            };
            Some(layout)
        } else {
            None
        };
        for pane in &mut state.panes {
            pane.tabs.retain(|id| *id != tab_id);
            if pane.active == tab_id {
                pane.active = pane.tabs.first().copied().unwrap_or(0);
            }
        }
        let Some(target) = state.panes.iter_mut().find(|pane| pane.id == target_pane) else {
            return false;
        };
        let index = before
            .and_then(|before_id| target.tabs.iter().position(|id| *id == before_id))
            .unwrap_or(target.tabs.len());
        target.tabs.insert(index, tab_id);
        target.active = tab_id;
        state.panes.retain(|pane| !pane.tabs.is_empty());
        if let Some(layout) = pruned_layout {
            state.layout = layout;
        }
        true
    })
}

pub(crate) fn reorder_pane_tabs(
    runtime: &WorkspaceRuntime,
    pane_id: u64,
    from: usize,
    to: usize,
) -> bool {
    runtime.update(|state| {
        let Some(pane) = state.panes.iter_mut().find(|pane| pane.id == pane_id) else {
            return false;
        };
        if from >= pane.tabs.len() || to >= pane.tabs.len() || from == to {
            return false;
        }
        let tab = pane.tabs.remove(from);
        pane.tabs.insert(to, tab);
        true
    })
}

pub(crate) fn remove_pane(node: LayoutNode, pane_id: u64) -> Option<LayoutNode> {
    match node {
        LayoutNode::Pane(id) => (id != pane_id).then_some(LayoutNode::Pane(id)),
        LayoutNode::Split {
            axis,
            first,
            second,
        } => match (remove_pane(*first, pane_id), remove_pane(*second, pane_id)) {
            (Some(first), Some(second)) => Some(LayoutNode::Split {
                axis,
                first: Box::new(first),
                second: Box::new(second),
            }),
            (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
            (None, None) => None,
        },
    }
}

pub(crate) fn split_pane(
    node: &mut LayoutNode,
    target_pane: u64,
    new_pane: u64,
    position: DockPosition,
) -> bool {
    match node {
        LayoutNode::Pane(id) if *id == target_pane => {
            let axis = match position {
                DockPosition::Left | DockPosition::Right => SplitAxis::Horizontal,
                DockPosition::Top | DockPosition::Bottom => SplitAxis::Vertical,
                DockPosition::Center => return false,
            };
            let target = LayoutNode::Pane(target_pane);
            let inserted = LayoutNode::Pane(new_pane);
            let (first, second) = match position {
                DockPosition::Left | DockPosition::Top => (inserted, target),
                DockPosition::Right | DockPosition::Bottom => (target, inserted),
                DockPosition::Center => unreachable!(),
            };
            *node = LayoutNode::Split {
                axis,
                first: Box::new(first),
                second: Box::new(second),
            };
            true
        }
        LayoutNode::Split { first, second, .. } => {
            split_pane(first, target_pane, new_pane, position)
                || split_pane(second, target_pane, new_pane, position)
        }
        LayoutNode::Pane(_) => false,
    }
}

pub(crate) fn split_group(
    node: &mut LayoutNode,
    target: &LayoutNode,
    new_pane: u64,
    axis: SplitAxis,
    position: DockPosition,
) -> bool {
    if node == target {
        let current = std::mem::replace(node, LayoutNode::Pane(new_pane));
        let inserted = LayoutNode::Pane(new_pane);
        *node = match (axis, position) {
            (SplitAxis::Horizontal, DockPosition::Top) => LayoutNode::Split {
                axis: SplitAxis::Vertical,
                first: Box::new(inserted),
                second: Box::new(current),
            },
            (SplitAxis::Horizontal, DockPosition::Bottom) => LayoutNode::Split {
                axis: SplitAxis::Vertical,
                first: Box::new(current),
                second: Box::new(inserted),
            },
            (SplitAxis::Vertical, DockPosition::Left) => LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(inserted),
                second: Box::new(current),
            },
            (SplitAxis::Vertical, DockPosition::Right) => LayoutNode::Split {
                axis: SplitAxis::Horizontal,
                first: Box::new(current),
                second: Box::new(inserted),
            },
            _ => {
                let LayoutNode::Split {
                    axis: current_axis,
                    first,
                    second,
                } = current
                else {
                    unreachable!("a gutter target must be a split");
                };
                debug_assert_eq!(current_axis, axis);
                LayoutNode::Split {
                    axis,
                    first,
                    second: Box::new(LayoutNode::Split {
                        axis,
                        first: Box::new(inserted),
                        second,
                    }),
                }
            }
        };
        return true;
    }
    match node {
        LayoutNode::Split { first, second, .. } => {
            split_group(first, target, new_pane, axis, position)
                || split_group(second, target, new_pane, axis, position)
        }
        LayoutNode::Pane(_) => false,
    }
}

pub(crate) fn dock_dragged(
    runtime: &WorkspaceRuntime,
    target_pane: u64,
    position: DockPosition,
) -> bool {
    if position == DockPosition::Center {
        return move_dragged_to_pane(runtime, target_pane, None);
    }
    runtime.update(|state| {
        let Some(tab_id) = state.dragging else {
            return false;
        };
        let Some(source_pane) = state
            .panes
            .iter()
            .find(|pane| pane.tabs.contains(&tab_id))
            .map(|pane| (pane.id, pane.tabs.len()))
        else {
            return false;
        };
        // Splitting the only tab out of the pane it is targeting would leave
        // nothing to occupy the opposite half.
        if source_pane == (target_pane, 1) {
            return false;
        }
        for pane in &mut state.panes {
            pane.tabs.retain(|id| *id != tab_id);
            if pane.active == tab_id {
                pane.active = pane.tabs.first().copied().unwrap_or(0);
            }
        }
        let source_became_empty = state
            .panes
            .iter()
            .find(|pane| pane.id == source_pane.0)
            .is_some_and(|pane| pane.tabs.is_empty());
        state.panes.retain(|pane| !pane.tabs.is_empty());
        let mut layout = state.layout.clone();
        if source_became_empty {
            let Some(pruned) = remove_pane(layout, source_pane.0) else {
                return false;
            };
            layout = pruned;
        }
        let pane_id = state.next_pane;
        state.next_pane += 1;
        if !split_pane(&mut layout, target_pane, pane_id, position) {
            return false;
        }
        state.panes.push(PaneState {
            id: pane_id,
            tabs: vec![tab_id],
            active: tab_id,
        });
        state.layout = layout;
        true
    })
}

pub(crate) fn dock_dragged_to_group(
    runtime: &WorkspaceRuntime,
    target: &LayoutNode,
    axis: SplitAxis,
    position: DockPosition,
) -> bool {
    runtime.update(|state| {
        let Some(tab_id) = state.dragging else {
            return false;
        };
        let Some((source_pane, source_tab_count)) = state
            .panes
            .iter()
            .find(|pane| pane.tabs.contains(&tab_id))
            .map(|pane| (pane.id, pane.tabs.len()))
        else {
            return false;
        };

        let pane_id = state.next_pane;
        let mut layout = state.layout.clone();
        if !split_group(&mut layout, target, pane_id, axis, position) {
            return false;
        }
        if source_tab_count == 1 {
            let Some(pruned) = remove_pane(layout, source_pane) else {
                return false;
            };
            layout = pruned;
        }

        for pane in &mut state.panes {
            pane.tabs.retain(|id| *id != tab_id);
            if pane.active == tab_id {
                pane.active = pane.tabs.first().copied().unwrap_or(0);
            }
        }
        state.panes.retain(|pane| !pane.tabs.is_empty());
        state.next_pane += 1;
        state.panes.push(PaneState {
            id: pane_id,
            tabs: vec![tab_id],
            active: tab_id,
        });
        state.layout = layout;
        true
    })
}

pub(crate) fn add_widget_to_pane(
    runtime: &WorkspaceRuntime,
    widget: WidgetType,
    target_pane: Option<u64>,
) {
    runtime.update(|state| {
        let id = state.next_tab;
        state.next_tab += 1;
        let count = state.tabs.iter().filter(|tab| tab.widget == widget).count();
        let title = if count == 0 {
            widget.title().to_string()
        } else {
            format!("{} {}", widget.title(), count + 1)
        };
        state.tabs.push(WorkspaceTab { id, title, widget });
        let pane_index = target_pane
            .and_then(|pane_id| state.panes.iter().position(|pane| pane.id == pane_id))
            .or_else(|| (!state.panes.is_empty()).then_some(0));
        if let Some(pane) = pane_index.and_then(|index| state.panes.get_mut(index)) {
            pane.tabs.push(id);
            pane.active = id;
        }
    });
    runtime.request_remount();
}

pub(crate) fn add_widget(runtime: &WorkspaceRuntime, widget: WidgetType) {
    add_widget_to_pane(runtime, widget, None);
}

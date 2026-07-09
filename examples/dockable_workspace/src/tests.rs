use crate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{drag, mount_workspace, point_in, workspace_metrics};
    use schnellui::a11y::to_access_id;
    use schnellui::accesskit_action::{Action, ActionRequest};
    use schnellui::accesskit_reexport::TreeId;
    use schnellui::widgets::DockPosition;

    fn initial_layout() -> LayoutNode {
        WorkspaceState::default().layout
    }

    fn assert_equal_pane_widths(app: &App, panes: &[&str]) {
        let widths: Vec<f32> = panes
            .iter()
            .map(|name| {
                let id = app
                    .find_widget(Role::Group, Some(name))
                    .unwrap_or_else(|| panic!("missing pane {name:?}"));
                app.scene().layout(id).expect("pane layout").rect.width
            })
            .collect();
        let minimum = widths.iter().copied().fold(f32::INFINITY, f32::min);
        let maximum = widths.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            maximum - minimum < 1.0,
            "pane widths must be equal: {widths:?}"
        );
    }

    fn canvas_rect(app: &App) -> Rect {
        let canvas = app
            .find_widget(Role::Group, Some("Dock between panes 1, 2"))
            .expect("workspace canvas");
        app.scene().layout(canvas).expect("canvas layout").rect
    }

    #[test]
    fn workspace_geometry_tracks_the_viewport() {
        let runtime = WorkspaceRuntime::default();
        let mut small = mount_workspace(runtime.clone(), 900, 600, 1.0);
        small.frame();
        let small_rect = canvas_rect(&small);
        let small_metrics = workspace_metrics(900.0, 600.0);
        assert!((small_rect.width - small_metrics.canvas_width).abs() < 1.0);
        assert!((small_rect.height - small_metrics.canvas_height).abs() < 1.0);
        assert!(small_rect.right() <= 900.0);
        assert!(small_rect.bottom() <= 600.0);
        let mut large = mount_workspace(runtime.clone(), 1400, 900, 1.0);
        large.frame();
        let large_rect = canvas_rect(&large);
        let large_metrics = workspace_metrics(1400.0, 900.0);
        assert!((large_rect.width - large_metrics.canvas_width).abs() < 1.0);
        assert!((large_rect.height - large_metrics.canvas_height).abs() < 1.0);
        assert!(large_rect.right() <= 1400.0);
        assert!(large_rect.bottom() <= 900.0);
        assert!(large_rect.width > small_rect.width + 400.0);
        assert!(large_rect.height > small_rect.height + 250.0);
    }

    #[test]
    fn trailing_commands_add_agent_and_terminal_tabs_to_their_pane() {
        let runtime = WorkspaceRuntime::default();
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert!(app.find_widget(Role::Tab, Some("AG Agent")).is_none());

        let add_agent = app
            .find_widget(Role::Button, Some("+ AGENT"))
            .expect("agent tab command");
        assert!(app.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(add_agent),
            data: None,
        }));
        assert!(runtime.take_remount());
        runtime.read(|state| {
            let agent = state
                .tabs
                .iter()
                .find(|tab| tab.widget == WidgetType::Agent)
                .expect("agent tab state");
            assert!(state.panes[0].tabs.contains(&agent.id));
            assert_eq!(state.panes[0].active, agent.id);
        });

        app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert!(app.find_widget(Role::Tab, Some("AG Agent")).is_some());
        let add_terminal = app
            .find_widget(Role::Button, Some("+ TERMINAL"))
            .expect("terminal tab command");
        assert!(app.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(add_terminal),
            data: None,
        }));
        assert!(runtime.take_remount());
        runtime.read(|state| {
            let terminal = state
                .tabs
                .iter()
                .find(|tab| tab.widget == WidgetType::Terminal)
                .expect("terminal tab state");
            assert!(state.panes[0].tabs.contains(&terminal.id));
            assert_eq!(state.panes[0].active, terminal.id);
        });

        let mut remounted = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        remounted.frame();
        assert!(remounted
            .find_widget(Role::Tab, Some("TR Terminal"))
            .is_some());
    }

    #[test]
    fn tabs_reorder_inside_a_pane_without_triggering_docking() {
        let runtime = WorkspaceRuntime::default();
        let initial_layout = runtime.read(|state| state.layout.clone());
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();

        drag(&mut app, "01 Pulse", Role::Tab, "02 Focus", 0.75, 0.5, true).unwrap();

        assert!(runtime.take_remount());
        runtime.read(|state| {
            assert_eq!(state.panes.len(), 2);
            assert_eq!(state.layout, initial_layout);
            assert_eq!(state.panes[0].tabs, vec![2, 1]);
            assert_eq!(state.panes[1].tabs, vec![3, 4]);
            assert_eq!(state.dragging, None);
        });
    }

    #[test]
    fn center_dock_prunes_an_empty_source_pane_from_the_layout() {
        let runtime = WorkspaceRuntime::default();
        runtime.update(|state| {
            state.panes[0].tabs = vec![1];
            state.panes[0].active = 1;
            state.panes[1].tabs.insert(0, 2);
            state.dragging = Some(1);
        });

        assert!(dock_dragged(&runtime, 2, DockPosition::Center));

        runtime.read(|state| {
            assert_eq!(state.layout, LayoutNode::Pane(2));
            assert_eq!(state.panes.len(), 1);
            assert_eq!(state.panes[0].id, 2);
            assert_eq!(state.panes[0].tabs, vec![2, 3, 4, 1]);
            assert_eq!(state.panes[0].active, 1);
        });

        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert!(app.find_widget(Role::Group, Some("Dock Pulse")).is_some());
    }

    #[test]
    fn single_tab_bar_visibility_follows_the_canvas_setting() {
        let runtime = WorkspaceRuntime::default();
        runtime.update(|state| {
            state.panes[0].tabs = vec![1];
            state.panes[0].active = 1;
            state.panes[1].tabs.insert(0, 2);
        });

        let mut visible = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        visible.frame();
        let hide_switch = visible
            .find_widget(Role::Switch, Some("Hide single tab bars"))
            .expect("hide-single-tab-bars switch");
        assert!(visible.find_widget(Role::Tab, Some("01 Pulse")).is_some());

        assert!(visible.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(hide_switch),
            data: None,
        }));
        assert!(runtime.take_remount());
        assert!(runtime.read(|state| state.hide_single_tab_bars));

        let mut hidden = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        hidden.frame();
        assert!(hidden.find_widget(Role::Tab, Some("01 Pulse")).is_none());
        assert!(hidden.find_widget(Role::Tab, Some("02 Focus")).is_some());
        assert!(hidden
            .find_widget(Role::Group, Some("Dock Pulse"))
            .is_some());
    }

    #[test]
    fn hidden_single_tab_pane_reveals_and_drags_from_its_corner_handle() {
        let runtime = WorkspaceRuntime::default();
        runtime.update(|state| {
            state.panes[0].tabs = vec![1];
            state.panes[0].active = 1;
            state.panes[1].tabs.insert(0, 2);
            state.hide_single_tab_bars = true;
        });
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();

        let handle = app
            .find_widget(Role::Group, Some("Move Pulse pane"))
            .expect("single-tab pane drag handle");
        assert!(app.scene().paint(handle).unwrap().primitives.is_empty());
        let handle_rect = app.scene().layout(handle).unwrap().rect;
        let nearby = Point {
            x: handle_rect.right() + 10.0,
            y: handle_rect.y + handle_rect.height * 0.5,
        };
        assert!(app.update_pointer_proximity(nearby));
        assert!(!app.scene().paint(handle).unwrap().primitives.is_empty());
        assert!(app.update_pointer_proximity(Point { x: 0.0, y: 0.0 }));
        assert!(app.scene().paint(handle).unwrap().primitives.is_empty());

        let from = Point {
            x: handle_rect.x + handle_rect.width * 0.5,
            y: handle_rect.y + handle_rect.height * 0.5,
        };
        let to = point_in(&app, Role::Group, "Dock Notes", 0.5, 0.5).unwrap();
        assert!(app.begin_drag(from));
        assert!(app.update_drag(to));
        assert!(matches!(
            app.end_drag(to),
            DragRelease::Drop { accepted: true }
        ));
        assert!(runtime.take_remount());
        assert_eq!(runtime.read(|state| state.panes.len()), 1);
        assert!(runtime.read(|state| state.panes[0].tabs.contains(&1)));
    }

    #[test]
    fn horizontal_gutter_supports_top_center_and_bottom_splits() {
        let cases = [
            (
                DockPosition::Top,
                LayoutNode::Split {
                    axis: SplitAxis::Vertical,
                    first: Box::new(LayoutNode::Pane(3)),
                    second: Box::new(initial_layout()),
                },
            ),
            (
                DockPosition::Center,
                LayoutNode::Split {
                    axis: SplitAxis::Horizontal,
                    first: Box::new(LayoutNode::Pane(1)),
                    second: Box::new(LayoutNode::Split {
                        axis: SplitAxis::Horizontal,
                        first: Box::new(LayoutNode::Pane(3)),
                        second: Box::new(LayoutNode::Pane(2)),
                    }),
                },
            ),
            (
                DockPosition::Bottom,
                LayoutNode::Split {
                    axis: SplitAxis::Vertical,
                    first: Box::new(initial_layout()),
                    second: Box::new(LayoutNode::Pane(3)),
                },
            ),
        ];

        for (position, expected) in cases {
            let runtime = WorkspaceRuntime::default();
            let target = initial_layout();
            runtime.update(|state| state.dragging = Some(2));

            assert!(dock_dragged_to_group(
                &runtime,
                &target,
                SplitAxis::Horizontal,
                position
            ));

            runtime.read(|state| {
                assert_eq!(state.layout, expected, "wrong layout for {position:?}");
                assert_eq!(state.panes.len(), 3);
            });
        }
    }

    #[test]
    fn gutter_drops_add_panes_recursively() {
        let runtime = WorkspaceRuntime::default();
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        drag(
            &mut app,
            "02 Focus",
            Role::Group,
            "Dock between panes 1, 2",
            0.5,
            0.5,
            true,
        )
        .unwrap();
        assert!(runtime.take_remount());

        app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert_eq!(runtime.read(|state| state.panes.len()), 3);
        assert_equal_pane_widths(&app, &["Dock Pulse", "Dock Focus", "Dock Notes"]);
        drag(
            &mut app,
            "04 Weather",
            Role::Group,
            "Dock between panes 3, 2",
            0.5,
            0.5,
            true,
        )
        .unwrap();
        assert!(runtime.take_remount());

        app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert_eq!(runtime.read(|state| state.panes.len()), 4);
        assert!(app.find_widget(Role::Tab, Some("04 Weather")).is_some());
        assert_equal_pane_widths(
            &app,
            &["Dock Pulse", "Dock Weather", "Dock Focus", "Dock Notes"],
        );
    }
}

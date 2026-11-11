use crate::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{drag, mount_workspace, point_in, workspace_metrics};
    use schnellui::a11y::to_access_id;
    use schnellui::accesskit_action::{Action, ActionRequest};
    use schnellui::accesskit_reexport::TreeId;
    use schnellui::widgets::DockPosition;

    /// Resize the live window: the workspace is rebuilt at the new size and
    /// matches a fresh mount there, at either scale.
    #[test]
    fn a_live_resize_matches_a_fresh_mount() {
        use crate::runtime::{remount_hook, scenario_app, Scenario};
        use schnellui::testing::fuzz::{layout_diff, HeadlessHost};
        use schnellui::{Remount, WindowUpdate};
        for scale in [1.0, 1.5] {
            let runtime = WorkspaceRuntime::default();
            let mut app = scenario_app(&runtime, Scenario::Starter, 1000, 700, scale).unwrap();
            app.frame();
            let mut host = HeadlessHost::new(app);
            let mut hook = remount_hook(runtime, (1000, 700), scale);
            host.set_remount(move |size: Size, _| {
                hook(size).map(|app| WindowUpdate::Remount(Remount::new(app, "test")))
            });
            host.resize(720.0, 520.0);
            assert_eq!(host.remounts(), 1);
            let mut fresh = scenario_app(
                &WorkspaceRuntime::default(),
                Scenario::Starter,
                720,
                520,
                scale,
            )
            .unwrap();
            assert_eq!(
                layout_diff(host.app_mut(), &mut fresh),
                None,
                "scale {scale}"
            );
        }
    }

    /// At a phone width no text or control is cut at a side with nothing to
    /// scroll it in: prose wraps and fixed-width content reflows or scrolls.
    #[ignore = "the board is a fixed size that cannot reflow, and a Scroll scrolls only down: open design item #12"]
    #[test]
    fn nothing_is_clipped_sideways_at_360() {
        use schnellui::testing::fuzz::sideways_clips;
        let mut faults = Vec::new();
        use crate::runtime::{scenario_app, Scenario};
        use strum::IntoEnumIterator;
        for scenario in Scenario::iter() {
            let runtime = WorkspaceRuntime::default();
            let mut app = scenario_app(&runtime, scenario, 360, 600, 1.0).expect("scenario");
            for fault in sideways_clips(&mut app) {
                faults.push(format!("{scenario:?}: {fault}"));
            }
        }
        assert!(faults.is_empty(), "{faults:#?}");
    }

    /// At 480x600 the Pulse pane scrolls and shows a bar. Its title and the
    /// "Energy this week" label end left of the bar instead of under it.
    #[test]
    fn the_scrollbar_covers_nothing_in_the_pulse_pane_at_480x600() {
        use crate::runtime::{scenario_app, Scenario};
        use schnellui::testing::fuzz::{scrollbar_overlaps, scrollbars};
        let runtime = WorkspaceRuntime::default();
        let mut app = scenario_app(&runtime, Scenario::Starter, 480, 600, 1.0).expect("scenario");
        app.frame();
        assert!(!scrollbars(&app).is_empty(), "a pane should scroll here");
        let covered = scrollbar_overlaps(&app);
        assert!(covered.is_empty(), "{covered:#?}");
        let scene = app.scene();
        let label = scene
            .preorder()
            .into_iter()
            .find(|&id| scene.a11y(id).and_then(|a| a.name.as_deref()) == Some("Energy this week"))
            .expect("the Pulse card's label");
        let bar = scrollbars(&app)
            .into_iter()
            .find(|bar| {
                let mut at = scene.node(label).and_then(|n| n.parent);
                while let Some(id) = at {
                    if id == bar.viewport {
                        return true;
                    }
                    at = scene.node(id).and_then(|n| n.parent);
                }
                false
            })
            .expect("the label sits in a pane with a scrollbar");
        assert!(
            scene.layout(label).unwrap().rect.right() <= bar.track.x,
            "label ends at {}, bar starts at {}",
            scene.layout(label).unwrap().rect.right(),
            bar.track.x
        );
    }

    /// The viewport named `name`, its content column (the viewport's first
    /// child) and the scrollbar's track, which exists only while it overflows.
    fn scroll_named(
        app: &schnellui::App,
        name: &str,
    ) -> (
        schnellui::scene::Rect,
        schnellui::scene::Rect,
        Option<schnellui::scene::Rect>,
    ) {
        let scene = app.scene();
        let viewport = app
            .find_widget(schnellui::a11y::Role::ScrollView, Some(name))
            .unwrap_or_else(|| panic!("no scroll named {name}"));
        let content = scene.node(viewport).unwrap().children[0];
        let bar = schnellui::testing::fuzz::scrollbars(app)
            .into_iter()
            .find(|bar| bar.viewport == viewport)
            .map(|bar| bar.track);
        (
            scene.layout(viewport).unwrap().rect,
            scene.layout(content).unwrap().rect,
            bar,
        )
    }

    /// A pane that overflows ends its content at the scrollbar's gutter, not
    /// 14px short of the viewport's edge on top of it.
    #[test]
    fn an_overflowing_pane_runs_up_to_the_scrollbar_gutter() {
        use crate::runtime::{scenario_app, Scenario};
        let runtime = WorkspaceRuntime::default();
        let mut app = scenario_app(&runtime, Scenario::Starter, 480, 600, 1.0).expect("scenario");
        app.frame();
        let mut checked = 0;
        for name in ["Focus pane", "Pulse pane", "Notes pane"] {
            if app
                .find_widget(schnellui::a11y::Role::ScrollView, Some(name))
                .is_none()
            {
                continue;
            }
            let (viewport, content, bar) = scroll_named(&app, name);
            let Some(bar) = bar else { continue };
            assert!(
                (viewport.right() - bar.x - schnellui::layout::SCROLLBAR_GUTTER).abs() <= 1.0,
                "{name}: the bar starts at {}, the viewport ends at {}",
                bar.x,
                viewport.right()
            );
            assert!(
                (content.right() - bar.x).abs() <= 1.0,
                "{name}: content ends at {}, the gutter begins at {}",
                content.right(),
                bar.x
            );
            checked += 1;
        }
        assert!(checked > 0, "no pane overflowed at 480x600");
    }

    /// The library card keeps equal margins while its list fits, and runs up to
    /// the gutter once a short window makes it scroll.
    #[test]
    fn the_widget_library_runs_up_to_the_scrollbar_gutter() {
        use crate::runtime::{scenario_app, Scenario};
        let runtime = WorkspaceRuntime::default();
        let mut short =
            scenario_app(&runtime, Scenario::Starter, 1210, 240, 1.0).expect("scenario");
        short.frame();
        let (_, content, bar) = scroll_named(&short, "Widget library");
        let bar = bar.expect("the library scrolls in a 240px window");
        assert!(
            (content.right() - bar.x).abs() <= 1.0,
            "content ends at {}, the gutter begins at {}",
            content.right(),
            bar.x
        );

        let runtime = WorkspaceRuntime::default();
        let mut tall = scenario_app(&runtime, Scenario::Starter, 1210, 720, 1.0).expect("scenario");
        tall.frame();
        let (viewport, content, bar) = scroll_named(&tall, "Widget library");
        assert!(bar.is_none(), "the library fits in 720px");
        // The card's margin is the same on both sides: 18px.
        let card_right = viewport.right() + 6.0;
        assert!(
            ((card_right - content.right()) - (content.x - (viewport.x - 18.0))).abs() <= 1.0,
            "content spans {}..{} in a card ending at {card_right}",
            content.x,
            content.right()
        );
    }

    /// Every Tab stop of every scenario can be shown, at the default size and
    /// in a narrow, short window: something must scroll it in.
    #[test]
    fn every_tab_stop_reaches_the_window() {
        use crate::runtime::{scenario_app, Scenario};
        use strum::IntoEnumIterator;
        let mut stops = 0;
        for scenario in Scenario::iter() {
            for (w, h) in [(1210, 720), (1210, 240), (1210, 120)] {
                // The other scenarios are reached by dragging a tab with the
                // pointer, which needs the tab on screen; the short windows
                // only cover the page the drags start from.
                if scenario != Scenario::Starter && h < 720 {
                    continue;
                }
                let runtime = WorkspaceRuntime::default();
                let mut app = scenario_app(&runtime, scenario, w, h, 1.0).expect("scenario");
                stops += schnellui::testing::fuzz::tab_stops_reachable(&mut app)
                    .unwrap_or_else(|e| panic!("{scenario:?} at {w}x{h}: {e}"));
            }
        }
        assert!(stops > 0, "no scenario has a Tab stop");
    }

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
    fn the_widget_library_keeps_its_content_inside_its_card() {
        let named = |app: &App, name: &str| {
            let scene = app.scene();
            scene
                .preorder()
                .into_iter()
                .find(|&id| scene.a11y(id).and_then(|a| a.name.as_deref()) == Some(name))
                .unwrap_or_else(|| panic!("missing {name:?}"))
        };
        for (width, height, scale) in [(847, 540, 1.0), (847, 540, 2.0), (1210, 480, 1.5)] {
            let runtime = WorkspaceRuntime::default();
            let mut app = mount_workspace(runtime, width, height, scale);
            app.frame();
            let metrics = workspace_metrics(width as f32, height as f32);
            let card_bottom = metrics.outer_padding + metrics.content_height;
            let tip = named(&app, "Tip: drag any tab to move it.");
            let scene = app.scene();
            let mut clip = None;
            let mut at = scene.node(tip).and_then(|n| n.parent);
            while let Some(id) = at {
                if scene.node(id).is_some_and(|n| n.kind == WidgetKind::Scroll) {
                    clip = Some(scene.composited_rect(id));
                    break;
                }
                at = scene.node(id).and_then(|n| n.parent);
            }
            let tip_rect = scene.composited_rect(tip);
            match clip {
                Some(viewport) => {
                    assert!(
                        viewport.bottom() <= card_bottom + 0.5,
                        "{width}x{height}@{scale}: library viewport {viewport:?} ends below the card at {card_bottom}"
                    );
                    // Scrolled to its end, the library shows the tip.
                    let inside = Point {
                        x: viewport.x + viewport.width * 0.5,
                        y: viewport.y + viewport.height * 0.5,
                    };
                    for _ in 0..10 {
                        app.dispatch_wheel_at(inside, 120.0);
                    }
                    app.frame();
                    let tip_rect = app.scene().composited_rect(tip);
                    assert!(
                        tip_rect.height > 0.0
                            && tip_rect.y >= viewport.y - 0.5
                            && tip_rect.bottom() <= viewport.bottom() + 0.5,
                        "{width}x{height}@{scale}: tip {tip_rect:?} unreachable below {viewport:?}"
                    );
                }
                None => assert!(
                    tip_rect.bottom() <= card_bottom + 0.5,
                    "{width}x{height}@{scale}: tip {tip_rect:?} spills below the card at {card_bottom}"
                ),
            }
        }
        // With room to spare the tip still sits at the bottom of the card.
        let mut app = mount_workspace(WorkspaceRuntime::default(), 1400, 900, 1.0);
        app.frame();
        let metrics = workspace_metrics(1400.0, 900.0);
        let tip = app
            .scene()
            .composited_rect(named(&app, "Tip: drag any tab to move it."));
        let card_bottom = metrics.outer_padding + metrics.content_height;
        assert!(
            card_bottom - tip.bottom() < 40.0,
            "tip {tip:?} should rest at the card bottom {card_bottom}"
        );
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

    /// Opens a pane's "+" menu and picks one of its rows.
    fn choose_new_tab(app: &mut App, button: &str, item: &str) {
        let add = app
            .find_widget(Role::Button, Some(button))
            .expect("new tab menu button");
        assert!(app.dispatch_action(&ActionRequest {
            action: Action::Click,
            target_tree: TreeId::ROOT,
            target_node: to_access_id(add),
            data: None,
        }));
        app.frame();
        let row = app
            .find_widget(Role::MenuItem, Some(item))
            .expect("new tab menu row");
        assert!(app.activate_context_menu_item(row).is_some());
    }

    #[test]
    fn trailing_commands_add_agent_and_terminal_tabs_to_their_pane() {
        let runtime = WorkspaceRuntime::default();
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();
        assert!(app.find_widget(Role::Tab, Some("Agent")).is_none());

        choose_new_tab(&mut app, "Add a widget to the Pulse pane", "Agent");
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
        assert!(app.find_widget(Role::Tab, Some("Agent")).is_some());
        choose_new_tab(&mut app, "Add a widget to the Agent pane", "Terminal");
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
        assert!(remounted.find_widget(Role::Tab, Some("Terminal")).is_some());
    }

    #[test]
    fn tabs_reorder_inside_a_pane_without_triggering_docking() {
        let runtime = WorkspaceRuntime::default();
        let initial_layout = runtime.read(|state| state.layout.clone());
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();

        drag(&mut app, "Pulse", Role::Tab, "Focus", 0.75, 0.5, true).unwrap();

        assert!(runtime.take_remount());
        runtime.read(|state| {
            assert_eq!(state.panes.len(), 2);
            assert_eq!(state.layout, initial_layout);
            assert_eq!(state.panes[0].tabs, vec![2, 1]);
            assert_eq!(state.panes[1].tabs, vec![3, 4]);
            assert_eq!(state.dragging, None);
        });
    }

    /// The empty strip beside a pane's tab labels is part of its tab bar: a
    /// tab dropped there joins the pane at the end, like a drop on a label,
    /// and the preview is the bar, not a split of the pane.
    #[test]
    fn dropping_on_the_empty_tab_strip_joins_the_pane() {
        use schnellui::scene::Primitive;
        let runtime = WorkspaceRuntime::default();
        let initial_layout = runtime.read(|state| state.layout.clone());
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();

        let from = point_in(&app, Role::Tab, "Pulse", 0.5, 0.5).unwrap();
        let last = app.find_widget(Role::Tab, Some("Weather")).unwrap();
        let rect = app.scene().layout(last).unwrap().rect;
        let to = Point {
            x: rect.right() + 30.0,
            y: rect.y + rect.height * 0.5,
        };
        let dock = app.find_widget(Role::Group, Some("Dock Notes")).unwrap();
        let pane = app.scene().layout(dock).unwrap().rect;
        assert!(pane.contains(to));
        assert!(app.begin_drag(from));
        assert!(app.update_drag(to));
        // The preview paints on the dock's last child.
        let layer = *app.scene().node(dock).unwrap().children.last().unwrap();
        let preview = app
            .scene()
            .paint(layer)
            .unwrap()
            .primitives
            .iter()
            .rev()
            .find_map(|primitive| match primitive {
                Primitive::SolidRect { rect, .. } => Some(*rect),
                _ => None,
            })
            .expect("a drop preview");
        assert!(
            preview.height < pane.height * 0.4 && preview.width >= pane.width * 0.9,
            "the preview is the tab bar band, not a split: {preview:?} in {pane:?}"
        );
        assert!(matches!(
            app.end_drag(to),
            DragRelease::Drop { accepted: true }
        ));
        assert!(runtime.take_remount());
        runtime.read(|state| {
            assert_eq!(state.panes.len(), 2);
            assert_eq!(state.layout, initial_layout);
            assert_eq!(state.panes[0].tabs, vec![2]);
            assert_eq!(state.panes[1].tabs, vec![3, 4, 1]);
        });
    }

    /// Dropped on the empty strip of its own pane's bar, a tab moves to the end.
    #[test]
    fn dropping_on_the_own_empty_tab_strip_moves_the_tab_to_the_end() {
        let runtime = WorkspaceRuntime::default();
        let initial_layout = runtime.read(|state| state.layout.clone());
        let mut app = mount_workspace(runtime.clone(), 1210, 720, 1.0);
        app.frame();

        let from = point_in(&app, Role::Tab, "Pulse", 0.5, 0.5).unwrap();
        let last = app.find_widget(Role::Tab, Some("Focus")).unwrap();
        let rect = app.scene().layout(last).unwrap().rect;
        let to = Point {
            x: rect.right() + 30.0,
            y: rect.y + rect.height * 0.5,
        };
        assert!(app.begin_drag(from));
        assert!(app.update_drag(to));
        assert!(matches!(
            app.end_drag(to),
            DragRelease::Drop { accepted: true }
        ));
        assert!(runtime.take_remount());
        runtime.read(|state| {
            assert_eq!(state.layout, initial_layout);
            assert_eq!(state.panes[0].tabs, vec![2, 1]);
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
        assert!(visible.find_widget(Role::Tab, Some("Pulse")).is_some());

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
        assert!(hidden.find_widget(Role::Tab, Some("Pulse")).is_none());
        assert!(hidden.find_widget(Role::Tab, Some("Focus")).is_some());
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
            "Focus",
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
            "Weather",
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
        assert!(app.find_widget(Role::Tab, Some("Weather")).is_some());
        assert_equal_pane_widths(
            &app,
            &["Dock Pulse", "Dock Weather", "Dock Focus", "Dock Notes"],
        );
    }
}

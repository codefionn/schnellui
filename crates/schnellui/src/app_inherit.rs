//! Remount inheritance: state, host configuration and geometry carried to a replacement app.

use super::*;

impl App {
    /// Transfers user-owned interaction state from a tree this app is replacing.
    ///
    /// Structural remounts create fresh [`WidgetId`]s and fresh widget runtimes.
    /// This method pairs surviving controls by an explicit [`scene::ComponentRef`]
    /// when present, otherwise by exact accessible role/name and occurrence. It
    /// preserves editable carets and selections, animated loading-spinner phases,
    /// scroll positions, adjusted dialog geometry, semantic focus, and the
    /// focus-input modality. Controlled values and replacement structure remain
    /// authoritative.
    ///
    /// Custom window hosts should call this after mounting the replacement and
    /// before dropping `previous`. [`App::run_windowed_with`] and theme rebuilds
    /// already do so automatically.
    pub fn inherit_remount_state(&mut self, previous: &App) {
        schnellui::widgets::set_clock(&self.widgets, schnellui::widgets::clock(&previous.widgets));
        let previous_focus = previous.focused_widget().map(|id| {
            let ring_visible = schnellui::widgets::focus_ring_visible(&previous.widgets, id);
            (id, ring_visible)
        });
        let previous_option_trigger = previous_focus.and_then(|(id, _)| {
            previous
                .scene
                .a11y(id)
                .is_some_and(|semantics| {
                    a11y::Role::from_u16(semantics.role) == a11y::Role::ListBoxOption
                })
                .then(|| owning_combo_trigger(&previous.scene, id))
                .flatten()
        });

        let mut stateful_nodes: Vec<_> = previous
            .scene
            .preorder()
            .filter(|id| {
                previous.scene.node(*id).is_some_and(|node| {
                    matches!(
                        node.kind,
                        WidgetKind::TextInput | WidgetKind::TextArea | WidgetKind::LoadingSpinner
                    )
                })
            })
            .collect();
        if let Some((focused, _)) = previous_focus {
            stateful_nodes.push(focused);
        }
        if let Some(trigger) = previous_option_trigger {
            stateful_nodes.push(trigger);
        }
        let counterparts = remount::CounterpartMap::new(
            &previous.scene,
            &self.scene,
            stateful_nodes.iter().copied(),
        );

        self.inherit_dialog_geometry(previous);
        self.inherit_scroll_offsets(previous);

        // An editor can temporarily lose focus to a popup or another field and
        // still regain it later. Carry every surviving retained counterpart, not
        // only the one that happened to be focused at the instant of the remount.
        for previous_id in stateful_nodes.iter().copied() {
            if let Some(target) = counterparts.get(previous_id) {
                schnellui::widgets::inherit_remount_state(
                    &self.widgets,
                    &mut self.scene,
                    target,
                    &previous.widgets,
                    previous_id,
                );
            }
        }

        let restored = previous_focus.and_then(|(previous_id, ring_visible)| {
            let target = counterparts.get(previous_id).or_else(|| {
                previous_option_trigger.and_then(|trigger| counterparts.get(trigger))
            })?;
            Some(RemountFocus {
                target,
                ring_visible,
            })
        });

        if let Some(focus) = restored {
            self.restore_focus_after_remount(focus);
        }
    }

    /// Moves app-level host registrations into an internally reconstructed tree.
    /// External remount hooks build and configure their replacement themselves;
    /// theme rebuilds do not, so they must retain these registrations explicitly.
    pub(crate) fn inherit_host_configuration(&mut self, previous: &mut App) {
        self.test_registry = std::mem::take(&mut previous.test_registry);
        self.shortcut_registry = std::mem::take(&mut previous.shortcut_registry);
        self.focused_key_bindings = std::mem::take(&mut previous.focused_key_bindings);
        self.focused_input_bindings = std::mem::take(&mut previous.focused_input_bindings);

        let cursor_counterparts = remount::CounterpartMap::new(
            &previous.scene,
            &self.scene,
            previous
                .cursor_bindings
                .iter()
                .map(|binding| binding.widget),
        );
        self.cursor_bindings = std::mem::take(&mut previous.cursor_bindings)
            .into_iter()
            .filter_map(|mut binding| {
                binding.widget = cursor_counterparts.get(binding.widget)?;
                Some(binding)
            })
            .collect();
        let paint_counterparts = remount::CounterpartMap::new(
            &previous.scene,
            &self.scene,
            previous.paint_bindings.node_ids(),
        );
        self.paint_bindings =
            std::mem::replace(&mut previous.paint_bindings, bindings::PaintBindings::new());
        self.paint_bindings
            .remap(|widget| paint_counterparts.get(widget));
        self.continuous_redraw = previous.continuous_redraw;
        self.redraw_interval = previous.redraw_interval;
        self.hot_reload = previous.hot_reload.take();
        self.redraw_signal = previous.redraw_signal.clone();
        self.window_title_provider = previous.window_title_provider.take();
        self.interaction_trace = previous.interaction_trace.take();
    }

    /// Carries user-adjusted dialog geometry across a structural remount.
    ///
    /// Widget ids are mount-local, so surviving dialogs are paired in tree order
    /// by semantic role and accessible name. Only the interactive geometry fields
    /// are copied; the replacement view remains authoritative for every other
    /// layout property.
    pub(crate) fn inherit_dialog_geometry(&mut self, previous: &App) {
        fn layer_for_panel(scene: &Scene, panel: WidgetId) -> Option<WidgetId> {
            let mut current = Some(panel);
            while let Some(id) = current {
                if scene
                    .node(id)
                    .is_some_and(|node| node.kind == WidgetKind::DialogLayer)
                {
                    return Some(id);
                }
                current = scene.node(id).and_then(|node| node.parent);
            }
            None
        }

        let mut previous_geometry: Vec<_> = previous
            .scene
            .preorder()
            .filter(|id| {
                previous
                    .scene
                    .node(*id)
                    .is_some_and(|node| node.kind == WidgetKind::Dialog)
            })
            .filter_map(|panel| {
                let a11y = previous.scene.a11y(panel)?;
                let style = previous.layout.container_style(panel)?;
                let layer = layer_for_panel(&previous.scene, panel)?;
                Some(DialogGeometry {
                    panel,
                    reference: previous.scene.component_ref(layer),
                    role: a11y.role,
                    name: a11y.name.clone(),
                    anchor: style.anchor,
                    width: style.width,
                    height: style.height,
                })
            })
            .collect();

        let panels: Vec<_> = self
            .scene
            .preorder()
            .filter(|id| {
                self.scene
                    .node(*id)
                    .is_some_and(|node| node.kind == WidgetKind::Dialog)
            })
            .collect();
        for panel in panels {
            let Some(a11y) = self.scene.a11y(panel) else {
                continue;
            };
            let reference = layer_for_panel(&self.scene, panel)
                .and_then(|layer| self.scene.component_ref(layer));
            let referenced = reference.and_then(|reference| {
                previous_geometry
                    .iter()
                    .position(|geometry| geometry.reference == Some(reference))
            });
            let fallback = || {
                previous_geometry.iter().position(|geometry| {
                    geometry.reference.is_none()
                        && geometry.role == a11y.role
                        && geometry.name == a11y.name
                })
            };
            let Some(index) = referenced.or_else(fallback) else {
                continue;
            };
            let geometry = previous_geometry.remove(index);
            if !schnellui::widgets::transfer_dialog_geometry_adjustment(
                &self.widgets,
                panel,
                &previous.widgets,
                geometry.panel,
            ) {
                continue;
            }
            let Some(mut style) = self.layout.container_style(panel) else {
                continue;
            };
            style.anchor = geometry.anchor;
            style.width = geometry.width;
            style.height = geometry.height;
            self.layout.set_container(panel, style);
            self.laid_out = false;
        }
    }

    /// Carries vertical viewport positions across a structural remount. Scrolls are
    /// paired by an explicit restoration key when configured, otherwise by accessible
    /// label and occurrence in tree order. End-following viewports remain pinned only
    /// when they were already at the end. Final layout clamps restored values against
    /// replacement content geometry.
    pub(crate) fn inherit_scroll_offsets(&mut self, previous: &App) {
        let previous_scrolls: Vec<_> = previous
            .scene
            .preorder()
            .filter(|id| {
                previous
                    .scene
                    .node(*id)
                    .is_some_and(|node| node.kind == WidgetKind::Scroll)
            })
            .collect();
        let counterparts = remount::CounterpartMap::new(
            &previous.scene,
            &self.scene,
            previous_scrolls.iter().copied(),
        );
        let mut previous_offsets: Vec<_> = previous_scrolls
            .into_iter()
            .map(|id| {
                (
                    id,
                    schnellui::widgets::scroll_restoration_key(&previous.widgets, id),
                    previous.scene.scroll_offset(id).y,
                    schnellui::widgets::scroll_is_at_end(&previous.scene, id),
                )
            })
            .collect();
        let scrolls: Vec<_> = self
            .scene
            .preorder()
            .filter(|id| {
                self.scene
                    .node(*id)
                    .is_some_and(|node| node.kind == WidgetKind::Scroll)
            })
            .collect();
        for id in scrolls {
            let restoration_key = schnellui::widgets::scroll_restoration_key(&self.widgets, id);
            let keyed = restoration_key.as_ref().and_then(|key| {
                previous_offsets
                    .iter()
                    .position(|(_, previous_key, _, _)| previous_key.as_ref() == Some(key))
            });
            let counterpart = || {
                previous_offsets
                    .iter()
                    .position(|(previous_id, previous_key, _, _)| {
                        previous_key.is_none() && counterparts.get(*previous_id) == Some(id)
                    })
            };
            let Some(index) = keyed.or_else(counterpart) else {
                continue;
            };
            let (_, _, offset, was_at_end) = previous_offsets.remove(index);
            let follows_end = schnellui::widgets::scroll_follows_end(&self.widgets, id);
            let restored_offset = if follows_end && was_at_end {
                f32::MAX
            } else {
                offset
            };
            self.scene.set_scroll_offset(
                id,
                Point {
                    x: 0.0,
                    y: restored_offset,
                },
            );
            if restored_offset.is_finite() {
                self.scene
                    .set_a11y_value_i64(id, restored_offset.round() as i64);
            }
        }
        self.laid_out = false;
    }
}

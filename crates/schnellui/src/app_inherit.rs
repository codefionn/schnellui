//! Remount inheritance: state, host configuration and geometry carried to a replacement app.

use super::*;

/// Whether `id` can hold focus in `scene`: it advertises the focus action and
/// is not disabled. A control rebuilt disabled keeps its counterpart but can
/// no longer take focus, like a browser's disabled `<button>`.
pub(crate) fn takes_focus(scene: &Scene, id: WidgetId) -> bool {
    scene.a11y(id).is_some_and(|a| {
        a11y::ActionFlags(a.actions).contains(a11y::ActionFlags::FOCUS)
            && !a11y::StateFlags(a.state).contains(a11y::StateFlags::DISABLED)
    })
}

/// For each control of `order` (the old tab order), whether it kept its place
/// relative to the other survivors in the new tab order. A survivor has a
/// counterpart in `replacement`'s tab order; the stayers are the longest run of
/// survivors whose counterparts appear in the same order as before. What is
/// left moved: a row archived into another section takes its children with it.
fn kept_place(
    order: &[WidgetId],
    counterparts: &remount::CounterpartMap,
    replacement: &Scene,
) -> Vec<bool> {
    schnellui::a11y::with_tab_order(replacement, |new_order| {
        let new_index: HashMap<WidgetId, usize> = new_order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        // The survivors as (old position, new position), in old order.
        let survivors: Vec<(usize, usize)> = order
            .iter()
            .enumerate()
            .filter_map(|(old, id)| Some((old, *new_index.get(&counterparts.get(*id)?)?)))
            .collect();
        // Longest increasing subsequence of the new positions, by patience
        // sorting: `tails[k]` ends the best run of length k + 1.
        let mut tails: Vec<usize> = Vec::new();
        let mut parent = vec![usize::MAX; survivors.len()];
        for (i, (_, new)) in survivors.iter().enumerate() {
            let k = tails.partition_point(|t| survivors[*t].1 < *new);
            if k > 0 {
                parent[i] = tails[k - 1];
            }
            if k == tails.len() {
                tails.push(i);
            } else {
                tails[k] = i;
            }
        }
        let mut kept = vec![false; order.len()];
        let mut at = tails.last().copied();
        while let Some(i) = at {
            kept[survivors[i].0] = true;
            at = Some(parent[i]).filter(|p| *p != usize::MAX);
        }
        kept
    })
}

/// Whether Tab can stop on `id`: it is no roving-group member, or the group's
/// entry. A tab bar's other tabs are reached by arrow keys, not Tab.
pub(crate) fn is_tab_stop(scene: &Scene, id: WidgetId) -> bool {
    match crate::app_interaction::roving_group(scene, id) {
        None => true,
        Some(group) => schnellui::a11y::with_tab_order(scene, |order| {
            crate::app_interaction::roving_entry(scene, order, group) == Some(id)
        }),
    }
}

/// The counterpart in `replacement` of the control nearest to `focused` in
/// `previous`'s tab order, trying later controls before earlier ones. Only runs
/// when the focused control has no counterpart, so it costs a remount nothing
/// in the common case. A counterpart that cannot take focus is skipped. The
/// flag is true when the neighbour came after `focused` in the old order.
///
/// Controls that moved are passed over: when `focused` was archived away its
/// children went with it to another part of the page, and the nearest of them
/// in the old order is far from the gap in the new one. The first neighbour
/// that kept its place wins (see [`kept_place`]). With none left, the nearest
/// survivor does, moved or not. With `stops_only` (keyboard focus, which shows
/// the ring) a neighbour Tab can reach is preferred over one it skips.
///
/// When the dialog or window that held `focused` is gone too, its neighbours in
/// the tab order are the controls of other windows. Focus would land on another
/// window's Close button, so only controls outside every dialog qualify then.
fn surviving_neighbour(
    previous: &App,
    next: &App,
    focused: WidgetId,
    stops_only: bool,
) -> Option<(WidgetId, bool)> {
    let replacement = &next.scene;
    let home =
        schnellui::widgets::enclosing_dialog_panel(&previous.widgets, &previous.scene, focused);
    schnellui::a11y::with_tab_order(&previous.scene, |order| {
        let position = order.iter().position(|id| *id == focused)?;
        let counterparts = remount::CounterpartMap::new(
            &previous.scene,
            replacement,
            order.iter().copied().chain(home),
        );
        let home_is_gone = home.is_some_and(|panel| counterparts.get(panel).is_none());
        let kept = kept_place(order, &counterparts, replacement);
        let walk = |only_kept: bool, only_stops: bool| {
            order[position + 1..]
                .iter()
                .enumerate()
                .map(|(i, id)| (position + 1 + i, id, true))
                .chain(
                    order[..position]
                        .iter()
                        .enumerate()
                        .rev()
                        .map(|(i, id)| (i, id, false)),
                )
                .filter(|(i, _, _)| !only_kept || kept[*i])
                .filter_map(|(_, id, after)| Some((counterparts.get(*id)?, after)))
                .filter(|(id, _)| {
                    !home_is_gone
                        || schnellui::widgets::enclosing_dialog_panel(
                            &next.widgets,
                            replacement,
                            *id,
                        )
                        .is_none()
                })
                .filter(|(id, _)| !only_stops || is_tab_stop(replacement, *id))
                .find(|(id, _)| takes_focus(replacement, *id))
        };
        // Keyboard focus lands where Tab can reach, when something there
        // qualifies; a pointer start point may be any neighbour.
        let stops = stops_only;
        (stops
            .then(|| walk(true, true).or_else(|| walk(false, true)))
            .flatten())
        .or_else(|| walk(true, false))
        .or_else(|| walk(false, false))
    })
}

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
        self.keyboard_modality = previous.keyboard_modality;
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

        let previous_menu = schnellui::widgets::open_context_menu_anchor(&previous.widgets);
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
        if let Some((source, _, _)) = previous_menu {
            stateful_nodes.push(source);
        }
        // Controls the user changed away from their authored value, such as a
        // ticked checkbox or a moved slider, need a counterpart too. Text
        // editors are listed above already.
        let changed_controls: Vec<_> = previous
            .scene
            .preorder()
            .filter(|id| {
                schnellui::widgets::control_changed_by_user(&previous.widgets, &previous.scene, *id)
            })
            .collect();
        stateful_nodes.extend(changed_controls.iter().copied());
        let previous_drag = schnellui::widgets::content_drag_source(&previous.widgets);
        stateful_nodes.extend(previous_drag);
        let previous_scrollbar = schnellui::widgets::scrollbar_pointer_target(&previous.widgets);
        stateful_nodes.extend(previous_scrollbar);
        let previous_panels = schnellui::widgets::dialog_panels(&previous.widgets);
        let previous_windows =
            schnellui::widgets::dialogs_front_first(&previous.widgets, &previous.scene);
        let modal_ids = previous
            .modal_openers
            .iter()
            .flat_map(|entry| [entry.panel, entry.opener])
            .chain(previous_panels.iter().copied())
            .chain(previous_windows.iter().filter_map(|&(_, last)| last));
        let previous_nav_start = schnellui::widgets::nav_start(&previous.widgets);
        let previous_reveal = schnellui::widgets::pending_reveal(&previous.widgets);
        let counterparts = remount::CounterpartMap::new(
            &previous.scene,
            &self.scene,
            stateful_nodes
                .iter()
                .copied()
                .chain(modal_ids)
                .chain(previous_nav_start.map(|start| start.widget))
                .chain(previous_reveal),
        );

        self.inherit_dialog_geometry(previous);
        self.inherit_scroll_offsets(previous);

        // An editor can temporarily lose focus to a popup or another field and
        // still regain it later. Carry every surviving retained counterpart, not
        // only the one that happened to be focused at the instant of the remount.
        for previous_id in stateful_nodes.iter().copied() {
            if let Some(target) = counterparts.get(previous_id) {
                // The value first: the caret and history below ride along
                // only when the replacement holds the same text.
                schnellui::widgets::inherit_control_value(
                    &self.widgets,
                    &mut self.scene,
                    &mut self.text,
                    &mut self.atlas,
                    target,
                    &previous.widgets,
                    &previous.scene,
                    previous_id,
                );
                schnellui::widgets::inherit_remount_state(
                    &self.widgets,
                    &mut self.scene,
                    target,
                    &previous.widgets,
                    previous_id,
                );
            }
        }

        // A press on a window's body took focus off its controls. When that
        // window is gone now, focus counts as having been in it.
        let effective_focus = previous_focus.or_else(|| {
            let stand_in =
                previous.pressed_dialog_stand_in(|panel| counterparts.get(panel).is_none())?;
            Some((stand_in, false))
        });
        let restored = previous_focus.and_then(|(previous_id, ring_visible)| {
            let target = counterparts.get(previous_id).or_else(|| {
                previous_option_trigger.and_then(|trigger| counterparts.get(trigger))
            })?;
            // A control rebuilt disabled cannot hold focus. Fall through to
            // the neighbour rule instead of ringing a control that is not
            // the focused one.
            if !takes_focus(&self.scene, target) {
                return None;
            }
            // A picked or dismissed list hands focus back to its field, with the ring
            // as the picking or the Escape left it.
            let ring_visible = if previous_option_trigger.is_some() {
                previous.keyboard_modality
            } else {
                ring_visible
            };
            Some(RemountFocus {
                target,
                ring_visible,
            })
        });

        // A closed dialog or window returns focus to the control that opened it, when
        // that control survives.
        let restored = restored.or_else(|| {
            let opener = previous.modal_opener_of(effective_focus?.0)?;
            let target = counterparts.get(opener)?;
            if !takes_focus(&self.scene, target) {
                return None;
            }
            // The ring follows how it closed, not how it opened.
            Some(RemountFocus {
                target,
                ring_visible: previous.keyboard_modality,
            })
        });

        // The rebuild took focus from its control and opened a window in the
        // same breath: focus goes into the window, as dialog.show() does, with
        // the ring as it was. The opener is recorded below, so closing the
        // window returns focus to it.
        let known: Vec<_> = previous_panels
            .iter()
            .filter_map(|panel| counterparts.get(*panel))
            .collect();
        let restored = restored.or_else(|| {
            let (_, ring_visible) = previous_focus?;
            let target = self.first_control_of_new_window(&known)?;
            Some(RemountFocus {
                target,
                ring_visible,
            })
        });

        // A window or modal with no opener (open at mount, or its opener is gone) closed
        // with focus in it: the front window left open gives focus back to its control,
        // else the page's first tab stop. Focus never ends on nothing.
        let restored = restored.or_else(|| {
            let (focused, _) = effective_focus?;
            let home = schnellui::widgets::enclosing_dialog_panel(
                &previous.widgets,
                &previous.scene,
                focused,
            )?;
            if counterparts.get(home).is_some() {
                return None;
            }
            let remaining: Vec<_> = previous_windows
                .iter()
                .filter_map(|&(panel, last)| {
                    Some((
                        counterparts.get(panel)?,
                        last.and_then(|id| counterparts.get(id)),
                    ))
                })
                .collect();
            let target = self.closed_window_fallback(&remaining)?;
            Some(RemountFocus {
                target,
                ring_visible: previous.keyboard_modality,
            })
        });

        // The focused control did not survive. Rather than strand a keyboard
        // user at the top of the page, hand focus to the nearest control of
        // the old tab order that did: the next one first, then the previous.
        //
        // That is for a keyboard user, who sees the ring move. Focus that was
        // not visible (a pointer press) is cleared instead: parking it on a
        // neighbour would let Enter activate a control nobody chose. The
        // neighbour becomes the starting point of the next Tab.
        let mut nav_start = None;
        let restored = restored.or_else(|| {
            let (focused, ring_visible) = effective_focus?;
            let (target, after) = surviving_neighbour(previous, self, focused, ring_visible)?;
            if !ring_visible {
                nav_start = Some(schnellui::widgets::NavStart {
                    widget: target,
                    before: after,
                });
                return None;
            }
            Some(RemountFocus {
                target,
                ring_visible,
            })
        });

        self.pressed_dialog = previous
            .pressed_dialog
            .and_then(|panel| counterparts.get(panel));
        self.modal_openers = previous
            .modal_openers
            .iter()
            .filter_map(|entry| {
                Some(ModalOpener {
                    panel: counterparts.get(entry.panel)?,
                    opener: counterparts.get(entry.opener)?,
                })
            })
            .collect();
        // A modal that opened with this remount took focus from the
        // counterpart of the previous focus.
        let focus_before = previous_focus.and_then(|(id, _)| counterparts.get(id));
        self.note_opened_modal(focus_before, &known);

        if let Some(focus) = restored {
            self.restore_focus_after_remount(focus);
        } else if self.focused_widget().is_none() {
            // A starting point that was already waiting survives the remount.
            let carried = previous_nav_start.and_then(|start| {
                Some(schnellui::widgets::NavStart {
                    widget: counterparts.get(start.widget)?,
                    ..start
                })
            });
            schnellui::widgets::set_nav_start(&self.widgets, nav_start.or(carried));
        }

        // A reveal that waited for layout still waits for it, on the
        // control's counterpart. A rebuild must not drop it: a theme fade
        // rebuilds every frame, and the action would never scroll.
        if let Some(target) = previous_reveal.and_then(|id| counterparts.get(id)) {
            schnellui::widgets::request_reveal(&self.widgets, target);
        }

        // A context menu open on a control that survives the rebuild stays
        // open where it was. Otherwise a theme fade or a background remount
        // would close it under the pointer a frame after it opened.
        self.reopen_context_menu = previous_menu.and_then(|(source, position, can_paste)| {
            Some((counterparts.get(source)?, position, can_paste))
        });
        // A scrollbar thumb held across the rebuild stays held on the
        // counterpart of its viewport.
        if let Some(target) = previous_scrollbar.and_then(|id| counterparts.get(id)) {
            schnellui::widgets::transfer_scrollbar_pointer(
                &self.widgets,
                target,
                &previous.widgets,
            );
        }
        // A drag in progress keeps going from the source's counterpart, so
        // a rebuild under a held button neither drops it nor fires a click.
        self.redrive_content_drag = previous_drag
            .and_then(|source| counterparts.get(source))
            .and_then(|source| {
                schnellui::widgets::transfer_content_drag(&self.widgets, source, &previous.widgets)
            });
    }

    /// Moves app-level host registrations into an internally reconstructed tree.
    /// External remount hooks build and configure their replacement themselves;
    /// theme rebuilds do not, so they must retain these registrations explicitly.
    pub(crate) fn inherit_host_configuration(&mut self, previous: &mut App) {
        self.test_registry = std::mem::take(&mut previous.test_registry);
        self.shortcut_registry = std::mem::take(&mut previous.shortcut_registry);
        self.inherit_global_shortcuts(previous);
        self.inherit_global_menu(previous);
        self.navigation_handler = previous.navigation_handler.take();
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
        self.escape_exits = previous.escape_exits;
        self.redraw_interval = previous.redraw_interval;
        self.input_observer = previous.input_observer.take();
        self.native_window_observer = previous.native_window_observer.take();
        self.hot_reload = previous.hot_reload.take();
        self.redraw_signal = previous.redraw_signal.clone();
        self.window_title_provider = previous.window_title_provider.take();
        self.interaction_trace = previous.interaction_trace.take();
    }

    /// Carries user-adjusted dialog geometry across a structural remount,
    /// and a move or resize still in progress.
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
            schnellui::widgets::transfer_dialog_pointer(
                &self.widgets,
                panel,
                &previous.widgets,
                geometry.panel,
            );
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

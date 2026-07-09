use super::event_handler::{raw_capture_target_survives, remount_allowed};
use super::*;

/// Deduplicates replacements addressed to the same component within one host
/// update. The first occurrence fixes the relative order among distinct
/// branches; every later occurrence replaces its payload, so the newest view
/// and reason win without rebuilding the branch more than once.
pub(super) fn coalesce_subtree_replacements(
    replacements: Vec<SubtreeReplacement>,
) -> Vec<SubtreeReplacement> {
    let mut positions = BTreeMap::new();
    let mut coalesced = Vec::with_capacity(replacements.len());
    for replacement in replacements {
        let target = replacement.target.id();
        if let Some(position) = positions.get(&target).copied() {
            coalesced[position] = replacement;
        } else {
            positions.insert(target, coalesced.len());
            coalesced.push(replacement);
        }
    }
    coalesced
}

impl WindowedApp {
    pub(super) fn apply_focused_input_result(&mut self, result: FocusedInputResult) -> bool {
        match result {
            FocusedInputResult::Ignored => false,
            FocusedInputResult::Handled => true,
            FocusedInputResult::CopyText(text) => {
                let _ = self.clipboard.write_text(text);
                true
            }
        }
    }

    pub(super) fn dispatch_raw_key(&mut self, event: &KeyEvent) -> bool {
        let raw = RawKeyEvent {
            logical_key: event.logical_key.clone(),
            physical_key: event.physical_key,
            key_without_modifiers: event.key_without_modifiers(),
            location: event.location,
            modifiers: raw_modifiers(self.modifiers, self.control_pressed),
            state: raw_input_state(event.state),
            repeat: event.repeat,
            text: event.text.as_ref().map(ToString::to_string),
            text_with_all_modifiers: event.text_with_all_modifiers().map(ToString::to_string),
        };
        let result = self.app.dispatch_focused_input(FocusedInputEvent::Key(raw));
        self.apply_focused_input_result(result)
    }

    pub(super) fn dispatch_raw_clipboard_chord(&mut self, event: &KeyEvent) -> bool {
        if event.state != ElementState::Pressed || event.repeat {
            return false;
        }
        let modifiers = raw_modifiers(self.modifiers, self.control_pressed);
        if !modifiers.control || !modifiers.shift || modifiers.alt || modifiers.super_key {
            return false;
        }
        let Key::Character(key) = &event.logical_key else {
            return false;
        };
        if key.eq_ignore_ascii_case("c") {
            let result = self
                .app
                .dispatch_focused_input(FocusedInputEvent::Clipboard(FocusedClipboardEvent::Copy));
            return self.apply_focused_input_result(result);
        }
        if key.eq_ignore_ascii_case("v") {
            let Ok(text) = self.clipboard.read_text() else {
                return false;
            };
            let result = self
                .app
                .dispatch_focused_input(FocusedInputEvent::Clipboard(
                    FocusedClipboardEvent::Paste(text),
                ));
            return self.apply_focused_input_result(result);
        }
        false
    }

    pub(super) fn copy_or_cut(&mut self, cut: bool) -> bool {
        let Some(source) = self.app.focused_widget() else {
            return false;
        };
        self.copy_or_cut_from(source, cut)
    }

    pub(super) fn copy_or_cut_from(&mut self, source: SceneWidgetId, cut: bool) -> bool {
        let Some(selected) = self.app.selected_text_for(source) else {
            return false;
        };
        let copied = self.clipboard.write_text(selected).is_ok();
        copied && cut && self.app.delete_text_selection_for(source)
    }

    pub(super) fn paste(&mut self) -> bool {
        let Some(source) = self.app.focused_widget() else {
            return false;
        };
        self.paste_into(source)
    }

    pub(super) fn paste_into(&mut self, source: SceneWidgetId) -> bool {
        let Ok(text) = self.clipboard.read_text() else {
            return false;
        };
        self.app.paste_text_for(source, &text)
    }

    pub(super) fn activate_context_menu_item(&mut self, id: SceneWidgetId) -> bool {
        let Some(activation) = self.app.activate_context_menu_item(id) else {
            return false;
        };
        match activation.action {
            schnellui_widgets::ContextMenuAction::Cut => {
                let _ = self.copy_or_cut_from(activation.source, true);
            }
            schnellui_widgets::ContextMenuAction::Copy => {
                let _ = self.copy_or_cut_from(activation.source, false);
            }
            schnellui_widgets::ContextMenuAction::Paste => {
                let _ = self.paste_into(activation.source);
            }
            schnellui_widgets::ContextMenuAction::SelectAll => {
                let _ = self.app.select_all_text_for(activation.source);
            }
            schnellui_widgets::ContextMenuAction::Custom => {}
        }
        true
    }

    pub(super) fn open_focused_context_menu(&mut self) -> bool {
        let Some(source) = self.app.focused_widget().and_then(|id| {
            schnellui_widgets::context_menu_source(&self.app.widgets.clone(), self.app.scene(), id)
        }) else {
            return false;
        };
        let position = self
            .app
            .scene()
            .layout(source)
            .map(|layout| Point {
                x: layout.rect.x,
                y: layout.rect.y + layout.rect.height,
            })
            .unwrap_or_default();
        let can_paste = self.clipboard.read_text().is_ok();
        self.app.open_context_menu(source, position, can_paste)
    }

    pub(super) fn text_press_action(
        &mut self,
        target: SceneWidgetId,
        position: Point,
    ) -> TextPointerAction {
        if self.modifiers.alt_key() {
            self.last_text_click = None;
            return TextPointerAction::AddCaret;
        }
        if self.modifiers.shift_key() {
            self.last_text_click = None;
            return TextPointerAction::Place { extend: true };
        }
        let now = Instant::now();
        let count = self
            .last_text_click
            .filter(|last| {
                last.target == target
                    && now.duration_since(last.at) <= MULTI_CLICK_INTERVAL
                    && (last.position.x - position.x).abs() <= MULTI_CLICK_SLOP
                    && (last.position.y - position.y).abs() <= MULTI_CLICK_SLOP
            })
            .map(|last| if last.count < 3 { last.count + 1 } else { 1 })
            .unwrap_or(1);
        self.last_text_click = Some(TextClick {
            target,
            position,
            at: now,
            count,
        });
        match count {
            2 => TextPointerAction::SelectWord,
            3 => TextPointerAction::SelectLine,
            _ => TextPointerAction::Place { extend: false },
        }
    }

    /// True once the autoclose deadline (if any) has elapsed.
    pub(super) fn autoclose_elapsed(&self) -> bool {
        self.deadline.map(|d| Instant::now() >= d).unwrap_or(false)
    }

    pub(super) fn interaction_snapshot(&self, point: Point) -> Value {
        let widget_state = schnellui_widgets::interaction_debug_state(&self.app.widgets);
        json!({
            "pointer": {
                "physical": { "x": self.cursor.x, "y": self.cursor.y },
                "logical": point_json(point),
            },
            "hit_path": hit_path_json(&self.app, point),
            "focus": focused_json(&self.app),
            "cursor": {
                "resolved": debug_label(self.app.cursor_at(point)),
                "applied": debug_label(self.cursor_icon),
            },
            "capture": {
                "raw_buttons": self.raw_pointer_capture
                    .iter()
                    .map(|button| debug_label(*button))
                    .collect::<Vec<_>>(),
                "text_selection": self.drag_text.map(|id| schnellui_a11y::to_access_id(id).0),
                "slider": self.drag_slider.map(|id| schnellui_a11y::to_access_id(id).0),
                "content_drag_source": widget_state
                    .content_drag_source
                    .map(|id| schnellui_a11y::to_access_id(id).0),
                "content_drag_active": widget_state.content_drag_active,
                "dialog_pointer": widget_state.dialog_pointer_capture,
            },
        })
    }

    pub(super) fn trace(&mut self, event: &'static str, payload: Value) {
        if let Some(trace) = &mut self.interaction_trace {
            trace.record(event, payload);
        }
    }

    /// Reconnects a freshly themed tree to the existing native surface.
    pub(super) fn sync_renderer_after_theme_rebuild(&mut self) {
        if let Some(r) = &mut self.renderer {
            r.set_scale(self.app.scale());
            r.set_clear_color(self.app.clear_color());
            r.invalidate_atlases();
            let (pw, ph) = r.size();
            let scale = self.app.scale();
            self.app.resize(pw as f32 / scale, ph as f32 / scale);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        self.accessibility_full_update_pending = true;
    }

    pub(super) fn sync_window_title(&mut self) {
        let Some(provider) = self.app.window_title_provider.as_mut() else {
            return;
        };
        let next = provider();
        if next == self.title {
            return;
        }
        if let Some(window) = &self.window {
            window.set_title(&next);
        }
        self.title = next;
        self.accessibility_full_update_pending = true;
    }

    /// Sends the current complete semantic tree when a platform first activates
    /// accessibility. The app has already completed an initial layout before the
    /// adapter is made visible, so bounds are valid even before the first redraw.
    pub(super) fn publish_full_accessibility_tree(&mut self) {
        let Some(adapter) = self.accessibility.as_mut() else {
            return;
        };
        let scale = self.app.scale();
        let title = self.title.as_str();
        let scene = self.app.scene();
        adapter.update_if_active(|| {
            schnellui_a11y::build_full_window_tree_update(scene, scale, title)
        });
    }

    /// Settles one retained frame, publishes its accessibility changes, presents
    /// pixels, then clears the shared dirty channels. Keeping that order prevents
    /// `App::frame` from retiring the one-node a11y delta before the adapter sees it.
    pub(super) fn redraw(&mut self) {
        if self.app.settle_frame() {
            // Layout bounds are part of every AccessKit node. A reflow can move
            // clean semantic siblings, so geometry changes require a full tree.
            self.accessibility_full_update_pending = true;
        }
        if let Some(adapter) = self.accessibility.as_mut() {
            let scale = self.app.scale();
            let title = self.title.as_str();
            let scene = self.app.scene();
            if self.accessibility_full_update_pending {
                adapter.update_if_active(|| {
                    schnellui_a11y::build_full_window_tree_update(scene, scale, title)
                });
            } else {
                adapter.update_if_active(|| {
                    schnellui_a11y::build_incremental_window_tree_update(scene, scale, title)
                });
            }
        }
        if let Some(renderer) = &mut self.renderer {
            self.app.render_to_surface(renderer);
        }
        self.app.scene.clear_dirty();
        self.accessibility_full_update_pending = false;
    }

    /// Polls the host's remount hook ([`App::run_windowed_with`]) and, when it
    /// yields a new [`App`], swaps it in **under the live window** (SOUL §8) —
    /// this is what lets an example switcher change screens without closing and
    /// reopening the window. Runs *after* the event that fired the handlers, so
    /// the old tree's closures have fully returned before it is dropped (the new
    /// mount's `widgets::reset()` has already cleared their registries). The new
    /// tree inherits the surface: renderer scale/clear follow the new app, the GPU
    /// glyph atlas is reconciled from actual texel content, the tree is laid out at
    /// the window's current size, and a redraw is requested.
    pub(super) fn poll_remount(&mut self) {
        // A structural replacement between pointer press and release
        // invalidates the WidgetId retained by click/drag handling. Leave
        // the remount hook unpolled so its pending state is preserved; the
        // release event clears `left_pointer_down` and applies the newest
        // tree after the interaction has completed.
        if !remount_allowed(self.left_pointer_down) {
            return;
        }
        let viewport = self.app.size();
        let update = self.remount.as_mut().and_then(|hook| hook(viewport));
        let Some(update) = update else {
            return;
        };
        let replacement = match update {
            WindowUpdate::Remount(replacement) => replacement,
            WindowUpdate::Subtrees(replacements) => {
                self.apply_subtree_replacements(replacements, viewport);
                return;
            }
        };
        let Remount {
            app: mut new_app,
            reason,
        } = replacement;
        let point = self.cursor_logical();
        let before = self.interaction_snapshot(point);
        let widget_state = schnellui_widgets::interaction_debug_state(&self.app.widgets);
        // Raw surfaces (browser/terminal/canvas-style integrations) are
        // addressed semantically rather than by retained WidgetId. When an
        // otherwise identical surface survives a frame-swap remount, its
        // active button stream can safely continue against the counterpart
        // in the new tree. This is essential for browser pages that produce
        // a new frame between pointer press and release.
        let continuing_raw_capture = !self.raw_pointer_capture.is_empty()
            && raw_capture_target_survives(&self.app, &new_app);
        let mut interrupted = Vec::new();
        if !self.raw_pointer_capture.is_empty() && !continuing_raw_capture {
            interrupted.push("raw_pointer");
        }
        if self.drag_text.is_some() {
            interrupted.push("text_selection");
        }
        if self.drag_slider.is_some() {
            interrupted.push("slider");
        }
        if widget_state.content_drag_source.is_some() {
            interrupted.push("content_drag");
        }
        if widget_state.dialog_pointer_capture {
            interrupted.push("dialog_pointer");
        }
        if !interrupted.is_empty() {
            self.trace(
                "interaction_interrupted_by_remount",
                json!({
                    "severity": "warning",
                    "reason": reason.as_ref(),
                    "trigger": self.remount_trigger,
                    "interrupted": interrupted,
                    "before": before.clone(),
                }),
            );
        }
        let reduce_motion = !self.app.animations_enabled();
        new_app.inherit_remount_state(&self.app);
        let _ = new_app.apply_reduced_motion(reduce_motion);
        new_app.redraw_signal.install(self.event_loop_proxy.clone());
        self.app = new_app;
        // A drag target from the old tree must not extend selections in the new one.
        self.drag_text = None;
        self.drag_slider = None;
        self.last_text_click = None;
        if !continuing_raw_capture {
            self.raw_pointer_capture.clear();
        }
        if let Some(r) = &mut self.renderer {
            r.set_scale(self.app.scale());
            r.set_clear_color(self.app.clear_color());
            // Lay the new tree out at the window's *current* logical size — the
            // window may have been resized since the app was mounted.
            let (pw, ph) = r.size();
            let scale = self.app.scale();
            self.app.resize(pw as f32 / scale, ph as f32 / scale);
        }
        self.accessibility_full_update_pending = true;
        // Settle replacement geometry immediately so a stationary pointer
        // keeps its hover treatment and native cursor without waiting for a
        // synthetic CursorMoved event or briefly falling back to Default.
        let layout_changed = self.app.settle_frame();
        self.accessibility_full_update_pending |= layout_changed;
        if let Some(renderer) = &mut self.renderer {
            // The replacement is now fully shaped. Reconcile its deterministic
            // glyph atlas against the renderer-owned resident shadow so a chat
            // stream remount changes only new coverage instead of recreating a
            // 1024×1024 texture and bind group for every event page.
            renderer.reconcile_remount_atlases(&mut self.app.atlas);
        }
        let pointer = self.cursor_logical();
        let _ = self.app.update_pointer_proximity(pointer);
        self.remount_count = self.remount_count.saturating_add(1);
        let reason_count = self
            .remount_counts_by_reason
            .entry(reason.to_string())
            .or_default();
        *reason_count = reason_count.saturating_add(1);
        self.last_remount = Some((reason.to_string(), self.remount_trigger));
        let after = self.interaction_snapshot(point);
        self.trace(
            "remount",
            json!({
                "count": self.remount_count,
                "reason": reason.as_ref(),
                "trigger": self.remount_trigger,
                "viewport": { "width": viewport.width, "height": viewport.height },
                "capture_was_interrupted": !interrupted.is_empty(),
                "before": before,
                "after": after,
            }),
        );
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    pub(super) fn apply_subtree_replacements(
        &mut self,
        replacements: Vec<SubtreeReplacement>,
        viewport: Size,
    ) {
        let replacements = coalesce_subtree_replacements(replacements);
        let point = self.cursor_logical();
        let before = self.interaction_snapshot(point);
        let mut applied = Vec::with_capacity(replacements.len());
        let mut focus_after = None;
        for replacement in replacements {
            let target = replacement.target;
            let reason = replacement.reason;
            if replacement.focus_after.is_some() {
                focus_after = replacement.focus_after;
            }
            match self.app.replace_subtree_boxed(target, replacement.view) {
                Ok(root) => applied.push((target.id(), root, reason)),
                Err(error) => self.trace(
                    "subtree_replacement_rejected",
                    json!({
                        "severity": "warning",
                        "reason": reason.as_ref(),
                        "trigger": self.remount_trigger,
                        "target_ref": target.id(),
                        "error": error.to_string(),
                    }),
                ),
            }
        }
        if applied.is_empty() {
            return;
        }

        if let Some(target) = focus_after {
            if let Some(widget) = self.app.scene().resolve_ref(target) {
                let _ = self.app.focus(Some(widget));
            }
        }

        let layout_changed = self.app.settle_frame();
        self.accessibility_full_update_pending |= layout_changed;
        let pointer = self.cursor_logical();
        let _ = self.app.update_pointer_proximity(pointer);
        self.subtree_replacement_count = self
            .subtree_replacement_count
            .saturating_add(u64::try_from(applied.len()).unwrap_or(u64::MAX));
        let after = self.interaction_snapshot(point);
        self.trace(
            "subtree_replacement",
            json!({
                "count": self.subtree_replacement_count,
                "batch_count": applied.len(),
                "reasons": applied.iter().map(|(_, _, reason)| reason.as_ref()).collect::<Vec<_>>(),
                "targets": applied.iter().map(|(target, _, _)| *target).collect::<Vec<_>>(),
                "trigger": self.remount_trigger,
                "viewport": { "width": viewport.width, "height": viewport.height },
                "before": before,
                "after": after,
            }),
        );
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// The last cursor position in **logical** px (hit-testing/layout space).
    pub(super) fn cursor_logical(&self) -> Point {
        let scale = self.app.scale();
        Point {
            x: self.cursor.x as f32 / scale,
            y: self.cursor.y as f32 / scale,
        }
    }

    /// Reconciles the widget system's semantic cursor with winit. Dialog
    /// capture is part of `App::cursor_at`, so move/resize feedback remains
    /// stable even after the pointer leaves the original chrome.
    pub(super) fn sync_cursor(&mut self) {
        let next = self.app.cursor_at(self.cursor_logical());
        if next == self.cursor_icon {
            return;
        }
        let previous = self.cursor_icon;
        let snapshot = self.interaction_snapshot(self.cursor_logical());
        self.trace(
            "cursor_changed",
            json!({
                "from": debug_label(previous),
                "to": debug_label(next),
                "trigger": self.remount_trigger,
                "interaction": snapshot,
            }),
        );
        if let Some(window) = &self.window {
            window.set_cursor_visible(next != UiCursorIcon::None);
        }
        if next == UiCursorIcon::None {
            self.cursor_icon = next;
            return;
        }
        let native = match next {
            UiCursorIcon::None => unreachable!("hidden cursor handled above"),
            UiCursorIcon::Default => WinitCursorIcon::Default,
            UiCursorIcon::ContextMenu => WinitCursorIcon::ContextMenu,
            UiCursorIcon::Help => WinitCursorIcon::Help,
            UiCursorIcon::Pointer => WinitCursorIcon::Pointer,
            UiCursorIcon::Progress => WinitCursorIcon::Progress,
            UiCursorIcon::Wait => WinitCursorIcon::Wait,
            UiCursorIcon::Cell => WinitCursorIcon::Cell,
            UiCursorIcon::Crosshair => WinitCursorIcon::Crosshair,
            UiCursorIcon::Text => WinitCursorIcon::Text,
            UiCursorIcon::VerticalText => WinitCursorIcon::VerticalText,
            UiCursorIcon::Alias => WinitCursorIcon::Alias,
            UiCursorIcon::Copy => WinitCursorIcon::Copy,
            UiCursorIcon::Move => WinitCursorIcon::Move,
            UiCursorIcon::NoDrop => WinitCursorIcon::NoDrop,
            UiCursorIcon::NotAllowed => WinitCursorIcon::NotAllowed,
            UiCursorIcon::Grab => WinitCursorIcon::Grab,
            UiCursorIcon::Grabbing => WinitCursorIcon::Grabbing,
            UiCursorIcon::EResize => WinitCursorIcon::EResize,
            UiCursorIcon::NResize => WinitCursorIcon::NResize,
            UiCursorIcon::NeResize => WinitCursorIcon::NeResize,
            UiCursorIcon::NwResize => WinitCursorIcon::NwResize,
            UiCursorIcon::SResize => WinitCursorIcon::SResize,
            UiCursorIcon::SeResize => WinitCursorIcon::SeResize,
            UiCursorIcon::SwResize => WinitCursorIcon::SwResize,
            UiCursorIcon::WResize => WinitCursorIcon::WResize,
            UiCursorIcon::NwseResize => WinitCursorIcon::NwseResize,
            UiCursorIcon::EwResize => WinitCursorIcon::EwResize,
            UiCursorIcon::NsResize => WinitCursorIcon::NsResize,
            UiCursorIcon::NeswResize => WinitCursorIcon::NeswResize,
            UiCursorIcon::ColResize => WinitCursorIcon::ColResize,
            UiCursorIcon::RowResize => WinitCursorIcon::RowResize,
            UiCursorIcon::AllScroll => WinitCursorIcon::AllScroll,
            UiCursorIcon::ZoomIn => WinitCursorIcon::ZoomIn,
            UiCursorIcon::ZoomOut => WinitCursorIcon::ZoomOut,
        };
        if let Some(window) = &self.window {
            window.set_cursor(native);
        }
        self.cursor_icon = next;
    }

    pub(super) fn settle_debug_command(&mut self) {
        self.poll_remount();
        if self.renderer.is_some() {
            self.redraw();
        } else {
            self.app.frame();
        }
        self.sync_cursor();
    }

    pub(super) fn debug_status(&self) -> Value {
        let point = self.cursor_logical();
        let viewport = self.app.size();
        let last_remount = self
            .last_remount
            .as_ref()
            .map(|(reason, trigger)| json!({ "reason": reason, "trigger": trigger }));
        json!({
            "schema": "schnellui-debug-v1",
            "title": self.title,
            "viewport": { "width": viewport.width, "height": viewport.height },
            "scale": self.app.scale(),
            "cursor": point_json(point),
            "cursor_icon": debug_label(self.cursor_icon),
            "focused": focused_json(&self.app),
            "hit_path": hit_path_json(&self.app, point),
            "remounts": {
                "total": self.remount_count,
                "by_reason": self.remount_counts_by_reason,
                "last": last_remount,
            },
        })
    }

    pub(super) fn debug_tree(&self) -> Value {
        fn add_rects(scene: &schnellui_scene::Scene, value: &mut Value) {
            let Some(object) = value.as_object_mut() else {
                return;
            };
            if let Some(id) = object.get("id").and_then(Value::as_u64) {
                let widget = schnellui_a11y::resolve_target(
                    scene,
                    schnellui_a11y::accesskit_reexport::NodeId(id),
                );
                if let Some(rect) = widget.and_then(|widget| scene.layout(widget)) {
                    object.insert(
                        "rect".into(),
                        json!({
                            "x": rect.rect.x,
                            "y": rect.rect.y,
                            "width": rect.rect.width,
                            "height": rect.rect.height,
                        }),
                    );
                }
            }
            if let Some(children) = object.get_mut("children").and_then(Value::as_array_mut) {
                for child in children {
                    add_rects(scene, child);
                }
            }
            if let Some(root) = object.get_mut("root") {
                add_rects(scene, root);
            }
        }

        let mut tree = serde_json::to_value(schnellui_a11y::dump_tree(self.app.scene()))
            .expect("debug tree serialization");
        add_rects(self.app.scene(), &mut tree);
        tree
    }

    pub(super) fn resolve_debug_target(
        &self,
        target: &DebugTarget,
    ) -> Result<SceneWidgetId, String> {
        if let Some(id) = target.id {
            return schnellui_a11y::resolve_target(
                self.app.scene(),
                schnellui_a11y::accesskit_reexport::NodeId(id),
            )
            .ok_or_else(|| format!("no live widget has id {id}"));
        }
        let role = target
            .role
            .as_deref()
            .ok_or_else(|| "target requires either id or role".to_string())?;
        fn find(
            node: &schnellui_a11y::A11yNodeDump,
            role: &str,
            name: Option<&str>,
        ) -> Option<u64> {
            if node.role == role
                && name
                    .map(|name| node.name.as_deref() == Some(name))
                    .unwrap_or(true)
            {
                return Some(node.id);
            }
            node.children
                .iter()
                .find_map(|child| find(child, role, name))
        }
        let tree = schnellui_a11y::dump_tree(self.app.scene());
        let id = tree
            .root
            .as_ref()
            .and_then(|root| find(root, role, target.name.as_deref()))
            .ok_or_else(|| {
                format!(
                    "no visible widget matches role {role:?} and name {:?}",
                    target.name
                )
            })?;
        schnellui_a11y::resolve_target(
            self.app.scene(),
            schnellui_a11y::accesskit_reexport::NodeId(id),
        )
        .ok_or_else(|| format!("matched widget {id} is no longer live"))
    }

    pub(super) fn move_debug_pointer(&mut self, point: &DebugPoint) -> Result<bool, String> {
        if !point.x.is_finite() || !point.y.is_finite() {
            return Err("pointer coordinates must be finite".into());
        }
        let viewport = self.app.size();
        if point.x < 0.0 || point.y < 0.0 || point.x > viewport.width || point.y > viewport.height {
            return Err(format!(
                "pointer ({}, {}) is outside viewport {}x{}",
                point.x, point.y, viewport.width, viewport.height
            ));
        }
        let scale = self.app.scale();
        self.cursor = PhysicalPosition::new(f64::from(point.x * scale), f64::from(point.y * scale));
        Ok(self.app.update_pointer_proximity(Point {
            x: point.x,
            y: point.y,
        }))
    }

    pub(super) fn handle_debug_action(&mut self, request: DebugAction) -> DebugReply {
        use schnellui_a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};

        let target = match self.resolve_debug_target(&request.target) {
            Ok(target) => target,
            Err(error) => return DebugReply::error(404, error),
        };
        let name = request.action.to_ascii_lowercase().replace('-', "_");
        let action = match name.as_str() {
            "click" => Action::Click,
            "focus" => Action::Focus,
            "blur" => Action::Blur,
            "increment" => Action::Increment,
            "decrement" => Action::Decrement,
            "scroll_up" => Action::ScrollUp,
            "scroll_down" => Action::ScrollDown,
            "show_context_menu" => Action::ShowContextMenu,
            "set_value" => Action::SetValue,
            _ => {
                return DebugReply::error(
                    422,
                    format!("unsupported semantic action {:?}", request.action),
                )
            }
        };
        let data = if action == Action::SetValue {
            let Some(value) = request.value else {
                return DebugReply::error(400, "set_value requires value");
            };
            Some(ActionData::Value(value.into()))
        } else {
            None
        };
        let changed = self.app.dispatch_action(&ActionRequest {
            action,
            target_tree: TreeId::ROOT,
            target_node: schnellui_a11y::to_access_id(target),
            data,
        });
        self.settle_debug_command();
        DebugReply::json(
            200,
            json!({
                "changed": changed,
                "target": schnellui_a11y::to_access_id(target).0,
                "tree": self.debug_tree(),
            }),
        )
    }

    pub(super) fn handle_debug_key(&mut self, request: DebugKey) -> DebugReply {
        let name = request.key.to_ascii_lowercase().replace('-', "_");
        let changed = match name.as_str() {
            "tab" => self.app.dispatch_key(UiKey::Tab {
                shift: request.shift,
            }),
            "enter" => self.app.dispatch_key(UiKey::Enter),
            "space" => self.app.dispatch_key(UiKey::Space {
                shift: request.shift,
            }),
            "backspace" => self.app.dispatch_key(UiKey::Backspace),
            "delete" => self.app.dispatch_key(UiKey::Delete),
            "left" => self.app.dispatch_key(UiKey::Left {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "right" => self.app.dispatch_key(UiKey::Right {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "up" => self.app.dispatch_key(UiKey::Up {
                shift: request.shift,
            }),
            "down" => self.app.dispatch_key(UiKey::Down {
                shift: request.shift,
            }),
            "home" => self.app.dispatch_key(UiKey::Home {
                shift: request.shift,
            }),
            "end" => self.app.dispatch_key(UiKey::End {
                shift: request.shift,
            }),
            "page_up" => self.app.dispatch_key(UiKey::PageUp),
            "page_down" => self.app.dispatch_key(UiKey::PageDown),
            "escape" => self.app.dispatch_key(UiKey::Escape),
            "select_all" => self.app.dispatch_key(UiKey::SelectAll),
            "text" => {
                let Some(text) = request.text.as_deref() else {
                    return DebugReply::error(400, "text key requires text");
                };
                self.app.dispatch_key(UiKey::Char(text))
            }
            _ => return DebugReply::error(422, format!("unsupported key {:?}", request.key)),
        };
        self.settle_debug_command();
        DebugReply::json(
            200,
            json!({ "changed": changed, "status": self.debug_status() }),
        )
    }

    pub(super) fn handle_debug_command(
        &mut self,
        event_loop: &ActiveEventLoop,
        command: DebugCommand,
    ) -> DebugReply {
        match command {
            DebugCommand::Tree => {
                self.settle_debug_command();
                DebugReply {
                    status: 200,
                    content_type: "application/json",
                    body: serde_json::to_vec_pretty(&self.debug_tree())
                        .expect("debug tree serialization"),
                }
            }
            DebugCommand::Status => {
                self.settle_debug_command();
                DebugReply::json(200, self.debug_status())
            }
            DebugCommand::Snapshot => {
                self.settle_debug_command();
                DebugReply::json(
                    200,
                    json!({
                        "status": self.debug_status(),
                        "tree": self.debug_tree(),
                    }),
                )
            }
            DebugCommand::Screenshot => {
                self.settle_debug_command();
                DebugReply::png(self.app.render_png_bytes())
            }
            DebugCommand::Action(request) => self.handle_debug_action(request),
            DebugCommand::PointerMove(point) => {
                let changed = match self.move_debug_pointer(&point) {
                    Ok(changed) => changed,
                    Err(error) => return DebugReply::error(422, error),
                };
                self.settle_debug_command();
                DebugReply::json(
                    200,
                    json!({ "changed": changed, "status": self.debug_status() }),
                )
            }
            DebugCommand::PointerClick(point) => {
                if let Err(error) = self.move_debug_pointer(&point) {
                    return DebugReply::error(422, error);
                }
                let logical = Point {
                    x: point.x,
                    y: point.y,
                };
                let Some(target) = schnellui_widgets::hit_test(
                    &self.app.widgets.clone(),
                    self.app.scene(),
                    logical,
                ) else {
                    return DebugReply::error(404, "no widget at pointer coordinates");
                };
                let request = DebugAction {
                    action: "click".into(),
                    target: DebugTarget {
                        id: Some(schnellui_a11y::to_access_id(target).0),
                        role: None,
                        name: None,
                    },
                    value: None,
                };
                self.handle_debug_action(request)
            }
            DebugCommand::Key(request) => self.handle_debug_key(request),
            DebugCommand::Quit => {
                event_loop.exit();
                DebugReply::json(200, json!({ "quitting": true }))
            }
        }
    }

    /// Translates a pressed key into a [`UiKey`] and routes it through
    /// [`App::dispatch_key`] — the standard-browser keyboard path (SOUL §6.3):
    /// Tab focus traversal, Enter/Space activation, slider and radio arrows,
    /// text editing on the focused editable, and page scrolling. Returns
    /// `true` if anything changed (⇒ redraw).
    pub(super) fn handle_key(&mut self, event: &KeyEvent) -> bool {
        let shift = self.modifiers.shift_key();
        let modifier_text = event.text_with_all_modifiers();
        let ctrl = self.modifiers.control_key()
            || self.control_pressed
            || modifier_text_implies_control(modifier_text, event.physical_key);
        let command = if cfg!(target_os = "macos") {
            self.modifiers.super_key()
        } else {
            ctrl
        };
        let logical_text = match &event.logical_key {
            Key::Character(value) => Some(value.as_ref()),
            _ => None,
        };
        let control_character = resolve_control_letter(modifier_text, logical_text, ctrl);
        if let Some(character) = control_character {
            if self
                .app
                .dispatch_focused_key_handler(UiKey::Control(character))
            {
                return true;
            }
        }
        if command {
            match &event.logical_key {
                Key::Character(c) if c.eq_ignore_ascii_case("c") => {
                    return self.copy_or_cut(false);
                }
                Key::Character(c) if c.eq_ignore_ascii_case("x") => {
                    return self.copy_or_cut(true);
                }
                Key::Character(c) if c.eq_ignore_ascii_case("v") => {
                    return self.paste();
                }
                _ => {}
            }
        }
        // Clipboard chords above retain native editing behavior. All other
        // single-character command chords can be claimed by the application.
        // Do not repeat application commands while a key is held (a toggling
        // command such as Ctrl+, would otherwise oscillate between screens).
        if !event.repeat {
            if let Key::Character(value) = &event.logical_key {
                let mut chars = value.chars();
                if let Some(character) = chars.next() {
                    if chars.next().is_none()
                        && self.app.dispatch_shortcut(Shortcut::new(
                            character,
                            command,
                            shift,
                            self.modifiers.alt_key(),
                        ))
                    {
                        return true;
                    }
                }
            }
        }
        let physical_key = special_key_from_physical(event.physical_key, shift, ctrl);
        let key = match &event.logical_key {
            Key::Named(NamedKey::Tab) => UiKey::Tab { shift },
            Key::Named(NamedKey::Enter) => UiKey::Enter,
            Key::Named(NamedKey::Space) => UiKey::Space { shift },
            Key::Named(NamedKey::Backspace) => UiKey::Backspace,
            Key::Named(NamedKey::Delete) => UiKey::Delete,
            Key::Named(NamedKey::ArrowLeft) => UiKey::Left { shift, ctrl },
            Key::Named(NamedKey::ArrowRight) => UiKey::Right { shift, ctrl },
            Key::Named(NamedKey::ArrowUp) => UiKey::Up { shift },
            Key::Named(NamedKey::ArrowDown) => UiKey::Down { shift },
            Key::Named(NamedKey::Home) => UiKey::Home { shift },
            Key::Named(NamedKey::End) => UiKey::End { shift },
            Key::Named(NamedKey::PageUp) => UiKey::PageUp,
            Key::Named(NamedKey::PageDown) => UiKey::PageDown,
            Key::Named(NamedKey::Escape) => UiKey::Escape,
            Key::Character(c) if command && c.eq_ignore_ascii_case("a") => UiKey::SelectAll,
            _ => {
                if let Some(key) = physical_key {
                    return self.app.dispatch_key(key);
                }
                // Character insertion rides `KeyEvent::text` (the layout- and
                // shift-resolved text). Command chords and control characters
                // (Enter, Esc, …) never insert.
                if ctrl || self.modifiers.alt_key() || self.modifiers.super_key() {
                    return false;
                }
                return match &event.text {
                    Some(t) if !t.is_empty() && !t.chars().any(|ch| ch.is_control()) => {
                        self.app.dispatch_key(UiKey::Char(t))
                    }
                    _ => false,
                };
            }
        };
        self.app.dispatch_key(key)
    }
}

/// Builds the event loop and runs `app` in a window titled `title` (SOUL §8).
/// `remount` is the optional in-window remount hook ([`App::run_windowed_with`]).
pub fn run(
    app: App,
    title: &str,
    remount: Option<Box<dyn FnMut(schnellui_scene::Size) -> Option<WindowUpdate>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Physical window size = logical viewport × scale (SOUL §7.1 `--scale` reused
    // as the logical→physical factor for the window).
    let scale = app.scale();
    let logical = app.size();
    let pw = (logical.width * scale).round().max(1.0) as u32;
    let ph = (logical.height * scale).round().max(1.0) as u32;
    let mut interaction_trace = InteractionRecorder::open(app.interaction_trace.clone())?;
    if let Some(trace) = &mut interaction_trace {
        trace.record(
            "session_started",
            json!({
                "title": title,
                "viewport": { "width": logical.width, "height": logical.height },
                "physical_size": { "width": pw, "height": ph },
                "scale": scale,
            }),
        );
    }

    let event_loop = EventLoop::<PlatformEvent>::with_user_event().build()?;
    // Reactive by default; `about_to_wait` narrows to `WaitUntil` when autoclosing.
    event_loop.set_control_flow(ControlFlow::Wait);
    let event_loop_proxy = event_loop.create_proxy();
    app.redraw_signal.install(event_loop_proxy.clone());
    let _ = watch_preferences({
        let proxy = event_loop_proxy.clone();
        move |change| match change {
            PreferenceChange::ReducedMotion(reduce) => proxy
                .send_event(PlatformEvent::ReducedMotionChanged(reduce))
                .is_ok(),
            _ => true,
        }
    });
    let debug_server = if crate::debug_server::enabled() {
        let proxy = event_loop_proxy.clone();
        match crate::debug_server::start(title, move |request| {
            proxy.send_event(PlatformEvent::Debug(request)).is_ok()
        }) {
            Ok(server) => {
                eprintln!(
                    "schnellui debug instrumentation listening on {}",
                    server.endpoint()
                );
                Some(server)
            }
            Err(error) => {
                eprintln!("schnellui debug instrumentation unavailable: {error}");
                None
            }
        }
    } else {
        None
    };

    let mut handler = WindowedApp {
        app,
        clipboard: SystemClipboard::new(),
        title: title.to_string(),
        window: None,
        renderer: None,
        accessibility: None,
        event_loop_proxy,
        accessibility_full_update_pending: true,
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_icon: UiCursorIcon::Default,
        interaction_trace,
        _debug_server: debug_server,
        remount_trigger: "startup",
        remount_count: 0,
        subtree_replacement_count: 0,
        remount_counts_by_reason: BTreeMap::new(),
        last_remount: None,
        init_phys: PhysicalSize::new(pw, ph),
        deadline: autoclose_deadline(),
        interval_deadline: None,
        animation_deadline: None,
        modifiers: ModifiersState::default(),
        control_pressed: false,
        raw_pointer_capture: Vec::new(),
        drag_text: None,
        last_text_click: None,
        drag_slider: None,
        left_pointer_down: false,
        remount,
    };
    event_loop.run_app(&mut handler)?;
    Ok(())
}

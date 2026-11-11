use super::event_handler::{raw_capture_target_survives, EventTail};
use super::*;

/// What the platform adapter last received, kept to decide the next update.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct AccessibilityPublished {
    /// [`Scene::render_revision`] when the tree was last sent. A different value
    /// means nodes were inserted, removed, reordered or shown, which an
    /// a11y-dirty list does not describe.
    revision: u64,
    /// The focus target of the last update.
    focus: Option<crate::accesskit_reexport::NodeId>,
    /// The accessibility scope root (active modal) of the last update.
    modal: Option<SceneWidgetId>,
}

impl AccessibilityPublished {
    pub(super) fn of(scene: &schnellui::scene::Scene) -> Self {
        Self {
            revision: scene.render_revision(),
            focus: Some(schnellui::a11y::focus_node_id(scene)),
            modal: schnellui::a11y::active_modal_root(scene),
        }
    }
}

/// The accessibility update one frame needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AccessibilityPlan {
    /// Nothing is a11y-dirty and focus is unchanged: send nothing.
    Skip,
    /// Send only the a11y-dirty nodes (text, state, and nodes whose bounds moved).
    Incremental,
    /// Send the whole tree: first publish, remount, resize, or a structural or
    /// modal-scope change the dirty list cannot express.
    Full,
}

/// Chooses the update for the frame just settled. A full tree is the exception
/// (SOUL §6.2); a reflow alone is not a reason for one, because the scene marks
/// every node whose box changed a11y-dirty.
pub(super) fn plan_accessibility_update(
    scene: &schnellui::scene::Scene,
    full_pending: bool,
    published: &AccessibilityPublished,
) -> AccessibilityPlan {
    if full_pending
        || published.revision != scene.render_revision()
        || published.modal != schnellui::a11y::active_modal_root(scene)
    {
        AccessibilityPlan::Full
    } else if published
        .focus
        .is_some_and(|last| schnellui::a11y::incremental_update_needed(scene, last))
    {
        AccessibilityPlan::Incremental
    } else {
        AccessibilityPlan::Skip
    }
}

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

/// Hands the running app's redraw signal, already wired to the event loop, to
/// its replacement. Worker threads keep the handle they got at startup. A fresh
/// signal in the new app would leave that handle's pending flag set forever,
/// since only the running app's signal is drained, and every later worker wake
/// would be dropped until input forced a frame.
pub(super) fn carry_redraw_signal(previous: &App, replacement: &mut App) {
    replacement.redraw_signal = previous.redraw_signal.clone();
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
            let paste = match self.clipboard.read_text() {
                Ok(text) => FocusedClipboardEvent::Paste(text),
                Err(_) => match self.clipboard.read_image() {
                    Ok(image) => FocusedClipboardEvent::PasteImage(image),
                    Err(_) => return false,
                },
            };
            let result = self
                .app
                .dispatch_focused_input(FocusedInputEvent::Clipboard(paste));
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
        let activated = self.app.activate_context_menu_item(id).is_some();
        self.drain_pending_clipboard();
        activated
    }

    /// Performs the Cut, Copy or Paste a context-menu row left on the app.
    /// Every dispatch that can activate a row ends here, so the pointer, key
    /// and accessibility paths share one copy of the logic.
    pub(super) fn drain_pending_clipboard(&mut self) -> bool {
        use schnellui::widgets::ContextMenuAction;
        let Some(request) = self.app.take_pending_clipboard() else {
            return false;
        };
        match request.action {
            ContextMenuAction::Cut => self.copy_or_cut_from(request.source, true),
            ContextMenuAction::Copy => self.copy_or_cut_from(request.source, false),
            ContextMenuAction::Paste => self.paste_into(request.source),
            ContextMenuAction::SelectAll | ContextMenuAction::Custom => false,
        }
    }

    /// Dispatches a key to the app, then performs any clipboard command a
    /// context-menu row asked for.
    pub(super) fn dispatch_ui_key(&mut self, key: UiKey<'_>) -> bool {
        let changed = self.app.dispatch_key(key);
        self.drain_pending_clipboard() | changed
    }

    /// A key pressed while a menu is open: the arrows, Home and End walk its
    /// rows, Enter or Space picks the focused row and any other key closes
    /// the menu.
    pub(super) fn context_menu_key(&mut self, event: &KeyEvent) -> bool {
        let shift = self.modifiers.shift_key();
        let walk = match &event.logical_key {
            Key::Named(NamedKey::ArrowDown) => Some(UiKey::Down { shift }),
            Key::Named(NamedKey::ArrowUp) => Some(UiKey::Up { shift }),
            Key::Named(NamedKey::Home) => Some(UiKey::Home { shift, ctrl: false }),
            Key::Named(NamedKey::End) => Some(UiKey::End { shift, ctrl: false }),
            _ => None,
        };
        if let Some(changed) = walk.and_then(|key| self.app.step_context_menu(key)) {
            return changed;
        }
        if matches!(
            &event.logical_key,
            Key::Named(NamedKey::Enter | NamedKey::Space)
        ) {
            if let Some(row) = self.app.focused_context_menu_row() {
                return self.activate_context_menu_item(row);
            }
        }
        self.app.dismiss_context_menu()
    }

    pub(super) fn open_focused_context_menu(&mut self) -> bool {
        let Some(source) = self.app.focused_widget().and_then(|id| {
            schnellui::widgets::context_menu_source(&self.app.widgets.clone(), self.app.scene(), id)
        }) else {
            return false;
        };
        let anchor = self.app.context_menu_anchor(source);
        let can_paste = self.clipboard.read_text().is_ok();
        self.app
            .open_context_menu_anchored(source, anchor, can_paste)
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
        // The app clock: the wall clock in a window, the pinned time in a
        // simulation, so a replay counts clicks from the gaps it chose.
        let now = self.app.clock_now();
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

    /// The widgets under the pointer. Empty once the pointer has left the
    /// window: the last position is kept for the cursor report, but nothing is
    /// under it any more (hover is gone). A key that merely hides hover keeps
    /// the pointer inside, so its hit path stays.
    fn hit_path_now(&self, point: Point) -> Value {
        if self.cursor_inside {
            hit_path_json(&self.app, point)
        } else {
            json!([])
        }
    }

    /// The pointer/hit/focus/capture state recorded with trace events. Building it
    /// walks the hit path and serializes JSON, so it is skipped (`Null`) unless a
    /// trace recorder is open; the value is only ever consumed by `trace`.
    pub(super) fn interaction_snapshot(&self, point: Point) -> Value {
        if self.interaction_trace.is_none() {
            return Value::Null;
        }
        self.build_interaction_snapshot(point)
    }

    /// Unconditional form of [`Self::interaction_snapshot`], for the debug
    /// server, which answers regardless of tracing.
    pub(super) fn build_interaction_snapshot(&self, point: Point) -> Value {
        let widget_state = schnellui::widgets::interaction_debug_state(&self.app.widgets);
        json!({
            "pointer": {
                "physical": { "x": self.cursor.x, "y": self.cursor.y },
                "logical": point_json(point),
            },
            "hit_path": self.hit_path_now(point),
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
                "text_selection": self.drag_text.map(|id| schnellui::a11y::to_access_id(id).0),
                "slider": self.drag_slider.map(|id| schnellui::a11y::to_access_id(id).0),
                "content_drag_source": widget_state
                    .content_drag_source
                    .map(|id| schnellui::a11y::to_access_id(id).0),
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

    /// Like [`Self::trace`], but builds the payload only when a trace recorder is
    /// open, so the JSON maps cost nothing while tracing is off.
    pub(super) fn trace_with(&mut self, event: &'static str, payload: impl FnOnce(&Self) -> Value) {
        if self.interaction_trace.is_some() {
            let payload = payload(self);
            self.trace(event, payload);
        }
    }

    /// Records one key, typed text or IME commit, from the window or the debug
    /// server, as a `key_input` record. The payload is built only while a
    /// recorder is open. Typed text is private, so only its length is recorded
    /// unless the trace opted in to text (`InteractionTrace::include_text`).
    pub(super) fn trace_key_input(
        &mut self,
        origin: &'static str,
        key: impl FnOnce() -> String,
        input: KeyTrace<'_>,
    ) {
        let Some(trace) = &self.interaction_trace else {
            return;
        };
        let include_text = trace.includes_text();
        let mut payload = json!({
            "origin": origin,
            "key": key(),
            "state": input.state,
            "repeat": input.repeat,
            "shift": input.shift,
            "ctrl": input.ctrl,
            "alt": input.alt,
            "super": input.super_key,
        });
        if let Some(text) = input.text {
            payload["text_len"] = json!(text.chars().count());
            if include_text {
                payload["text"] = json!(text);
            }
        }
        self.trace("key_input", payload);
    }

    /// Records an accessibility action, from the debug server or the platform
    /// adapter, as an `a11y_action` record naming the target by id, role and
    /// name. A `set_value` value is private like typed text: length only.
    pub(super) fn trace_a11y_action(
        &mut self,
        origin: &'static str,
        action: schnellui::a11y::accesskit_reexport::Action,
        target: schnellui::a11y::accesskit_reexport::NodeId,
        value: Option<&str>,
    ) {
        let Some(trace) = &self.interaction_trace else {
            return;
        };
        let include_text = trace.includes_text();
        let mut label = String::new();
        for (i, c) in format!("{action:?}").chars().enumerate() {
            if c.is_ascii_uppercase() && i > 0 {
                label.push('_');
            }
            label.push(c.to_ascii_lowercase());
        }
        let widget = schnellui::a11y::resolve_target(self.app.scene(), target);
        let a11y = widget.and_then(|id| self.app.scene().a11y(id));
        let mut payload = json!({
            "origin": origin,
            "action": label,
            "target": {
                "id": target.0,
                "role": a11y.map(|node| schnellui::a11y::Role::from_u16(node.role).label()),
                "name": a11y.and_then(|node| node.name.as_deref()),
            },
        });
        if let Some(value) = value {
            payload["value_len"] = json!(value.chars().count());
            if include_text {
                payload["value"] = json!(value);
            }
        }
        self.trace("a11y_action", payload);
    }

    /// Reconnects a freshly themed tree to the existing native surface.
    pub(super) fn sync_renderer_after_theme_rebuild(&mut self) {
        if let Some(r) = &mut self.renderer {
            r.set_scale(self.app.scale());
            r.set_clear_color(self.app.clear_color());
            // A theme rebuild carries the old atlas over; the renderer's own
            // generation tracking then uploads only what changed. A replaced atlas
            // (or a custom rebuild) still gets the conservative full reset.
            let atlas_id = self.app.atlas.id();
            if atlas_id != self.glyph_atlas_id {
                r.invalidate_atlases();
                self.glyph_atlas_id = atlas_id;
            }
            let (pw, ph) = r.size();
            let scale = self.app.scale();
            self.app.resize(pw as f32 / scale, ph as f32 / scale);
        }
        // Lay the rebuilt tree out now, as a remount does, so input that
        // arrives before the redraw hits the new widgets and a resting
        // pointer keeps its hover treatment.
        let _ = self.app.settle_frame();
        let pointer = self.resting_pointer();
        let _ = self.app.update_pointer_proximity(pointer);
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
            schnellui::a11y::build_full_window_tree_update(scene, scale, title)
        });
        self.accessibility_published = AccessibilityPublished::of(self.app.scene());
    }

    /// Settles one retained frame, publishes its accessibility changes, presents
    /// pixels, then clears the shared dirty channels. Keeping that order prevents
    /// `App::frame` from retiring the one-node a11y delta before the adapter sees it.
    pub(super) fn redraw(&mut self) {
        // Layout bounds reach the tree through per-node a11y dirt (the scene marks
        // a node whose box moved), so a reflow no longer forces a full tree.
        let _ = self.app.settle_frame();
        if let Some(adapter) = self.accessibility.as_mut() {
            let scale = self.app.scale();
            let title = self.title.as_str();
            let scene = self.app.scene();
            match plan_accessibility_update(
                scene,
                self.accessibility_full_update_pending,
                &self.accessibility_published,
            ) {
                AccessibilityPlan::Skip => {}
                AccessibilityPlan::Full => {
                    adapter.update_if_active(|| {
                        schnellui::a11y::build_full_window_tree_update(scene, scale, title)
                    });
                }
                AccessibilityPlan::Incremental => {
                    adapter.update_if_active(|| {
                        schnellui::a11y::build_incremental_window_tree_update(scene, scale, title)
                    });
                }
            }
            self.accessibility_published = AccessibilityPublished::of(self.app.scene());
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
    ///
    /// A held pointer button does not hold the update back: the drag in
    /// progress follows its control to the replacement tree
    /// ([`WindowedApp::apply_update`]), so input during a drag acts on the
    /// current tree at once.
    pub(super) fn poll_remount(&mut self) {
        let viewport = self.app.size();
        let Some(hook) = self.remount.as_mut() else {
            return;
        };
        // Hooks that build on `App::with_remount_engine` fork the running
        // app's shaper and atlas while the hook runs.
        let (update, _engine_claimed) =
            crate::app_new::with_engine_donor(&mut self.app, || hook(viewport));
        let Some(update) = update else {
            return;
        };
        self.apply_update(update, viewport);
    }

    fn apply_update(&mut self, update: WindowUpdate, viewport: Size) {
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
        let widget_state = schnellui::widgets::interaction_debug_state(&self.app.widgets);
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
        // A text selection or slider drag carries on in the control's
        // counterpart. It ends only when the replacement has none.
        let held_controls = crate::remount::CounterpartMap::new(
            self.app.scene(),
            new_app.scene(),
            [self.drag_text, self.drag_slider, self.backdrop_press]
                .into_iter()
                .flatten(),
        );
        let drag_text = self.drag_text.and_then(|id| held_controls.get(id));
        let drag_slider = self.drag_slider.and_then(|id| held_controls.get(id));
        let backdrop_press = self.backdrop_press.and_then(|id| held_controls.get(id));
        if self.drag_text.is_some() && drag_text.is_none() {
            interrupted.push("text_selection");
        }
        if self.drag_slider.is_some() && drag_slider.is_none() {
            interrupted.push("slider");
        }
        if widget_state.content_drag_source.is_some() {
            interrupted.push("content_drag");
        }
        if widget_state.dialog_pointer_capture {
            interrupted.push("dialog_pointer");
        }
        if !interrupted.is_empty() {
            self.trace_with("interaction_interrupted_by_remount", |this| {
                json!({
                    "severity": "warning",
                    "reason": reason.as_ref(),
                    "trigger": this.remount_trigger,
                    "interrupted": interrupted,
                    "before": before.clone(),
                })
            });
        }
        let reduce_motion = !self.app.animations_enabled();
        // A replacement built with `App::with_remount_engine` already owns the old
        // shaper and atlas (same atlas id, so no reconcile below); others bring
        // their own, and their glyph rects already point into it.
        // A fork keeps the running atlas id only if the running app has not
        // rasterized anything since the fork; otherwise the ids would alias
        // different texels and the reconcile below must run.
        new_app.atlas.resolve_fork(&self.app.atlas);
        new_app.inherit_remount_state(&self.app);
        let _ = new_app.apply_reduced_motion(reduce_motion);
        carry_redraw_signal(&self.app, &mut new_app);
        self.app = new_app;
        // A drag target of the old tree continues on its counterpart; one
        // without a counterpart is gone, and the release clicks nothing.
        self.drag_text = drag_text;
        self.drag_slider = drag_slider;
        self.backdrop_press = backdrop_press;
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
        let _ = self.app.settle_frame();
        if let Some(renderer) = &mut self.renderer {
            // The replacement is now fully shaped. Reconcile its deterministic
            // glyph atlas against the renderer-owned resident shadow so a chat
            // stream remount changes only new coverage instead of recreating a
            // 1024×1024 texture and bind group for every event page.
            let atlas_id = self.app.atlas.id();
            if atlas_id != self.glyph_atlas_id {
                renderer.reconcile_remount_atlases(&mut self.app.atlas);
                self.glyph_atlas_id = atlas_id;
            }
        }
        let pointer = self.resting_pointer();
        let _ = self.app.update_pointer_proximity(pointer);
        self.remount_count = self.remount_count.saturating_add(1);
        let reason_count = self
            .remount_counts_by_reason
            .entry(reason.to_string())
            .or_default();
        *reason_count = reason_count.saturating_add(1);
        self.last_remount = Some((reason.to_string(), self.remount_trigger));
        let after = self.interaction_snapshot(point);
        self.trace_with("remount", |this| {
            json!({
                "count": this.remount_count,
                "reason": reason.as_ref(),
                "trigger": this.remount_trigger,
                "viewport": { "width": viewport.width, "height": viewport.height },
                "capture_was_interrupted": !interrupted.is_empty(),
                "before": before,
                "after": after,
            })
        });
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
        let ring = self.app.focus_ring_shown();
        for replacement in replacements {
            let target = replacement.target;
            let reason = replacement.reason;
            if replacement.focus_after.is_some() {
                focus_after = replacement.focus_after;
            }
            let region = self.app.region_focus_before(target);
            let escape_return = replacement.escape_return;
            let escape_fallback = replacement.escape_fallback;
            match self.app.replace_subtree_boxed(target, replacement.view) {
                Ok(root) => {
                    self.app
                        .note_region_return(target, escape_return, escape_fallback, region);
                    applied.push((target.id(), root, reason));
                }
                Err(error) => self.trace_with("subtree_replacement_rejected", |this| {
                    json!({
                        "severity": "warning",
                        "reason": reason.as_ref(),
                        "trigger": this.remount_trigger,
                        "target_ref": target.id(),
                        "error": error.to_string(),
                    })
                }),
            }
        }
        if applied.is_empty() {
            return;
        }
        // A drag whose control sat in a replaced subtree has nothing to drive.
        let scene = self.app.scene();
        let alive = |id: Option<_>| id.filter(|&id| scene.node(id).is_some());
        let (text, slider, backdrop) = (
            alive(self.drag_text),
            alive(self.drag_slider),
            alive(self.backdrop_press),
        );
        self.drag_text = text;
        self.drag_slider = slider;
        self.backdrop_press = backdrop;

        if let Some(target) = focus_after {
            if let Some(widget) = self.app.scene().resolve_ref(target) {
                let _ = self.app.focus_after_replacement(widget, ring);
            }
        }

        let _ = self.app.settle_frame();
        let pointer = self.resting_pointer();
        let _ = self.app.update_pointer_proximity(pointer);
        self.subtree_replacement_count = self
            .subtree_replacement_count
            .saturating_add(u64::try_from(applied.len()).unwrap_or(u64::MAX));
        let after = self.interaction_snapshot(point);
        self.trace_with("subtree_replacement", |this| {
            json!({
                "count": this.subtree_replacement_count,
                "batch_count": applied.len(),
                "reasons": applied.iter().map(|(_, _, reason)| reason.as_ref()).collect::<Vec<_>>(),
                "targets": applied.iter().map(|(target, _, _)| *target).collect::<Vec<_>>(),
                "trigger": this.remount_trigger,
                "viewport": { "width": viewport.width, "height": viewport.height },
                "before": before,
                "after": after,
            })
        });
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Where a rebuilt or remounted tree should find the pointer: its last
    /// position while it is inside the window, nowhere once it has left.
    pub(super) fn resting_pointer(&self) -> Point {
        if self.cursor_inside {
            self.cursor_logical()
        } else {
            Point {
                x: f32::NEG_INFINITY,
                y: f32::NEG_INFINITY,
            }
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

    /// Turns the input method on while a text field has focus and keeps its
    /// window over that field; turns it off when focus moves elsewhere.
    pub(super) fn sync_ime(&mut self) {
        let area = self.app.ime_area();
        if area == self.ime_area {
            return;
        }
        if let Some(window) = &self.window {
            if area.is_some() != self.ime_area.is_some() {
                window.set_ime_allowed(area.is_some());
            }
            if let Some(rect) = area {
                let scale = f64::from(self.app.scale());
                window.set_ime_cursor_area(
                    PhysicalPosition::new(f64::from(rect.x) * scale, f64::from(rect.y) * scale),
                    PhysicalSize::new(
                        f64::from(rect.width) * scale,
                        f64::from(rect.height) * scale,
                    ),
                );
            }
        }
        self.ime_area = area;
    }

    /// Scene state `sync_cursor` resolves against.
    pub(super) fn cursor_sync_state(&self) -> (schnellui::scene::SceneRenderKey, u64) {
        let scene = self.app.scene();
        (scene.render_key(), scene.paint_epoch())
    }

    /// Reconciles the widget system's semantic cursor with winit. Dialog
    /// capture is part of `App::cursor_at`, so move/resize feedback remains
    /// stable even after the pointer leaves the original chrome.
    pub(super) fn sync_cursor(&mut self) {
        self.cursor_synced_key = Some(self.cursor_sync_state());
        // A pointer outside the window hits nothing: only a capture (dialog
        // move/resize, a held drag) still decides the icon.
        let next = self.app.cursor_at(self.resting_pointer());
        if next == self.cursor_icon {
            return;
        }
        let previous = self.cursor_icon;
        let snapshot = self.interaction_snapshot(self.cursor_logical());
        self.trace_with("cursor_changed", |this| {
            json!({
                "from": debug_label(previous),
                "to": debug_label(next),
                "trigger": this.remount_trigger,
                "interaction": snapshot,
            })
        });
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
        self.on_redraw_requested();
        self.sync_cursor();
        self.sync_ime();
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
            "mode": if self.headless { "headless_windowed" } else { "windowed" },
            "title": self.title,
            "viewport": { "width": viewport.width, "height": viewport.height },
            "scale": self.app.scale(),
            "cursor": point_json(point),
            "cursor_icon": debug_label(self.cursor_icon),
            "cursor_inside": self.cursor_inside,
            "pointer": {
                "left_down": self.left_pointer_down,
                "raw_captured": self.raw_pointer_capture.len(),
                "text_drag": self.drag_text.is_some(),
                "slider_drag": self.drag_slider.is_some(),
                "scrollbar_drag": self.app.scrollbar_pointer_active(),
            },
            "focused": focused_json(&self.app),
            "ime_area": self.ime_area.map(|rect| json!({
                "x": rect.x,
                "y": rect.y,
                "width": rect.width,
                "height": rect.height,
            })),
            "hit_path": self.hit_path_now(point),
            "remounts": {
                "total": self.remount_count,
                "by_reason": self.remount_counts_by_reason,
                "last": last_remount,
            },
        })
    }

    pub(super) fn debug_tree(&self) -> Value {
        debug_tree_json(self.app.scene())
    }

    pub(super) fn resolve_debug_target(
        &self,
        target: &DebugTarget,
    ) -> Result<SceneWidgetId, String> {
        if let Some(id) = target.id {
            return schnellui::a11y::resolve_target(
                self.app.scene(),
                schnellui::a11y::accesskit_reexport::NodeId(id),
            )
            .ok_or_else(|| format!("no live widget has id {id}"));
        }
        let role = target
            .role
            .as_deref()
            .ok_or_else(|| "target requires either id or role".to_string())?;
        fn find(
            node: &schnellui::a11y::A11yNodeDump,
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
        let tree = schnellui::a11y::dump_tree(self.app.scene());
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
        schnellui::a11y::resolve_target(
            self.app.scene(),
            schnellui::a11y::accesskit_reexport::NodeId(id),
        )
        .ok_or_else(|| format!("matched widget {id} is no longer live"))
    }

    /// Moves the pointer through the window's own `CursorMoved` handler, so
    /// hover, drags, sliders, text selection and dialog moves see the same
    /// events a real mouse sends. Returns the handler's epilogue request.
    fn debug_cursor_moved(&mut self, point: Point) -> Result<EventTail, String> {
        self.check_debug_point(point)?;
        let scale = f64::from(self.app.scale());
        self.remount_trigger = "pointer_move";
        Ok(self.on_cursor_moved(PhysicalPosition::new(
            f64::from(point.x) * scale,
            f64::from(point.y) * scale,
        )))
    }

    fn check_debug_point(&self, point: Point) -> Result<(), String> {
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
        Ok(())
    }

    /// What `window_event` runs after a pointer handler: the remount poll the
    /// handler's tail asks for, then a settled frame.
    fn finish_debug_pointer(&mut self, tail: EventTail) {
        if tail == EventTail::Normal {
            self.poll_remount();
        }
        self.settle_debug_command();
    }

    pub(super) fn move_debug_pointer(&mut self, point: &DebugPoint) -> Result<bool, String> {
        let tail = self.debug_cursor_moved(Point {
            x: point.x,
            y: point.y,
        })?;
        self.finish_debug_pointer(tail);
        Ok(true)
    }

    fn debug_mouse_button(&mut self, state: winit::event::ElementState, button: DebugButton) {
        let button = match button {
            DebugButton::Left => winit::event::MouseButton::Left,
            DebugButton::Right => winit::event::MouseButton::Right,
            DebugButton::Middle => winit::event::MouseButton::Middle,
        };
        self.remount_trigger = "pointer_button";
        let tail = self.on_mouse_input(state, button);
        self.finish_debug_pointer(tail);
    }

    fn debug_press_point(&mut self, request: &DebugPress) -> Result<(), String> {
        match (request.x, request.y) {
            (Some(x), Some(y)) => {
                let tail = self.debug_cursor_moved(Point { x, y })?;
                self.finish_debug_pointer(tail);
                Ok(())
            }
            (None, None) => Ok(()),
            _ => Err("give both x and y, or neither".into()),
        }
    }

    fn handle_debug_press(
        &mut self,
        request: &DebugPress,
        state: winit::event::ElementState,
    ) -> DebugReply {
        if let Err(error) = self.debug_press_point(request) {
            return DebugReply::error(422, error);
        }
        self.debug_mouse_button(state, request.button);
        self.debug_pointer_reply()
    }

    fn handle_debug_drag(&mut self, request: &DebugDrag) -> DebugReply {
        let steps = request.steps.unwrap_or(8).clamp(1, 1000);
        let start = Point {
            x: request.x1,
            y: request.y1,
        };
        let end = Point {
            x: request.x2,
            y: request.y2,
        };
        // Both ends are checked before anything is pressed; the points
        // between lie inside the viewport with them.
        for point in [start, end] {
            if let Err(error) = self.check_debug_point(point) {
                return DebugReply::error(422, error);
            }
        }
        let tail = self.debug_cursor_moved(start).expect("checked above");
        self.finish_debug_pointer(tail);
        self.debug_mouse_button(winit::event::ElementState::Pressed, request.button);
        for step in 1..=steps {
            let t = step as f32 / steps as f32;
            let point = Point {
                x: start.x + (end.x - start.x) * t,
                y: start.y + (end.y - start.y) * t,
            };
            let tail = self.debug_cursor_moved(point).expect("checked above");
            self.finish_debug_pointer(tail);
        }
        self.debug_mouse_button(winit::event::ElementState::Released, request.button);
        self.debug_pointer_reply()
    }

    fn handle_debug_leave(&mut self) -> DebugReply {
        self.remount_trigger = "pointer_left";
        let tail = self.on_cursor_left();
        self.finish_debug_pointer(tail);
        self.debug_pointer_reply()
    }

    fn debug_pointer_reply(&self) -> DebugReply {
        DebugReply::json(
            200,
            json!({ "changed": true, "status": self.debug_status() }),
        )
    }

    pub(super) fn handle_debug_action(&mut self, request: DebugAction) -> DebugReply {
        use schnellui::a11y::accesskit_reexport::{Action, ActionData, ActionRequest, TreeId};

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
        self.trace_a11y_action(
            "debug",
            action,
            schnellui::a11y::to_access_id(target),
            request.value.as_deref(),
        );
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
            target_node: schnellui::a11y::to_access_id(target),
            data,
        });
        let changed = self.drain_pending_clipboard() | changed;
        self.settle_debug_command();
        DebugReply::json(
            200,
            json!({
                "changed": changed,
                "target": schnellui::a11y::to_access_id(target).0,
                "tree": self.debug_tree(),
            }),
        )
    }

    pub(super) fn handle_debug_key(&mut self, request: DebugKey) -> DebugReply {
        let name = request.key.to_ascii_lowercase().replace('-', "_");
        self.trace_key_input(
            "debug",
            || name.clone(),
            KeyTrace {
                state: "pressed",
                repeat: false,
                shift: request.shift,
                ctrl: request.ctrl,
                alt: false,
                super_key: false,
                text: request.text.as_deref(),
            },
        );
        // As the windowed host does: a key takes hover from the resting
        // pointer until the pointer moves again.
        let _ = self.app.hold_pointer_hover();
        // As the windowed host does: a raw surface with its own key handler
        // (a terminal grid) gets the key first.
        if let Some(raw) = debug_raw_key(&name, &request) {
            let result = self.app.dispatch_focused_input(FocusedInputEvent::Key(raw));
            if self.apply_focused_input_result(result) {
                self.settle_debug_command();
                return DebugReply::json(
                    200,
                    json!({ "changed": true, "status": self.debug_status() }),
                );
            }
        }
        let changed = match name.as_str() {
            "tab" => self.dispatch_ui_key(UiKey::Tab {
                shift: request.shift,
            }),
            "enter" => self.dispatch_ui_key(UiKey::Enter),
            "space" => self.dispatch_ui_key(UiKey::Space {
                shift: request.shift,
            }),
            "backspace" => self.dispatch_ui_key(UiKey::Backspace),
            "delete" => self.dispatch_ui_key(UiKey::Delete),
            "left" => self.dispatch_ui_key(UiKey::Left {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "right" => self.dispatch_ui_key(UiKey::Right {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "up" => self.dispatch_ui_key(UiKey::Up {
                shift: request.shift,
            }),
            "down" => self.dispatch_ui_key(UiKey::Down {
                shift: request.shift,
            }),
            "home" => self.dispatch_ui_key(UiKey::Home {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "end" => self.dispatch_ui_key(UiKey::End {
                shift: request.shift,
                ctrl: request.ctrl,
            }),
            "page_up" => self.dispatch_ui_key(UiKey::PageUp),
            "page_down" => self.dispatch_ui_key(UiKey::PageDown),
            "escape" => self.dispatch_ui_key(UiKey::Escape),
            "browser_back" => self.app.dispatch_navigation(crate::Navigation::Back),
            "browser_forward" => self.app.dispatch_navigation(crate::Navigation::Forward),
            "select_all" => self.dispatch_ui_key(UiKey::SelectAll),
            "undo" => self.dispatch_ui_key(UiKey::Undo),
            "redo" => self.dispatch_ui_key(UiKey::Redo),
            "text" => {
                let Some(text) = request.text.as_deref() else {
                    return DebugReply::error(400, "text key requires text");
                };
                // A real keyboard delivers one character per key event, so
                // each character reaches application shortcuts first and
                // then the focused control. An IME commit stays whole.
                let mut changed = false;
                let mut buf = [0u8; 4];
                for c in text.chars() {
                    if request.ctrl {
                        // A chord: the native key path's table (clipboard,
                        // select all, undo, application shortcuts), never
                        // the letter itself.
                        let logical = Key::Character(c.encode_utf8(&mut buf).into());
                        changed |= self.handle_key_chord(KeyChord {
                            logical_key: &logical,
                            physical_key: PhysicalKey::Unidentified(
                                winit::keyboard::NativeKeyCode::Unidentified,
                            ),
                            text: None,
                            modifier_text: None,
                            repeat: false,
                            shift: request.shift,
                            ctrl: true,
                            alt: false,
                            super_key: cfg!(target_os = "macos"),
                        });
                        continue;
                    }
                    changed |= self.app.dispatch_shortcut(crate::Shortcut::new(
                        c,
                        request.ctrl,
                        request.shift,
                        false,
                    )) || self.dispatch_ui_key(UiKey::Char(c.encode_utf8(&mut buf)));
                }
                changed
            }
            // An input method's committed text, as the window's IME event
            // delivers it.
            "ime_commit" => {
                let Some(text) = request.text.as_deref() else {
                    return DebugReply::error(400, "ime_commit requires text");
                };
                let result = self
                    .app
                    .dispatch_ime(crate::RawImeEvent::Commit(text.to_owned()));
                self.apply_focused_input_result(result)
            }
            _ => return DebugReply::error(422, format!("unsupported key {:?}", request.key)),
        };
        self.settle_debug_command();
        DebugReply::json(
            200,
            json!({ "changed": changed, "status": self.debug_status() }),
        )
    }

    /// Runs what the system menu reported: an action's handler, or the host's
    /// own clipboard, undo or select-all path for an edit item (a schnellui
    /// window has no `NSTextView`, so the native selectors would find nothing).
    /// Returns whether anything ran, so the caller knows to redraw.
    pub(super) fn apply_global_menu_event(
        &mut self,
        event: crate::platform::GlobalMenuEvent,
    ) -> bool {
        use crate::platform::{GlobalMenuEvent, StandardItem};
        match event {
            GlobalMenuEvent::Action(id) => self.app.dispatch_global_menu_item(&id),
            GlobalMenuEvent::Standard(item) => match item {
                StandardItem::Cut => self.copy_or_cut(true),
                StandardItem::Copy => self.copy_or_cut(false),
                StandardItem::Paste => self.paste(),
                StandardItem::SelectAll => self.dispatch_ui_key(UiKey::SelectAll),
                StandardItem::Undo => self.dispatch_ui_key(UiKey::Undo),
                StandardItem::Redo => self.dispatch_ui_key(UiKey::Redo),
                // The system carries these out itself and never reports them.
                _ => false,
            },
        }
    }

    /// The menu tree the app set, and whether the system shows it.
    fn debug_global_menu(&self) -> Value {
        fn menu_json(menu: &crate::platform::Menu) -> Value {
            use crate::platform::MenuItem;
            let items: Vec<Value> = menu
                .items()
                .iter()
                .map(|item| match item {
                    MenuItem::Separator => json!({ "type": "separator" }),
                    MenuItem::Standard(standard) => {
                        json!({ "type": "standard", "item": format!("{standard:?}") })
                    }
                    MenuItem::Submenu(sub) => {
                        let mut value = menu_json(sub);
                        value["type"] = json!("submenu");
                        value
                    }
                    MenuItem::Action(action) => json!({
                        "type": "action",
                        "id": action.id(),
                        "label": action.label(),
                        "enabled": action.is_enabled(),
                        "checked": action.checked_state(),
                        "key_equivalent": action.key_equivalent().map(|k| json!({
                            "key": format!("{:?}", k.logical_key()),
                            "command": k.uses_command(),
                            "shift": k.uses_shift(),
                            "alt": k.uses_alt(),
                        })),
                    }),
                })
                .collect();
            json!({ "title": menu.title(), "items": items })
        }
        let menus: Vec<Value> = self
            .app
            .global_menu()
            .menus()
            .iter()
            .map(menu_json)
            .collect();
        json!({
            "supported": crate::platform::GlobalMenu::supported(),
            "menus": menus,
        })
    }

    fn handle_debug_global_menu_item(&mut self, request: &DebugGlobalMenuItem) -> DebugReply {
        if self.app.global_menu().find_action(&request.id).is_none() {
            return DebugReply::error(404, format!("no global menu item {:?}", request.id));
        }
        if !self.app.dispatch_global_menu_item(&request.id) {
            return DebugReply::error(
                409,
                format!(
                    "global menu item {:?} is disabled or has no handler",
                    request.id
                ),
            );
        }
        self.poll_remount();
        self.settle_debug_command();
        DebugReply::json(200, json!({ "fired": true, "status": self.debug_status() }))
    }

    /// What the app registered, what the system bound, and why not if it failed.
    fn debug_global_shortcuts(&self) -> Value {
        let registered: Vec<Value> = self
            .app
            .registered_global_shortcuts()
            .map(|shortcut| {
                json!({
                    "id": shortcut.id(),
                    "description": shortcut.description(),
                    "preferred_trigger": shortcut.trigger().map(|t| json!({
                        "key": format!("{:?}", t.logical_key()),
                        "command": t.uses_command(),
                        "shift": t.uses_shift(),
                        "alt": t.uses_alt(),
                    })),
                })
            })
            .collect();
        let bindings: Vec<Value> = self
            .app
            .global_shortcuts()
            .iter()
            .map(|binding| {
                json!({
                    "id": binding.id,
                    "description": binding.description,
                    "trigger": binding.trigger,
                })
            })
            .collect();
        json!({
            "supported": crate::platform::GlobalShortcuts::supported(),
            "registered": registered,
            "bindings": bindings,
            "error": self.app.global_shortcut_error().map(|error| error.to_string()),
        })
    }

    fn handle_debug_global_shortcut(&mut self, request: &DebugGlobalShortcut) -> DebugReply {
        use crate::platform::GlobalShortcutPhase::{Pressed, Released};
        let phases: &[_] = match request.phase.as_deref() {
            None | Some("tap") => &[Pressed, Released],
            Some("press") => &[Pressed],
            Some("release") => &[Released],
            Some(other) => {
                return DebugReply::error(
                    422,
                    format!("phase must be press, release or tap, not {other:?}"),
                )
            }
        };
        let mut fired = false;
        for phase in phases {
            fired |= self.app.dispatch_global_shortcut(&request.id, *phase);
        }
        if !fired {
            return DebugReply::error(404, format!("no global shortcut {:?}", request.id));
        }
        self.poll_remount();
        self.settle_debug_command();
        DebugReply::json(200, json!({ "fired": true, "status": self.debug_status() }))
    }

    pub(super) fn handle_debug_command(
        &mut self,
        event_loop: Option<&ActiveEventLoop>,
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
                match self.app.render_png_bytes() {
                    Ok(bytes) => DebugReply::png(bytes),
                    Err(error) => DebugReply::error(503, error.to_string()),
                }
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
            DebugCommand::PointerPress(request) => {
                self.handle_debug_press(&request, winit::event::ElementState::Pressed)
            }
            DebugCommand::PointerRelease(request) => {
                self.handle_debug_press(&request, winit::event::ElementState::Released)
            }
            DebugCommand::PointerDrag(request) => self.handle_debug_drag(&request),
            DebugCommand::PointerLeave => self.handle_debug_leave(),
            DebugCommand::PointerClick(point) => {
                if let Err(error) = self.move_debug_pointer(&point) {
                    return DebugReply::error(422, error);
                }
                // The widget under the point, or null for the bare page. The
                // click goes ahead either way: a click on the page is what
                // blurs a field and closes a dropdown or menu.
                let target = schnellui::widgets::hit_test(
                    &self.app.widgets.clone(),
                    self.app.scene(),
                    Point {
                        x: point.x,
                        y: point.y,
                    },
                )
                .map_or(Value::Null, |id| widget_json(&self.app, id));
                // A real click: the press and the release go through the
                // window's own handlers, so a text field places its caret,
                // a button clicks once and focus moves as for a mouse.
                self.debug_mouse_button(winit::event::ElementState::Pressed, DebugButton::Left);
                self.debug_mouse_button(winit::event::ElementState::Released, DebugButton::Left);
                self.settle_debug_command();
                DebugReply::json(
                    200,
                    json!({ "changed": true, "target": target, "status": self.debug_status() }),
                )
            }
            DebugCommand::Key(request) => self.handle_debug_key(request),
            DebugCommand::GlobalShortcuts => DebugReply::json(200, self.debug_global_shortcuts()),
            DebugCommand::GlobalShortcutTrigger(request) => {
                self.handle_debug_global_shortcut(&request)
            }
            DebugCommand::GlobalMenu => DebugReply::json(200, self.debug_global_menu()),
            DebugCommand::GlobalMenuTrigger(request) => {
                self.handle_debug_global_menu_item(&request)
            }
            DebugCommand::Quit => {
                if let Some(event_loop) = event_loop {
                    event_loop.exit();
                }
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
        self.handle_key_chord(KeyChord {
            logical_key: &event.logical_key,
            physical_key: event.physical_key,
            text: event.text.as_deref(),
            modifier_text,
            repeat: event.repeat,
            shift,
            ctrl,
            alt: self.modifiers.alt_key(),
            super_key: self.modifiers.super_key(),
        })
    }

    /// The key path shared by the window's key events and the debug server's
    /// modified text keys, so both run one chord table.
    pub(super) fn handle_key_chord(&mut self, event: KeyChord<'_>) -> bool {
        let KeyChord {
            shift,
            ctrl,
            alt,
            super_key,
            modifier_text,
            ..
        } = event;
        let command = if cfg!(target_os = "macos") {
            super_key
        } else {
            ctrl
        };
        let logical_text = match event.logical_key {
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
            match event.logical_key {
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
            if let Key::Named(named) = &event.logical_key {
                if let Some(key) = crate::ShortcutKey::from_window_key(named) {
                    if self
                        .app
                        .dispatch_shortcut(Shortcut::key(key, command, shift, alt))
                    {
                        return true;
                    }
                }
            }
            if let Key::Character(value) = event.logical_key {
                let mut chars = value.chars();
                if let Some(character) = chars.next() {
                    if chars.next().is_none()
                        && self
                            .app
                            .dispatch_shortcut(Shortcut::new(character, command, shift, alt))
                    {
                        return true;
                    }
                }
            }
        }
        let physical_key = special_key_from_physical(event.physical_key, shift, ctrl);
        let key = match event.logical_key {
            Key::Named(NamedKey::Tab) => UiKey::Tab { shift },
            Key::Named(NamedKey::Enter) => UiKey::Enter,
            Key::Named(NamedKey::Space) => UiKey::Space { shift },
            Key::Named(NamedKey::Backspace) => UiKey::Backspace,
            Key::Named(NamedKey::Delete) => UiKey::Delete,
            Key::Named(NamedKey::ArrowLeft) => UiKey::Left { shift, ctrl },
            Key::Named(NamedKey::ArrowRight) => UiKey::Right { shift, ctrl },
            Key::Named(NamedKey::ArrowUp) => UiKey::Up { shift },
            Key::Named(NamedKey::ArrowDown) => UiKey::Down { shift },
            Key::Named(NamedKey::Home) => UiKey::Home { shift, ctrl },
            Key::Named(NamedKey::End) => UiKey::End { shift, ctrl },
            Key::Named(NamedKey::PageUp) => UiKey::PageUp,
            Key::Named(NamedKey::PageDown) => UiKey::PageDown,
            Key::Named(NamedKey::Escape) => UiKey::Escape,
            Key::Character(c) if command && c.eq_ignore_ascii_case("a") => UiKey::SelectAll,
            Key::Character(c) if command && c.eq_ignore_ascii_case("z") => {
                if shift {
                    UiKey::Redo
                } else {
                    UiKey::Undo
                }
            }
            Key::Character(c) if command && c.eq_ignore_ascii_case("y") => UiKey::Redo,
            _ => {
                if let Some(key) = physical_key {
                    return self.dispatch_ui_key(key);
                }
                // Character insertion rides `KeyEvent::text` (the layout- and
                // shift-resolved text). Command chords and control characters
                // (Enter, Esc, …) never insert.
                if ctrl || alt || super_key {
                    return false;
                }
                return match event.text {
                    Some(t) if !t.is_empty() && !t.chars().any(|ch| ch.is_control()) => {
                        self.dispatch_ui_key(UiKey::Char(t))
                    }
                    _ => false,
                };
            }
        };
        self.dispatch_ui_key(key)
    }
}

/// A key press reduced to what the chord table reads, so a window's `KeyEvent`
/// and a debug-server key (which cannot build one) take the same path.
pub(super) struct KeyChord<'a> {
    pub(super) logical_key: &'a Key,
    pub(super) physical_key: PhysicalKey,
    pub(super) text: Option<&'a str>,
    pub(super) modifier_text: Option<&'a str>,
    pub(super) repeat: bool,
    pub(super) shift: bool,
    pub(super) ctrl: bool,
    pub(super) alt: bool,
    pub(super) super_key: bool,
}

/// Builds the event loop and runs `app` in a window titled `title` (SOUL §8).
/// `remount` is the optional in-window remount hook ([`App::run_windowed_with`]).
pub fn run(
    app: App,
    title: &str,
    remount: Option<Box<dyn FnMut(schnellui::scene::Size) -> Option<WindowUpdate>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("SCHNELLUI_HEADLESS_WINDOWED").as_deref() == Ok("1") {
        return run_headless(app, title, remount);
    }
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

    let app_atlas_id = app.atlas.id();
    let mut handler = WindowedApp {
        app,
        clipboard: HostClipboard::Native(SystemClipboard::new()),
        headless: false,
        title: title.to_string(),
        window: None,
        renderer: None,
        accessibility: None,
        event_loop_proxy: Some(event_loop_proxy),
        accessibility_full_update_pending: true,
        accessibility_published: AccessibilityPublished::default(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_inside: false,
        cursor_icon: UiCursorIcon::Default,
        cursor_synced_key: None,
        ime_area: None,
        glyph_atlas_id: app_atlas_id,
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
        backdrop_press: None,
        left_pointer_down: false,
        remount,
    };
    event_loop.run_app(&mut handler)?;
    Ok(())
}

/// Drives the windowed host on the current thread using an offscreen scene and
/// the same debug commands, remount hook, and controller wake as the native host.
pub(crate) fn run_headless(
    app: App,
    title: &str,
    remount: Option<Box<dyn FnMut(Size) -> Option<WindowUpdate>>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !crate::debug_server::enabled() {
        return Err("headless windowed mode requires debug instrumentation (enable the debug-instrumentation feature in release builds and do not disable SCHNELLUI_DEBUG_SERVER)".into());
    }

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
                "mode": "headless_windowed",
            }),
        );
    }

    let (sender, receiver) = mpsc::channel();
    app.redraw_signal.install_headless(sender.clone());
    let debug_sender = sender.clone();
    let debug_server = crate::debug_server::start(title, move |request| {
        debug_sender.send(PlatformEvent::Debug(request)).is_ok()
    })
    .map_err(|error| format!("headless windowed debug instrumentation unavailable: {error}"))?;
    eprintln!(
        "schnellui debug instrumentation listening on {}",
        debug_server.endpoint()
    );

    let app_atlas_id = app.atlas.id();
    let mut handler = WindowedApp {
        app,
        clipboard: HostClipboard::Memory(None),
        headless: true,
        title: title.to_string(),
        window: None,
        renderer: None,
        accessibility: None,
        event_loop_proxy: None,
        accessibility_full_update_pending: true,
        accessibility_published: AccessibilityPublished::default(),
        cursor: PhysicalPosition::new(0.0, 0.0),
        cursor_inside: false,
        cursor_icon: UiCursorIcon::Default,
        cursor_synced_key: None,
        ime_area: None,
        glyph_atlas_id: app_atlas_id,
        interaction_trace,
        _debug_server: Some(debug_server),
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
        backdrop_press: None,
        left_pointer_down: false,
        remount,
    };
    handler.on_redraw_requested();
    handler.sync_cursor();

    loop {
        if handler.autoclose_elapsed() {
            break;
        }
        let now = Instant::now();
        if schnellui::widgets::fire_due_scroll_callbacks(&handler.app.widgets, now) {
            handler.remount_trigger = "scroll_callback";
            handler.on_redraw_requested();
            handler.sync_cursor();
        }
        let now = Instant::now();
        let animation_active = handler.app.wants_animation_frame();
        let (animation_due, _) =
            pace_animation_redraw(animation_active, now, &mut handler.animation_deadline);
        if animation_due {
            handler.remount_trigger = "animation";
            handler.on_redraw_requested();
            handler.sync_cursor();
        }
        // A frame above may have taken longer than an interval. Schedule from
        // its completion, and read the deadline after the shared redraw path
        // has rebased it.
        let now = Instant::now();
        let interval_wake = if let Some(interval) = handler.app.redraw_interval {
            let deadline = *handler
                .interval_deadline
                .get_or_insert_with(|| now + interval);
            if now >= deadline {
                handler.remount_trigger = "redraw_interval";
                handler.on_redraw_requested();
                handler.sync_cursor();
            }
            handler.interval_deadline
        } else {
            handler.interval_deadline = None;
            None
        };
        let scroll_wake = schnellui::widgets::next_scroll_callback_deadline(&handler.app.widgets);
        let next_wake = [
            handler.deadline,
            interval_wake,
            handler.animation_deadline,
            scroll_wake,
        ]
        .into_iter()
        .flatten()
        .min();
        let event = match next_wake {
            Some(deadline) => receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .ok(),
            None => receiver.recv().ok(),
        };
        match event {
            Some(PlatformEvent::RedrawRequested) => {
                let signal = handler.app.redraw_signal();
                signal.drain_controller_wake(|| {
                    handler.remount_trigger = "controller_wake";
                    handler.poll_remount();
                });
                handler.on_redraw_requested();
                handler.sync_cursor();
            }
            Some(PlatformEvent::Debug(request)) => {
                let quit = matches!(&request.command, DebugCommand::Quit);
                handler.remount_trigger = "debug_instrumentation";
                let reply = handler.handle_debug_command(None, request.command);
                let _ = request.reply.send(reply);
                if quit {
                    break;
                }
            }
            Some(PlatformEvent::ReducedMotionChanged(reduce)) => {
                handler.remount_trigger = "reduced_motion";
                if handler.app.apply_reduced_motion(reduce) {
                    handler.on_redraw_requested();
                    handler.sync_cursor();
                }
            }
            Some(PlatformEvent::GlobalShortcut(event)) => {
                handler.remount_trigger = "global_shortcut";
                if handler.app.apply_global_shortcut_event(event) {
                    handler.on_redraw_requested();
                    handler.sync_cursor();
                }
            }
            Some(PlatformEvent::GlobalMenu(event)) => {
                handler.remount_trigger = "global_menu";
                if handler.apply_global_menu_event(event) {
                    handler.on_redraw_requested();
                    handler.sync_cursor();
                }
            }
            Some(PlatformEvent::AccessKit(_)) => {}
            None => {}
        }
    }
    Ok(())
}

/// The raw key event a named debug key stands for, as a keyboard would send
/// it; `None` for names without one.
fn debug_raw_key(name: &str, request: &DebugKey) -> Option<RawKeyEvent> {
    use crate::raw_keyboard::{Key, KeyCode, KeyLocation, NamedKey, PhysicalKey};
    let (named, code) = match name {
        "tab" => (NamedKey::Tab, KeyCode::Tab),
        "enter" => (NamedKey::Enter, KeyCode::Enter),
        "space" => (NamedKey::Space, KeyCode::Space),
        "backspace" => (NamedKey::Backspace, KeyCode::Backspace),
        "delete" => (NamedKey::Delete, KeyCode::Delete),
        "left" => (NamedKey::ArrowLeft, KeyCode::ArrowLeft),
        "right" => (NamedKey::ArrowRight, KeyCode::ArrowRight),
        "up" => (NamedKey::ArrowUp, KeyCode::ArrowUp),
        "down" => (NamedKey::ArrowDown, KeyCode::ArrowDown),
        "home" => (NamedKey::Home, KeyCode::Home),
        "end" => (NamedKey::End, KeyCode::End),
        "page_up" => (NamedKey::PageUp, KeyCode::PageUp),
        "page_down" => (NamedKey::PageDown, KeyCode::PageDown),
        "escape" => (NamedKey::Escape, KeyCode::Escape),
        _ => return None,
    };
    let text = match named {
        NamedKey::Enter => Some("\r".to_owned()),
        NamedKey::Tab => Some("\t".to_owned()),
        NamedKey::Space => Some(" ".to_owned()),
        _ => None,
    };
    Some(RawKeyEvent {
        logical_key: Key::Named(named),
        physical_key: PhysicalKey::Code(code),
        key_without_modifiers: Key::Named(named),
        location: KeyLocation::Standard,
        modifiers: RawModifiers {
            shift: request.shift,
            control: request.ctrl,
            alt: false,
            super_key: false,
        },
        state: RawInputState::Pressed,
        repeat: false,
        text: text.clone(),
        text_with_all_modifiers: text,
    })
}

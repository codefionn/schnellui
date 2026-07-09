use super::*;

impl ApplicationHandler<PlatformEvent> for WindowedApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` can fire more than once; only create the window once.
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.title.clone())
            // AccessKit must attach before the native window is shown. Creating
            // it visible first silently prevents reliable platform registration.
            .with_visible(false)
            .with_inner_size(self.init_phys);
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("windowed: could not create window: {e}");
                event_loop.exit();
                return;
            }
        };
        if let Some(native) = window.theme() {
            let scheme = match native {
                WinitTheme::Light => crate::ColorScheme::Light,
                WinitTheme::Dark => crate::ColorScheme::Dark,
            };
            let _ = self.app.apply_color_scheme(scheme);
        }
        let size = window.inner_size();
        let (pw, ph) = (size.width.max(1), size.height.max(1));
        match SurfaceRenderer::new(window.clone(), pw, ph, Backend::Auto) {
            Ok(mut r) => {
                r.set_scale(self.app.scale());
                r.set_clear_color(self.app.clear_color());
                self.renderer = Some(r);
            }
            Err(e) => {
                eprintln!("windowed: renderer init failed: {e}");
                event_loop.exit();
                return;
            }
        }
        // Relayout at the surface's logical size (physical ÷ scale).
        let scale = self.app.scale();
        self.app.resize(pw as f32 / scale, ph as f32 / scale);
        // Establish valid semantic bounds before AT-SPI can request the initial
        // tree. Later redraws preserve dirty state until the adapter consumes it.
        self.app.frame();
        let accessibility = AccessKitAdapter::with_event_loop_proxy(
            event_loop,
            window.as_ref(),
            self.event_loop_proxy.clone(),
        );
        window.set_visible(true);
        self.accessibility = Some(accessibility);
        self.window = Some(window);
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        self.remount_trigger = window_event_name(&event);
        let mut structural_update_polled = false;
        let mut structural_update_deferred = false;
        // AccessKit needs focus, move, and resize notifications before the app
        // handles them. This ordering is part of the adapter contract.
        if let (Some(accessibility), Some(window)) =
            (self.accessibility.as_mut(), self.window.as_ref())
        {
            accessibility.process_event(window.as_ref(), &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = m.state();
            }
            WindowEvent::Focused(false) => {
                let before = self.interaction_snapshot(self.cursor_logical());
                let widget_state = schnellui_widgets::interaction_debug_state(&self.app.widgets);
                if !self.raw_pointer_capture.is_empty()
                    || self.drag_text.is_some()
                    || self.drag_slider.is_some()
                    || widget_state.content_drag_source.is_some()
                    || widget_state.dialog_pointer_capture
                {
                    self.trace(
                        "interaction_interrupted_by_window_blur",
                        json!({ "severity": "warning", "before": before.clone() }),
                    );
                }
                let result = self
                    .app
                    .dispatch_focused_input(FocusedInputEvent::Focus(RawFocusEvent::WindowLost));
                if self.apply_focused_input_result(result) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                self.modifiers = ModifiersState::default();
                self.control_pressed = false;
                self.raw_pointer_capture.clear();
                self.left_pointer_down = false;
                let _ = self.app.end_scrollbar_pointer();
                let _ = self.app.update_edge_auto_scroll(
                    Point {
                        x: f32::NEG_INFINITY,
                        y: f32::NEG_INFINITY,
                    },
                    false,
                );
                self.trace(
                    "window_focus",
                    json!({ "focused": false, "before": before }),
                );
            }
            WindowEvent::Focused(true) => {
                let result = self
                    .app
                    .dispatch_focused_input(FocusedInputEvent::Focus(RawFocusEvent::WindowGained));
                if self.apply_focused_input_result(result) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                let snapshot = self.interaction_snapshot(self.cursor_logical());
                self.trace(
                    "window_focus",
                    json!({ "focused": true, "interaction": snapshot }),
                );
            }
            WindowEvent::ThemeChanged(native) => {
                let scheme = match native {
                    WinitTheme::Light => crate::ColorScheme::Light,
                    WinitTheme::Dark => crate::ColorScheme::Dark,
                };
                if self.app.apply_color_scheme(scheme) {
                    self.sync_renderer_after_theme_rebuild();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if is_control_key(&event.logical_key, event.physical_key) {
                    self.control_pressed = event.state == ElementState::Pressed;
                }
                // Full-fidelity raw surfaces see modifiers, releases, repeats,
                // F/keypad identities and layout text before any browser-like
                // SchnellUI shortcut or editing behavior can consume them.
                if self.dispatch_raw_clipboard_chord(&event) || self.dispatch_raw_key(&event) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if event.state == ElementState::Pressed {
                    // Escape dismisses the top-most dialog first. A
                    // persistent dialog still consumes it; only an
                    // overlay-less application exits the window.
                    if matches!(event.logical_key, Key::Named(NamedKey::Escape)) {
                        if self.app.dismiss_context_menu() {
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        match schnellui_widgets::dispatch_dialog_escape(
                            &self.app.widgets.clone(),
                            self.app.scene_mut(),
                        ) {
                            Some(changed) => {
                                if changed {
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                }
                                // Dismiss handlers commonly request a
                                // structural remount; perform it before the
                                // early return from this Escape event.
                                self.poll_remount();
                                self.sync_cursor();
                            }
                            None => event_loop.exit(),
                        }
                        return;
                    }
                    let context_key =
                        matches!(&event.logical_key, Key::Named(NamedKey::ContextMenu))
                            || (self.modifiers.shift_key()
                                && matches!(&event.logical_key, Key::Named(NamedKey::F10)));
                    if context_key {
                        let changed =
                            self.app.dismiss_context_menu() | self.open_focused_context_menu();
                        if changed {
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                        return;
                    }
                    if self.app.dismiss_context_menu() {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                    // Tab focus traversal + text editing on the focused input
                    // (SOUL §6.3 keyboard path — the same dispatches an inbound
                    // AccessKit Focus/SetValue action reaches).
                    if self.handle_key(&event) {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                let ime = match ime {
                    winit::event::Ime::Enabled => RawImeEvent::Enabled,
                    winit::event::Ime::Disabled => RawImeEvent::Disabled,
                    winit::event::Ime::Preedit(text, cursor) => {
                        RawImeEvent::Preedit { text, cursor }
                    }
                    winit::event::Ime::Commit(text) => RawImeEvent::Commit(text),
                };
                let result = self.app.dispatch_focused_input(FocusedInputEvent::Ime(ime));
                if self.apply_focused_input_result(result) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let p = self.cursor_logical();
                if self
                    .interaction_trace
                    .as_ref()
                    .is_some_and(InteractionRecorder::includes_pointer_moves)
                {
                    let snapshot = self.interaction_snapshot(p);
                    self.trace("pointer_move", snapshot);
                }
                if self.app.update_pointer_proximity(p) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                let captured = !self.raw_pointer_capture.is_empty();
                let modifiers = raw_modifiers(self.modifiers, self.control_pressed);
                let result = if captured {
                    self.app
                        .dispatch_focused_pointer(p, modifiers, RawPointerAction::Move, true)
                } else {
                    self.app.dispatch_hover_pointer(p, modifiers)
                };
                if self.apply_focused_input_result(result) || captured {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.app.scrollbar_pointer_active() {
                    let _ = self.app.update_scrollbar_pointer(p);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.app.update_edge_auto_scroll(p, self.left_pointer_down) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                if self.app.update_dialog_pointer(p) {
                    self.sync_cursor();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.app.update_drag(p) {
                    self.sync_cursor();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
                // A live left-button drag on a text input extends its selection
                // (SOUL §6.3 pointer-selection path).
                if let Some(id) = self.drag_text {
                    if self
                        .app
                        .dispatch_text_pointer_action(id, p, TextPointerAction::Drag)
                    {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
                if let Some(id) = self.drag_slider {
                    if self.app.dispatch_slider_pointer(id, p) {
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                let snapshot = self.interaction_snapshot(self.cursor_logical());
                self.trace("pointer_left", snapshot);
                let _ = self.app.update_edge_auto_scroll(
                    Point {
                        x: f32::NEG_INFINITY,
                        y: f32::NEG_INFINITY,
                    },
                    false,
                );
                if self.app.update_pointer_proximity(Point {
                    x: f32::NEG_INFINITY,
                    y: f32::NEG_INFINITY,
                }) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let p = self.cursor_logical();
                let scrollbar_released = if button == MouseButton::Left {
                    self.left_pointer_down = state == ElementState::Pressed;
                    if state == ElementState::Released {
                        let _ = self.app.update_edge_auto_scroll(p, false);
                        self.app.end_scrollbar_pointer()
                    } else {
                        false
                    }
                } else {
                    false
                };
                let raw_button = raw_pointer_button(button);
                let captured = self.raw_pointer_capture.contains(&raw_button);
                let snapshot = self.interaction_snapshot(p);
                self.trace(
                    "pointer_button",
                    json!({
                        "state": debug_label(state),
                        "button": debug_label(button),
                        "was_raw_captured": captured,
                        "interaction": snapshot,
                    }),
                );
                if state == ElementState::Pressed {
                    let hit =
                        schnellui_widgets::hit_test(&self.app.widgets.clone(), self.app.scene(), p);
                    if let Some(target) = hit.and_then(|id| self.app.focused_input_target_from(id))
                    {
                        let focus_changed = self.app.pointer_focus(Some(target));
                        let result = self.app.dispatch_focused_pointer(
                            p,
                            raw_modifiers(self.modifiers, self.control_pressed),
                            RawPointerAction::Button {
                                button: raw_button,
                                state: RawInputState::Pressed,
                            },
                            false,
                        );
                        if self.apply_focused_input_result(result) {
                            if !self.raw_pointer_capture.contains(&raw_button) {
                                self.raw_pointer_capture.push(raw_button);
                            }
                            let interaction = self.interaction_snapshot(p);
                            self.trace(
                                "pointer_button_result",
                                json!({
                                    "route": "focused_raw_surface",
                                    "state": "pressed",
                                    "handled": true,
                                    "interaction": interaction,
                                }),
                            );
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        if focus_changed {
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                } else {
                    let result = self.app.dispatch_focused_pointer(
                        p,
                        raw_modifiers(self.modifiers, self.control_pressed),
                        RawPointerAction::Button {
                            button: raw_button,
                            state: RawInputState::Released,
                        },
                        captured,
                    );
                    self.raw_pointer_capture
                        .retain(|pressed| *pressed != raw_button);
                    if self.apply_focused_input_result(result) || captured {
                        let interaction = self.interaction_snapshot(p);
                        self.trace(
                            "pointer_button_result",
                            json!({
                                "route": "focused_raw_surface",
                                "state": "released",
                                "handled": true,
                                "interaction": interaction,
                            }),
                        );
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                        return;
                    }
                }
                if scrollbar_released {
                    let _ = self.app.set_active_interaction(None);
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if button == MouseButton::Right && state == ElementState::Pressed {
                    let mut redraw = self.app.dismiss_context_menu();
                    let hit =
                        schnellui_widgets::hit_test(&self.app.widgets.clone(), self.app.scene(), p);
                    let source = hit.and_then(|id| {
                        schnellui_widgets::context_menu_source(
                            &self.app.widgets.clone(),
                            self.app.scene(),
                            id,
                        )
                    });
                    if let Some(id) = source {
                        redraw |= self.app.pointer_focus(Some(id));
                        let can_paste = self.clipboard.read_text().is_ok();
                        redraw |= self.app.open_context_menu(id, p, can_paste);
                    }
                    self.drag_text = None;
                    self.drag_slider = None;
                    self.last_text_click = None;
                    if redraw {
                        self.app.update_pointer_proximity(p);
                        self.sync_cursor();
                        if let Some(w) = &self.window {
                            w.request_redraw();
                        }
                    }
                    return;
                }
                if button == MouseButton::Left {
                    if state == ElementState::Pressed {
                        if self.app.begin_scrollbar_pointer(p) {
                            let _ = self.app.update_edge_auto_scroll(p, false);
                            let _ = self.app.set_active_interaction(None);
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        let _ = self.app.update_edge_auto_scroll(p, true);
                        // physical → logical: hit-testing + layout are in logical px.
                        let hit = schnellui_widgets::hit_test(
                            &self.app.widgets.clone(),
                            self.app.scene(),
                            p,
                        );
                        if self.app.context_menu_is_open() {
                            let changed = match hit {
                                Some(id)
                                    if schnellui_widgets::context_menu_item(
                                        &self.app.widgets,
                                        self.app.scene(),
                                        id,
                                    ) =>
                                {
                                    self.activate_context_menu_item(id)
                                }
                                _ => self.app.dismiss_context_menu(),
                            };
                            if changed {
                                self.app.update_pointer_proximity(p);
                                self.sync_cursor();
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                            }
                            return;
                        }
                        if self.app.begin_dialog_pointer(p) {
                            let dismissed = schnellui_widgets::dismiss_open_dropdowns(
                                &self.app.widgets.clone(),
                                self.app.scene_mut(),
                                hit,
                            );
                            self.drag_text = None;
                            self.drag_slider = None;
                            self.last_text_click = None;
                            if dismissed {
                                self.poll_remount();
                            }
                            self.sync_cursor();
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                            return;
                        }
                        // The pointer path converges with the AccessKit action path
                        // on one handler (SOUL §6.3): hit_test → focus + click. A
                        // press on a text input focuses it and places the caret
                        // (Shift extends the selection); a press anywhere else
                        // blurs and dispatches the click.
                        let mut redraw = self.app.set_active_interaction(hit);
                        let hit_input = hit.filter(|&id| {
                            matches!(self.app.scene().node(id),
                                Some(n) if n.kind == WidgetKind::TextInput
                                    || n.kind == WidgetKind::TextArea)
                        });
                        let hit_slider = hit.filter(|&id| {
                            self.app.scene().node(id).map(|n| n.kind) == Some(WidgetKind::Slider)
                        });
                        if let Some(id) = hit_input {
                            redraw |= self.app.pointer_focus(Some(id));
                            let action = self.text_press_action(id, p);
                            redraw |= self.app.dispatch_text_pointer_action(id, p, action);
                            let collapsed_combo = self.app.scene().a11y(id).is_some_and(|a| {
                                schnellui_a11y::Role::from_u16(a.role)
                                    == schnellui_a11y::Role::ComboBox
                                    && !schnellui_a11y::StateFlags(a.state)
                                        .contains(schnellui_a11y::StateFlags::EXPANDED)
                            });
                            if collapsed_combo {
                                redraw |= schnellui_widgets::dispatch_click(
                                    &self.app.widgets.clone(),
                                    self.app.scene_mut(),
                                    id,
                                );
                            }
                            self.drag_text = (action != TextPointerAction::AddCaret).then_some(id);
                            self.drag_slider = None;
                        } else if let Some(id) = hit_slider {
                            self.last_text_click = None;
                            redraw |= self.app.pointer_focus(Some(id));
                            redraw |= self.app.dispatch_slider_pointer(id, p);
                            self.drag_slider = Some(id);
                            self.drag_text = None;
                        } else {
                            self.last_text_click = None;
                            // Click-to-focus, the browser way (SOUL §6.3): a
                            // press on a focusable widget focuses it; a press
                            // on anything else (or nothing) blurs — `focus`
                            // filters non-focusable targets down to a blur.
                            redraw |= self.app.pointer_focus(hit);
                            if !self.app.begin_drag(p) {
                                if let Some(id) = hit {
                                    redraw |= schnellui_widgets::dispatch_click(
                                        &self.app.widgets.clone(),
                                        self.app.scene_mut(),
                                        id,
                                    );
                                }
                            }
                        }
                        if redraw {
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    } else {
                        let active_changed = self.app.set_active_interaction(None);
                        self.app.end_dialog_pointer();
                        self.drag_text = None;
                        self.drag_slider = None;
                        let release = self.app.end_drag(p);
                        self.trace(
                            "content_drag_release",
                            json!({ "result": debug_label(release) }),
                        );
                        let proximity_changed = self.app.update_pointer_proximity(p);
                        match release {
                            UiDragRelease::None => {}
                            UiDragRelease::Click(id) => {
                                if schnellui_widgets::dispatch_click(
                                    &self.app.widgets.clone(),
                                    self.app.scene_mut(),
                                    id,
                                ) {
                                    if let Some(w) = &self.window {
                                        w.request_redraw();
                                    }
                                }
                            }
                            UiDragRelease::Drop { .. } => {
                                if let Some(w) = &self.window {
                                    w.request_redraw();
                                }
                            }
                        }
                        if active_changed || proximity_changed {
                            if let Some(w) = &self.window {
                                w.request_redraw();
                            }
                        }
                    }
                    let interaction = self.interaction_snapshot(p);
                    self.trace(
                        "pointer_button_result",
                        json!({
                            "route": "widgets",
                            "state": debug_label(state),
                            "completed": true,
                            "interaction": interaction,
                        }),
                    );
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let scale = self.app.scale();
                let p = self.cursor_logical();
                let raw_delta = match delta {
                    MouseScrollDelta::LineDelta(x, y) => RawWheelDelta::Lines { x, y },
                    MouseScrollDelta::PixelDelta(pos) => RawWheelDelta::Pixels {
                        x: pos.x as f32 / scale,
                        y: pos.y as f32 / scale,
                    },
                };
                let result = self.app.dispatch_focused_pointer(
                    p,
                    raw_modifiers(self.modifiers, self.control_pressed),
                    RawPointerAction::Wheel(raw_delta),
                    false,
                );
                if self.apply_focused_input_result(result) {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                if self.app.dismiss_context_menu() {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                    return;
                }
                // Sign convention (SOUL §3.2 scroll): winit's vertical scroll is
                // **positive when the wheel moves up** (content travels down toward
                // the top). schnellui's offset grows as the content scrolls *up*
                // (revealing what's below), so we negate winit's y — a wheel-down
                // gesture increases the offset. `LineDelta` is in notches (one
                // SCROLL_STEP each); `PixelDelta` is physical px → logical (÷ scale).
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * crate::SCROLL_STEP,
                    MouseScrollDelta::PixelDelta(pos) => -(pos.y as f32) / scale,
                };
                // A wheel leaves dropdowns alone; pointer presses still dismiss
                // them. This avoids two unrelated full-tree walks per wheel event
                // (generic hit testing + expanded-dropdown collection).
                // The deepest scroll viewport gets the wheel first. If it cannot
                // move at that edge, routing walks upward to a scroll ancestor.
                if schnellui_widgets::dispatch_wheel_at(
                    &self.app.widgets.clone(),
                    self.app.scene_mut(),
                    p,
                    dy,
                ) {
                    // A scroll callback may request an expensive virtualized
                    // subtree replacement. Leave it pending until the native
                    // redraw so wheel events already queued by the compositor
                    // can overwrite application state first; the redraw then
                    // builds only the newest window instead of synchronously
                    // chasing every intermediate one.
                    structural_update_deferred = true;
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::Resized(size) => {
                let _ = self.app.dismiss_context_menu();
                let (pw, ph) = (size.width.max(1), size.height.max(1));
                if let Some(r) = &mut self.renderer {
                    r.resize(pw, ph);
                }
                let scale = self.app.scale();
                self.app.resize(pw as f32 / scale, ph as f32 / scale);
                self.accessibility_full_update_pending = true;
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                // A controller wake remains pending until the native redraw it
                // caused starts. Acknowledge first, so a controller update that
                // lands during this frame requests a following one rather than
                // being coalesced into an already-consumed wake.
                self.app.redraw_signal.acknowledge_native_redraw();
                // Fold the newest structural update into this frame before
                // producing pixels. Polling again below would consume a later
                // update after rendering and make the displayed tree chase one
                // frame behind the controller.
                self.poll_remount();
                structural_update_polled = true;
                self.sync_window_title();
                if self.app.advance_theme_transition() {
                    self.sync_renderer_after_theme_rebuild();
                }
                // Reconcile the surface to the window's *true* current size before
                // producing pixels. A `Resized` event can arrive after (or be
                // coalesced around) a redraw, and fractional-scaling compositors can
                // hand back a size that drifts from the last `Resized` by a pixel or
                // two; either way a render against the stale config presents a
                // wrong-extent buffer — the garbage strip along the top and the dark
                // bands down the right/bottom the user saw (SOUL §8 resize path).
                // `resize` early-returns when unchanged, so this is free in the
                // steady state (Directive #3 — still purely reactive).
                if let (Some(w), Some(r)) = (self.window.as_ref(), self.renderer.as_mut()) {
                    let size = w.inner_size();
                    let (pw, ph) = (size.width.max(1), size.height.max(1));
                    if r.size() != (pw, ph) {
                        r.resize(pw, ph);
                        let scale = self.app.scale();
                        self.app.resize(pw as f32 / scale, ph as f32 / scale);
                        self.accessibility_full_update_pending = true;
                    }
                }
                // The one place pixels and native semantics are produced.
                self.redraw();
                // A slow frame may finish after the deadline that requested it.
                // Rebase animation pacing on completion so it still yields to
                // input instead of immediately starting another expensive frame.
                let frame_finished = Instant::now();
                rebase_redraw_after_frame(
                    &mut self.animation_deadline,
                    Some(ANIMATION_REDRAW_INTERVAL),
                    frame_finished,
                );
                rebase_redraw_after_frame(
                    &mut self.interval_deadline,
                    self.app.redraw_interval,
                    frame_finished,
                );
            }
            _ => {}
        }
        // An input handler above may have asked the host for a different screen
        // (e.g. a switcher tab's on:select) — swap the new tree in under the live
        // window before going back to sleep (SOUL §8).
        if !structural_update_polled && !structural_update_deferred {
            self.poll_remount();
        }
        self.sync_cursor();
        if self.autoclose_elapsed() {
            event_loop.exit();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: PlatformEvent) {
        match event {
            PlatformEvent::AccessKit(event) => {
                if self.window.as_ref().map(|window| window.id()) != Some(event.window_id) {
                    return;
                }
                match event.window_event {
                    AccessKitWindowEvent::InitialTreeRequested => {
                        self.publish_full_accessibility_tree();
                    }
                    AccessKitWindowEvent::ActionRequested(request) => {
                        self.remount_trigger = "accesskit_action";
                        if self.app.dispatch_action(&request) {
                            self.poll_remount();
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                    AccessKitWindowEvent::AccessibilityDeactivated => {}
                }
            }
            PlatformEvent::ReducedMotionChanged(reduce) => {
                self.remount_trigger = "reduced_motion";
                if self.app.apply_reduced_motion(reduce) {
                    // Completing a theme transition may have reconstructed the
                    // retained tree; syncing is harmless when only animation
                    // policy changed and essential in the reconstruction case.
                    self.sync_renderer_after_theme_rebuild();
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
            }
            PlatformEvent::RedrawRequested => {
                // This merely bridges the controller-thread wake to winit. The
                // signal is acknowledged at the beginning of the corresponding
                // native redraw, after which a new controller update can queue
                // another frame without being stranded.
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            PlatformEvent::Debug(request) => {
                self.remount_trigger = "debug_instrumentation";
                let reply = self.handle_debug_command(event_loop, request.command);
                let _ = request.reply.send(reply);
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.autoclose_elapsed() {
            event_loop.exit();
            return;
        }
        let now = Instant::now();
        // Trailing scroll callbacks own application state and may request a
        // structural replacement. They are taken out of the widget runtime
        // before they run; immediately poll the update hook so a settled scroll
        // can replace virtualized content without waiting for another input.
        if schnellui_widgets::fire_due_scroll_callbacks(&self.app.widgets, now) {
            self.poll_remount();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
        // Async SVG rasterizations in flight (SOUL §8.1): keep redrawing —
        // each frame drains what finished, vsync paces the loop, and the
        // requests stop the moment the count hits zero. Windowed mode never
        // blocks on images; they pop in a frame or two after a (re)mount.
        let animation_active = schnellui_widgets::pending_svg_rasters(&self.app.widgets) > 0
            || self.app.continuous_redraw
            || (self.app.animations_enabled()
                && schnellui_widgets::has_loading_spinners(&self.app.widgets, self.app.scene()))
            || (self.app.animations_enabled()
                && schnellui_widgets::has_floating_label_animations(&self.app.widgets.clone()))
            || schnellui_widgets::has_active_edge_auto_scroll(&self.app.widgets, self.app.scene())
            || self.app.theme_transition_active();
        let (animation_due, animation_wake) =
            pace_animation_redraw(animation_active, now, &mut self.animation_deadline);
        if animation_due {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
        }
        let interval_wake = if let Some(interval) = self.app.redraw_interval {
            let deadline = self.interval_deadline.get_or_insert_with(|| now + interval);
            if now >= *deadline {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                *deadline = now + interval;
            }
            Some(*deadline)
        } else {
            self.interval_deadline = None;
            None
        };
        let scroll_wake = schnellui_widgets::next_scroll_callback_deadline(&self.app.widgets);
        let next_wake = [self.deadline, interval_wake, animation_wake, scroll_wake]
            .into_iter()
            .flatten()
            .min();
        match next_wake {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

pub(super) fn raw_capture_target_survives(previous: &App, replacement: &App) -> bool {
    previous
        .focused_input_semantics()
        .is_some_and(|(role, name)| {
            let Some(name) = name else {
                return false;
            };
            let role = schnellui_a11y::Role::from_u16(role);
            replacement
                .find_widget(role, Some(&name))
                .and_then(|target| replacement.scene().a11y(target))
                .is_some_and(|target| {
                    replacement.has_focused_input_binding(target.role, target.name.as_deref())
                })
        })
}

pub(super) const fn remount_allowed(left_pointer_down: bool) -> bool {
    !left_pointer_down
}

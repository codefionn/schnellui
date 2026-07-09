use super::*;

impl App {
    // --- introspection outputs (SOUL §7) ---

    /// Renders one frame to an offscreen texture and writes a PNG to `path`
    /// (SOUL §7.2). Lazily creates the GPU renderer on first call.
    pub fn render_to_png(&mut self, path: impl AsRef<Path>) -> std::io::Result<()> {
        std::fs::write(path, self.render_png_bytes())
    }

    /// Renders the current frame as encoded PNG bytes for the debug HTTP bridge.
    pub(crate) fn render_png_bytes(&mut self) -> Vec<u8> {
        // The one-shot determinism contract (SOUL §7.3): block until every async
        // SVG rasterization has landed, so this single frame contains the images.
        schnellui_widgets::settle_svg_rasters(&self.widgets, &mut self.scene);
        // Physical target dimensions = logical × scale (SOUL §7.1 `--scale`).
        let pw = (self.size.width * self.scale).round().max(1.0) as u32;
        let ph = (self.size.height * self.scale).round().max(1.0) as u32;
        let renderer = if let Some(renderer) = self.renderer.as_mut() {
            renderer
        } else {
            let mut r = Renderer::new(pw, ph, Backend::Auto);
            r.set_scale(self.scale);
            r.set_clear_color(self.clear);
            self.renderer.insert(r)
        };
        // Keep a cached renderer sized to the *current* viewport: a frame taken
        // after `App::resize` must render at the new extent, not the stale one
        // the renderer was first created at (SOUL §8 resize path). No-op when the
        // size is unchanged.
        renderer.resize(pw, ph);
        renderer.render_to_png(&self.scene, &self.atlas)
    }

    /// Renders one frame to tightly-packed RGBA8 bytes at the current **physical**
    /// size (`size × scale`), or `None` when no GPU adapter is available so callers
    /// (tests) skip gracefully (SOUL §7.2). Lazily creates the renderer and keeps it
    /// sized to the viewport across [`App::resize`] (a grow event, §4). This is the
    /// byte-level oracle behind the retained==reconstructed equivalence check
    /// (SOUL §3.2): after any event sequence the retained app's pixels must equal a
    /// freshly-constructed app mounted directly in that end state.
    pub fn render_rgba8(&mut self) -> Option<Vec<u8>> {
        // Same one-shot determinism contract as `render_to_png` (SOUL §7.3).
        schnellui_widgets::settle_svg_rasters(&self.widgets, &mut self.scene);
        let pw = (self.size.width * self.scale).round().max(1.0) as u32;
        let ph = (self.size.height * self.scale).round().max(1.0) as u32;
        let renderer = if let Some(renderer) = self.renderer.as_mut() {
            renderer
        } else {
            let mut r = Renderer::try_new(pw, ph, Backend::Auto).ok()?;
            r.set_scale(self.scale);
            r.set_clear_color(self.clear);
            self.renderer.insert(r)
        };
        renderer.resize(pw, ph);
        Some(renderer.render_rgba8(&self.scene, &self.atlas))
    }

    /// Renders the current scene to a window surface (opt-in windowed mode, SOUL §8).
    /// A disjoint borrow of the retained scene + glyph atlas, so the surface renderer
    /// can push any glyphs rasterized since the last frame (SOUL §3.2). Call after
    /// [`App::frame`]. This never runs on the headless PNG path (§7).
    pub fn render_to_surface(&mut self, renderer: &mut SurfaceRenderer) {
        renderer.render(&self.scene, &mut self.atlas);
    }

    /// Opens a native window and runs the winit event loop (opt-in, **non-headless** —
    /// SOUL §8). The window opens at the app's logical size × [`App::scale`] in physical
    /// pixels. This is a separate, explicit entry point: headless one-shot
    /// screenshotting (§7) is entirely unaffected and remains the default.
    /// The attached AccessKit adapter publishes the same retained semantic tree through
    /// AT-SPI on Linux, UI Automation on Windows, and NSAccessibility on macOS; native
    /// actions on all three platforms converge on [`App::dispatch_action`].
    ///
    /// **Redraw scheduling is reactive, never a busy loop** (SOUL Directive #3 spirit):
    /// the loop *waits* (`ControlFlow::Wait`) and only repaints when something changed.
    /// `RedrawRequested` runs [`App::frame`] then renders; a redraw is *requested* after
    /// a mouse click mutates a signal, after a window resize, and once on first show —
    /// nothing spins.
    ///
    /// - `RedrawRequested` → [`App::frame`] + render to the surface.
    /// - Pointer hover selects native cursors for enabled controls, text editing,
    ///   movable titlebars, and resize handles; captured titlebar/resize drags retain
    ///   their cursor until release.
    /// - Left mouse press → hit-test the cursor (physical→logical, ÷ scale) → the same
    ///   [`dispatch_click`](widgets::dispatch_click) handler an inbound AccessKit
    ///   `ActionRequest` fires (SOUL §6.3) → request a redraw. A press on a text
    ///   input focuses it and places the caret (Shift-click / drag extends the
    ///   selection); double-click selects a Unicode word and triple-click selects
    ///   a line, with following drags preserving that granularity; Alt-click
    ///   adds/removes a VS Code-style additional caret. A press on any other
    ///   focusable widget focuses it (browser click-to-focus); a press elsewhere
    ///   blurs.
    /// - Right mouse press on a configured widget opens its themed context menu
    ///   (editables default to Cut/Copy/Paste/Select All). The keyboard Context
    ///   Menu key and Shift+F10 open the same menu; Escape, an outside press,
    ///   scrolling, or resizing dismisses it.
    /// - Keyboard → **standard browser controls** via [`App::dispatch_key`]
    ///   (SOUL §6.3): Tab / Shift+Tab walk the a11y tab order; Enter / Space
    ///   activate the focused control (button ← both, link ← Enter,
    ///   checkbox/switch/radio ← Space, tabs/items/rows/dropdowns ← both); arrows
    ///   adjust a focused slider (PageUp/PageDown ±10 steps, Home/End min/max) and
    ///   move+select within a focused radio group; the focused text input/area
    ///   takes typing, Backspace/Delete, arrows/Home/End (Shift extends the
    ///   selection, Ctrl jumps words), select-all, and native clipboard
    ///   copy/cut/paste shortcuts; everything left scrolls the enclosing (or
    ///   first) scroll viewport — arrows a notch,
    ///   PageUp/PageDown/Space a page (Shift+Space back), Home/End to the ends.
    /// - `Esc` requests dismissal of the top-most dialog; with no dialog it exits.
    ///   `CloseRequested` always exits cleanly.
    /// - Window resize → reconfigure the surface + relayout at the new logical size
    ///   (coarse — see [`App::resize`]).
    ///
    /// **`SCHNELLUI_AUTOCLOSE_MS=<n>`**: if set, the loop auto-exits after `n`
    /// milliseconds (scheduled via a `ControlFlow::WaitUntil` wake, so it stays
    /// reactive). This exists so an agent or CI can smoke-test windowed mode without a
    /// human closing the window — e.g.
    /// `SCHNELLUI_AUTOCLOSE_MS=2000 counter --scenario counter_zero --windowed`.
    pub fn run_windowed(self, title: &str) -> Result<(), Box<dyn std::error::Error>> {
        windowed::run(self, title, None)
    }

    /// Like [`App::run_windowed`], with a compatibility whole-app **remount hook**.
    /// This coarse API remains useful for switching unrelated screens. Hosts with
    /// local structural updates should use
    /// [`App::run_windowed_with_viewport_updates`] and [`SubtreeReplacement`]. After
    /// after input events, the loop polls `remount` (scroll input is folded at
    /// the next native redraw so queued offsets coalesce); when it returns
    /// `Some(new_app)`, the new tree takes over the existing window and surface: the
    /// renderer's scale/clear follow the new app, its deterministic glyph atlas is
    /// reconciled against the resident GPU texture, the new tree is laid out at the
    /// window's **current** size, and a redraw is requested.
    /// Return `None` from the hook in the steady state — polling is then free.
    pub fn run_windowed_with(
        self,
        title: &str,
        mut remount: impl FnMut() -> Option<App> + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        windowed::run(
            self,
            title,
            Some(Box::new(move |_| {
                remount()
                    .map(Remount::unspecified)
                    .map(WindowUpdate::Remount)
            })),
        )
    }

    /// Like [`App::run_windowed_with`], but supplies the current logical viewport
    /// size whenever the remount hook is polled. Responsive static view trees can
    /// use this to rebuild geometry after a native window resize.
    pub fn run_windowed_with_viewport(
        self,
        title: &str,
        remount: impl FnMut(scene::Size) -> Option<App> + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut remount = remount;
        windowed::run(
            self,
            title,
            Some(Box::new(move |viewport| {
                remount(viewport)
                    .map(Remount::unspecified)
                    .map(WindowUpdate::Remount)
            })),
        )
    }

    /// Like [`App::run_windowed_with`], but every replacement carries a stable
    /// reason into the interaction trace.
    pub fn run_windowed_with_reasoned_remount(
        self,
        title: &str,
        mut remount: impl FnMut() -> Option<Remount> + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        windowed::run(
            self,
            title,
            Some(Box::new(move |_| remount().map(WindowUpdate::Remount))),
        )
    }

    /// Viewport-aware variant of [`App::run_windowed_with_reasoned_remount`].
    pub fn run_windowed_with_viewport_reasoned_remount(
        self,
        title: &str,
        remount: impl FnMut(scene::Size) -> Option<Remount> + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut remount = remount;
        windowed::run(
            self,
            title,
            Some(Box::new(move |viewport| {
                remount(viewport).map(WindowUpdate::Remount)
            })),
        )
    }

    /// Runs the native host with a viewport-aware structural update callback.
    ///
    /// Unlike the compatibility `run_windowed_with*` hooks, this callback may
    /// yield [`WindowUpdate::Subtrees`] to rebuild only referenced branches.
    pub fn run_windowed_with_viewport_updates(
        self,
        title: &str,
        updates: impl FnMut(scene::Size) -> Option<WindowUpdate> + 'static,
    ) -> Result<(), Box<dyn std::error::Error>> {
        windowed::run(self, title, Some(Box::new(updates)))
    }

    /// Writes the AccessKit tree as JSON to `path` (SOUL §7.1 `--dump-a11y`). This
    /// is fully implemented — the semantic ground truth for the agent loop (§6.5).
    pub fn dump_a11y(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let json = schnellui_a11y::dump_json(&self.scene);
        std::fs::write(path, json)
    }

    /// Returns the AccessKit tree as a JSON string without touching the filesystem.
    pub fn a11y_json(&self) -> String {
        schnellui_a11y::dump_json(&self.scene)
    }

    /// Dispatches an inbound AccessKit action to the same handler pointer input
    /// would fire (SOUL §6.3, §7.5 drive step). Returns `true` if a target handler
    /// was found and run.
    pub fn dispatch_action(&mut self, request: &accesskit_action::ActionRequest) -> bool {
        use accesskit_action::{Action, ActionData};
        // Reverse-map the AccessKit target id → WidgetId (SOUL §6.2: the a11y NodeId
        // *is* the WidgetId), verify liveness in the scene, then fire the widget's
        // stored handler through the *same* inbound path a pointer/wheel event takes
        // (SOUL §6.3 — assistive and pointer input converge on one code path). The
        // action selects which handler: `Click` → the click path; `ScrollUp`/
        // `ScrollDown` → one notch of scroll (SOUL §3.2); `Focus`/`Blur` → the
        // keyboard-focus path; `SetValue` → the text-edit path; any other action
        // keeps the click behavior.
        let Some(id) = schnellui_a11y::route_action(&self.scene, request) else {
            return false;
        };
        // Assistive actions obey the same modal boundary as pointer and keyboard
        // input; obscured controls cannot be activated behind a dialog.
        if let Some(panel) = schnellui_widgets::active_modal_panel(&self.scene) {
            if !schnellui_widgets::is_in_subtree(&self.scene, id, panel) {
                return false;
            }
        }
        // Assistive activation/scroll on a modeless background window mirrors a
        // pointer press: raise its shared dialog layer before routing the input.
        // Focus uses `self.focus` below, which performs the same raise once.
        let raised = (request.action != Action::Focus)
            && schnellui_widgets::foreground_dialog_for_widget(&mut self.scene, id);
        let dismissed = if matches!(request.action, Action::Focus | Action::Blur) {
            false
        } else {
            schnellui_widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, Some(id))
        };
        let changed = match request.action {
            Action::ScrollUp => {
                schnellui_widgets::dispatch_scroll(&self.widgets, &mut self.scene, id, -SCROLL_STEP)
            }
            Action::ScrollDown => {
                schnellui_widgets::dispatch_scroll(&self.widgets, &mut self.scene, id, SCROLL_STEP)
            }
            Action::Focus => self.focus(Some(id)),
            Action::Blur => self.focus(None),
            // Increment/Decrement adjust a slider by one step — the same
            // dispatch the keyboard arrows reach (SOUL §6.3).
            Action::Increment => schnellui_widgets::dispatch_adjust(
                &self.widgets,
                &mut self.scene,
                id,
                widgets::Adjust::Steps(1),
            ),
            Action::Decrement => schnellui_widgets::dispatch_adjust(
                &self.widgets,
                &mut self.scene,
                id,
                widgets::Adjust::Steps(-1),
            ),
            Action::SetValue => match &request.data {
                Some(ActionData::Value(v))
                    if self.scene.node(id).map(|n| n.kind) == Some(WidgetKind::Slider) =>
                {
                    v.parse::<f32>().ok().is_some_and(|value| {
                        schnellui_widgets::dispatch_set_slider_value(
                            &self.widgets,
                            &mut self.scene,
                            id,
                            value,
                        )
                    })
                }
                Some(ActionData::Value(v)) => self.set_text_value(id, v),
                _ => false,
            },
            Action::ShowContextMenu => {
                let position = self
                    .scene
                    .layout(id)
                    .map(|layout| Point {
                        x: layout.rect.x,
                        y: layout.rect.y + layout.rect.height,
                    })
                    .unwrap_or_default();
                self.open_context_menu(id, position, true)
            }
            Action::Click
                if schnellui_widgets::context_menu_trigger_source(&self.widgets, id).is_some() =>
            {
                let source = schnellui_widgets::context_menu_trigger_source(&self.widgets, id)
                    .expect("checked above");
                let position = self
                    .scene
                    .layout(source)
                    .map(|layout| Point {
                        x: layout.rect.x,
                        y: layout.rect.y + layout.rect.height,
                    })
                    .unwrap_or_default();
                self.open_context_menu(source, position, true)
            }
            Action::Click
                if schnellui_widgets::context_menu_item(&self.widgets, &self.scene, id) =>
            {
                let activation = self.activate_context_menu_item(id);
                if let Some(activation) = activation {
                    if activation.action == widgets::ContextMenuAction::SelectAll {
                        let _ = self.select_all_text_for(activation.source);
                    }
                    true
                } else {
                    false
                }
            }
            Action::Click => schnellui_widgets::dispatch_click(&self.widgets, &mut self.scene, id),
            _ => schnellui_widgets::dispatch_click(&self.widgets, &mut self.scene, id),
        };
        dismissed | raised | changed
    }

    /// Drives a backend-neutral semantic scenario action through the retained
    /// backend's existing AccessKit dispatch path.
    ///
    /// The same [`DriveAction`] slice can be passed to the native HTML renderer's
    /// `render_scenario`, keeping scenario definitions independent of WGPU/DOM.
    pub fn drive_action(&mut self, action: &DriveAction) -> bool {
        use accesskit_action::{Action, ActionData, ActionRequest};
        use accesskit_reexport::TreeId;

        let (target, request_action, data) = match action {
            DriveAction::Click(target) => (target, Action::Click, None),
            DriveAction::SetValue(target, value) => (
                target,
                Action::SetValue,
                Some(ActionData::Value(value.clone().into())),
            ),
            DriveAction::Increment(target) => (target, Action::Increment, None),
            DriveAction::Decrement(target) => (target, Action::Decrement, None),
        };
        let Some(widget) = self.find_widget(target.role, target.name.as_deref()) else {
            return false;
        };
        self.dispatch_action(&ActionRequest {
            action: request_action,
            target_tree: TreeId::ROOT,
            target_node: schnellui_a11y::to_access_id(widget),
            data,
        })
    }
}

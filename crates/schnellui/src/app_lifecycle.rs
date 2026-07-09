use super::*;

impl App {
    /// An empty app with a fixed viewport (SOUL §7.3). No root mounted yet.
    pub fn new(width: u32, height: u32) -> App {
        Self::new_with_context(width, height, Context::new())
    }

    pub(crate) fn new_with_context(width: u32, height: u32, context: Context) -> App {
        let widgets = schnellui_widgets::Runtime::new();
        if let Some(selected) = context.get::<Theme>() {
            schnellui_widgets::set_theme(&widgets, selected);
        }
        let active_theme = schnellui_widgets::theme(&widgets);
        App {
            context,
            widgets,
            scene: Scene::new(),
            layout: LayoutEngine::new(),
            text: TextShaper::new(),
            atlas: GlyphAtlas::new(1024, 1024),
            renderer: None,
            test_registry: HashMap::new(),
            shortcut_registry: HashMap::new(),
            focused_key_bindings: Vec::new(),
            focused_input_bindings: Vec::new(),
            cursor_bindings: Vec::new(),
            size: Size {
                width: width as f32,
                height: height as f32,
            },
            scale: 1.0,
            clear: Color::WHITE,
            now: 0,
            laid_out: false,
            paint_bindings: Vec::new(),
            theme_factory: None,
            theme_mode: ThemeMode::Fixed(active_theme),
            color_scheme: ColorScheme::Light,
            active_theme,
            theme_transition_duration: Duration::ZERO,
            theme_transition: None,
            theme_binding: None,
            animations_enabled: true,
            continuous_redraw: false,
            redraw_interval: None,
            redraw_signal: RedrawSignal::default(),
            window_title_provider: None,
            interaction_trace: None,
        }
    }

    /// Sets the logical→physical scale (SOUL §7.1 `--scale`). Affects shaping,
    /// painting, and the PNG's physical dimensions. For a mounted tree, set the scale
    /// via [`App::mount_with_size_scaled`] so build-time glyphs rasterize at the right
    /// physical size; this bare setter suits scenarios with no text (e.g. `Empty`).
    pub fn set_scale(&mut self, scale: f32) {
        self.scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
    }

    /// The logical→physical scale factor (SOUL §7.1).
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Sets the fixed background clear color (SOUL §7.3).
    pub fn set_clear_color(&mut self, color: Color) {
        self.clear = color;
    }

    /// The fixed background clear color (SOUL §7.3). Read by windowed mode to paint
    /// the surface under the scene with the same background as a headless shot.
    pub fn clear_color(&self) -> Color {
        self.clear
    }

    /// Enables structured JSONL diagnostics for the built-in native window host.
    ///
    /// The trace covers semantic hit paths, focus, cursor selection, raw pointer
    /// capture, drag state, and structural remounts. Configuration is consumed
    /// when a `run_windowed*` method starts; it has no headless rendering cost.
    pub fn set_interaction_trace(&mut self, trace: InteractionTrace) {
        self.interaction_trace = Some(trace);
    }

    /// The design theme owned by this app's widget runtime.
    pub fn theme(&self) -> Theme {
        self.active_theme
    }

    /// Builds the retained tree from a root [`View`] **once** (SOUL §3.3 — the setup
    /// function runs once) and returns the app. Mount may allocate (SOUL §4).
    pub fn mount(root: impl View) -> App {
        App::mount_with_size(root, 800, 600)
    }

    /// Builds a retained application from an explicit dependency context.
    ///
    /// The factory receives the same context stored by the app. Components may
    /// pass `context.with(value)` to a child to create an inline child scope.
    pub fn mount_with_context<V, F>(context: Context, view: F) -> App
    where
        V: View + 'static,
        F: FnOnce(&Context) -> V,
    {
        Self::mount_with_context_size_scaled(context, view, 800, 600, 1.0)
    }

    /// Like [`App::mount_with_context`] with an explicit viewport and scale.
    pub fn mount_with_context_size_scaled<V, F>(
        context: Context,
        view: F,
        width: u32,
        height: u32,
        scale: f32,
    ) -> App
    where
        V: View + 'static,
        F: FnOnce(&Context) -> V,
    {
        let root = view(&context);
        let mut app = App::new_with_context(width, height, context);
        app.set_scale(scale);
        app.mount_boxed(Box::new(root));
        app
    }

    /// Like [`App::mount`] but with an explicit viewport (SOUL §7.3 fixed viewport,
    /// driven by `--width`/`--height`), at scale `1.0`.
    pub fn mount_with_size(root: impl View, width: u32, height: u32) -> App {
        App::mount_with_size_scaled(root, width, height, 1.0)
    }

    /// Like [`App::mount_with_size`] but at an explicit logical→physical `scale`
    /// (SOUL §7.1 `--scale`). The scale is set **before** `build`, so glyphs rasterize
    /// at their physical size and the intrinsic measures come out glyph-exact.
    pub fn mount_with_size_scaled(root: impl View, width: u32, height: u32, scale: f32) -> App {
        let mut app = App::new(width, height);
        app.set_scale(scale);
        app.mount_boxed(Box::new(root));
        app
    }

    /// Mounts a one-shot tree with an explicitly app-owned design theme.
    pub fn mount_with_theme_size_scaled(
        theme: Theme,
        root: impl View,
        width: u32,
        height: u32,
        scale: f32,
    ) -> App {
        Self::mount_with_context_size_scaled(
            Context::new().provide(theme),
            |_| root,
            width,
            height,
            scale,
        )
    }

    /// Mounts a renderer-generic component template through the retained scene
    /// adapter. The same template value type can instead be consumed by the native
    /// HTML renderer.
    pub fn mount_template(root: impl Template) -> App {
        App::mount_template_with_size_scaled(root, 800, 600, 1.0)
    }

    /// Like [`App::mount_template`], with an explicit viewport and scale.
    pub fn mount_template_with_size_scaled(
        root: impl Template,
        width: u32,
        height: u32,
        scale: f32,
    ) -> App {
        let retained = root.render(&mut schnellui_widgets::SceneTemplate);
        App::mount_with_size_scaled(retained, width, height, scale)
    }

    /// Mounts a reconstructible themed application at the default viewport.
    ///
    /// `view` is retained and called again when the theme or native color scheme
    /// changes. Put persistent state (signals/model handles) outside the closure
    /// and capture clones inside it so a theme remount preserves application state.
    pub fn mount_themed<V, F>(mode: impl Into<ThemeMode>, view: F) -> App
    where
        V: View + 'static,
        F: FnMut() -> V + 'static,
    {
        App::mount_themed_with_size_scaled(mode, view, 800, 600, 1.0)
    }

    /// Like [`App::mount_themed`], with an explicit viewport and scale.
    pub fn mount_themed_with_size_scaled<V, F>(
        mode: impl Into<ThemeMode>,
        mut view: F,
        width: u32,
        height: u32,
        scale: f32,
    ) -> App
    where
        V: View + 'static,
        F: FnMut() -> V + 'static,
    {
        let mode = mode.into();
        let scheme = ColorScheme::Light;
        let selected = mode.resolve(scheme);
        let mut factory: Box<dyn FnMut() -> Box<dyn View>> = Box::new(move || Box::new(view()));
        let root = factory();
        let mut app = App::new(width, height);
        schnellui_widgets::set_theme(&app.widgets, selected);
        app.set_scale(scale);
        app.active_theme = selected;
        app.theme_mode = mode;
        app.color_scheme = scheme;
        app.clear = selected.page;
        app.mount_boxed(root);
        app.theme_factory = Some(factory);
        app
    }

    pub(crate) fn mount_boxed(&mut self, boxed: Box<dyn View>) {
        // Clear any stale handlers/dynamic slots from a prior mount on this thread so
        // reused WidgetId keys cannot alias into another tree's registry (SOUL §3.3).
        schnellui_widgets::reset(&self.widgets);
        let mut ctx = BuildCtx {
            context: self.context.clone(),
            runtime: self.widgets.clone(),
            scene: &mut self.scene,
            layout: &mut self.layout,
            text: &mut self.text,
            atlas: &mut self.atlas,
            scale: self.scale,
        };
        // NOTE: View::build bodies are the widgets-owner's responsibility (§8.1).
        let root_id = boxed.build(&mut ctx, None);
        self.scene.set_root(root_id);
        // A modal is a focus-grabbing boundary from its first exposed frame.
        // Move keyboard/AccessKit focus to its first focusable descendant now;
        // `focus_step` is already scoped to the highest modal in a stack. A
        // content-only dialog has no tab stop, so AccessKit falls back to the
        // dialog itself as the temporary accessibility-tree root.
        if schnellui_widgets::active_modal_panel(&self.scene).is_some() {
            let _ = self.focus_step(false);
        }
    }

    /// The theme currently used to construct this app.
    pub fn active_theme(&self) -> Theme {
        self.active_theme
    }

    /// The current light/dark platform appearance.
    pub fn color_scheme(&self) -> ColorScheme {
        self.color_scheme
    }

    /// Configures the duration used for native light/dark changes.
    ///
    /// The default is zero, which applies the new design in one remount.
    pub fn set_theme_transition_duration(&mut self, duration: Duration) {
        self.theme_transition_duration = duration;
    }

    /// Returns whether animations are currently allowed.
    ///
    /// The built-in window host keeps this synchronized with the platform's
    /// reduced-motion accessibility preference. Headless and custom hosts start
    /// with animations enabled.
    pub fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    /// Applies a platform reduced-motion accessibility preference.
    ///
    /// Enabling reduced motion immediately completes an in-flight theme transition
    /// and finite input-label transitions, while freezing continuously animated
    /// widgets on their current frame. Disabling it lets subsequent transitions and
    /// continuously animated widgets run again.
    /// Custom window hosts should call this when their equivalent native preference
    /// changes; [`App::run_windowed`] handles it automatically.
    pub fn apply_reduced_motion(&mut self, reduce: bool) -> bool {
        let enabled = !reduce;
        if self.animations_enabled == enabled {
            return false;
        }
        self.animations_enabled = enabled;
        if reduce {
            if let Some(transition) = self.theme_transition.take() {
                let _ = self.rebuild_with_theme(transition.to);
            }
            schnellui_widgets::finish_floating_label_animations(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
        }
        true
    }

    /// Rebuilds a `mount_themed*` app under a fixed theme.
    ///
    /// Returns `false` for a one-shot `App::mount*`, which deliberately does not
    /// retain a view factory.
    pub fn set_theme(&mut self, theme: Theme) -> bool {
        self.theme_mode = ThemeMode::Fixed(theme);
        self.theme_transition = None;
        self.theme_binding = None;
        self.rebuild_with_theme(theme)
    }

    /// Transitions a reconstructible app to a fixed theme.
    ///
    /// Intermediate frames interpolate both palette and shape tokens. Each frame
    /// is a normal retained-tree remount, ensuring intrinsic geometry, hit targets,
    /// and paint never disagree.
    pub fn transition_theme(&mut self, theme: Theme, duration: Duration) -> bool {
        if self.theme_factory.is_none() {
            return false;
        }
        self.theme_mode = ThemeMode::Fixed(theme);
        self.theme_binding = None;
        if duration.is_zero() || !self.animations_enabled || self.active_theme == theme {
            self.theme_transition = None;
            return self.rebuild_with_theme(theme);
        }
        self.theme_transition = Some(ActiveThemeTransition {
            from: self.active_theme,
            to: theme,
            started: Instant::now(),
            duration,
        });
        true
    }

    /// Applies a platform light/dark appearance.
    ///
    /// Windowed apps call this automatically from winit's initial appearance and
    /// `ThemeChanged` events. Custom hosts can feed their equivalent notification.
    /// A fixed [`ThemeMode`] records the scheme but does not rebuild.
    pub fn apply_color_scheme(&mut self, scheme: ColorScheme) -> bool {
        self.color_scheme = scheme;
        let ThemeMode::System { .. } = self.theme_mode else {
            return false;
        };
        let target = self.theme_mode.resolve(scheme);
        if target == self.active_theme {
            return false;
        }
        if self.theme_transition_duration.is_zero() || !self.animations_enabled {
            self.theme_transition = None;
            self.rebuild_with_theme(target)
        } else {
            self.theme_transition = Some(ActiveThemeTransition {
                from: self.active_theme,
                to: target,
                started: Instant::now(),
                duration: self.theme_transition_duration,
            });
            true
        }
    }

    pub(crate) fn theme_transition_active(&self) -> bool {
        self.animations_enabled && self.theme_transition.is_some()
    }

    pub(crate) fn advance_theme_transition(&mut self) -> bool {
        let Some(transition) = self.theme_transition else {
            return false;
        };
        let elapsed = transition.started.elapsed();
        let amount = (elapsed.as_secs_f32() / transition.duration.as_secs_f32()).min(1.0);
        let selected = transition.from.lerp(transition.to, amount);
        let changed = self.rebuild_with_theme(selected);
        if amount >= 1.0 {
            self.theme_transition = None;
        }
        changed
    }

    pub(crate) fn rebuild_with_theme(&mut self, selected: Theme) -> bool {
        let Some(mut factory) = self.theme_factory.take() else {
            return false;
        };
        if self.active_theme == selected {
            self.theme_factory = Some(factory);
            return false;
        }

        let root = factory();
        let mut replacement = App::new(self.size.width as u32, self.size.height as u32);
        schnellui_widgets::set_theme(&replacement.widgets, selected);
        replacement.size = self.size;
        replacement.scale = self.scale;
        replacement.clear = selected.page;
        replacement.now = self.now;
        replacement.theme_factory = Some(factory);
        replacement.theme_mode = self.theme_mode;
        replacement.color_scheme = self.color_scheme;
        replacement.active_theme = selected;
        replacement.theme_transition_duration = self.theme_transition_duration;
        replacement.theme_transition = self.theme_transition;
        replacement.theme_binding = self.theme_binding.take();
        replacement.animations_enabled = self.animations_enabled;
        replacement.mount_boxed(root);
        replacement.inherit_remount_state(self);
        replacement.inherit_host_configuration(self);
        *self = replacement;
        true
    }

    /// Binds the app theme to reactive application state.
    ///
    /// `evaluate` runs in the pull phase of each frame. When its value changes,
    /// a `mount_themed*` app reconstructs its retained tree immediately under the
    /// new palette and shape tokens. Returns `false` for one-shot mounts.
    pub fn bind_theme(&mut self, evaluate: impl FnMut() -> Theme + 'static) -> bool {
        if self.theme_factory.is_none() {
            return false;
        }
        self.theme_binding = Some(Box::new(evaluate));
        true
    }

    pub(crate) fn poll_theme_binding(&mut self) {
        let Some(mut evaluate) = self.theme_binding.take() else {
            return;
        };
        let selected = evaluate();
        self.theme_binding = Some(evaluate);
        if selected != self.active_theme {
            self.theme_mode = ThemeMode::Fixed(selected);
            self.theme_transition = None;
            let _ = self.rebuild_with_theme(selected);
        }
    }

    /// The logical viewport size.
    pub fn size(&self) -> Size {
        self.size
    }

    /// Resizes the logical viewport and forces a relayout on the next [`App::frame`]
    /// (SOUL §8.1). Used by windowed mode on a window resize.
    ///
    /// **Coarse for now:** this clears `laid_out` so the *whole* tree relayouts on the
    /// next frame, rather than the minimal affected subtree. A window resize is a rare
    /// grow event (the `resize` budget row, SOUL §4.1), so a full re-layout is
    /// acceptable; a finer per-subtree resize path is a future refinement. Ignores a
    /// non-finite or non-positive dimension (keeps the current one).
    pub fn resize(&mut self, width: f32, height: f32) {
        let w = if width.is_finite() && width > 0.0 {
            width
        } else {
            self.size.width
        };
        let h = if height.is_finite() && height > 0.0 {
            height
        } else {
            self.size.height
        };
        self.size = Size {
            width: w,
            height: h,
        };
        self.laid_out = false;
    }

    /// The injected logical clock (SOUL §7.3 — `now == 0`).
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Immutable / mutable access to the retained scene (for tests + scenario setup).
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// The root dependency context used to build this application.
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn scene_mut(&mut self) -> &mut Scene {
        &mut self.scene
    }

    /// Resolves an application-created component ref in the current mount.
    pub fn resolve_ref(&self, reference: scene::ComponentRef) -> Option<scene::WidgetId> {
        self.scene.resolve_ref(reference)
    }

    /// Rebuilds one referenced branch without reconstructing the surrounding app.
    ///
    /// Widget/layout records owned by the old branch are retired before `view` is
    /// built at the same parent and sibling index. Scroll/edit/focus state outside
    /// the branch, host callbacks, the window and renderer all remain live.
    pub fn replace_subtree(
        &mut self,
        target: scene::ComponentRef,
        view: impl View,
    ) -> Result<scene::WidgetId, MissingSubtreeTarget> {
        self.replace_subtree_boxed(target, Box::new(view))
    }

    /// Applies a type-erased replacement produced for a native or custom host.
    pub fn apply_subtree_replacement(
        &mut self,
        replacement: SubtreeReplacement,
    ) -> Result<scene::WidgetId, MissingSubtreeTarget> {
        let focus_after = replacement.focus_after;
        let root = self.replace_subtree_boxed(replacement.target, replacement.view)?;
        if let Some(target) = focus_after {
            if let Some(widget) = self.scene.resolve_ref(target) {
                let _ = self.focus(Some(widget));
            }
        }
        Ok(root)
    }

    pub(crate) fn replace_subtree_boxed(
        &mut self,
        target: scene::ComponentRef,
        view: Box<dyn View>,
    ) -> Result<scene::WidgetId, MissingSubtreeTarget> {
        let old_root = self
            .scene
            .resolve_ref(target)
            .ok_or_else(|| structural_update::missing(target))?;
        let removed_nodes = self.scene.subtree_nodes(old_root);
        // End-following viewports keep their pin through a subtree replacement
        // exactly as they do across a full remount (`inherit_scroll_offsets`):
        // record which ones rest at the end of the outgoing content, then
        // re-arm the sentinel after the swap so the next layout clamp lands on
        // the replacement final extent. A viewport the user scrolled away from
        // the end keeps its reading offset untouched.
        // Only a viewport *containing this branch* participates. Re-arming every
        // scene scroll here makes an unrelated header/card replacement jump an
        // otherwise independently-scrolled transcript to its end.
        let mut follow_end_pinned = Vec::new();
        let mut ancestor = Some(old_root);
        while let Some(id) = ancestor {
            if self
                .scene
                .node(id)
                .is_some_and(|node| node.kind == WidgetKind::Scroll)
                && schnellui_widgets::scroll_follows_end(&self.widgets, id)
                && schnellui_widgets::scroll_is_at_end(&self.scene, id)
            {
                follow_end_pinned.push(id);
            }
            ancestor = self.scene.node(id).and_then(|node| node.parent);
        }

        if self
            .focused_widget()
            .is_some_and(|focused| removed_nodes.contains(&focused))
        {
            let _ = self.focus(None);
        } else {
            let _ = schnellui_widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
            let _ = schnellui_widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, None);
        }
        schnellui_widgets::purge_nodes(&self.widgets, &mut self.scene, &removed_nodes);
        self.layout.remove_nodes(&removed_nodes);
        let removed = self
            .scene
            .remove_subtree(old_root)
            .expect("resolved subtree root must remain live until removal");

        self.paint_bindings
            .retain(|(id, _)| self.scene.node(*id).is_some());
        self.cursor_bindings
            .retain(|binding| self.scene.node(binding.widget).is_some());

        let mut ctx = BuildCtx {
            context: self.context.clone(),
            runtime: self.widgets.clone(),
            scene: &mut self.scene,
            layout: &mut self.layout,
            text: &mut self.text,
            atlas: &mut self.atlas,
            scale: self.scale,
        };
        let new_root = view.build(&mut ctx, removed.parent);
        if let Some(parent) = removed.parent {
            self.scene
                .move_child_to_index(parent, new_root, removed.child_index);
        } else {
            self.scene.set_root(new_root);
        }
        self.scene.set_component_ref(new_root, target);
        self.layout
            .sync_replacement(&self.scene, new_root, removed.parent);
        for id in follow_end_pinned {
            if self.scene.node(id).is_some() {
                // Clamped onto the replacement real content end by the next
                // layout pass, mirroring the follow_end mount-time sentinel.
                self.scene.set_scroll_offset(
                    id,
                    Point {
                        x: 0.0,
                        y: f32::MAX,
                    },
                );
            }
        }
        self.scene
            .mark_dirty(removed.parent.unwrap_or(new_root), scene::DirtyFlags::ALL);
        Ok(new_root)
    }

    /// Runs one synchronous frame: **pull → layout → paint → a11y**, each pass
    /// walked only over its dirty subtree (SOUL §8.1). No event loop (SOUL §7.1).
    pub fn frame(&mut self) {
        let _ = self.settle_frame();
        self.scene.clear_dirty();
    }

    /// Runs the retained passes while keeping the resulting dirty channels alive.
    /// The native host uses this narrow seam to publish AccessKit changes and
    /// present pixels before retiring the frame; headless callers use [`App::frame`].
    pub(crate) fn settle_frame(&mut self) -> bool {
        let mut layout_changed = false;
        // An opt-in reactive theme binding is a whole-tree structural dependency:
        // poll it before every other pull so a changed design rebuilds first and
        // the remainder of this frame operates exclusively on the replacement.
        self.poll_theme_binding();
        // ---- PULL: settle the signal graph, then the widgets-side dynamic slots.
        // Runtime::flush drains queued effects/subscriptions (SOUL §3.1).
        // run_dynamic_slots re-runs only ready producers, and on a *changed* value
        // mutates the retained node in place — updating a11y (name/value) + paint and
        // flagging exactly the channels it touched (PAINT + A11Y, plus LAYOUT if the
        // measured width moved). Work is proportional to affected dynamic sites
        // (Directive #4), not all registered sites or the tree size.
        schnellui_signal::Runtime::flush();
        // Reactive paint bindings (the zero-alloc rerender_1_signal path): evaluate
        // each `|| -> Color` and write it to its node. `set_color` compares before
        // writing, so an unchanged colour stays clean; a changed one flags PAINT and
        // folds the node's rect into `scene.damage()` — no relayout, no allocation.
        for (node, eval) in self.paint_bindings.iter_mut() {
            if self.scene.node(*node).is_some() {
                let c = eval();
                self.scene.set_color(*node, c);
            }
        }
        schnellui_widgets::run_dynamic_slots(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
        );
        schnellui_widgets::poll_dynamic_images(&self.widgets, &mut self.scene);
        // Pointer-edge scrolling is an input-driven animation. It remains active
        // under reduced-motion preferences because it directly tracks a held
        // pointer rather than adding decorative motion.
        schnellui_widgets::tick_edge_auto_scroll(&self.widgets, &mut self.scene);
        // Continuously animated indicators and short input-label transitions mutate
        // retained fragments in place. Trees without either pay only sparse-map
        // emptiness checks and remain purely reactive.
        if self.animations_enabled {
            schnellui_widgets::tick_loading_spinners(&self.widgets, &mut self.scene);
            schnellui_widgets::tick_floating_labels(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
        } else {
            // Reduced motion keeps the state change but removes its interpolation.
            schnellui_widgets::finish_floating_label_animations(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
        }
        // Pull in any finished async SVG rasterizations (SOUL §8.1 image pipeline):
        // each landing writes its reserved atlas rect (revision bump ⇒ the renderer
        // re-uploads) and flags the widget paint-dirty. Non-blocking; with nothing
        // in flight this is a single counter check — no lock, no allocation.
        schnellui_widgets::drain_svg_rasters(&self.widgets, &mut self.scene);

        // ---- LAYOUT (if layout-dirty): recompute geometry, then anchor paint to it.
        // A virtual list is the one retained view that needs a bounded feedback
        // loop: it reconciles its keyed pixel window before layout, then observes
        // actual variable row heights afterwards. At most two follow-up passes
        // absorb an initial viewport and changed estimates; clean frames still do
        // no list work and take the ordinary single-pass path.
        for _ in 0..3 {
            let virtual_changed = schnellui_widgets::reconcile_virtual_lists(
                &self.widgets,
                &self.context,
                &mut self.scene,
                &mut self.layout,
                &mut self.text,
                &mut self.atlas,
                self.scale,
            );
            let Some(root) = self.scene.root() else { break };
            let needs_layout =
                virtual_changed || !self.laid_out || !self.scene.layout_dirty().is_empty();
            if !needs_layout {
                break;
            }
            // Structural mounts synchronize once. Later style/text dirtiness
            // reuses the resident Taffy graph; virtual rows refresh only their
            // list parent and newly entering row branches.
            if !self.laid_out {
                self.layout.sync_tree(&self.scene, root);
            } else {
                self.layout
                    .sync_dirty_nodes(&self.scene, self.scene.layout_dirty());
            }
            let text = &mut self.text;
            let widgets = &self.widgets;
            self.layout
                .compute_with(&mut self.scene, root, self.size, &mut |id, avail| {
                    schnellui_widgets::measure_text(widgets, id, avail, text)
                });
            schnellui_widgets::clamp_scroll_offsets(&self.widgets, &mut self.scene);
            schnellui_widgets::emit_wrapped_paint(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
            schnellui_widgets::reposition_paint(&self.widgets, &mut self.scene);
            self.laid_out = true;
            layout_changed = true;
            if !schnellui_widgets::measure_virtual_lists(
                &self.widgets,
                &mut self.scene,
                &mut self.layout,
            ) {
                break;
            }
        }

        // PTY output ordinarily changes pixels without changing terminal grid
        // dimensions. Emit that retained paint on every settled frame; the widget
        // skips clean grids, while changed grids no longer wait for an unrelated
        // layout or pointer event before their primitives reach the renderer.
        schnellui_widgets::emit_terminal_grid_paint(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
        );

        // ---- PAINT / A11Y: the scene has already folded every changed node's rect
        // into `scene.damage()` and pushed changed ids onto `scene.a11y_dirty()`
        // (via the mutation setters in the pull phase). The GPU delta upload reads
        // that damage in `render_to_png`/`render_rgba8`; the incremental AccessKit
        // `TreeUpdate` is built on demand from the a11y-dirty set by an attached
        // platform adapter (headless has none — see `a11y_tree_update`). Nothing more
        // to do here. The caller retires the dirty channels after accessibility and
        // pixels have both consumed them (SOUL §8.1 — clear_dirty after present).
        layout_changed
    }

    /// Builds an incremental AccessKit [`TreeUpdate`](accesskit_reexport::TreeUpdate)
    /// for the nodes currently in the a11y-dirty set (SOUL §6.2) — the payload a
    /// platform adapter would push after a frame. Foreign AccessKit types own their
    /// storage, so this allocates proportionally to the changed nodes (the budgeted
    /// a11y row, §6.2 — never the literal-zero paint path). Call it *before*
    /// [`App::frame`] clears the dirty set, or right after a mutation.
    pub fn a11y_tree_update(&self) -> accesskit_reexport::TreeUpdate {
        schnellui_a11y::build_incremental_tree_update(&self.scene)
    }

    /// Registers a reactive **paint binding**: on every [`App::frame`], `eval` is
    /// run and its `Color` written to `node` via `scene.set_color` (SOUL §1 — a
    /// signal changing repaints one node). `eval` should read `Copy` signal values
    /// and return a `Copy` `Color` so the steady-state frame stays literal-zero
    /// (SOUL §4.1). This is the non-text sibling of the widgets' dynamic-text slots.
    pub fn bind_paint(&mut self, node: WidgetId, eval: impl FnMut() -> Color + 'static) {
        self.paint_bindings.push((node, Box::new(eval)));
    }

    /// Locates the first widget carrying `role` and (if `name` is `Some`) that
    /// accessible name, in tree pre-order (SOUL §7.5 — locate by semantics, never by
    /// pixel coordinates). The seam a drive script uses to aim an `ActionRequest`.
    pub fn find_widget(&self, role: a11y::Role, name: Option<&str>) -> Option<scene::WidgetId> {
        let target = role.as_u16();
        fn walk(
            scene: &Scene,
            id: scene::WidgetId,
            target: u16,
            name: Option<&str>,
        ) -> Option<scene::WidgetId> {
            if !scene.is_visible(id) {
                return None;
            }
            if let Some(a) = scene.a11y(id) {
                if a.role == target && name.map(|n| a.name.as_deref() == Some(n)).unwrap_or(true) {
                    return Some(id);
                }
            }
            if let Some(node) = scene.node(id) {
                for &c in &node.children {
                    if let Some(found) = walk(scene, c, target, name) {
                        return Some(found);
                    }
                }
            }
            None
        }
        self.scene
            .root()
            .and_then(|r| walk(&self.scene, r, target, name))
    }

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
        let previous_focus = previous.focused_widget().map(|id| {
            let ring_visible = schnellui_widgets::focus_ring_visible(&previous.widgets, id);
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
                schnellui_widgets::inherit_remount_state(
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
            previous.paint_bindings.iter().map(|(widget, _)| *widget),
        );
        self.paint_bindings = std::mem::take(&mut previous.paint_bindings)
            .into_iter()
            .filter_map(|(widget, eval)| Some((paint_counterparts.get(widget)?, eval)))
            .collect();
        self.continuous_redraw = previous.continuous_redraw;
        self.redraw_interval = previous.redraw_interval;
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
            if !schnellui_widgets::transfer_dialog_geometry_adjustment(
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
                    schnellui_widgets::scroll_restoration_key(&previous.widgets, id),
                    previous.scene.scroll_offset(id).y,
                    schnellui_widgets::scroll_is_at_end(&previous.scene, id),
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
            let restoration_key = schnellui_widgets::scroll_restoration_key(&self.widgets, id);
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
            let follows_end = schnellui_widgets::scroll_follows_end(&self.widgets, id);
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

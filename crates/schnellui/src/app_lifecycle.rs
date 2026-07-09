//! App state accessors, subtree replacement and the per-frame pass.

use super::*;

impl App {
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
                let widget = self.first_focusable_in(widget).unwrap_or(widget);
                let _ = self.focus(Some(widget));
            }
        }
        Ok(root)
    }

    /// `id` when it takes focus, otherwise its first focusable descendant in
    /// tab order. A reference often lands on a composite's wrapper node, such
    /// as the column around a combo box's field.
    fn first_focusable_in(&self, id: scene::WidgetId) -> Option<scene::WidgetId> {
        a11y::with_tab_order(&self.scene, |order| {
            order
                .iter()
                .copied()
                .find(|&candidate| schnellui::widgets::is_in_subtree(&self.scene, candidate, id))
        })
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
                && schnellui::widgets::scroll_follows_end(&self.widgets, id)
                && schnellui::widgets::scroll_is_at_end(&self.scene, id)
            {
                follow_end_pinned.push(id);
            }
            ancestor = self.scene.node(id).and_then(|node| node.parent);
        }

        // A replacement is not an outside click, so it must not dismiss open
        // dropdowns. Dismissal fires the host's `on_toggle`, which flips state
        // the host already settled for this replacement: a dropdown closed by
        // its option or an outside press would toggle back open and the next
        // update would rebuild it expanded.
        let _ = schnellui::widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        if self
            .focused_widget()
            .is_some_and(|focused| removed_nodes.contains(&focused))
        {
            let previous = self.focused_input_semantics();
            let _ = schnellui::widgets::dispatch_focus(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
                None,
            );
            self.notify_focused_widget_transition(previous);
        }
        schnellui::widgets::purge_nodes(&self.widgets, &mut self.scene, &removed_nodes);
        self.layout.remove_nodes(&removed_nodes);
        let removed = self
            .scene
            .remove_subtree_nodes(old_root, removed_nodes)
            .expect("resolved subtree root must remain live until removal");

        self.paint_bindings.retain_live(&self.scene);
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

    /// Layout passes run since mount. A diagnostic for tests and tooling: a pass
    /// that repeats without a new layout request shows up here.
    pub fn layout_runs(&self) -> u64 {
        self.layout_runs
    }

    /// Runs one synchronous frame: **pull → layout → paint → a11y**, each pass
    /// walked only over its dirty subtree (SOUL §8.1). No event loop (SOUL §7.1).
    pub fn frame(&mut self) {
        let _ = self.settle_frame();
        self.scene.clear_dirty();
    }

    /// Pins the clock read by theme transitions, loading spinners and trailing
    /// scroll callbacks. `None` returns to the wall clock. Simulations pin it so
    /// a run depends only on its inputs and the time steps it chooses.
    pub fn set_clock(&mut self, now: Option<Instant>) {
        schnellui::widgets::set_clock(&self.widgets, now);
    }

    /// The current app time: the pinned clock, else the wall clock.
    pub fn clock_now(&self) -> Instant {
        schnellui::widgets::now(&self.widgets)
    }

    /// Runs the native host's redraw at a pinned time `now`: due trailing scroll
    /// callbacks fire, an active theme transition advances, then one [`App::frame`]
    /// settles everything else. The clock stays pinned at `now` afterwards.
    pub fn frame_at(&mut self, now: Instant) {
        self.set_clock(Some(now));
        let _ = schnellui::widgets::fire_due_scroll_callbacks(&self.widgets, now);
        let _ = self.advance_theme_transition();
        self.frame();
    }

    /// Whether the app needs another frame without new input: an animation is
    /// running, an SVG raster is in flight, edge auto-scroll is held, or the app
    /// asked for continuous redraw. Native hosts pace redraws on this.
    pub fn wants_animation_frame(&self) -> bool {
        schnellui::widgets::pending_svg_rasters(&self.widgets) > 0
            || self.continuous_redraw
            || (self.animations_enabled
                && schnellui::widgets::has_loading_spinners(&self.widgets, &self.scene))
            || (self.animations_enabled
                && schnellui::widgets::has_floating_label_animations(&self.widgets))
            || schnellui::widgets::has_active_edge_auto_scroll(&self.widgets, &self.scene)
            || self.theme_transition_active()
    }

    /// Runs the retained passes while keeping the resulting dirty channels alive.
    /// The native host uses this narrow seam to publish AccessKit changes and
    /// present pixels before retiring the frame; headless callers use [`App::frame`].
    pub(crate) fn settle_frame(&mut self) -> bool {
        let mut layout_changed = false;
        // An opt-in reactive theme binding is a whole-tree structural dependency:
        // poll it before every other pull so a changed design rebuilds first and
        // the remainder of this frame operates exclusively on the replacement.
        // Subscription readiness is delivered by the flush, so settle first.
        schnellui::signal::Runtime::flush();
        self.poll_theme_binding();
        self.poll_hot_reload();
        // ---- PULL: settle the signal graph, then the widgets-side dynamic slots.
        // Runtime::flush drains queued effects/subscriptions (SOUL §3.1).
        // run_dynamic_slots re-runs only ready producers, and on a *changed* value
        // mutates the retained node in place — updating a11y (name/value) + paint and
        // flagging exactly the channels it touched (PAINT + A11Y, plus LAYOUT if the
        // measured width moved). Work is proportional to affected dynamic sites
        // (Directive #4), not all registered sites or the tree size.
        schnellui::signal::Runtime::flush();
        // Reactive paint bindings (the zero-alloc rerender_1_signal path): evaluate
        // each `|| -> Color` and write it to its node. `set_color` compares before
        // writing, so an unchanged colour stays clean; a changed one flags PAINT and
        // folds the node's rect into `scene.paint_dirty()` — no relayout, no allocation.
        self.paint_bindings.run(&mut self.scene);
        schnellui::widgets::run_reactive_attributes(
            &self.widgets,
            &mut self.scene,
            &mut self.layout,
            &mut self.text,
            &mut self.atlas,
        );
        schnellui::widgets::run_dynamic_slots(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
        );
        schnellui::widgets::poll_dynamic_images(&self.widgets, &mut self.scene);
        // Pointer-edge scrolling is an input-driven animation. It remains active
        // under reduced-motion preferences because it directly tracks a held
        // pointer rather than adding decorative motion.
        schnellui::widgets::tick_edge_auto_scroll(&self.widgets, &mut self.scene);
        // Continuously animated indicators and short input-label transitions mutate
        // retained fragments in place. Trees without either pay only sparse-map
        // emptiness checks and remain purely reactive.
        if self.animations_enabled {
            schnellui::widgets::tick_loading_spinners(&self.widgets, &mut self.scene);
            schnellui::widgets::tick_floating_labels(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
        } else {
            schnellui::widgets::pause_loading_spinners(&self.widgets);
            // Reduced motion keeps the state change but removes its interpolation.
            schnellui::widgets::finish_floating_label_animations(
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
        schnellui::widgets::drain_svg_rasters(&self.widgets, &mut self.scene);

        // ---- LAYOUT (if layout-dirty): recompute geometry, then anchor paint to it.
        // A virtual list is the one retained view that needs a bounded feedback
        // loop: it reconciles its keyed pixel window before layout, then observes
        // actual variable row heights afterwards. At most two follow-up passes
        // absorb an initial viewport and changed estimates; clean frames still do
        // no list work and take the ordinary single-pass path.
        for _ in 0..3 {
            let virtual_changed = schnellui::widgets::reconcile_virtual_lists(
                &self.widgets,
                &self.context,
                &mut self.scene,
                &mut self.layout,
                &mut self.text,
                &mut self.atlas,
                self.scale,
            );
            let Some(root) = self.scene.root() else { break };
            let layout_requested = !self.scene.layout_dirty().is_empty()
                && self.scene.layout_epoch() != self.layout_consumed_epoch;
            let needs_layout = virtual_changed || !self.laid_out || layout_requested;
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
                    schnellui::widgets::measure_text(widgets, id, avail, text)
                });
            self.layout_consumed_epoch = self.scene.layout_epoch();
            self.layout_runs += 1;
            schnellui::widgets::clamp_scroll_offsets(&self.widgets, &mut self.scene);
            schnellui::widgets::emit_wrapped_paint(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
            );
            schnellui::widgets::reposition_paint(&self.widgets, &mut self.scene);
            self.laid_out = true;
            layout_changed = true;
            if !schnellui::widgets::measure_virtual_lists(
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
        schnellui::widgets::emit_terminal_grid_paint(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
        );
        // Scrolling never relayouts, so text areas in a Scroll re-emit their
        // visible row band here when the offset leaves it.
        schnellui::widgets::emit_scrolled_text_areas(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
        );

        // Widget repaints above re-emit primitives in theme colours; put bound
        // colours back on the repainted nodes (no closure calls) before the
        // dirty channels are consumed.
        schnellui::widgets::reapply_reactive_colors(&self.widgets, &mut self.scene);
        self.paint_bindings.reapply(&mut self.scene);

        // ---- PAINT / A11Y: every setter in the pull phase has already flagged its
        // node on `scene.paint_dirty()` / `scene.a11y_dirty()` and advanced the paint
        // epoch. The GPU renderer patches the retained instance slots of those nodes
        // in `render_to_png`/`render_rgba8`; the incremental AccessKit `TreeUpdate`
        // is built on demand from the a11y-dirty set by an attached platform adapter
        // (headless has none, see `a11y_tree_update`). Nothing more to do here. The
        // caller retires the dirty channels after accessibility and pixels have both
        // consumed them (SOUL §8.1: clear_dirty after present).
        layout_changed
    }

    /// Builds an incremental AccessKit [`TreeUpdate`](accesskit_reexport::TreeUpdate)
    /// for the nodes currently in the a11y-dirty set (SOUL §6.2) — the payload a
    /// platform adapter would push after a frame. Foreign AccessKit types own their
    /// storage, so this allocates proportionally to the changed nodes (the budgeted
    /// a11y row, §6.2 — never the literal-zero paint path). Call it *before*
    /// [`App::frame`] clears the dirty set, or right after a mutation.
    pub fn a11y_tree_update(&self) -> accesskit_reexport::TreeUpdate {
        schnellui::a11y::build_incremental_tree_update(&self.scene)
    }

    /// Registers a reactive **paint binding**: `eval` runs under a tracked
    /// subscription on the next [`App::frame`] and again only on frames after a
    /// signal it read changed (a clean frame calls no binding). Its `Color` is
    /// written to `node` via `scene.set_color` (SOUL §1 — a signal changing
    /// repaints one node). State that is not a signal never re-runs `eval`. `eval`
    /// should read `Copy` signal values
    /// and return a `Copy` `Color` so the steady-state frame stays literal-zero
    /// (SOUL §4.1). This is the non-text sibling of the widgets' dynamic-text slots.
    pub fn bind_paint(&mut self, node: WidgetId, eval: impl FnMut() -> Color + 'static) {
        self.paint_bindings.push(node, Box::new(eval));
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
}

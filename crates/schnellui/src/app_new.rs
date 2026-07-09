//! App construction: mount entry points and text-engine hand-over between remounts.

use super::*;

thread_local! {
    /// The app a remount hook may take its text engine from, set by the host only
    /// while it polls the hook. Null at every other time.
    static ENGINE_DONOR: std::cell::Cell<*mut App> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    /// True inside [`App::with_remount_engine`]: the next `App::new` may claim the donor's engine.
    static ENGINE_CLAIM_OPEN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Set when a claim succeeded during the current hook call.
    static ENGINE_CLAIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Lends `donor`'s engine to remount-hook constructors for the duration of `f`.
/// Returns `f`'s result and whether an app claimed the engine.
pub(crate) fn with_engine_donor<R>(donor: &mut App, f: impl FnOnce() -> R) -> (R, bool) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ENGINE_DONOR.with(|d| d.set(std::ptr::null_mut()));
        }
    }
    ENGINE_CLAIMED.with(|c| c.set(false));
    ENGINE_DONOR.with(|d| d.set(donor as *mut App));
    let _reset = Reset;
    let result = f();
    (result, ENGINE_CLAIMED.with(|c| c.replace(false)))
}

/// Opens the engine claim around `f`, closing it again on every exit path.
pub(crate) fn claim_remount_engine<R>(f: impl FnOnce() -> R) -> R {
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            ENGINE_CLAIM_OPEN.with(|c| c.set(false));
        }
    }
    ENGINE_CLAIM_OPEN.with(|c| c.set(true));
    let _close = Close;
    f()
}

#[cfg(test)]
thread_local! {
    /// Apps built with a freshly allocated text engine on this thread.
    pub(crate) static FRESH_ENGINES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl App {
    /// Builds the replacement app inside a remount hook on the running app's text
    /// shaper and glyph atlas, so glyphs it already rasterized stay cached and the
    /// renderer keeps its atlas texture instead of re-reconciling a fresh one.
    ///
    /// Only the first `App::new` (or `App::mount*`) inside `build` claims the
    /// engine, and only while the host is polling the hook; anywhere else this just
    /// calls `build`. The running app keeps its own copy of the engine, so a built
    /// app that is dropped or not returned costs nothing. The fork shares the
    /// running atlas id; when the host installs it, the id is kept only if the
    /// running app wrote no glyphs since the fork, otherwise the fork gets a fresh
    /// id and the renderer reconciles it. A built app must not be rendered by the
    /// host's renderer before it is installed.
    pub fn with_remount_engine(build: impl FnOnce() -> App) -> App {
        claim_remount_engine(build)
    }

    /// An empty app with a fixed viewport (SOUL §7.3). No root mounted yet.
    pub fn new(width: u32, height: u32) -> App {
        Self::new_with_context(width, height, Context::new())
    }

    pub(crate) fn new_with_context(width: u32, height: u32, context: Context) -> App {
        #[cfg(test)]
        FRESH_ENGINES.with(|c| c.set(c.get() + 1));
        Self::new_with_engine(
            width,
            height,
            context,
            TextShaper::new(),
            GlyphAtlas::new(1024, 1024),
        )
    }

    /// Like `new_with_context`, but builds around an existing shaper and atlas
    /// so a caller that already has an engine to keep never allocates a second.
    pub(crate) fn new_with_engine(
        width: u32,
        height: u32,
        context: Context,
        text: TextShaper,
        atlas: GlyphAtlas,
    ) -> App {
        let widgets = schnellui::widgets::Runtime::new();
        if let Some(selected) = context.get::<Theme>() {
            schnellui::widgets::set_theme(&widgets, selected);
        }
        let active_theme = schnellui::widgets::theme(&widgets);
        let mut app = App {
            context,
            widgets,
            scene: Scene::new(),
            layout: LayoutEngine::new(),
            text,
            atlas,
            renderer: None,
            hot_reload: None,
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
            layout_consumed_epoch: 0,
            layout_runs: 0,
            pointer_hit: std::cell::Cell::new(None),
            paint_bindings: bindings::PaintBindings::new(),
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
        };
        // A hook-built replacement inside `with_remount_engine` takes over the
        // running app's engine before it mounts anything.
        if ENGINE_CLAIM_OPEN.with(|c| c.replace(false)) {
            let donor = ENGINE_DONOR.with(|d| d.replace(std::ptr::null_mut()));
            if !donor.is_null() {
                // SAFETY: the host sets the pointer from `&mut self.app` only
                // for the duration of the hook call and touches no app meanwhile.
                let donor = unsafe { &mut *donor };
                // Fork rather than move: the donor stays intact when the hook
                // returns None or something other than this app. The fork keeps
                // the atlas id and write log, so the renderer's view stays valid.
                // A full atlas is not carried; the replacement starts fresh.
                if donor.atlas.has_headroom() {
                    app.text = donor.text.fork();
                    app.atlas = donor.atlas.fork();
                    ENGINE_CLAIMED.with(|c| c.set(true));
                }
            }
        }
        app
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
        let retained = root.render(&mut schnellui::widgets::SceneTemplate);
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
        schnellui::widgets::set_theme(&app.widgets, selected);
        app.set_scale(scale);
        app.active_theme = selected;
        app.theme_mode = mode;
        app.color_scheme = scheme;
        app.clear = selected.page;
        app.mount_boxed(root);
        app.theme_factory = Some(factory);
        app
    }

    /// Takes over `previous`'s text shaper and glyph atlas so glyphs it already
    /// rasterized stay cached and the atlas keeps its identity (a renderer then
    /// uploads only new coverage instead of the whole texture).
    ///
    /// Call this on a freshly constructed app **before** mounting: a mounted
    /// scene holds atlas rects from its own atlas, and swapping the atlas under it
    /// would leave them pointing at the wrong texels. Returns `false` and changes
    /// nothing when this app already has a mounted tree or `previous`'s atlas is
    /// past [`GlyphAtlas::has_headroom`] (a fresh one is then better than a full
    /// one that would drop new glyphs). On success `previous` is
    /// left holding this app's fresh engine and must be dropped, not rendered.
    pub fn adopt_text_engine(&mut self, previous: &mut App) -> bool {
        if self.scene.root().is_some() || !previous.atlas.has_headroom() {
            return false;
        }
        std::mem::swap(&mut self.text, &mut previous.text);
        std::mem::swap(&mut self.atlas, &mut previous.atlas);
        true
    }

    pub(crate) fn mount_boxed(&mut self, boxed: Box<dyn View>) {
        // Clear any stale handlers/dynamic slots from a prior mount on this thread so
        // reused WidgetId keys cannot alias into another tree's registry (SOUL §3.3).
        schnellui::widgets::reset(&self.widgets);
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
        if schnellui::widgets::active_modal_panel(&self.scene).is_some() {
            let _ = self.focus_step(false);
        }
    }
}

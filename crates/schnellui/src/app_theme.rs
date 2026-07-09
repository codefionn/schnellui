//! App theming: theme selection, transitions, reduced motion and theme binding.

use super::*;

impl App {
    /// The design theme owned by this app's widget runtime.
    pub fn theme(&self) -> Theme {
        self.active_theme
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
            schnellui::widgets::finish_floating_label_animations(
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
            started: schnellui::widgets::now(&self.widgets),
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
                started: schnellui::widgets::now(&self.widgets),
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
        let elapsed =
            schnellui::widgets::now(&self.widgets).saturating_duration_since(transition.started);
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
        // Keep the shaper and atlas: every glyph the old tree rasterized is a
        // cache hit for the new one, and the renderer's texture stays valid.
        // The old app is dropped below, so it gets a detached shaper (no font
        // clone) and a 1x1 atlas instead of a second engine. A full atlas is not carried; the replacement
        // starts fresh, as in `adopt_text_engine`.
        let (width, height) = (self.size.width as u32, self.size.height as u32);
        let mut replacement = if self.atlas.has_headroom() {
            let text = std::mem::replace(&mut self.text, TextShaper::detached());
            let atlas = std::mem::replace(&mut self.atlas, GlyphAtlas::new(1, 1));
            App::new_with_engine(width, height, Context::new(), text, atlas)
        } else {
            App::new(width, height)
        };
        schnellui::widgets::set_theme(&replacement.widgets, selected);
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
        self.theme_binding = Some(bindings::ThemeBinding::new(Box::new(evaluate)));
        true
    }

    pub(crate) fn poll_theme_binding(&mut self) {
        // Only a changed signal the closure read (or a fresh binding) re-runs it.
        let Some(selected) = self.theme_binding.as_mut().and_then(|b| b.poll()) else {
            return;
        };
        if selected != self.active_theme {
            self.theme_mode = ThemeMode::Fixed(selected);
            self.theme_transition = None;
            let _ = self.rebuild_with_theme(selected);
        }
    }
}

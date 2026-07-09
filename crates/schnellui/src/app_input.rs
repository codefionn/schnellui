use super::*;

impl App {
    // --- test-signal registry (SOUL §7) ---

    /// Registers a named setter so `set_signal(name, …)` drives the underlying
    /// signal (SOUL §7). Widgets/scenarios call this at mount for each named slot.
    pub fn register_signal(
        &mut self,
        name: impl Into<String>,
        setter: impl FnMut(TestValue) + 'static,
    ) {
        self.test_registry.insert(name.into(), Box::new(setter));
    }

    /// Injects a value into a named signal via the registry (SOUL §7). Returns
    /// `true` if the name was registered. Does **not** itself run a frame — the
    /// caller calls [`App::frame`] to observe the effect (SOUL §7.5 synchronous
    /// drive step).
    pub fn set_signal(&mut self, name: &str, value: impl Into<TestValue>) -> bool {
        if let Some(setter) = self.test_registry.get_mut(name) {
            setter(value.into());
            true
        } else {
            false
        }
    }

    /// The registered test-signal names (diagnostics).
    pub fn signal_names(&self) -> impl Iterator<Item = &str> {
        self.test_registry.keys().map(|s| s.as_str())
    }

    /// Registers an application keyboard shortcut, replacing any handler
    /// already assigned to the same chord.
    pub fn register_shortcut(&mut self, shortcut: Shortcut, handler: impl FnMut() + 'static) {
        self.shortcut_registry
            .insert(shortcut.normalized(), Box::new(handler));
    }

    /// Registers a raw-key handler for a focused widget identified by its
    /// accessible role and optional name. The handler runs before built-in
    /// widget editing, activation, and page scrolling and returns whether it
    /// consumed the key.
    pub fn register_focused_key_handler(
        &mut self,
        role: a11y::Role,
        name: Option<&str>,
        handler: impl for<'a> FnMut(UiKey<'a>) -> bool + 'static,
    ) {
        self.focused_key_bindings.push(FocusedKeyBinding {
            role: role.as_u16(),
            name: name.map(str::to_owned),
            handler: Box::new(handler),
        });
    }

    /// Registers a full-fidelity input handler for a focused widget identified
    /// by accessible role and optional name.
    ///
    /// The handler receives both key states (including modifier keys and
    /// repeats), captured pointer input, widget/window focus transitions, IME
    /// composition and terminal-style clipboard requests. It runs before the
    /// compatibility [`register_focused_key_handler`](Self::register_focused_key_handler)
    /// and built-in widget behavior.
    pub fn register_focused_input_handler(
        &mut self,
        role: a11y::Role,
        name: Option<&str>,
        handler: impl FnMut(FocusedInputEvent) -> FocusedInputResult + 'static,
    ) {
        self.focused_input_bindings.push(FocusedInputBinding {
            role: role.as_u16(),
            name: name.map(str::to_owned),
            handler: Box::new(handler),
        });
    }

    /// Registers a dynamic pointer cursor for the semantic widget subtree.
    ///
    /// Built-in capture and control cursors retain priority. The provider is
    /// queried only while the pointer hits this widget or one of its children,
    /// so leaving an embedded surface immediately restores SchnellUI's cursor.
    /// Returns `false` when no matching widget exists in the current mount.
    pub fn register_cursor_provider(
        &mut self,
        role: a11y::Role,
        name: Option<&str>,
        provider: impl Fn() -> widgets::CursorIcon + 'static,
    ) -> bool {
        let Some(widget) = self.find_widget(role, name) else {
            return false;
        };
        self.cursor_bindings.push(CursorBinding {
            widget,
            provider: Box::new(provider),
        });
        true
    }

    /// Enables or disables continuous native redraws for streaming content.
    pub fn set_continuous_redraw(&mut self, enabled: bool) {
        self.continuous_redraw = enabled;
    }

    /// Requests periodic native redraws at a bounded cadence. Unlike continuous
    /// redraw this lets the event loop sleep between frames.
    pub fn set_redraw_interval(&mut self, interval: Option<Duration>) {
        self.redraw_interval = interval.filter(|interval| !interval.is_zero());
    }

    /// Returns a thread-safe handle that wakes and redraws the native window.
    /// Calls made before window startup are harmless.
    pub fn redraw_signal(&self) -> RedrawSignal {
        self.redraw_signal.clone()
    }

    /// Supplies the title used by the built-in native window host. The provider
    /// is polled during redraw and the platform title is updated only when its
    /// value changes. Without a provider, the title passed to
    /// [`App::run_windowed`] remains in effect.
    pub fn set_window_title_provider(&mut self, provider: impl FnMut() -> String + 'static) {
        self.window_title_provider = Some(Box::new(provider));
    }

    /// Dispatches one resolved application shortcut. Returns `true` when a
    /// handler was registered for the chord.
    pub fn dispatch_shortcut(&mut self, shortcut: Shortcut) -> bool {
        let Some(handler) = self.shortcut_registry.get_mut(&shortcut.normalized()) else {
            return false;
        };
        handler();
        true
    }

    pub(crate) fn dispatch_focused_key_handler(&mut self, key: UiKey<'_>) -> bool {
        let focused_semantics = self
            .focused_widget()
            .and_then(|id| self.scene.a11y(id))
            .map(|a| (a.role, a.name.clone()));
        let Some((role, name)) = focused_semantics else {
            return false;
        };
        self.focused_key_bindings
            .iter_mut()
            .find(|binding| {
                binding.role == role
                    && binding
                        .name
                        .as_deref()
                        .is_none_or(|expected| name.as_deref() == Some(expected))
            })
            .is_some_and(|binding| (binding.handler)(key))
    }

    /// Offers an event to the full-fidelity handler registered for the current
    /// focused widget. This public path makes raw surfaces fully testable without
    /// starting a native event loop.
    pub fn dispatch_focused_input(&mut self, event: FocusedInputEvent) -> FocusedInputResult {
        let Some((role, name)) = self.focused_input_semantics() else {
            return FocusedInputResult::Ignored;
        };
        self.dispatch_focused_input_to(role, name.as_deref(), event)
    }

    pub(crate) fn focused_input_semantics(&self) -> Option<(u16, Option<String>)> {
        self.focused_widget()
            .and_then(|id| self.scene.a11y(id))
            .map(|a| (a.role, a.name.clone()))
    }

    pub(crate) fn has_focused_input_binding(&self, role: u16, name: Option<&str>) -> bool {
        self.focused_input_bindings.iter().any(|binding| {
            binding.role == role
                && binding
                    .name
                    .as_deref()
                    .is_none_or(|expected| name == Some(expected))
        })
    }

    pub(crate) fn dispatch_focused_input_to(
        &mut self,
        role: u16,
        name: Option<&str>,
        event: FocusedInputEvent,
    ) -> FocusedInputResult {
        self.focused_input_bindings
            .iter_mut()
            .find(|binding| {
                binding.role == role
                    && binding
                        .name
                        .as_deref()
                        .is_none_or(|expected| name == Some(expected))
            })
            .map_or(FocusedInputResult::Ignored, |binding| {
                (binding.handler)(event)
            })
    }

    pub(crate) fn dispatch_focused_pointer(
        &mut self,
        window_position: Point,
        modifiers: RawModifiers,
        action: RawPointerAction,
        captured: bool,
    ) -> FocusedInputResult {
        let Some(target) = self.focused_widget() else {
            return FocusedInputResult::Ignored;
        };
        self.dispatch_pointer_to(target, window_position, modifiers, action, captured)
    }

    pub(crate) fn dispatch_hover_pointer(
        &mut self,
        window_position: Point,
        modifiers: RawModifiers,
    ) -> FocusedInputResult {
        let Some(target) = schnellui_widgets::hit_test(&self.widgets, &self.scene, window_position)
            .and_then(|hit| self.focused_input_target_from(hit))
        else {
            return FocusedInputResult::Ignored;
        };
        self.dispatch_pointer_to(
            target,
            window_position,
            modifiers,
            RawPointerAction::Move,
            false,
        )
    }

    pub(crate) fn dispatch_pointer_to(
        &mut self,
        target: WidgetId,
        window_position: Point,
        modifiers: RawModifiers,
        action: RawPointerAction,
        captured: bool,
    ) -> FocusedInputResult {
        let Some(a11y) = self.scene.a11y(target) else {
            return FocusedInputResult::Ignored;
        };
        let semantics = (a11y.role, a11y.name.clone());
        if !self.has_focused_input_binding(semantics.0, semantics.1.as_deref()) {
            return FocusedInputResult::Ignored;
        }
        let Some(layout) = self.scene.layout(target) else {
            return FocusedInputResult::Ignored;
        };
        let rect = layout.rect;
        let inside = window_position.x >= rect.x
            && window_position.y >= rect.y
            && window_position.x < rect.x + rect.width
            && window_position.y < rect.y + rect.height;
        if !captured && !inside {
            return FocusedInputResult::Ignored;
        }
        let position = Point {
            x: window_position.x - rect.x,
            y: window_position.y - rect.y,
        };
        self.dispatch_focused_input_to(
            semantics.0,
            semantics.1.as_deref(),
            FocusedInputEvent::Pointer(RawPointerEvent {
                position,
                window_position,
                modifiers,
                action,
            }),
        )
    }

    pub(crate) fn focused_input_target_from(
        &self,
        target: scene::WidgetId,
    ) -> Option<scene::WidgetId> {
        let mut current = Some(target);
        while let Some(id) = current {
            if let Some(a11y) = self.scene.a11y(id) {
                let focusable = a11y::ActionFlags(a11y.actions).contains(a11y::ActionFlags::FOCUS);
                if focusable && self.has_focused_input_binding(a11y.role, a11y.name.as_deref()) {
                    return Some(id);
                }
            }
            current = self.scene.node(id).and_then(|node| node.parent);
        }
        None
    }
}

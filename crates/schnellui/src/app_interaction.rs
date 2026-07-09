use super::*;

impl App {
    /// Moves keyboard focus to `target` (or clears it with `None`) — the same
    /// exclusive [`StateFlags::FOCUSED`](a11y::StateFlags::FOCUSED) path an inbound
    /// AccessKit `Focus`/`Blur` action and the windowed Tab/click take (SOUL §6.3).
    /// Returns `true` if focus actually moved (⇒ redraw).
    pub fn focus(&mut self, target: Option<scene::WidgetId>) -> bool {
        let previous = self.focused_input_semantics();
        let dismissed =
            schnellui_widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, target);
        let context_dismissed =
            schnellui_widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        let raised = target
            .filter(|id| {
                schnellui_widgets::active_modal_panel(&self.scene)
                    .map(|panel| schnellui_widgets::is_in_subtree(&self.scene, *id, panel))
                    .unwrap_or(true)
            })
            .map(|id| schnellui_widgets::foreground_dialog_for_widget(&mut self.scene, id))
            .unwrap_or(false);
        let changed = dismissed
            | context_dismissed
            | raised
            | schnellui_widgets::dispatch_focus(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
                target,
            );
        self.notify_focused_widget_transition(previous);
        changed
    }

    /// Moves focus from a pointer press. This preserves the semantic focus used
    /// by keyboard dispatch and AccessKit while matching native HTML
    /// `:focus-visible`: ordinary pointer-focused controls do not wear the
    /// keyboard ring. Text editables do wear it and also paint their focused
    /// border and caret, matching browser keyboard-input heuristics.
    pub(crate) fn pointer_focus(&mut self, target: Option<scene::WidgetId>) -> bool {
        let previous = self.focused_input_semantics();
        let target = target.and_then(|target| {
            let mut current = Some(target);
            while let Some(id) = current {
                if self
                    .scene
                    .a11y(id)
                    .map(|a| a11y::ActionFlags(a.actions).contains(a11y::ActionFlags::FOCUS))
                    .unwrap_or(false)
                {
                    return Some(id);
                }
                current = self.scene.node(id).and_then(|node| node.parent);
            }
            None
        });
        let dismissed =
            schnellui_widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, target);
        let context_dismissed =
            schnellui_widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        let raised = target
            .filter(|id| {
                schnellui_widgets::active_modal_panel(&self.scene)
                    .map(|panel| schnellui_widgets::is_in_subtree(&self.scene, *id, panel))
                    .unwrap_or(true)
            })
            .map(|id| schnellui_widgets::foreground_dialog_for_widget(&mut self.scene, id))
            .unwrap_or(false);
        let changed = dismissed
            | context_dismissed
            | raised
            | schnellui_widgets::dispatch_pointer_focus(
                &self.widgets,
                &mut self.scene,
                &mut self.text,
                &mut self.atlas,
                target,
            );
        self.notify_focused_widget_transition(previous);
        changed
    }

    pub(crate) fn notify_focused_widget_transition(
        &mut self,
        previous: Option<(u16, Option<String>)>,
    ) {
        let current = self.focused_input_semantics();
        if previous == current {
            return;
        }
        if let Some((role, name)) = previous {
            let _ = self.dispatch_focused_input_to(
                role,
                name.as_deref(),
                FocusedInputEvent::Focus(RawFocusEvent::WidgetLost),
            );
        }
        if let Some((role, name)) = current {
            let _ = self.dispatch_focused_input_to(
                role,
                name.as_deref(),
                FocusedInputEvent::Focus(RawFocusEvent::WidgetGained),
            );
        }
    }

    /// Restores focus after replacing the retained tree without treating that
    /// restoration as a new interaction. In particular, a structural remount
    /// that just opened a dropdown must not immediately dismiss it merely
    /// because the previous tree's focused control is restored elsewhere.
    pub(crate) fn restore_focus_after_remount(&mut self, focus: RemountFocus) -> bool {
        if schnellui_widgets::active_modal_panel(&self.scene).is_some_and(|panel| {
            !schnellui_widgets::is_in_subtree(&self.scene, focus.target, panel)
        }) {
            // A newly opened modal owns focus. Do not resurrect a surviving
            // control behind it merely because that control existed before the
            // structural replacement.
            return false;
        }
        let raised = schnellui_widgets::foreground_dialog_for_widget(&mut self.scene, focus.target);
        let focused = schnellui_widgets::dispatch_focus(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            Some(focus.target),
        );
        let modality = schnellui_widgets::set_focus_ring_visible(
            &self.widgets,
            &mut self.scene,
            focus.target,
            focus.ring_visible,
        );
        // This is continuity, not a blur/focus interaction. In particular raw
        // terminal and browser surfaces must not receive WidgetGained on every
        // content remount.
        raised | focused | modality
    }

    /// Steps keyboard focus through the a11y tab order (SOUL §6.3): forward on
    /// Tab, backward on Shift+Tab, wrapping at either end. With nothing focused —
    /// or focus stranded on a widget that has left the order (e.g. it became
    /// disabled while focused) — enters the order at its first (or, backwards,
    /// last) widget. Returns `true` if focus moved (⇒ redraw).
    pub fn focus_step(&mut self, backwards: bool) -> bool {
        let scene = &self.scene;
        let mut order = schnellui_a11y::tab_order(scene);
        // A modal dialog owns the keyboard until it closes. Modeless dialogs
        // deliberately leave the global tab order untouched.
        if let Some(panel) = schnellui_widgets::active_modal_panel(scene) {
            order.retain(|id| schnellui_widgets::is_in_subtree(scene, *id, panel));
        }
        let current = schnellui_a11y::focused(scene);
        let next = current
            .and_then(|cur| order.iter().position(|id| *id == cur))
            .and_then(|position| {
                if backwards {
                    position
                        .checked_sub(1)
                        .and_then(|index| order.get(index).copied())
                        .or_else(|| order.last().copied())
                } else {
                    order
                        .get(position + 1)
                        .copied()
                        .or_else(|| order.first().copied())
                }
            })
            .or_else(|| {
                if backwards {
                    order.last().copied()
                } else {
                    order.first().copied()
                }
            });
        match next {
            Some(n) => self.focus(Some(n)),
            None => false,
        }
    }

    /// Routes one resolved key press with **standard browser semantics**
    /// (SOUL §6.3) — the single keyboard path shared by the windowed loop and
    /// headless drives (Directive #5). Precedence mirrors a browser:
    ///
    /// 1. **Tab / Shift+Tab** walk the a11y tab order ([`App::focus_step`]).
    /// 2. A focused **editable** (text input / area) consumes its editing keys —
    ///    typing, arrows, Home/End, Backspace/Delete, Ctrl+A — and they never
    ///    leak to page scrolling. PageUp/PageDown fall through (as in a browser
    ///    input field).
    /// 3. A focused **slider** takes arrows (±1 step), PageUp/PageDown (±10),
    ///    Home/End (min/max) via [`widgets::dispatch_adjust`].
    /// 4. A focused **radio** takes arrows to move *and select* within its group.
    /// 5. **Enter / Space** activate the focused widget per the browser matrix
    ///    ([`widgets::dispatch_key_activate`]): button ← both, link ← Enter,
    ///    checkbox/switch/radio ← Space, tabs/items/rows/dropdowns ← both.
    /// 6. Anything left scrolls: arrows one notch, PageUp/PageDown/Space (and
    ///    Shift+Space) one page, Home/End to the boundary — targeting the scroll
    ///    viewport enclosing the focused widget, else the first viewport in the
    ///    tree (the "document" fallback).
    ///
    /// Returns `true` if anything changed (⇒ redraw).
    pub fn dispatch_key(&mut self, key: UiKey<'_>) -> bool {
        use widgets::{ActivateKey, Adjust, EditKey};

        if self.context_menu_is_open() {
            return self.dismiss_context_menu();
        }
        if self.dispatch_focused_key_handler(key) {
            return true;
        }
        if key == UiKey::Escape {
            return schnellui_widgets::dispatch_dialog_escape(&self.widgets, &mut self.scene)
                .unwrap_or(false);
        }
        if let UiKey::Tab { shift } = key {
            return self.focus_step(shift);
        }
        let focused = self.focused_widget();
        let focused_kind = focused.and_then(|f| self.scene.node(f).map(|n| n.kind));
        let focused_combo = focused.filter(|id| {
            self.scene
                .a11y(*id)
                .is_some_and(|a| a11y::Role::from_u16(a.role) == a11y::Role::ComboBox)
        });
        if let Some(combo) = focused_combo {
            let expanded = self
                .scene
                .a11y(combo)
                .is_some_and(|a| a11y::StateFlags(a.state).contains(a11y::StateFlags::EXPANDED));
            match key {
                UiKey::Escape if expanded => {
                    return schnellui_widgets::dismiss_open_dropdowns(
                        &self.widgets,
                        &mut self.scene,
                        None,
                    );
                }
                UiKey::Enter if !expanded => {
                    return schnellui_widgets::dispatch_click(
                        &self.widgets,
                        &mut self.scene,
                        combo,
                    );
                }
                UiKey::Down { shift: false } if expanded => {
                    let wrapper = self.scene.node(combo).and_then(|node| node.parent);
                    let option = wrapper
                        .and_then(|wrapper| self.scene.node(wrapper))
                        .and_then(|node| {
                            node.children.iter().copied().find_map(|child| {
                                self.scene.node(child).and_then(|popup| {
                                    popup.children.iter().copied().find(|option| {
                                        self.scene.is_effectively_visible(*option)
                                            && self.scene.a11y(*option).is_some_and(|a| {
                                                a11y::Role::from_u16(a.role)
                                                    == a11y::Role::ListBoxOption
                                            })
                                    })
                                })
                            })
                        });
                    if let Some(option) = option {
                        return self.focus(Some(option));
                    }
                }
                _ => {}
            }
        }
        // Native HTML's `:focus-visible` follows input modality. Any real
        // keyboard interaction promotes an existing pointer focus to a visible
        // keyboard focus before routing the key.
        let focus_visible = focused
            .map(|id| {
                schnellui_widgets::dispatch_focus(
                    &self.widgets,
                    &mut self.scene,
                    &mut self.text,
                    &mut self.atlas,
                    Some(id),
                )
            })
            .unwrap_or(false);

        match focused_kind {
            // A focused editable consumes its keys (browser: keys in a field
            // never scroll the page); PageUp/PageDown fall through below.
            Some(WidgetKind::TextInput) | Some(WidgetKind::TextArea) => {
                let edit = match key {
                    UiKey::Char(t) => Some(EditKey::Insert(t)),
                    UiKey::Space { .. } => Some(EditKey::Insert(" ")),
                    UiKey::Backspace => Some(EditKey::Backspace),
                    UiKey::Delete => Some(EditKey::Delete),
                    UiKey::Left { shift, ctrl } => Some(EditKey::Left {
                        select: shift,
                        word: ctrl,
                    }),
                    UiKey::Right { shift, ctrl } => Some(EditKey::Right {
                        select: shift,
                        word: ctrl,
                    }),
                    UiKey::Up { shift } => Some(EditKey::Up { select: shift }),
                    UiKey::Down { shift } => Some(EditKey::Down { select: shift }),
                    UiKey::Home { shift } => Some(EditKey::Home { select: shift }),
                    UiKey::End { shift } => Some(EditKey::End { select: shift }),
                    UiKey::Enter => Some(EditKey::Enter),
                    UiKey::SelectAll => Some(EditKey::SelectAll),
                    UiKey::PageUp
                    | UiKey::PageDown
                    | UiKey::Tab { .. }
                    | UiKey::Escape
                    | UiKey::Control(_) => None,
                };
                if let Some(e) = edit {
                    return focus_visible | self.dispatch_edit_key(e);
                }
            }
            // A focused slider takes the range keys (browser `<input type=range>`).
            Some(WidgetKind::Slider) => {
                let adjust = match key {
                    UiKey::Right { .. } | UiKey::Up { .. } => Some(Adjust::Steps(1)),
                    UiKey::Left { .. } | UiKey::Down { .. } => Some(Adjust::Steps(-1)),
                    UiKey::PageUp => Some(Adjust::Steps(10)),
                    UiKey::PageDown => Some(Adjust::Steps(-10)),
                    UiKey::Home { .. } => Some(Adjust::ToMin),
                    UiKey::End { .. } => Some(Adjust::ToMax),
                    _ => None,
                };
                if let Some(a) = adjust {
                    return focus_visible
                        | schnellui_widgets::dispatch_adjust(
                            &self.widgets,
                            &mut self.scene,
                            focused.expect("focused slider"),
                            a,
                        );
                }
            }
            // Arrows on a focused radio move focus *and* selection within the
            // group (the browser radio-group contract).
            Some(WidgetKind::Radio) => {
                let dir = match key {
                    UiKey::Right { .. } | UiKey::Down { .. } => Some(1i32),
                    UiKey::Left { .. } | UiKey::Up { .. } => Some(-1i32),
                    _ => None,
                };
                if let Some(d) = dir {
                    return focus_visible | self.radio_step(focused.expect("focused radio"), d);
                }
            }
            _ => {}
        }

        // Enter / Space activation per the browser matrix. A consumed activation
        // stops here even when it changed nothing (Space on a focused button
        // never scrolls the page).
        let activate = match key {
            UiKey::Enter => Some(ActivateKey::Enter),
            UiKey::Space { .. } => Some(ActivateKey::Space),
            _ => None,
        };
        if let (Some(f), Some(k)) = (focused, activate) {
            if let Some(changed) =
                schnellui_widgets::dispatch_key_activate(&self.widgets, &mut self.scene, f, k)
            {
                return focus_visible | changed;
            }
        }

        // Page scrolling — the browser document-scroll fallback. A focused scroll
        // viewport scrolls itself; otherwise the viewport enclosing the focused
        // widget, else the first viewport in the tree.
        let target = if focused_kind == Some(WidgetKind::Scroll) {
            focused
        } else {
            focused
                .and_then(|f| schnellui_widgets::enclosing_scroll(&self.scene, f))
                .or_else(
                    || match schnellui_widgets::active_modal_panel(&self.scene) {
                        Some(panel) => schnellui_widgets::first_scroll_in(&self.scene, panel),
                        None => schnellui_widgets::first_scroll(&self.scene),
                    },
                )
        };
        let Some(sv) = target else {
            return focus_visible;
        };
        // One "page" is the viewport height minus a notch of overlap for context.
        let page = (self.scene.layout(sv).map(|b| b.rect.height).unwrap_or(0.0) - SCROLL_STEP)
            .max(SCROLL_STEP);
        let delta = match key {
            UiKey::Up { .. } => -SCROLL_STEP,
            UiKey::Down { .. } => SCROLL_STEP,
            UiKey::PageUp => -page,
            UiKey::PageDown => page,
            UiKey::Space { shift: false } => page,
            UiKey::Space { shift: true } => -page,
            UiKey::Home { .. } => -SCROLL_TO_END,
            UiKey::End { .. } => SCROLL_TO_END,
            _ => return focus_visible,
        };
        focus_visible
            | schnellui_widgets::dispatch_scroll(&self.widgets, &mut self.scene, sv, delta)
    }

    /// Moves focus and selection one radio over within the focused radio's group
    /// (SOUL §6.3), wrapping — the standard browser radio-group arrows. `dir` is
    /// `±1`. Returns `true` if anything changed.
    pub(crate) fn radio_step(&mut self, id: scene::WidgetId, dir: i32) -> bool {
        let Some(parent) = self.scene.node(id).and_then(|n| n.parent) else {
            return false;
        };
        let radios: Vec<scene::WidgetId> = self
            .scene
            .node(parent)
            .map(|n| {
                n.children
                    .iter()
                    .copied()
                    .filter(|&c| self.scene.node(c).map(|cn| cn.kind) == Some(WidgetKind::Radio))
                    .collect()
            })
            .unwrap_or_default();
        let Some(pos) = radios.iter().position(|&r| r == id) else {
            return false;
        };
        if radios.len() < 2 {
            return false;
        }
        let len = radios.len() as i32;
        let next = radios[((pos as i32 + dir).rem_euclid(len)) as usize];
        let mut changed = self.focus(Some(next));
        changed |= schnellui_widgets::dispatch_click(&self.widgets, &mut self.scene, next);
        changed
    }

    /// The widget currently holding keyboard focus, if any (SOUL §6.2).
    pub fn focused_widget(&self) -> Option<scene::WidgetId> {
        schnellui_a11y::focused(&self.scene)
    }

    /// Routes one text-editing key to the **focused** text input (SOUL §6.3) —
    /// the windowed keyboard path, also callable headlessly by tests/agents
    /// (Directive #5). Returns `true` if anything changed (⇒ redraw).
    pub fn dispatch_edit_key(&mut self, key: widgets::EditKey<'_>) -> bool {
        let Some(id) = schnellui_a11y::focused(&self.scene) else {
            return false;
        };
        let changed = schnellui_widgets::dispatch_edit_key(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            key,
        );
        if changed {
            self.refresh_combobox_filter(id);
        }
        changed
    }

    /// Returns the selected text in the focused text input or text area.
    pub fn selected_text(&self) -> Option<String> {
        let id = schnellui_a11y::focused(&self.scene)?;
        schnellui_widgets::selected_text(&self.widgets, &self.scene, id)
    }

    pub(crate) fn selected_text_for(&self, id: scene::WidgetId) -> Option<String> {
        schnellui_widgets::selected_text(&self.widgets, &self.scene, id)
    }

    /// Deletes the active selection in the focused editable. This is the
    /// mutation half of a native Cut operation; callers should first place
    /// [`App::selected_text`] on their platform clipboard.
    pub fn delete_text_selection(&mut self) -> bool {
        let Some(id) = schnellui_a11y::focused(&self.scene) else {
            return false;
        };
        self.delete_text_selection_for(id)
    }

    pub(crate) fn delete_text_selection_for(&mut self, id: scene::WidgetId) -> bool {
        let changed = schnellui_widgets::delete_text_selection(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
        );
        if changed {
            self.refresh_combobox_filter(id);
        }
        changed
    }

    /// Pastes plain text into the focused editable, replacing its selection.
    pub fn paste_text(&mut self, text: &str) -> bool {
        let Some(id) = schnellui_a11y::focused(&self.scene) else {
            return false;
        };
        self.paste_text_for(id, text)
    }

    pub(crate) fn paste_text_for(&mut self, id: scene::WidgetId, text: &str) -> bool {
        let changed = schnellui_widgets::dispatch_paste(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            text,
        );
        if changed {
            self.refresh_combobox_filter(id);
        }
        changed
    }

    pub(crate) fn refresh_combobox_filter(&mut self, id: scene::WidgetId) {
        if schnellui_widgets::refresh_combobox_filter(
            &self.widgets,
            &mut self.scene,
            &mut self.layout,
            &mut self.text,
            &mut self.atlas,
            id,
        ) {
            self.laid_out = false;
        }
    }

    pub(crate) fn select_all_text_for(&mut self, id: scene::WidgetId) -> bool {
        schnellui_widgets::dispatch_edit_key(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            widgets::EditKey::SelectAll,
        )
    }

    /// Opens a widget's configured context menu at a logical window point.
    /// `can_paste` lets a host disable Paste for editable menu sources when its
    /// clipboard has no text.
    pub fn open_context_menu(
        &mut self,
        id: scene::WidgetId,
        position: scene::Point,
        can_paste: bool,
    ) -> bool {
        schnellui_widgets::open_context_menu(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            position,
            self.size,
            self.scale,
            can_paste,
        )
    }

    /// Backward-compatible spelling for opening an editable context menu.
    pub fn open_text_context_menu(
        &mut self,
        id: scene::WidgetId,
        position: scene::Point,
        can_paste: bool,
    ) -> bool {
        self.open_context_menu(id, position, can_paste)
    }

    /// Dismisses the active transient context menu.
    pub fn dismiss_context_menu(&mut self) -> bool {
        schnellui_widgets::dismiss_context_menu(&self.widgets, &mut self.scene)
    }

    /// Whether a transient context menu is currently open.
    pub fn context_menu_is_open(&self) -> bool {
        schnellui_widgets::context_menu_is_open(&self.widgets)
    }

    /// Activates an open context-menu row. Custom callbacks fire here; native
    /// hosts receive built-in clipboard commands in the returned activation.
    pub fn activate_context_menu_item(
        &mut self,
        id: scene::WidgetId,
    ) -> Option<widgets::ContextMenuActivation> {
        schnellui_widgets::activate_context_menu_item(&self.widgets, &mut self.scene, id)
    }

    /// Places the caret (press) or extends the selection (`extend`: drag /
    /// shift-click) on a text input from a pointer position in **logical** window
    /// coordinates (SOUL §6.3). Returns `true` if the caret/selection moved.
    pub fn dispatch_text_pointer(
        &mut self,
        id: scene::WidgetId,
        p: scene::Point,
        extend: bool,
    ) -> bool {
        schnellui_widgets::dispatch_text_pointer(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            p,
            extend,
        )
    }

    /// Applies a semantic pointer-selection gesture to a text input or area.
    /// Native hosts use this for double-click word selection, triple-click line
    /// selection, and unit-preserving drag extension.
    pub fn dispatch_text_pointer_action(
        &mut self,
        id: scene::WidgetId,
        p: scene::Point,
        action: widgets::TextPointerAction,
    ) -> bool {
        schnellui_widgets::dispatch_text_pointer_action(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            p,
            action,
        )
    }

    /// Scrubs the slider under `point`, if any, to the corresponding range value.
    /// This is the headless/testable counterpart of the windowed pointer path.
    pub fn dispatch_slider_pointer(&mut self, id: scene::WidgetId, point: scene::Point) -> bool {
        schnellui_widgets::dispatch_slider_pointer(&self.widgets, &mut self.scene, id, point)
    }

    /// Dispatches a click to a previously resolved widget id.
    pub fn dispatch_click(&mut self, id: scene::WidgetId) -> bool {
        if let Some(source) = schnellui_widgets::context_menu_trigger_source(&self.widgets, id) {
            // Trigger button wants to open its source's menu. Mirror the
            // keyboard ShowContextMenu path: anchor at the source's bottom-left
            // and enable paste for editable sources when the clipboard has text.
            let position = self
                .scene
                .layout(source)
                .map(|layout| scene::Point {
                    x: layout.rect.x,
                    y: layout.rect.y + layout.rect.height,
                })
                .unwrap_or(scene::Point { x: 0.0, y: 0.0 });
            // For trigger-sourced opens we always probe clipboard once (editable
            // Paste gating mirrors the generic Action::ShowContextMenu handler).
            let can_paste = true;
            return self.open_context_menu(source, position, can_paste);
        }
        schnellui_widgets::dispatch_click(&self.widgets, &mut self.scene, id)
    }

    /// Resolves the deepest interactive widget at a logical point.
    pub fn hit_test(&self, point: scene::Point) -> Option<scene::WidgetId> {
        schnellui_widgets::hit_test(&self.widgets, &self.scene, point)
    }

    /// Dispatches a scroll delta to a known retained viewport. This is the
    /// allocation-sensitive direct path for virtualized hosts that already know
    /// their target; it mutates only the scroll property and a11y value, never
    /// layout (SOUL §3.2, §4.1).
    pub fn dispatch_scroll(&mut self, id: scene::WidgetId, delta_y: f32) -> bool {
        schnellui_widgets::dispatch_scroll(&self.widgets, &mut self.scene, id, delta_y)
    }

    /// Resolves the innermost clipped scroll viewport at `point` and dispatches one
    /// wheel delta. If that viewport is at its requested edge, the delta bubbles to
    /// a scroll ancestor. Native hosts use the same route; this headless seam keeps
    /// routing measurable without a window-system event loop.
    pub fn dispatch_wheel_at(&mut self, point: scene::Point, delta_y: f32) -> bool {
        schnellui_widgets::dispatch_wheel_at(&self.widgets, &mut self.scene, point, delta_y)
    }

    /// Returns the next trailing scroll-callback deadline for a native host's wake
    /// schedule. `None` means no debounced scroll notification is pending.
    pub fn next_scroll_callback_deadline(&self) -> Option<Instant> {
        schnellui_widgets::next_scroll_callback_deadline(&self.widgets)
    }

    /// Fires every trailing scroll callback due at `now`. The callback registry is
    /// never borrowed across user code, and the retained callback is moved out then
    /// restored without a per-fire allocation.
    pub fn fire_due_scroll_callbacks_at(&mut self, now: Instant) -> bool {
        schnellui_widgets::fire_due_scroll_callbacks(&self.widgets, now)
    }

    /// Returns the semantic pointer cursor for a logical window position.
    ///
    /// This is window-system independent and therefore usable by custom hosts
    /// and headless interaction tests as well as [`App::run_windowed`].
    pub fn cursor_at(&self, point: scene::Point) -> widgets::CursorIcon {
        let built_in = schnellui_widgets::cursor_at(&self.widgets, &self.scene, point);
        if built_in != widgets::CursorIcon::Default {
            return built_in;
        }
        let Some(hit) = schnellui_widgets::hit_test(&self.widgets, &self.scene, point) else {
            return built_in;
        };
        self.cursor_bindings
            .iter()
            .rev()
            .find(|binding| schnellui_widgets::is_in_subtree(&self.scene, hit, binding.widget))
            .map_or(built_in, |binding| (binding.provider)())
    }

    /// Updates proximity-revealed controls for a logical pointer position.
    /// Returns `true` when their paint changed and a redraw is needed.
    pub fn update_pointer_proximity(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::update_pointer_proximity(&self.widgets, &mut self.scene, point)
    }

    /// Captures or pages an optional scrollbar at `point`.
    pub fn begin_scrollbar_pointer(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::begin_scrollbar_pointer(&self.widgets, &mut self.scene, point)
    }

    /// Drags the currently captured scrollbar thumb.
    pub fn update_scrollbar_pointer(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::update_scrollbar_pointer(&self.widgets, &mut self.scene, point)
    }

    /// Releases a captured scrollbar thumb.
    pub fn end_scrollbar_pointer(&mut self) -> bool {
        schnellui_widgets::end_scrollbar_pointer(&self.widgets)
    }

    /// Whether a scrollbar thumb currently owns the pointer stream.
    pub fn scrollbar_pointer_active(&self) -> bool {
        schnellui_widgets::scrollbar_pointer_active(&self.widgets)
    }

    /// Updates opt-in pointer-edge scrolling for custom hosts.
    pub fn update_edge_auto_scroll(&mut self, point: scene::Point, held: bool) -> bool {
        schnellui_widgets::update_edge_auto_scroll(&self.widgets, &self.scene, point, held)
    }

    /// Applies or clears the theme's active/pressed interaction state without
    /// remounting widgets or changing GPU pipeline state.
    pub fn set_active_interaction(&mut self, target: Option<scene::WidgetId>) -> bool {
        schnellui_widgets::set_active_interaction(&self.widgets, &mut self.scene, target)
    }

    /// Captures a press on a configured content drag source. The source still
    /// behaves like a normal click unless pointer movement crosses the drag
    /// threshold.
    pub fn begin_drag(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::begin_drag(&self.widgets, &self.scene, point)
    }

    /// Advances a captured content drag and its visible drop-target preview.
    pub fn update_drag(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::update_drag(&self.widgets, &mut self.scene, point)
    }

    /// Releases a possible/active content drag.
    pub fn end_drag(&mut self, point: scene::Point) -> widgets::DragRelease {
        schnellui_widgets::end_drag(&self.widgets, &mut self.scene, point)
    }

    /// Captures a pointer press on movable dialog title chrome or an enabled
    /// resize handle.
    pub fn begin_dialog_pointer(&mut self, point: scene::Point) -> bool {
        schnellui_widgets::foreground_dialog_at(&self.widgets, &mut self.scene, point);
        schnellui_widgets::begin_dialog_pointer(&self.widgets, &self.scene, point)
    }

    /// Moves or resizes the currently captured dialog. Geometry is updated in
    /// the retained layout style and resolved on the next frame.
    pub fn update_dialog_pointer(&mut self, point: scene::Point) -> bool {
        let changed = schnellui_widgets::update_dialog_pointer(
            &self.widgets,
            &self.scene,
            &mut self.layout,
            point,
        );
        if changed {
            self.laid_out = false;
        }
        changed
    }

    /// Releases a title-bar/resize-handle pointer capture.
    pub fn end_dialog_pointer(&mut self) -> bool {
        schnellui_widgets::end_dialog_pointer(&self.widgets)
    }

    /// Replaces a text input's whole value — the inbound AccessKit `SetValue`
    /// path (SOUL §6.3): same repaint, accessible-value update, and `on_input`
    /// as typing. Returns `true` if the value changed.
    pub fn set_text_value(&mut self, id: scene::WidgetId, value: &str) -> bool {
        let changed = schnellui_widgets::set_text_value(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            id,
            value,
        );
        if changed {
            self.refresh_combobox_filter(id);
        }
        changed
    }
}

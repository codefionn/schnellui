use super::*;

impl App {
    /// Moves keyboard focus to `target` (or clears it with `None`) — the same
    /// exclusive [`StateFlags::FOCUSED`](a11y::StateFlags::FOCUSED) path an inbound
    /// AccessKit `Focus`/`Blur` action and the windowed Tab/click take (SOUL §6.3).
    /// Returns `true` if focus actually moved (⇒ redraw).
    pub fn focus(&mut self, target: Option<scene::WidgetId>) -> bool {
        let previous = self.focused_input_semantics();
        let dismissed =
            schnellui::widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, target);
        // Focus walking the rows of an open menu keeps it open.
        let context_dismissed =
            !target.is_some_and(|id| {
                schnellui::widgets::context_menu_item(&self.widgets, &self.scene, id)
            }) && schnellui::widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        let raised = target
            .filter(|id| {
                schnellui::widgets::active_modal_panel(&self.scene)
                    .map(|panel| schnellui::widgets::is_in_subtree(&self.scene, *id, panel))
                    .unwrap_or(true)
            })
            .map(|id| schnellui::widgets::foreground_dialog_for_widget(&mut self.scene, id))
            .unwrap_or(false);
        let changed = dismissed
            | context_dismissed
            | raised
            | schnellui::widgets::dispatch_focus(
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
            schnellui::widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, target);
        // Focus walking the rows of an open menu keeps it open.
        let context_dismissed =
            !target.is_some_and(|id| {
                schnellui::widgets::context_menu_item(&self.widgets, &self.scene, id)
            }) && schnellui::widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        let raised = target
            .filter(|id| {
                schnellui::widgets::active_modal_panel(&self.scene)
                    .map(|panel| schnellui::widgets::is_in_subtree(&self.scene, *id, panel))
                    .unwrap_or(true)
            })
            .map(|id| schnellui::widgets::foreground_dialog_for_widget(&mut self.scene, id))
            .unwrap_or(false);
        let changed = dismissed
            | context_dismissed
            | raised
            | schnellui::widgets::dispatch_pointer_focus(
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
        if schnellui::widgets::active_modal_panel(&self.scene).is_some_and(|panel| {
            !schnellui::widgets::is_in_subtree(&self.scene, focus.target, panel)
        }) {
            // A newly opened modal owns focus. Do not resurrect a surviving
            // control behind it merely because that control existed before the
            // structural replacement.
            return false;
        }
        let raised =
            schnellui::widgets::foreground_dialog_for_widget(&mut self.scene, focus.target);
        let focused = schnellui::widgets::dispatch_focus(
            &self.widgets,
            &mut self.scene,
            &mut self.text,
            &mut self.atlas,
            Some(focus.target),
        );
        let modality = schnellui::widgets::set_focus_ring_visible(
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
        // A modal dialog owns the keyboard until it closes. Modeless dialogs
        // deliberately leave the global tab order untouched.
        let modal = schnellui::widgets::active_modal_panel(scene);
        let current = schnellui::a11y::focused(scene);
        let current_group = current.and_then(|cur| roving_group(scene, cur));
        let next = schnellui::a11y::with_tab_order(scene, |order| {
            // A tab bar, radio group, list or table is one Tab stop: Tab
            // enters it at its chosen member (else its first) and leaves it
            // for the next control; arrows move within it.
            // A scroll viewport is a stop only when nothing inside it can take
            // focus, as in browsers: arrows and paging already scroll the
            // viewport around the focused control.
            let is_stop = |id: WidgetId| match roving_group(scene, id) {
                // A viewport with nothing to scroll has nothing for the
                // keyboard to do there either.
                None if scene.node(id).is_some_and(|n| n.kind == WidgetKind::Scroll) => {
                    schnellui::widgets::scroll_metrics(scene, id)
                        .is_some_and(|metrics| metrics.max_offset > 0.0)
                        && !order.iter().any(|&other| {
                            other != id && schnellui::widgets::is_in_subtree(scene, other, id)
                        })
                }
                None => true,
                Some(group) if Some(group) == current_group => false,
                Some(group) => roving_entry(scene, order, group) == Some(id),
            };
            let in_modal = |id: WidgetId| {
                modal.is_none_or(|p| schnellui::widgets::is_in_subtree(scene, id, p))
            };
            let in_scope = |id: &&WidgetId| in_modal(**id) && is_stop(**id);
            let first = || order.iter().find(in_scope).copied();
            let last = || order.iter().rev().find(in_scope).copied();
            // The focused member of a group is not itself a stop; it is
            // still where the walk starts.
            current
                .filter(|&cur| in_modal(cur))
                .and_then(|cur| {
                    let position = order.iter().position(|id| *id == cur)?;
                    if backwards {
                        order[..position]
                            .iter()
                            .rev()
                            .find(in_scope)
                            .copied()
                            .or_else(last)
                    } else {
                        order[position + 1..]
                            .iter()
                            .find(in_scope)
                            .copied()
                            .or_else(first)
                    }
                })
                .or_else(|| if backwards { last() } else { first() })
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
            if let Some(changed) = self.step_context_menu(key) {
                return changed;
            }
            if matches!(key, UiKey::Enter | UiKey::Space { .. }) {
                if let Some(row) = self.focused_context_menu_row() {
                    let Some(activation) = self.activate_context_menu_item(row) else {
                        return false;
                    };
                    if activation.action == widgets::ContextMenuAction::SelectAll {
                        let _ = self.select_all_text_for(activation.source);
                    }
                    return true;
                }
            }
            return self.dismiss_context_menu();
        }
        if self.dispatch_focused_key_handler(key) {
            return true;
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
                    return schnellui::widgets::dismiss_open_dropdowns(
                        &self.widgets,
                        &mut self.scene,
                        None,
                    );
                }
                UiKey::Enter if !expanded => {
                    return schnellui::widgets::dispatch_click(
                        &self.widgets,
                        &mut self.scene,
                        combo,
                    );
                }
                UiKey::Down { shift: false } if expanded => {
                    let popup = self
                        .scene
                        .node(combo)
                        .and_then(|node| node.parent)
                        .and_then(|wrapper| self.scene.node(wrapper))
                        .and_then(|node| node.children.get(1).copied());
                    let option = popup.and_then(|popup| self.first_visible_option(popup));
                    if let Some(option) = option {
                        return self.focus(Some(option));
                    }
                }
                _ => {}
            }
        }
        if key == UiKey::Escape {
            return self.dispatch_escape().unwrap_or(false);
        }
        if let UiKey::Tab { shift } = key {
            return self.focus_step(shift);
        }
        // Native HTML's `:focus-visible` follows input modality. Any real
        // keyboard interaction promotes an existing pointer focus to a visible
        // keyboard focus before routing the key.
        let focus_visible = focused
            .map(|id| {
                schnellui::widgets::dispatch_focus(
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
                if key == UiKey::Enter && focused_kind == Some(WidgetKind::TextInput) {
                    let id = focused.expect("focused text input");
                    if let Some(changed) =
                        schnellui::widgets::dispatch_submit(&self.widgets, &self.scene, id)
                    {
                        return focus_visible | changed;
                    }
                }
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
                        | schnellui::widgets::dispatch_adjust(
                            &self.widgets,
                            &mut self.scene,
                            focused.expect("focused slider"),
                            a,
                        );
                }
            }
            // Arrows walk the members of a tab bar, list or table: focus
            // moves, selection waits for Enter or Space (manual activation).
            Some(WidgetKind::Tab)
            | Some(WidgetKind::ListItem)
            | Some(WidgetKind::TableRow)
            | Some(WidgetKind::DropdownOption) => {
                // Left and Right fold and unfold a tree branch row.
                if let (Some(WidgetKind::Tab), UiKey::Left { .. } | UiKey::Right { .. }) =
                    (focused_kind, key)
                {
                    let expand = matches!(key, UiKey::Right { .. });
                    if let Some(changed) = schnellui::widgets::dispatch_branch_fold(
                        &self.widgets,
                        &mut self.scene,
                        focused.expect("focused tab"),
                        expand,
                    ) {
                        return focus_visible | changed;
                    }
                }
                let step = match key {
                    UiKey::Right { .. } | UiKey::Down { .. } => Some(RovingStep::By(1)),
                    UiKey::Left { .. } | UiKey::Up { .. } => Some(RovingStep::By(-1)),
                    UiKey::Home { .. } => Some(RovingStep::First),
                    UiKey::End { .. } => Some(RovingStep::Last),
                    _ => None,
                };
                if let Some(step) = step {
                    let id = focused.expect("focused group member");
                    if let Some(next) = roving_target(&self.scene, id, step) {
                        return focus_visible | self.focus(Some(next));
                    }
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
            // A menu button opens its menu with focus on the first row, the
            // keyboard half of what a click on it does.
            if schnellui::widgets::context_menu_trigger_source(&self.widgets, f).is_some() {
                let opened = self.dispatch_click(f);
                if opened {
                    self.menu_opened_from_keyboard(f);
                }
                return focus_visible | opened;
            }
            if let Some(changed) =
                schnellui::widgets::dispatch_key_activate(&self.widgets, &mut self.scene, f, k)
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
                .and_then(|f| schnellui::widgets::enclosing_scroll(&self.scene, f))
                .or_else(
                    || match schnellui::widgets::active_modal_panel(&self.scene) {
                        Some(panel) => schnellui::widgets::first_scroll_in(&self.scene, panel),
                        None => schnellui::widgets::first_scroll(&self.scene),
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
            | schnellui::widgets::dispatch_scroll(&self.widgets, &mut self.scene, sv, delta)
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
        changed |= schnellui::widgets::dispatch_click(&self.widgets, &mut self.scene, next);
        changed
    }

    /// Whether the focused widget wears the keyboard focus ring. A focused
    /// terminal counts as not wearing one: keys typed into it are text for
    /// its program, not navigation, so an overlay opened from it starts
    /// without a ring until the first navigation key.
    pub(crate) fn focus_ring_shown(&self) -> bool {
        self.focused_widget().is_some_and(|id| {
            self.scene
                .node(id)
                .is_some_and(|node| node.kind != scene::WidgetKind::TerminalGrid)
                && schnellui::widgets::focus_ring_visible(&self.widgets, id)
        })
    }

    /// Focuses `target` without the keyboard focus ring, as a pointer press
    /// would: for a host that puts focus into an overlay it opened itself.
    /// The first navigation key shows the ring.
    pub fn focus_without_ring(&mut self, target: Option<scene::WidgetId>) -> bool {
        match target {
            Some(target) => self.focus_after_replacement(target, false),
            None => self.focus(None),
        }
    }

    /// Focuses `target` after a structural replacement, with the ring shown
    /// only when `ring_visible`: the state of the focus before the
    /// replacement. Opening an overlay with the pointer or from a terminal
    /// then focuses its first control without a keyboard ring, as native
    /// `:focus-visible` does; Tab or an arrow key brings the ring back.
    pub(crate) fn focus_after_replacement(
        &mut self,
        target: scene::WidgetId,
        ring_visible: bool,
    ) -> bool {
        let changed = self.focus(Some(target));
        changed
            | schnellui::widgets::set_focus_ring_visible(
                &self.widgets,
                &mut self.scene,
                target,
                ring_visible,
            )
    }

    /// The widget currently holding keyboard focus, if any (SOUL §6.2).
    pub fn focused_widget(&self) -> Option<scene::WidgetId> {
        schnellui::a11y::focused(&self.scene)
    }

    /// Routes one text-editing key to the **focused** text input (SOUL §6.3) —
    /// the windowed keyboard path, also callable headlessly by tests/agents
    /// (Directive #5). Returns `true` if anything changed (⇒ redraw).
    pub fn dispatch_edit_key(&mut self, key: widgets::EditKey<'_>) -> bool {
        let Some(id) = schnellui::a11y::focused(&self.scene) else {
            return false;
        };
        let changed = schnellui::widgets::dispatch_edit_key(
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
        let id = schnellui::a11y::focused(&self.scene)?;
        schnellui::widgets::selected_text(&self.widgets, &self.scene, id)
    }

    pub(crate) fn selected_text_for(&self, id: scene::WidgetId) -> Option<String> {
        schnellui::widgets::selected_text(&self.widgets, &self.scene, id)
    }

    /// Deletes the active selection in the focused editable. This is the
    /// mutation half of a native Cut operation; callers should first place
    /// [`App::selected_text`] on their platform clipboard.
    pub fn delete_text_selection(&mut self) -> bool {
        let Some(id) = schnellui::a11y::focused(&self.scene) else {
            return false;
        };
        self.delete_text_selection_for(id)
    }

    pub(crate) fn delete_text_selection_for(&mut self, id: scene::WidgetId) -> bool {
        let changed = schnellui::widgets::delete_text_selection(
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
        let Some(id) = schnellui::a11y::focused(&self.scene) else {
            return false;
        };
        self.paste_text_for(id, text)
    }

    pub(crate) fn paste_text_for(&mut self, id: scene::WidgetId, text: &str) -> bool {
        let changed = schnellui::widgets::dispatch_paste(
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

    /// Where Down lands in an open popup: the option after the chosen one
    /// (the chosen one when it is last), else the first visible option.
    /// Looks through the scroll viewport or padded surface around the rows.
    fn first_visible_option(&self, popup: scene::WidgetId) -> Option<scene::WidgetId> {
        let mut first = None;
        let chosen = self.find_option(popup, &mut first);
        let Some(chosen) = chosen else {
            return first;
        };
        let next = self
            .scene
            .node(chosen)
            .and_then(|node| node.parent)
            .and_then(|parent| self.scene.node(parent))
            .and_then(|parent| {
                let at = parent.children.iter().position(|&c| c == chosen)?;
                parent.children[at + 1..]
                    .iter()
                    .copied()
                    .find(|&c| self.scene.is_effectively_visible(c))
            });
        Some(next.unwrap_or(chosen))
    }

    /// Walks a popup for visible options: returns the selected one and
    /// records the first in `first`.
    fn find_option(
        &self,
        id: scene::WidgetId,
        first: &mut Option<scene::WidgetId>,
    ) -> Option<scene::WidgetId> {
        let node = self.scene.node(id)?;
        for &child in &node.children {
            let option = self
                .scene
                .a11y(child)
                .filter(|a| a11y::Role::from_u16(a.role) == a11y::Role::ListBoxOption);
            match option {
                Some(semantics) => {
                    if !self.scene.is_effectively_visible(child) {
                        continue;
                    }
                    first.get_or_insert(child);
                    if a11y::StateFlags(semantics.state).contains(a11y::StateFlags::SELECTED) {
                        return Some(child);
                    }
                }
                None => {
                    if let Some(found) = self.find_option(child, first) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }

    pub(crate) fn refresh_combobox_filter(&mut self, id: scene::WidgetId) {
        if schnellui::widgets::refresh_combobox_filter(
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
        schnellui::widgets::dispatch_edit_key(
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
        schnellui::widgets::open_context_menu(
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

    /// Dismisses the active transient context menu. Focus on one of its rows
    /// goes back to the control that opened it from the keyboard.
    pub fn dismiss_context_menu(&mut self) -> bool {
        let back = self.menu_focus_return();
        let dismissed = schnellui::widgets::dismiss_context_menu(&self.widgets, &mut self.scene);
        dismissed | back.is_some_and(|id| self.focus(Some(id)))
    }

    /// Escape's ladder, the same from the keyboard path and the window: an
    /// open menu closes first, then any open dropdown or combo list, then the
    /// top dialog. `None` when nothing took it.
    pub(crate) fn dispatch_escape(&mut self) -> Option<bool> {
        if self.context_menu_is_open() {
            return Some(self.dismiss_context_menu());
        }
        if schnellui::widgets::dismiss_open_dropdowns(&self.widgets, &mut self.scene, None) {
            return Some(true);
        }
        schnellui::widgets::dispatch_dialog_escape(&self.widgets, &mut self.scene)
    }

    /// Records `opener` as the control a keyboard-opened menu returns focus
    /// to and moves focus to the menu's first enabled row.
    pub(crate) fn menu_opened_from_keyboard(&mut self, opener: scene::WidgetId) -> bool {
        schnellui::widgets::set_context_menu_opener(&self.widgets, opener);
        let rows = schnellui::widgets::context_menu_enabled_rows(&self.widgets, &self.scene);
        rows.first().is_some_and(|&row| self.focus(Some(row)))
    }

    /// The open menu row that holds focus, if any.
    pub(crate) fn focused_context_menu_row(&self) -> Option<scene::WidgetId> {
        self.focused_widget()
            .filter(|&id| schnellui::widgets::context_menu_item(&self.widgets, &self.scene, id))
    }

    /// Walks focus through the open menu's enabled rows: Down and Up step
    /// and wrap, Home and End jump to the ends. `None` for any other key.
    pub(crate) fn step_context_menu(&mut self, key: UiKey<'_>) -> Option<bool> {
        let rows = schnellui::widgets::context_menu_enabled_rows(&self.widgets, &self.scene);
        let last = rows.len().checked_sub(1)?;
        let at = self
            .focused_context_menu_row()
            .and_then(|id| rows.iter().position(|&row| row == id));
        let next = match key {
            UiKey::Down { .. } => at.map_or(0, |i| if i == last { 0 } else { i + 1 }),
            UiKey::Up { .. } => at.map_or(last, |i| if i == 0 { last } else { i - 1 }),
            UiKey::Home { .. } => 0,
            UiKey::End { .. } => last,
            _ => return None,
        };
        if at.is_none() {
            // A menu opened by the pointer starts its keyboard walk here and
            // returns focus to whatever held it.
            if let Some(opener) = self.focused_widget() {
                schnellui::widgets::set_context_menu_opener(&self.widgets, opener);
            }
        }
        Some(self.focus(Some(rows[next])))
    }

    /// Where focus goes when the open menu closes: the opener, if focus is on
    /// one of the menu's rows and the opener is still mounted.
    fn menu_focus_return(&self) -> Option<scene::WidgetId> {
        self.focused_context_menu_row()?;
        schnellui::widgets::context_menu_opener(&self.widgets)
            .filter(|&id| self.scene.node(id).is_some())
    }

    /// Whether a transient context menu is currently open.
    pub fn context_menu_is_open(&self) -> bool {
        schnellui::widgets::context_menu_is_open(&self.widgets)
    }

    /// Activates an open context-menu row. Custom callbacks fire here; native
    /// hosts receive built-in clipboard commands in the returned activation.
    pub fn activate_context_menu_item(
        &mut self,
        id: scene::WidgetId,
    ) -> Option<widgets::ContextMenuActivation> {
        let back = self.menu_focus_return();
        let activation =
            schnellui::widgets::activate_context_menu_item(&self.widgets, &mut self.scene, id)?;
        if let Some(back) = back {
            let _ = self.focus(Some(back));
        }
        Some(activation)
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
        schnellui::widgets::dispatch_text_pointer(
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
        schnellui::widgets::dispatch_text_pointer_action(
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
        schnellui::widgets::dispatch_slider_pointer(&self.widgets, &mut self.scene, id, point)
    }

    /// Dispatches a click to a previously resolved widget id.
    pub fn dispatch_click(&mut self, id: scene::WidgetId) -> bool {
        if let Some(source) = schnellui::widgets::context_menu_trigger_source(&self.widgets, id) {
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
        schnellui::widgets::dispatch_click(&self.widgets, &mut self.scene, id)
    }

    /// Resolves the deepest interactive widget at a logical point.
    pub fn hit_test(&self, point: scene::Point) -> Option<scene::WidgetId> {
        schnellui::widgets::hit_test(&self.widgets, &self.scene, point)
    }

    /// Dispatches a scroll delta to a known retained viewport. This is the
    /// allocation-sensitive direct path for virtualized hosts that already know
    /// their target; it mutates only the scroll property and a11y value, never
    /// layout (SOUL §3.2, §4.1).
    pub fn dispatch_scroll(&mut self, id: scene::WidgetId, delta_y: f32) -> bool {
        schnellui::widgets::dispatch_scroll(&self.widgets, &mut self.scene, id, delta_y)
    }

    /// Resolves the innermost clipped scroll viewport at `point` and dispatches one
    /// wheel delta. If that viewport is at its requested edge, the delta bubbles to
    /// a scroll ancestor. Native hosts use the same route; this headless seam keeps
    /// routing measurable without a window-system event loop.
    pub fn dispatch_wheel_at(&mut self, point: scene::Point, delta_y: f32) -> bool {
        schnellui::widgets::dispatch_wheel_at(&self.widgets, &mut self.scene, point, delta_y)
    }

    /// Returns the next trailing scroll-callback deadline for a native host's wake
    /// schedule. `None` means no debounced scroll notification is pending.
    pub fn next_scroll_callback_deadline(&self) -> Option<Instant> {
        schnellui::widgets::next_scroll_callback_deadline(&self.widgets)
    }

    /// Fires every trailing scroll callback due at `now`. The callback registry is
    /// never borrowed across user code, and the retained callback is moved out then
    /// restored without a per-fire allocation.
    pub fn fire_due_scroll_callbacks_at(&mut self, now: Instant) -> bool {
        schnellui::widgets::fire_due_scroll_callbacks(&self.widgets, now)
    }

    /// Returns the semantic pointer cursor for a logical window position.
    ///
    /// This is window-system independent and therefore usable by custom hosts
    /// and headless interaction tests as well as [`App::run_windowed`].
    pub fn cursor_at(&self, point: scene::Point) -> widgets::CursorIcon {
        let hit = std::cell::OnceCell::new();
        let resolve = || *hit.get_or_init(|| self.pointer_hit_at(point));
        let built_in =
            schnellui::widgets::cursor_at_with(&self.widgets, &self.scene, point, resolve);
        if built_in != widgets::CursorIcon::Default {
            return built_in;
        }
        let Some(hit) = resolve() else {
            return built_in;
        };
        self.cursor_bindings
            .iter()
            .rev()
            .find(|binding| schnellui::widgets::is_in_subtree(&self.scene, hit, binding.widget))
            .map_or(built_in, |binding| (binding.provider)())
    }

    /// Updates proximity-revealed controls for a logical pointer position.
    /// Returns `true` when their paint changed and a redraw is needed.
    pub fn update_pointer_proximity(&mut self, point: scene::Point) -> bool {
        let hit = schnellui::widgets::hit_test(&self.widgets, &self.scene, point);
        // A pointer that has not moved since the keyboard took over lights
        // nothing; the first move hands hover back to it.
        let held = match self.hover_hold.get() {
            Some(rest) if rest == point => true,
            Some(_) => {
                self.hover_hold.set(None);
                false
            }
            None => false,
        };
        let changed = schnellui::widgets::update_pointer_proximity_held(
            &self.widgets,
            &mut self.scene,
            point,
            if held { None } else { hit },
            held,
        );
        // Hover decoration is paint-only, so the hit still stands afterwards.
        // Remember it with the scene state it is valid for; `cursor_at` for the
        // same pointer position then skips a second walk.
        self.pointer_hit.set(Some(PointerHit {
            point,
            key: self.scene.render_key(),
            paint_epoch: self.scene.paint_epoch(),
            hit,
        }));
        changed
    }

    /// Takes hover away from the resting pointer while the keyboard moves a
    /// highlight, as native lists do: a row the pointer happens to rest on no
    /// longer competes with the row the keys picked. Hover comes back when
    /// the pointer moves. Hosts call this for navigation keys (arrows, Page
    /// Up and Down, Home, End, Tab). Returns whether paint changed.
    pub fn hold_pointer_hover(&mut self) -> bool {
        let Some(rest) = self.pointer_hit.get().map(|hit| hit.point) else {
            return false;
        };
        self.hover_hold.set(Some(rest));
        let tooltips = schnellui::widgets::hide_hover_tooltips(&self.widgets, &mut self.scene);
        schnellui::widgets::strip_hover_decoration(&self.widgets, &mut self.scene) | tooltips
    }

    /// The hit at `point`, reused from the last pointer update when nothing it
    /// depends on has changed since.
    fn pointer_hit_at(&self, point: scene::Point) -> Option<scene::WidgetId> {
        if let Some(cached) = self.pointer_hit.get() {
            if cached.point == point
                && cached.key == self.scene.render_key()
                && cached.paint_epoch == self.scene.paint_epoch()
            {
                return cached.hit;
            }
        }
        schnellui::widgets::hit_test(&self.widgets, &self.scene, point)
    }

    /// Captures or pages an optional scrollbar at `point`.
    pub fn begin_scrollbar_pointer(&mut self, point: scene::Point) -> bool {
        schnellui::widgets::begin_scrollbar_pointer(&self.widgets, &mut self.scene, point)
    }

    /// Drags the currently captured scrollbar thumb.
    pub fn update_scrollbar_pointer(&mut self, point: scene::Point) -> bool {
        schnellui::widgets::update_scrollbar_pointer(&self.widgets, &mut self.scene, point)
    }

    /// Releases a captured scrollbar thumb.
    pub fn end_scrollbar_pointer(&mut self) -> bool {
        schnellui::widgets::end_scrollbar_pointer(&self.widgets)
    }

    /// Whether a scrollbar thumb currently owns the pointer stream.
    pub fn scrollbar_pointer_active(&self) -> bool {
        schnellui::widgets::scrollbar_pointer_active(&self.widgets)
    }

    /// Updates opt-in pointer-edge scrolling for custom hosts.
    pub fn update_edge_auto_scroll(&mut self, point: scene::Point, held: bool) -> bool {
        schnellui::widgets::update_edge_auto_scroll(&self.widgets, &self.scene, point, held)
    }

    /// Applies or clears the theme's active/pressed interaction state without
    /// remounting widgets or changing GPU pipeline state.
    pub fn set_active_interaction(&mut self, target: Option<scene::WidgetId>) -> bool {
        schnellui::widgets::set_active_interaction(&self.widgets, &mut self.scene, target)
    }

    /// Captures a press on a configured content drag source. The source still
    /// behaves like a normal click unless pointer movement crosses the drag
    /// threshold.
    pub fn begin_drag(&mut self, point: scene::Point) -> bool {
        schnellui::widgets::begin_drag(&self.widgets, &self.scene, point)
    }

    /// Advances a captured content drag and its visible drop-target preview.
    pub fn update_drag(&mut self, point: scene::Point) -> bool {
        schnellui::widgets::update_drag(&self.widgets, &mut self.scene, point)
    }

    /// Releases a possible/active content drag.
    pub fn end_drag(&mut self, point: scene::Point) -> widgets::DragRelease {
        schnellui::widgets::end_drag(&self.widgets, &mut self.scene, point)
    }

    /// Captures a pointer press on movable dialog title chrome or an enabled
    /// resize handle.
    pub fn begin_dialog_pointer(&mut self, point: scene::Point) -> bool {
        schnellui::widgets::foreground_dialog_at(&self.widgets, &mut self.scene, point);
        schnellui::widgets::begin_dialog_pointer(&self.widgets, &self.scene, point)
    }

    /// Moves or resizes the currently captured dialog. Geometry is updated in
    /// the retained layout style and resolved on the next frame.
    pub fn update_dialog_pointer(&mut self, point: scene::Point) -> bool {
        let changed = schnellui::widgets::update_dialog_pointer(
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
        schnellui::widgets::end_dialog_pointer(&self.widgets)
    }

    /// Replaces a text input's whole value — the inbound AccessKit `SetValue`
    /// path (SOUL §6.3): same repaint, accessible-value update, and `on_input`
    /// as typing. Returns `true` if the value changed.
    pub fn set_text_value(&mut self, id: scene::WidgetId, value: &str) -> bool {
        let changed = schnellui::widgets::set_text_value(
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

/// A step within a roving group.
#[derive(Clone, Copy)]
enum RovingStep {
    By(i32),
    First,
    Last,
}

/// The group a focusable member belongs to, for widgets that share one Tab
/// stop: tabs in a tab list, radios in a row, items in a list, rows in a
/// table. `None` for every other widget.
fn roving_group(scene: &scene::Scene, id: scene::WidgetId) -> Option<scene::WidgetId> {
    let kind = scene.node(id)?.kind;
    let parent = scene.node(id)?.parent?;
    match kind {
        WidgetKind::Radio
        | WidgetKind::ListItem
        | WidgetKind::TableRow
        | WidgetKind::DropdownOption => Some(parent),
        WidgetKind::Tab => {
            // A closable tab sits in a wrapper row inside the tab list; a
            // grouped list's rows sit in several levels of group and branch
            // columns, all one roving group.
            let mut ancestor = Some(parent);
            while let Some(node) = ancestor {
                if scene
                    .a11y(node)
                    .is_some_and(|a| a11y::Role::from_u16(a.role) == a11y::Role::TabList)
                {
                    return Some(node);
                }
                ancestor = scene.node(node).and_then(|n| n.parent);
            }
            Some(parent)
        }
        _ => None,
    }
}

fn roving_members<'a>(
    scene: &'a scene::Scene,
    order: &'a [scene::WidgetId],
    group: scene::WidgetId,
) -> impl Iterator<Item = scene::WidgetId> + 'a {
    order
        .iter()
        .copied()
        .filter(move |&id| roving_group(scene, id) == Some(group))
}

/// Where Tab enters a group: its selected or checked member, else its first.
fn roving_entry(
    scene: &scene::Scene,
    order: &[scene::WidgetId],
    group: scene::WidgetId,
) -> Option<scene::WidgetId> {
    let chosen = roving_members(scene, order, group).find(|&id| {
        scene.a11y(id).is_some_and(|a| {
            let state = a11y::StateFlags(a.state);
            state.contains(a11y::StateFlags::SELECTED) || state.contains(a11y::StateFlags::CHECKED)
        })
    });
    chosen.or_else(|| roving_members(scene, order, group).next())
}

/// The member an arrow key moves focus to, wrapping at the ends.
fn roving_target(
    scene: &scene::Scene,
    id: scene::WidgetId,
    step: RovingStep,
) -> Option<scene::WidgetId> {
    let group = roving_group(scene, id)?;
    schnellui::a11y::with_tab_order(scene, |order| {
        let members: Vec<_> = roving_members(scene, order, group).collect();
        let position = members.iter().position(|&m| m == id)?;
        let len = members.len() as i32;
        let index = match step {
            RovingStep::By(delta) => (position as i32 + delta).rem_euclid(len),
            RovingStep::First => 0,
            RovingStep::Last => len - 1,
        };
        members.get(index as usize).copied().filter(|&m| m != id)
    })
}

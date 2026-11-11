//! Returning focus to the control that opened a dialog or window once it closes.

use super::*;

impl App {
    /// The opener of the innermost tracked dialog containing `focused`.
    pub(crate) fn modal_opener_of(&self, focused: WidgetId) -> Option<WidgetId> {
        self.modal_openers
            .iter()
            .rev()
            .find(|entry| schnellui::widgets::is_in_subtree(&self.scene, focused, entry.panel))
            .map(|entry| entry.opener)
    }

    /// A control standing in for focus that a press on a window's body took
    /// off every control: the last control of the window the user pressed, once
    /// `gone` says the window is closing. Closing then treats focus as having
    /// been in that window, so it returns to the opener, or else leaves the way
    /// focus in a closing window does. `None` while a control holds focus,
    /// since that focus is the user's own.
    pub(crate) fn pressed_dialog_stand_in(
        &self,
        gone: impl Fn(WidgetId) -> bool,
    ) -> Option<WidgetId> {
        let panel = self.pressed_dialog.filter(|&panel| gone(panel))?;
        if self.focused_widget().is_some() {
            return None;
        }
        schnellui::a11y::with_tab_order(&self.scene, |order| {
            order
                .iter()
                .rev()
                .copied()
                .find(|&id| schnellui::widgets::is_in_subtree(&self.scene, id, panel))
        })
    }

    /// The control a dialog or window that is new in this rebuild (not among
    /// the `known` panels) takes focus at: its first control, content before
    /// the title bar's close button as at mount. HTML `dialog.show()` runs the
    /// dialog focusing steps, so focus that a rebuild took from its control
    /// (removed or disabled) goes there, not to the control's neighbour.
    pub(crate) fn first_control_of_new_window(&self, known: &[WidgetId]) -> Option<WidgetId> {
        let panels = schnellui::widgets::dialog_panels(&self.widgets);
        schnellui::a11y::with_tab_order(&self.scene, |order| {
            panels
                .iter()
                .filter(|panel| !known.contains(panel))
                .find_map(|&panel| {
                    let mut inside = order.iter().copied().filter(|&id| {
                        schnellui::widgets::is_in_subtree(&self.scene, id, panel)
                            && crate::app_inherit::takes_focus(&self.scene, id)
                    });
                    let first = inside.next()?;
                    let content = std::iter::once(first).chain(inside).find(|&id| {
                        !schnellui::widgets::is_dialog_chrome_control(
                            &self.widgets,
                            &self.scene,
                            id,
                        )
                    });
                    Some(content.unwrap_or(first))
                })
        })
    }

    /// Where focus goes when the window or modal that held it closed and had no opener to
    /// return to: the control that last had focus in the front window still open, else the
    /// first tab stop of the page. `windows` lists the open windows, front first, with their
    /// remembered control. Never `None` while a control can take focus.
    pub(crate) fn closed_window_fallback(
        &self,
        windows: &[(WidgetId, Option<WidgetId>)],
    ) -> Option<WidgetId> {
        let remembered = windows.iter().find_map(|&(panel, last)| {
            let last = last?;
            (self.scene.node(last).is_some()
                && schnellui::widgets::is_in_subtree(&self.scene, last, panel)
                && crate::app_inherit::takes_focus(&self.scene, last))
            .then_some(last)
        });
        let stops = |id: &WidgetId| {
            crate::app_inherit::takes_focus(&self.scene, *id)
                && crate::app_inherit::is_tab_stop(&self.scene, *id)
        };
        // The first stop that is no window's control (the one that holds the page); with
        // every control inside a window (an open modal limits the tab order to it), the
        // first of those.
        remembered.or_else(|| {
            schnellui::a11y::with_tab_order(&self.scene, |order| {
                order
                    .iter()
                    .filter(|id| stops(id))
                    .find(|&&id| {
                        schnellui::widgets::enclosing_dialog_panel(&self.widgets, &self.scene, id)
                            .is_none()
                    })
                    .or_else(|| order.iter().find(|id| stops(id)))
                    .copied()
            })
        })
    }

    /// Drops dialogs that left the tree, then records every dialog that is
    /// new, modal or modeless, with `focus_before` as its opener: the control
    /// that held focus when it appeared. `known` lists the panels that were
    /// already open before this remount or replacement; they keep whatever
    /// opener they had, or none when they were open at mount. A focus already
    /// inside the new dialog is no opener. Remount and subtree replacement
    /// time only.
    pub(crate) fn note_opened_modal(&mut self, focus_before: Option<WidgetId>, known: &[WidgetId]) {
        self.modal_openers
            .retain(|entry| self.scene.node(entry.panel).is_some());
        let Some(opener) = focus_before.filter(|&id| self.scene.node(id).is_some()) else {
            return;
        };
        for panel in schnellui::widgets::dialog_panels(&self.widgets) {
            if known.contains(&panel)
                || self.modal_openers.iter().any(|entry| entry.panel == panel)
                || schnellui::widgets::is_in_subtree(&self.scene, opener, panel)
            {
                continue;
            }
            self.modal_openers.push(ModalOpener { panel, opener });
        }
    }
}

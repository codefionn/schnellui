//! Returning focus to the control that opened a dialog or window once it closes.

use super::*;

impl App {
    /// The opener of the innermost tracked dialog containing `focused`.
    pub(crate) fn modal_opener_of(&self, focused: WidgetId) -> Option<(WidgetId, bool)> {
        self.modal_openers
            .iter()
            .rev()
            .find(|entry| schnellui::widgets::is_in_subtree(&self.scene, focused, entry.panel))
            .map(|entry| (entry.opener, entry.ring_visible))
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

    /// Drops dialogs that left the tree, then records every dialog that is
    /// new, modal or modeless, with `focus_before` as its opener: the control
    /// that held focus when it appeared. `known` lists the panels that were
    /// already open before this remount or replacement; they keep whatever
    /// opener they had, or none when they were open at mount. A focus already
    /// inside the new dialog is no opener. Remount and subtree replacement
    /// time only.
    pub(crate) fn note_opened_modal(
        &mut self,
        focus_before: Option<(WidgetId, bool)>,
        known: &[WidgetId],
    ) {
        self.modal_openers
            .retain(|entry| self.scene.node(entry.panel).is_some());
        let Some((opener, ring_visible)) =
            focus_before.filter(|&(id, _)| self.scene.node(id).is_some())
        else {
            return;
        };
        for panel in schnellui::widgets::dialog_panels(&self.widgets) {
            if known.contains(&panel)
                || self.modal_openers.iter().any(|entry| entry.panel == panel)
                || schnellui::widgets::is_in_subtree(&self.scene, opener, panel)
            {
                continue;
            }
            self.modal_openers.push(ModalOpener {
                panel,
                opener,
                ring_visible,
            });
        }
    }
}

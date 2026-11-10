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

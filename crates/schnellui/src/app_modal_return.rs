//! Returning focus to the control that opened a modal dialog once it closes.

use super::*;

impl App {
    /// The opener of the innermost tracked modal containing `focused`.
    pub(crate) fn modal_opener_of(&self, focused: WidgetId) -> Option<(WidgetId, bool)> {
        self.modal_openers
            .iter()
            .rev()
            .find(|entry| schnellui::widgets::is_in_subtree(&self.scene, focused, entry.panel))
            .map(|entry| (entry.opener, entry.ring_visible))
    }

    /// Drops modals that left the tree, then records the top modal when it is
    /// new, with `focus_before` as its opener: the control that held focus
    /// when it appeared. A focus already inside the modal is no opener.
    /// Remount and subtree replacement time only.
    pub(crate) fn note_opened_modal(&mut self, focus_before: Option<(WidgetId, bool)>) {
        self.modal_openers
            .retain(|entry| self.scene.node(entry.panel).is_some());
        let Some(panel) = schnellui::widgets::active_modal_panel(&self.scene) else {
            return;
        };
        if self.modal_openers.iter().any(|entry| entry.panel == panel) {
            return;
        }
        let Some((opener, ring_visible)) = focus_before.filter(|&(id, _)| {
            self.scene.node(id).is_some()
                && !schnellui::widgets::is_in_subtree(&self.scene, id, panel)
        }) else {
            return;
        };
        self.modal_openers.push(ModalOpener {
            panel,
            opener,
            ring_visible,
        });
    }
}

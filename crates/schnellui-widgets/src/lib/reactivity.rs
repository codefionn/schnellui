//! Retained signal bindings for UI-thread-owned dynamic widget producers.
//!
//! The signal crate owns dependency discovery and readiness delivery; this
//! module owns the `!Send` producer closures indirectly through the widget
//! registries.  Keeping that split here means callers continue to use
//! `Text::dynamic`/`RichText::dynamic`/`TerminalGrid::dynamic` without learning
//! any scheduling machinery.

use schnellui_scene::WidgetId;
use schnellui_signal::{Subscription, SubscriptionGroup};
use slotmap::{Key, SecondaryMap};

/// One runtime's bridge from signal readiness keys to retained widget ids.
///
/// This is deliberately a deep, private module: it hides subscription lifetime,
/// key conversion, and scratch-vector reuse behind the small operations the
/// widget registry needs at mount, frame settlement, and subtree disposal.
pub(crate) struct RetainedReactivity {
    group: SubscriptionGroup,
    subscriptions: SecondaryMap<WidgetId, Subscription>,
    ready: Vec<u64>,
}

impl Default for RetainedReactivity {
    fn default() -> Self {
        Self {
            group: SubscriptionGroup::new(),
            subscriptions: SecondaryMap::new(),
            ready: Vec::new(),
        }
    }
}

impl RetainedReactivity {
    /// Registers `id` and returns its subscription. The caller must invoke
    /// [`Subscription::track`] only after dropping the widget-runtime borrow.
    pub(crate) fn subscribe(&mut self, id: WidgetId) -> Subscription {
        let subscription = self.group.subscribe(id.data().as_ffi());
        if let Some(previous) = self.subscriptions.insert(id, subscription.clone()) {
            self.group.unsubscribe(previous);
        }
        subscription
    }

    /// Re-tracks a mounted producer against its existing observer. Dynamic
    /// dependencies are replaced by the signal module; no new subscription is
    /// allocated on ordinary updates.
    pub(crate) fn subscription(&self, id: WidgetId) -> Option<Subscription> {
        self.subscriptions.get(id).cloned()
    }

    /// Takes only signal-ready widget ids.  The caller must return this vector
    /// with [`Self::return_ready`] after running user producers.
    pub(crate) fn take_ready(&mut self) -> Vec<u64> {
        let mut ready = std::mem::take(&mut self.ready);
        self.group.drain_ready_into(&mut ready);
        ready
    }

    pub(crate) fn return_ready(&mut self, mut ready: Vec<u64>) {
        ready.clear();
        self.ready = ready;
    }

    pub(crate) fn forget(&mut self, id: WidgetId) {
        if let Some(subscription) = self.subscriptions.remove(id) {
            self.group.unsubscribe(subscription);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.subscriptions.clear();
        self.ready.clear();
        self.group.clear();
    }
}

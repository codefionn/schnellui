//! Callback-free subscriptions and their per-owner ready queues.

use super::*;

/// Runtime-owned data for callback-free subscriptions. The group owns the node
/// identities and the grow-only ready-key buffer; UI code remains outside it.
pub(crate) const NOT_READY: u32 = u32::MAX;

pub(crate) struct SubscriptionGroupState {
    /// member nodes; each node records its index here (`Node::group_index`) so
    /// removal is an O(1) `swap_remove`.
    pub(crate) subscriptions: SmallVec<[NodeId; 4]>,
    pub(crate) ready: Vec<u64>,
    /// `ready_nodes[i]` is the subscription whose key is `ready[i]`, so a
    /// retrack or removal can `swap_remove` its queued key in O(1).
    pub(crate) ready_nodes: Vec<NodeId>,
}

/// A callback-free owner for UI-local reactive producers.
///
/// A group is intentionally `!Send`: it is designed for a retained UI tree on
/// one thread. The reactive arena stores only dependency edges and integer keys,
/// never the producer closures or other UI-local data.
pub struct SubscriptionGroup {
    pub(crate) id: SubscriptionGroupId,
    pub(crate) _not_send: PhantomData<Rc<()>>,
}

impl Default for SubscriptionGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl SubscriptionGroup {
    /// Creates an empty group with a reusable ready-key queue.
    pub fn new() -> Self {
        let id = RUNTIME
            .lock()
            .subscription_groups
            .insert(SubscriptionGroupState {
                subscriptions: SmallVec::new(),
                ready: Vec::new(),
                ready_nodes: Vec::new(),
            });
        Self {
            id,
            _not_send: PhantomData,
        }
    }

    /// Creates a manual dependency tracker whose changes make `key` ready.
    pub fn subscribe(&self, key: u64) -> Subscription {
        let mut rt = RUNTIME.lock();
        if !rt.subscription_groups.contains_key(self.id) {
            unreachable!("SubscriptionGroup used after drop");
        }
        let id = rt.nodes.insert(Node::subscription(self.id, key));
        let group = rt
            .subscription_groups
            .get_mut(self.id)
            .expect("subscription group disappeared");
        let index = group.subscriptions.len() as u32;
        group.subscriptions.push(id);
        rt.nodes[id].group_index = index;
        Subscription {
            handle: Arc::new(SubscriptionHandle { id, key }),
            _not_send: PhantomData,
        }
    }

    /// Appends ready keys and clears the group's queue. Keys remain coalesced on
    /// their subscriptions until those subscriptions are tracked again.
    pub fn drain_ready_into(&self, out: &mut Vec<u64>) {
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.get_mut(self.id) else {
            return;
        };
        out.extend_from_slice(&group.ready);
        group.ready.clear();
        let nodes = std::mem::take(&mut group.ready_nodes);
        for &id in &nodes {
            if let Some(node) = rt.nodes.get_mut(id) {
                node.ready_index = NOT_READY;
            }
        }
        // Hand the emptied buffer back so its allocation is reused.
        if let Some(group) = rt.subscription_groups.get_mut(self.id) {
            group.ready_nodes = nodes;
            group.ready_nodes.clear();
        }
    }

    /// Invalidates a subscription and every clone of its handle. The group stays
    /// usable and may receive new subscriptions afterwards.
    pub fn unsubscribe(&self, subscription: Subscription) {
        subscription.unsubscribe();
    }

    /// Removes every subscription and pending key while retaining this group's
    /// queue allocation for reuse (for example, after a UI runtime reset).
    pub fn clear(&self) {
        let mut grave = Graveyard::new();
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.get_mut(self.id) else {
            return;
        };
        let subscriptions = std::mem::take(&mut group.subscriptions);
        group.ready.clear();
        group.ready_nodes.clear();
        for id in subscriptions {
            dispose_node(&mut rt, id, &mut grave);
        }
        drop(rt);
        drop(grave);
    }
}

impl Drop for SubscriptionGroup {
    fn drop(&mut self) {
        let mut grave = Graveyard::new();
        let mut rt = RUNTIME.lock();
        let Some(group) = rt.subscription_groups.remove(self.id) else {
            return;
        };
        for id in group.subscriptions {
            dispose_node(&mut rt, id, &mut grave);
        }
        drop(rt);
        drop(grave);
    }
}

/// One manually tracked, callback-free reactive producer in a
/// [`SubscriptionGroup`]. Dropping its final handle removes all dependency edges.
#[derive(Clone)]
pub struct Subscription {
    pub(crate) handle: Arc<SubscriptionHandle>,
    pub(crate) _not_send: PhantomData<Rc<()>>,
}

/// The shared registration behind cloneable [`Subscription`] handles. The last
/// handle cleans up automatically; explicit unsubscription invalidates it for
/// every clone immediately.
pub(crate) struct SubscriptionHandle {
    pub(crate) id: NodeId,
    pub(crate) key: u64,
}

impl Subscription {
    /// Runs `f` with this subscription as the dependency observer. Old edges are
    /// replaced before the call, and the ready/coalesced state is consumed. `f`
    /// is unconstrained (`!Send` is fine) and always executes without the runtime
    /// lock held.
    pub fn track<R>(&self, f: impl FnOnce() -> R) -> R {
        let mut grave = Graveyard::new();
        // A nested track on a subscription that is already running joins the
        // outer run: restarting would reset the cursor and sweep the edges the
        // outer run has read so far.
        let nested;
        let previous = {
            let mut rt = RUNTIME.lock();
            let valid = rt
                .nodes
                .get(self.handle.id)
                .is_some_and(|node| node.subscription.is_some());
            nested = valid && rt.nodes[self.handle.id].flags.contains(NodeFlags::RUNNING);
            if nested {
                Some(replace_current_observer(&mut rt, Some(self.handle.id)))
            } else if valid {
                clear_subscription_ready(&mut rt, self.handle.id, self.handle.key);
                // Nodes the previous run created are owned by this subscription;
                // dispose them so each run starts fresh. Their values drop after
                // the run, outside the lock and with no observer installed.
                dispose_children(&mut rt, self.handle.id, &mut grave);
                // The old edges stay in place: `track_read` confirms the ones this
                // run reads again and `end_run` sweeps the rest.
                begin_run(&mut rt, self.handle.id);
                let node = rt
                    .nodes
                    .get_mut(self.handle.id)
                    .expect("subscription disappeared");
                node.flags.set_color(NodeFlags::CLEAN);
                node.flags.insert(NodeFlags::RUNNING);
                let previous = replace_current_observer(&mut rt, Some(self.handle.id));
                Some(previous)
            } else {
                None
            }
        };

        // The runtime lock is deliberately absent while user/UI code executes.
        // Catch only long enough to restore graph context, then resume the same
        // panic so callers observe ordinary Rust unwind behavior.
        let result = catch_unwind(AssertUnwindSafe(f));

        if let Some(previous) = previous {
            let mut rt = RUNTIME.lock();
            if !nested {
                if let Some(node) = rt.nodes.get_mut(self.handle.id) {
                    node.flags.remove(NodeFlags::RUNNING);
                    node.flags.set_color(NodeFlags::CLEAN);
                }
                end_run(&mut rt, self.handle.id);
            }
            replace_current_observer(&mut rt, previous);
        }
        let dropped = catch_unwind(AssertUnwindSafe(|| drop_graveyard(grave)));
        match (result, dropped) {
            (Ok(value), Ok(())) => value,
            (Err(payload), _) | (_, Err(payload)) => resume_unwind(payload),
        }
    }

    /// Removes dependency edges and any undrained ready key while retaining this
    /// subscription for a future [`track`](Self::track).
    pub fn clear(&self) {
        let mut rt = RUNTIME.lock();
        if rt.nodes.contains_key(self.handle.id) {
            clear_subscription_ready(&mut rt, self.handle.id, self.handle.key);
            unlink_all_sources(&mut rt, self.handle.id);
            if let Some(node) = rt.nodes.get_mut(self.handle.id) {
                node.flags.set_color(NodeFlags::CLEAN);
            }
        }
    }

    /// Immediately removes this subscription from its group and from the graph.
    pub fn unsubscribe(&self) {
        let mut grave = Graveyard::new();
        let mut rt = RUNTIME.lock();
        remove_subscription(&mut rt, self.handle.id, self.handle.key, &mut grave);
        drop(rt);
        drop(grave);
    }
}

impl Drop for SubscriptionHandle {
    fn drop(&mut self) {
        let mut grave = Graveyard::new();
        let mut rt = RUNTIME.lock();
        remove_subscription(&mut rt, self.id, self.key, &mut grave);
        drop(rt);
        drop(grave);
    }
}

/// Clears one subscription's notification bit and removes its key from the
/// group's undrained queue. [`RetainedReactivity`]-style callers keep exactly one
/// subscription per key, so no sibling can still represent the key.
pub(crate) fn clear_subscription_ready(rt: &mut Runtime, id: NodeId, key: u64) {
    let Some(node) = rt.nodes.get_mut(id) else {
        return;
    };
    let Some(group_id) = node.subscription else {
        return;
    };
    if !node.flags.contains(NodeFlags::NOTIFIED) {
        return;
    }
    node.flags.remove(NodeFlags::NOTIFIED);
    let index = std::mem::replace(&mut node.ready_index, NOT_READY);
    if index == NOT_READY {
        return; // already drained
    }
    let index = index as usize;
    if let Some(group) = rt.subscription_groups.get_mut(group_id) {
        debug_assert_eq!(group.ready.get(index), Some(&key));
        #[cfg(test)]
        test_work::add(1);
        group.ready.swap_remove(index);
        group.ready_nodes.swap_remove(index);
        if let Some(&moved) = group.ready_nodes.get(index) {
            if let Some(node) = rt.nodes.get_mut(moved) {
                node.ready_index = index as u32;
            }
        }
    }
}

/// Marks a settled manual subscription ready. The flag is set even after its key
/// has been drained, which suppresses duplicate work until the producer retracks;
/// the flag is also what keeps `ready` free of duplicate keys.
pub(crate) fn queue_subscription_ready(rt: &mut Runtime, id: NodeId) {
    let Some((group_id, key, already_notified)) = rt.nodes.get(id).and_then(|node| {
        node.subscription
            .zip(node.subscription_key)
            .map(|(group, key)| (group, key, node.flags.contains(NodeFlags::NOTIFIED)))
    }) else {
        return;
    };
    if already_notified {
        return;
    }
    if let Some(group) = rt.subscription_groups.get_mut(group_id) {
        let index = group.ready.len() as u32;
        group.ready.push(key);
        group.ready_nodes.push(id);
        if let Some(node) = rt.nodes.get_mut(id) {
            node.flags.insert(NodeFlags::NOTIFIED);
            node.ready_index = index;
        }
    }
}

/// Removes a subscription's graph node and ownership entry. This is idempotent
/// so explicit unsubscribe, dropping, and group teardown compose safely.
pub(crate) fn remove_subscription(rt: &mut Runtime, id: NodeId, key: u64, grave: &mut Graveyard) {
    let Some((group_id, index)) = rt
        .nodes
        .get(id)
        .and_then(|node| node.subscription.map(|group| (group, node.group_index)))
    else {
        return;
    };
    clear_subscription_ready(rt, id, key);
    if let Some(group) = rt.subscription_groups.get_mut(group_id) {
        let index = index as usize;
        if group.subscriptions.get(index) == Some(&id) {
            #[cfg(test)]
            test_work::add(1);
            group.subscriptions.swap_remove(index);
            if let Some(&moved) = group.subscriptions.get(index) {
                if let Some(node) = rt.nodes.get_mut(moved) {
                    node.group_index = index as u32;
                }
            }
        }
    }
    dispose_node(rt, id, grave);
}

/// Test-only count of queue and member-list elements moved or visited by subscription-group
/// bookkeeping, per thread so parallel tests do not disturb each other.
#[cfg(test)]
pub(crate) mod test_work {
    use std::cell::Cell;
    thread_local!(static WORK: Cell<usize> = const { Cell::new(0) });
    pub(crate) fn add(n: usize) {
        WORK.with(|w| w.set(w.get() + n));
    }
    pub(crate) fn take() -> usize {
        WORK.with(|w| w.replace(0))
    }
}

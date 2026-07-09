//! The owner tree: scopes, attachment of new nodes, and disposal.

use super::*;

/// An ownership scope (owner tree, SOUL §3.1). Disposing it removes every node it
/// owns from the arena; subsequent access returns `None` via `try_*`.
#[derive(Clone, Copy)]
pub struct Scope {
    pub(crate) id: NodeId,
}

impl Scope {
    /// The underlying arena key.
    #[inline]
    pub fn id(self) -> NodeId {
        self.id
    }

    /// Disposes this scope and every node it owns (SOUL §3.1).
    pub fn dispose(self) {
        dispose(self);
    }
}

/// Opens a new ownership scope as a child of the current one (SOUL §3.1).
pub fn create_scope() -> Scope {
    let mut rt = RUNTIME.lock();
    let id = rt.nodes.insert(Node::signal(Box::new(()), None));
    rt.scopes.insert(id, SmallVec::new());
    attach_to_current_scope(&mut rt, id);
    Scope { id }
}

/// Runs `f` with `scope` installed as the current ownership scope, so signals,
/// memos and effects created inside `f` are owned by `scope` and torn down when it
/// is disposed (SOUL §3.1). The previous scope is restored afterwards.
pub fn run_in_scope<R>(scope: Scope, f: impl FnOnce() -> R) -> R {
    let prev = {
        let mut rt = RUNTIME.lock();
        replace_current_scope(&mut rt, Some(scope.id))
    };
    // Scope restoration is part of the runtime invariant: a panic from user
    // code must not leave this thread creating future nodes under a stale owner.
    let result = catch_unwind(AssertUnwindSafe(f));
    let mut rt = RUNTIME.lock();
    replace_current_scope(&mut rt, prev);
    drop(rt);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

/// Disposes a scope and every node it owns (recursively),
/// unsubscribing their graph edges, and removing them from the arena (SOUL §3.1).
/// Disposal is a teardown event and may allocate (§4).
pub fn dispose(scope: Scope) {
    let mut grave = Graveyard::new();
    {
        let mut rt = RUNTIME.lock();
        dispose_node(&mut rt, scope.id, &mut grave);
    }
    drop(grave);
}

/// Attaches a freshly created node to the current owner (effect or scope), if any
/// (§3.1). The owner's child list is created on demand.
pub(crate) fn attach_to_current_scope(rt: &mut Runtime, id: NodeId) {
    let Some(owner) = current_observer(rt).or_else(|| current_scope(rt)) else {
        return;
    };
    if owner == id {
        return;
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.owner = Some(owner);
    }
    match rt.scopes.get_mut(owner) {
        Some(list) => list.push(id),
        None => {
            let mut list: SmallVec<[NodeId; 4]> = SmallVec::new();
            list.push(id);
            rt.scopes.insert(owner, list);
        }
    }
}

/// Values and compute closures removed from the arena. User `Drop` impls may
/// re-enter the runtime (a captured scope owner disposes its scope), so they are
/// dropped only after the arena lock is released.
pub(crate) type Graveyard = Vec<(Option<AnyCell>, Option<ComputeFn>)>;

/// Drops removed values with no observer installed on this thread, so a `Drop`
/// that reads a signal cannot become a dependency of whatever is tracking. The
/// observer is restored even if a `Drop` panics, and the panic then continues.
pub(crate) fn drop_graveyard(grave: Graveyard) {
    if grave.is_empty() {
        return;
    }
    let previous = replace_current_observer(&mut RUNTIME.lock(), None);
    let result = catch_unwind(AssertUnwindSafe(|| drop(grave)));
    replace_current_observer(&mut RUNTIME.lock(), previous);
    if let Err(payload) = result {
        resume_unwind(payload);
    }
}

/// Disposes every node `id` owns, leaving `id` itself and its (emptied) child list
/// in place. Removed nodes go to `grave`.
pub(crate) fn dispose_children(rt: &mut Runtime, id: NodeId, grave: &mut Graveyard) {
    dispose_tree(rt, id, false, grave);
}

/// Disposes `id` and everything it owns, unsubscribing graph edges, unlinking it
/// from its owner and removing the nodes from the arena (§3.1). The caller drops
/// `grave` after releasing the lock.
pub(crate) fn dispose_node(rt: &mut Runtime, id: NodeId, grave: &mut Graveyard) {
    dispose_tree(rt, id, true, grave);
}

/// Iterative owner-tree teardown over a pooled work stack, so a deeply nested
/// scope tree cannot overflow the call stack. A node's child list is taken whole
/// before the node is removed, so children never scan their parent's list.
pub(crate) fn dispose_tree(
    rt: &mut Runtime,
    root: NodeId,
    include_root: bool,
    grave: &mut Graveyard,
) {
    let mut stack = std::mem::take(&mut rt.dispose_stack);
    stack.clear();
    if include_root {
        stack.push(root);
    } else if let Some(list) = rt.scopes.get_mut(root) {
        stack.extend(list.drain(..));
    }
    while let Some(id) = stack.pop() {
        if let Some(list) = rt.scopes.remove(id) {
            stack.extend(list);
        }
        remove_node(rt, id, grave);
    }
    rt.dispose_stack = stack;
}

// Test-only count of owner-list positions inspected while unlinking disposed
// nodes from their owner.
#[cfg(test)]
thread_local! {
    pub(crate) static OWNER_SCAN_STEPS: Cell<usize> = const { Cell::new(0) };
}

/// Detaches one node (children already queued separately): severs its graph
/// edges, unlinks it from its owner and removes it from the arena. The pending
/// queues are left alone: the id goes stale and the next drain skips it.
pub(crate) fn remove_node(rt: &mut Runtime, id: NodeId, grave: &mut Graveyard) {
    if !rt.nodes.contains_key(id) {
        return;
    }
    // Detach from the graph so dangling ids never re-mark or re-run this node.
    unlink_all_sources(rt, id);
    while let Some(edge) = rt.nodes.get(id).and_then(|n| n.observers.last().copied()) {
        let before = rt.nodes[id].observers.len();
        unlink_edge(rt, edge.node, edge.back as usize);
        if rt.nodes[id].observers.len() == before {
            // The observer vanished without unlinking; drop the stale entry.
            rt.nodes[id].observers.pop();
        }
    }
    // Unlink from the owner's child list so it does not grow forever.
    let owner = rt.nodes[id].owner;
    if let Some(list) = owner.and_then(|o| rt.scopes.get_mut(o)) {
        if let Some(pos) = list.iter().rposition(|&x| x == id) {
            #[cfg(test)]
            OWNER_SCAN_STEPS.with(|steps| steps.set(steps.get() + list.len() - pos));
            list.swap_remove(pos);
        }
    }
    if let Some(node) = rt.nodes.remove(id) {
        // Nodes without user-owned payload (subscriptions) drop in place, which
        // keeps their teardown allocation-free.
        if node.value.is_some() || node.compute.is_some() {
            grave.push((node.value, node.compute));
        }
    }
}

//! The arena: node storage, coloring flags, dependency edges, push propagation
//! and the pull/recompute pass (SOUL §3.1).

use super::*;

/// Node coloring, packed into one byte of bitflags — never several bools
/// (SOUL §3.1). `Clean`/`Check`/`Dirty` are mutually exclusive *color* states;
/// the high bits carry orthogonal status.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct NodeFlags(u8);

impl NodeFlags {
    /// value valid, nothing to do.
    pub const CLEAN: NodeFlags = NodeFlags(0b0000_0000);
    /// a *transitive* ancestor changed; must verify sources before trusting cache.
    pub const CHECK: NodeFlags = NodeFlags(0b0000_0001);
    /// a *direct* source changed; must recompute.
    pub const DIRTY: NodeFlags = NodeFlags(0b0000_0010);
    /// mask over the color bits (`CLEAN`/`CHECK`/`DIRTY`).
    pub const COLOR_MASK: NodeFlags = NodeFlags(0b0000_0011);
    /// this node subscribes to reads while its compute runs. In this runtime the
    /// flag doubles as the *effect* (sink) marker: effects carry it and are queued
    /// on push; memos track deps the same way but omit it so they stay pull-only.
    pub const TRACKING: NodeFlags = NodeFlags(0b0000_0100);
    /// currently executing (cycle guard).
    pub const RUNNING: NodeFlags = NodeFlags(0b0001_0000);
    /// A manual subscription has already put its key in its group's ready queue.
    /// It stays coalesced until the producer is tracked again.
    pub(crate) const NOTIFIED: NodeFlags = NodeFlags(0b0010_0000);
    /// The node sits in `pending_effects` / `pending_subscriptions`. Set on
    /// enqueue, cleared when a flush drains the queue, so enqueueing is an O(1)
    /// flag test instead of a scan of the queue.
    pub(crate) const QUEUED: NodeFlags = NodeFlags(0b0100_0000);

    /// Returns the color component (`CLEAN`/`CHECK`/`DIRTY`).
    #[inline]
    pub fn color(self) -> NodeFlags {
        NodeFlags(self.0 & Self::COLOR_MASK.0)
    }

    /// Replaces the color component, leaving status bits untouched.
    #[inline]
    pub fn set_color(&mut self, color: NodeFlags) {
        self.0 = (self.0 & !Self::COLOR_MASK.0) | (color.0 & Self::COLOR_MASK.0);
    }

    /// `true` if every bit in `flag` is set.
    #[inline]
    pub fn contains(self, flag: NodeFlags) -> bool {
        (self.0 & flag.0) == flag.0
    }

    /// Sets every bit in `flag`.
    #[inline]
    pub fn insert(&mut self, flag: NodeFlags) {
        self.0 |= flag.0;
    }

    /// Clears every bit in `flag`.
    #[inline]
    pub fn remove(&mut self, flag: NodeFlags) {
        self.0 &= !flag.0;
    }
}

/// Type-erased boxed cell; `Send + Sync` so the runtime stays `Send` (§3.1).
pub(crate) type AnyCell = Box<dyn Any + Send + Sync>;
/// Opt-in equality gate (`None` = the compute decides change) (SOUL §3.1).
pub(crate) type EqualsFn = fn(&dyn Any, &dyn Any) -> bool;
/// A memo/effect compute: writes into the cell, returns whether it changed.
pub(crate) type ComputeFn = Box<dyn FnMut(&mut dyn Any) -> bool + Send>;

/// One dependency edge as seen from one end. `back` is the index of the mirror
/// entry in the peer's list: for `a.sources[i] = Edge { node: b, back: j }` the
/// mirror is `b.observers[j] = Edge { node: a, back: i }`. That makes unlinking an
/// O(1) `swap_remove` plus one patched back index, never a search.
#[derive(Clone, Copy)]
pub(crate) struct Edge {
    pub(crate) node: NodeId,
    pub(crate) back: u32,
}

/// One arena node (SOUL §3.1). Signals have `compute == None`; memos/effects own one.
pub(crate) struct Node {
    pub(crate) value: Option<AnyCell>,
    pub(crate) compute: Option<ComputeFn>,
    /// inline the common single-dep case (Sycamore's trick).
    pub(crate) sources: SmallVec<[Edge; 1]>,
    pub(crate) observers: SmallVec<[Edge; 2]>,
    /// bumped only on a real value change.
    pub(crate) version: u64,
    pub(crate) flags: NodeFlags,
    pub(crate) equals: Option<EqualsFn>,
    /// owner scope, for tree disposal.
    pub(crate) owner: Option<NodeId>,
    /// `Some` for a callback-free, UI-local manual subscription. Its key and
    /// ready queue live in [`Runtime`], so this arena node never holds UI data.
    pub(crate) subscription: Option<SubscriptionGroupId>,
    pub(crate) subscription_key: Option<u64>,
    /// index of this subscription in its group's member list.
    pub(crate) group_index: u32,
    /// index of this subscription's key in its group's `ready` queue, or
    /// `NOT_READY` when it is not queued.
    pub(crate) ready_index: u32,
    /// Id of the tracking run in progress (0 when idle). Unique per run.
    pub(crate) run: u64,
    /// During a run, `sources[..cursor]` were read in this run; the rest are last
    /// run's edges not yet confirmed (swept when the run ends).
    pub(crate) cursor: u32,
    /// Dedupe hint: the last run that linked this node as a source, and the slot
    /// it used in that observer's source list. Only an optimisation; a stale or
    /// overwritten mark costs at worst one extra, still consistent, edge.
    pub(crate) mark_observer: NodeId,
    pub(crate) mark_run: u64,
    pub(crate) mark_slot: u32,
}

impl Node {
    pub(crate) fn blank() -> Node {
        Node {
            value: None,
            compute: None,
            sources: SmallVec::new(),
            observers: SmallVec::new(),
            version: 0,
            flags: NodeFlags::default(),
            equals: None,
            owner: None,
            subscription: None,
            subscription_key: None,
            group_index: 0,
            ready_index: NOT_READY,
            run: 0,
            cursor: 0,
            mark_observer: NodeId::default(),
            mark_run: 0,
            mark_slot: 0,
        }
    }

    pub(crate) fn signal(value: AnyCell, equals: Option<EqualsFn>) -> Node {
        Node {
            value: Some(value),
            equals,
            ..Node::blank()
        }
    }

    /// A derived compute node (memo/effect). Born `DIRTY` so the first pull runs
    /// it. Effects additionally carry `TRACKING` (the sink marker, §3.1).
    pub(crate) fn compute_node(
        value: AnyCell,
        compute: ComputeFn,
        equals: Option<EqualsFn>,
        tracking: bool,
    ) -> Node {
        let mut flags = NodeFlags::default();
        flags.set_color(NodeFlags::DIRTY);
        if tracking {
            flags.insert(NodeFlags::TRACKING);
        }
        Node {
            value: Some(value),
            compute: Some(compute),
            flags,
            equals,
            ..Node::blank()
        }
    }

    pub(crate) fn subscription(group: SubscriptionGroupId, key: u64) -> Node {
        Node {
            subscription: Some(group),
            subscription_key: Some(key),
            ..Node::blank()
        }
    }
}

/// The process-global reactive arena (SOUL §3.1). Access it through the free
/// functions and handle methods; the single lock is acquired per operation and
/// **never** held across user code.
pub struct Runtime {
    pub(crate) nodes: SlotMap<NodeId, Node>,
    /// scope/effect → nodes it owns, for O(scope) disposal.
    pub(crate) scopes: SecondaryMap<NodeId, SmallVec<[NodeId; 4]>>,
    /// effects queued by the push phase, drained by [`Runtime::flush`]. Retains
    /// capacity across frames (grow-only pool) — zero alloc in steady state. A
    /// node is listed at most once (`QUEUED`); ids of nodes disposed while queued
    /// stay until the next drain and are skipped there (keys are generational).
    pub(crate) pending_effects: Vec<NodeId>,
    /// Manual subscriptions awaiting pull/settling at the next flush.
    pub(crate) pending_subscriptions: Vec<NodeId>,
    /// double-buffer for [`Runtime::flush`]: the queued effects are drained here
    /// so re-queues during the flush land in a fresh `pending_effects` without a
    /// realloc. Grow-only, cleared-and-refilled.
    pub(crate) flush_scratch: Vec<NodeId>,
    pub(crate) subscription_flush_scratch: Vec<NodeId>,
    pub(crate) subscription_groups: SlotMap<SubscriptionGroupId, SubscriptionGroupState>,
    /// pooled work stack for the push propagation walk. Grow-only, `mem::take`n
    /// out per call and put back — zero alloc once warm.
    pub(crate) propagate_stack: Vec<(NodeId, bool)>,
    /// pooled explicit stacks for `update_if_necessary`. A pull can nest (user
    /// code reads a memo that is not settled yet), so this is a pool, not one
    /// buffer; each call takes one and returns it.
    pub(crate) pull_pool: Vec<Vec<PullFrame>>,
    /// pooled work stack for iterative tree disposal.
    pub(crate) dispose_stack: Vec<NodeId>,
    /// source of unique tracking-run ids (never 0).
    pub(crate) run_counter: u64,
    /// flushes in progress (they hold drained work outside the queues).
    pub(crate) flushers: u32,
    /// The node whose compute is currently running on each participating thread,
    /// for automatic dependency tracking (§3.3). Context remains runtime-owned
    /// rather than thread-local, but parallel producers cannot steal one another's
    /// observer while user code runs outside the arena lock.
    pub(crate) current_observers: SmallVec<[(u64, NodeId); 4]>,
    /// Ownership scope installed for each participating thread. Like observer
    /// tracking, scopes are runtime-owned but must be thread-local in effect:
    /// user code runs outside the arena lock, so one app/test must never attach
    /// newly created nodes under another thread's scope.
    pub(crate) current_scopes: SmallVec<[(u64, NodeId); 4]>,
    /// O(1) "did anything change anywhere" gate (§3.1).
    pub(crate) global_version: u64,
}

pub(crate) static RUNTIME: Lazy<Mutex<Runtime>> = Lazy::new(|| {
    Mutex::new(Runtime {
        nodes: SlotMap::with_key(),
        scopes: SecondaryMap::new(),
        pending_effects: Vec::new(),
        pending_subscriptions: Vec::new(),
        flush_scratch: Vec::new(),
        subscription_flush_scratch: Vec::new(),
        subscription_groups: SlotMap::with_key(),
        propagate_stack: Vec::new(),
        pull_pool: Vec::new(),
        dispose_stack: Vec::new(),
        run_counter: 0,
        flushers: 0,
        current_observers: SmallVec::new(),
        current_scopes: SmallVec::new(),
        global_version: 0,
    })
});

/// Serializes complete flushes without keeping the arena lock across user code.
/// A second app/thread must not observe the pending queues as empty while the
/// first flusher still owns their drained work in local scratch buffers. It is
/// reentrant so an effect may settle newly queued work on the same thread.
pub(crate) static FLUSH_LOCK: Lazy<ReentrantMutex<()>> = Lazy::new(|| ReentrantMutex::new(()));

/// Most drain rounds one [`Runtime::flush`] runs before it assumes effects are
/// feeding each other forever. Legitimate effect chains are a handful deep.
pub(crate) const MAX_FLUSH_ROUNDS: usize = 1000;
pub(crate) static FLUSH_CYCLE_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Counts a flush in progress so concurrent callers cannot take the "nothing
/// queued" shortcut while another flusher still holds drained work.
pub(crate) struct FlushGuard;

impl FlushGuard {
    pub(crate) fn enter() -> FlushGuard {
        RUNTIME.lock().flushers += 1;
        FlushGuard
    }
}

impl Drop for FlushGuard {
    fn drop(&mut self) {
        RUNTIME.lock().flushers -= 1;
    }
}

impl Runtime {
    /// Runs the **pull** phase: drains the queued-effect set and re-runs each dirty
    /// effect via `update_if_necessary`, settling the graph glitch-free (SOUL §3.1).
    /// Called at every frame boundary by the umbrella `App::frame` (§8.1).
    ///
    /// Buffer discipline (§4): the queued effects are drained into a persistent
    /// scratch (both buffers retain capacity), so an effect that re-queues another
    /// effect mid-flush does so without a heap allocation once warm.
    ///
    /// A flush keeps draining until the queues are empty, because effects may
    /// write signals that queue more effects. It gives up after
    /// [`MAX_FLUSH_ROUNDS`] (1000) drain rounds, which means effects are feeding
    /// each other forever (a reactive cycle). In debug builds that is a bug and
    /// panics with the queued node ids after clearing the queues. In release
    /// builds the flush logs the cycle once to stderr and returns with the
    /// remaining work still queued, so the next flush continues it; the frame
    /// loop keeps running instead of hanging or aborting.
    pub fn flush() {
        // O(1) exit when nothing changed since the last flush: both queues are
        // empty and no other flusher holds drained work. Skips the flush lock.
        {
            let rt = RUNTIME.lock();
            if rt.pending_effects.is_empty()
                && rt.pending_subscriptions.is_empty()
                && rt.flushers == 0
            {
                return;
            }
        }
        let _flush = FLUSH_LOCK.lock();
        let _in_flight = FlushGuard::enter();
        let mut rounds = 0usize;
        loop {
            let (mut scratch, mut subscription_scratch) = {
                let mut rt = RUNTIME.lock();
                if rt.pending_effects.is_empty() && rt.pending_subscriptions.is_empty() {
                    break;
                }
                rounds += 1;
                if rounds > MAX_FLUSH_ROUNDS {
                    // Effects keep re-queueing each other: a reactive cycle.
                    let nodes = format!(
                        "effects {:?}, subscriptions {:?}",
                        rt.pending_effects, rt.pending_subscriptions
                    );
                    if cfg!(debug_assertions) {
                        clear_queues(&mut rt);
                        drop(rt);
                        panic!(
                            "reactive cycle: flush still has queued work after \
                             {MAX_FLUSH_ROUNDS} rounds ({nodes})"
                        );
                    }
                    if !FLUSH_CYCLE_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                        eprintln!(
                            "schnellui::signal: reactive cycle, flush stopped after \
                             {MAX_FLUSH_ROUNDS} rounds ({nodes})"
                        );
                    }
                    break;
                }
                // Take the scratch pool and refill it from the drained queue. A
                // same-thread nested flush simply borrows a fresh empty buffer.
                let mut scratch = std::mem::take(&mut rt.flush_scratch);
                scratch.clear();
                scratch.append(&mut rt.pending_effects);
                let mut subscription_scratch = std::mem::take(&mut rt.subscription_flush_scratch);
                subscription_scratch.clear();
                subscription_scratch.append(&mut rt.pending_subscriptions);
                // Drained nodes may be queued again by the work below.
                for &id in scratch.iter().chain(subscription_scratch.iter()) {
                    if let Some(node) = rt.nodes.get_mut(id) {
                        node.flags.remove(NodeFlags::QUEUED);
                    }
                }
                (scratch, subscription_scratch)
            };
            for &eff in &scratch {
                update_if_necessary(eff);
            }
            // Subscription nodes have no callback. Pulling them settles memo
            // chains first, then records a ready key only when their source did
            // actually change.
            for &subscription in &subscription_scratch {
                update_if_necessary(subscription);
            }
            scratch.clear();
            subscription_scratch.clear();
            let mut rt = RUNTIME.lock();
            rt.flush_scratch = scratch;
            rt.subscription_flush_scratch = subscription_scratch;
        }
    }

    /// O(1) gate: the monotonic global version, bumped on every real value change.
    pub fn global_version() -> u64 {
        RUNTIME.lock().global_version
    }

    /// Number of live nodes in the arena (diagnostics/tests).
    pub fn node_count() -> usize {
        RUNTIME.lock().nodes.len()
    }
}

/// Runs `f` without the calling thread's current reactive observer.
///
/// Reads inside `f` do not become dependencies of the enclosing memo, effect,
/// or manual subscription. The prior observer is restored even if `f` panics.
/// Prefer [`Signal::peek`] or [`Memo::peek`] for one untracked closure read.
pub fn untrack<R>(f: impl FnOnce() -> R) -> R {
    let previous = {
        let mut rt = RUNTIME.lock();
        replace_current_observer(&mut rt, None)
    };
    let result = catch_unwind(AssertUnwindSafe(f));
    let mut rt = RUNTIME.lock();
    replace_current_observer(&mut rt, previous);
    drop(rt);
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

// --- internal helpers ---

pub(crate) static NEXT_THREAD_KEY: AtomicU64 = AtomicU64::new(1);

thread_local! {
    /// Cached per-thread key for the runtime's observer/scope tables. Cheaper than
    /// `std::thread::current()`, which clones an `Arc` on every call. This is only
    /// a lookup key; the tables themselves stay runtime fields (Directive #7).
    pub(crate) static THREAD_KEY: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn thread_key() -> u64 {
    THREAD_KEY.with(|key| {
        let value = key.get();
        if value != 0 {
            return value;
        }
        let value = NEXT_THREAD_KEY.fetch_add(1, Ordering::Relaxed);
        key.set(value);
        value
    })
}

/// Returns the observer installed for the calling thread. User computes run
/// without the arena lock, so several threads may legitimately have active
/// tracking contexts at once.
pub(crate) fn current_observer(rt: &Runtime) -> Option<NodeId> {
    let thread = thread_key();
    rt.current_observers
        .iter()
        .find_map(|(owner, observer)| (*owner == thread).then_some(*observer))
}

/// Replaces the calling thread's observer and returns its previous value. The
/// small inline table keeps the ordinary one-thread UI path allocation-free.
pub(crate) fn replace_current_observer(rt: &mut Runtime, next: Option<NodeId>) -> Option<NodeId> {
    let thread = thread_key();
    let position = rt
        .current_observers
        .iter()
        .position(|(owner, _)| *owner == thread);
    match (position, next) {
        (Some(index), Some(observer)) => Some(std::mem::replace(
            &mut rt.current_observers[index].1,
            observer,
        )),
        (Some(index), None) => Some(rt.current_observers.swap_remove(index).1),
        (None, Some(observer)) => {
            rt.current_observers.push((thread, observer));
            None
        }
        (None, None) => None,
    }
}

/// Returns the ownership scope installed for the calling thread.
pub(crate) fn current_scope(rt: &Runtime) -> Option<NodeId> {
    let thread = thread_key();
    rt.current_scopes
        .iter()
        .find_map(|(owner, scope)| (*owner == thread).then_some(*scope))
}

/// Replaces the calling thread's ownership scope and returns its previous one.
/// The inline table mirrors `current_observers` so the ordinary UI path remains
/// allocation-free after warmup.
pub(crate) fn replace_current_scope(rt: &mut Runtime, next: Option<NodeId>) -> Option<NodeId> {
    let thread = thread_key();
    let position = rt
        .current_scopes
        .iter()
        .position(|(owner, _)| *owner == thread);
    match (position, next) {
        (Some(index), Some(scope)) => {
            Some(std::mem::replace(&mut rt.current_scopes[index].1, scope))
        }
        (Some(index), None) => Some(rt.current_scopes.swap_remove(index).1),
        (None, Some(scope)) => {
            rt.current_scopes.push((thread, scope));
            None
        }
        (None, None) => None,
    }
}

// --- dependency edges (SOUL §3.1 slot reuse) ---
//
// Invariant: `a.sources[i] == Edge { node: b, back: j }` exactly when
// `b.observers[j] == Edge { node: a, back: i }`. Every mutation below keeps both
// halves in step, so no operation searches a list.

// Test-only count of edges created or destroyed, to prove that a run with
// unchanged dependencies does no edge work at all.
#[cfg(test)]
thread_local! {
    pub(crate) static EDGE_CHANGES: Cell<usize> = const { Cell::new(0) };
}

#[inline]
pub(crate) fn count_edge_change() {
    #[cfg(test)]
    EDGE_CHANGES.with(|count| count.set(count.get() + 1));
}

/// Starts a tracking run for `id`: the old source list stays, `cursor` restarts.
pub(crate) fn begin_run(rt: &mut Runtime, id: NodeId) {
    rt.run_counter += 1;
    let run = rt.run_counter;
    if let Some(node) = rt.nodes.get_mut(id) {
        node.run = run;
        node.cursor = 0;
    }
}

/// Ends the run of `id`: sweeps the sources it did not read again.
pub(crate) fn end_run(rt: &mut Runtime, id: NodeId) {
    loop {
        let Some(node) = rt.nodes.get(id) else {
            return;
        };
        let len = node.sources.len();
        if len <= node.cursor as usize {
            break;
        }
        unlink_edge(rt, id, len - 1);
    }
    if let Some(node) = rt.nodes.get_mut(id) {
        node.run = 0;
        node.cursor = 0;
    }
}

/// Re-points the mirror of `observer.sources[slot]` (and the dedupe mark) at `slot`
/// after that entry moved.
pub(crate) fn fix_source_slot(rt: &mut Runtime, observer: NodeId, slot: usize) {
    let Some(node) = rt.nodes.get(observer) else {
        return;
    };
    let run = node.run;
    let edge = node.sources[slot];
    if let Some(source) = rt.nodes.get_mut(edge.node) {
        source.observers[edge.back as usize].back = slot as u32;
        if run != 0 && source.mark_run == run && source.mark_observer == observer {
            source.mark_slot = slot as u32;
        }
    }
}

/// Re-points the mirror of `source.observers[slot]` at `slot` after that entry moved.
pub(crate) fn fix_observer_slot(rt: &mut Runtime, source: NodeId, slot: usize) {
    let Some(node) = rt.nodes.get(source) else {
        return;
    };
    let edge = node.observers[slot];
    if let Some(observer) = rt.nodes.get_mut(edge.node) {
        observer.sources[edge.back as usize].back = slot as u32;
    }
}

/// Removes `source.observers[slot]` with an O(1) `swap_remove`.
pub(crate) fn remove_observer_slot(rt: &mut Runtime, source: NodeId, slot: usize) {
    let Some(node) = rt.nodes.get_mut(source) else {
        return;
    };
    node.observers.swap_remove(slot);
    if slot < node.observers.len() {
        fix_observer_slot(rt, source, slot);
    }
}

/// Removes `observer.sources[slot]` with an O(1) `swap_remove`. While the observer
/// runs, the confirmed prefix `sources[..cursor]` stays contiguous: a confirmed
/// slot is first swapped to the end of the prefix and the prefix shrinks.
pub(crate) fn remove_source_slot(rt: &mut Runtime, observer: NodeId, slot: usize) {
    let Some(node) = rt.nodes.get_mut(observer) else {
        return;
    };
    let mut slot = slot;
    if node.run != 0 && slot < node.cursor as usize {
        let last = node.cursor as usize - 1;
        node.cursor -= 1;
        if slot != last {
            node.sources.swap(slot, last);
            fix_source_slot(rt, observer, slot);
            fix_source_slot(rt, observer, last);
        }
        slot = last;
    }
    let Some(node) = rt.nodes.get_mut(observer) else {
        return;
    };
    node.sources.swap_remove(slot);
    if slot < node.sources.len() {
        fix_source_slot(rt, observer, slot);
    }
}

/// Destroys the edge `observer.sources[slot]` together with its mirror.
pub(crate) fn unlink_edge(rt: &mut Runtime, observer: NodeId, slot: usize) {
    let Some(node) = rt.nodes.get(observer) else {
        return;
    };
    let edge = node.sources[slot];
    count_edge_change();
    // The observer half goes first: its reshuffle still finds this edge's mirror
    // in place, which is removed right after.
    remove_source_slot(rt, observer, slot);
    remove_observer_slot(rt, edge.node, edge.back as usize);
}

/// Removes every source edge of `observer` (not part of a run).
pub(crate) fn unlink_all_sources(rt: &mut Runtime, observer: NodeId) {
    while let Some(len) = rt
        .nodes
        .get(observer)
        .map(|node| node.sources.len())
        .filter(|&len| len > 0)
    {
        unlink_edge(rt, observer, len - 1);
    }
}

/// Records `source` as a dependency of the currently-tracking observer (§3.3).
///
/// The observer's previous source list is reused as a template. A read of the same
/// source at the cursor confirms the existing edge and touches nothing else, so
/// unchanged dependencies cost O(1) per read with no allocation. A different
/// source replaces the stale slot. A source already read in this run is found
/// through its mark and ignored.
pub(crate) fn track_read(rt: &mut Runtime, source: NodeId) {
    let Some(observer) = current_observer(rt) else {
        return;
    };
    if observer == source {
        return; // a compute reading its own cell: ignore (no self-edge)
    }
    let Some(node) = rt.nodes.get(observer) else {
        return;
    };
    if node.run == 0 {
        return; // not inside a tracking run
    }
    let (run, cursor, len) = (node.run, node.cursor as usize, node.sources.len());
    let stale = node.sources.get(cursor).copied();
    let Some(src) = rt.nodes.get(source) else {
        return;
    };

    if stale.is_some_and(|edge| edge.node == source) {
        // Same source in the same position as last run: confirm the edge.
        let src = rt.nodes.get_mut(source).expect("source checked above");
        src.mark_observer = observer;
        src.mark_run = run;
        src.mark_slot = cursor as u32;
        rt.nodes
            .get_mut(observer)
            .expect("observer checked above")
            .cursor += 1;
        return;
    }
    if src.mark_run == run && src.mark_observer == observer {
        let slot = src.mark_slot as usize;
        let confirmed = slot < cursor && rt.nodes[observer].sources[slot].node == source;
        if confirmed {
            return; // already a dependency from earlier in this run
        }
    }

    // New dependency at the cursor. A stale slot there is recycled.
    if let Some(edge) = stale {
        count_edge_change();
        remove_observer_slot(rt, edge.node, edge.back as usize);
    }
    count_edge_change();
    let src = rt.nodes.get_mut(source).expect("source checked above");
    let back = src.observers.len() as u32;
    src.observers.push(Edge {
        node: observer,
        back: cursor as u32,
    });
    src.mark_observer = observer;
    src.mark_run = run;
    src.mark_slot = cursor as u32;
    let obs = rt.nodes.get_mut(observer).expect("observer checked above");
    let edge = Edge { node: source, back };
    if cursor < len {
        obs.sources[cursor] = edge;
    } else {
        obs.sources.push(edge);
    }
    obs.cursor += 1;
}

/// Empties both pending queues and their `QUEUED` bits.
pub(crate) fn clear_queues(rt: &mut Runtime) {
    let effects = std::mem::take(&mut rt.pending_effects);
    let subscriptions = std::mem::take(&mut rt.pending_subscriptions);
    for &id in effects.iter().chain(subscriptions.iter()) {
        if let Some(node) = rt.nodes.get_mut(id) {
            node.flags.remove(NodeFlags::QUEUED);
        }
    }
    rt.pending_effects = effects;
    rt.pending_effects.clear();
    rt.pending_subscriptions = subscriptions;
    rt.pending_subscriptions.clear();
}

/// **PUSH** propagation from a changed node: direct observers → `Dirty`, deeper
/// descendants → `Check`, effects queued (SOUL §3.1). Iterative, holds the lock
/// only over arena bookkeeping (no user code), uses a pooled scratch stack so it
/// allocates nothing once warm.
pub(crate) fn propagate_change(rt: &mut Runtime, changed: NodeId) {
    let mut stack = std::mem::take(&mut rt.propagate_stack);
    stack.clear();
    if let Some(n) = rt.nodes.get(changed) {
        for edge in &n.observers {
            stack.push((edge.node, true));
        }
    }
    while let Some((id, direct)) = stack.pop() {
        // `Some(is_subscription)` when the node must be queued.
        let enqueue = {
            let Some(node) = rt.nodes.get_mut(id) else {
                continue;
            };
            // A running node already observes fresh inputs (demand-driven pull);
            // re-marking it would spuriously re-run it — skip (§3.1 glitch-free).
            if node.flags.contains(NodeFlags::RUNNING) {
                continue;
            }
            let cur = node.flags.color();
            if cur == NodeFlags::DIRTY {
                continue; // already dirty; its subtree is already marked
            }
            if cur == NodeFlags::CHECK && !direct {
                continue; // already check; deeper re-mark is redundant
            }
            node.flags.set_color(if direct {
                NodeFlags::DIRTY
            } else {
                NodeFlags::CHECK
            });
            let is_sink = (node.compute.is_some() && node.flags.contains(NodeFlags::TRACKING))
                || node.subscription.is_some();
            if is_sink && !node.flags.contains(NodeFlags::QUEUED) {
                node.flags.insert(NodeFlags::QUEUED);
                Some(node.subscription.is_some())
            } else {
                None
            }
        };
        match enqueue {
            Some(true) => rt.pending_subscriptions.push(id),
            Some(false) => rt.pending_effects.push(id),
            None => {}
        }
        if let Some(node) = rt.nodes.get(id) {
            for edge in &node.observers {
                stack.push((edge.node, false));
            }
        }
    }
    rt.propagate_stack = stack;
}

/// One level of the explicit `update_if_necessary` stack: a node and the index of
/// the next source to verify.
pub(crate) struct PullFrame {
    pub(crate) id: NodeId,
    pub(crate) next: u32,
}

/// `true` when reading `id` needs no pull: clean, running (it reads its own stale
/// cache, as before) or gone.
pub(crate) fn is_settled(rt: &Runtime, id: NodeId) -> bool {
    rt.nodes
        .get(id)
        .is_none_or(|n| n.flags.contains(NodeFlags::RUNNING) || n.flags.color() == NodeFlags::CLEAN)
}

/// Locks the runtime with `id` settled. A clean node costs exactly one lock
/// acquisition; otherwise the pull runs first and the lock is taken again.
pub(crate) fn settle(id: NodeId) -> MutexGuard<'static, Runtime> {
    let rt = RUNTIME.lock();
    if is_settled(&rt, id) {
        return rt;
    }
    drop(rt);
    update_if_necessary(id);
    RUNTIME.lock()
}

/// **PULL**: ensures `id` holds a valid value (SOUL §3.1). `Clean` → nothing;
/// `Check` → verify sources in read order, stopping at the first that resolves
/// `Dirty`; `Dirty` (or a source turned us `Dirty`) → recompute.
///
/// The walk down the source graph is an explicit pooled stack, so a 100k-deep memo
/// chain does not grow the call stack. Each step takes the lock once and never
/// holds it across the recompute, which runs user code.
pub(crate) fn update_if_necessary(root: NodeId) {
    let mut stack = {
        let mut rt = RUNTIME.lock();
        if is_settled(&rt, root) {
            return;
        }
        rt.pull_pool.pop().unwrap_or_default()
    };
    stack.clear();
    stack.push(PullFrame { id: root, next: 0 });
    loop {
        let mut rt = RUNTIME.lock();
        let Some(frame) = stack.last_mut() else {
            rt.pull_pool.push(stack);
            return;
        };
        let id = frame.id;
        let Some(node) = rt.nodes.get(id) else {
            stack.pop();
            continue;
        };
        if node.flags.contains(NodeFlags::RUNNING) {
            stack.pop();
            continue;
        }
        match node.flags.color() {
            NodeFlags::CLEAN => {
                stack.pop();
                continue;
            }
            NodeFlags::DIRTY => {
                drop(rt);
                recompute(id);
                stack.pop();
                continue;
            }
            _ => {}
        }
        // CHECK: descend into the next source that is not settled. The frame
        // resumes after it, and a source that really changed has marked us DIRTY.
        let mut next = frame.next as usize;
        let mut unsettled = None;
        while let Some(edge) = node.sources.get(next) {
            next += 1;
            if !is_settled(&rt, edge.node) {
                unsettled = Some(edge.node);
                break;
            }
        }
        frame.next = next as u32;
        match unsettled {
            Some(source) => stack.push(PullFrame {
                id: source,
                next: 0,
            }),
            None => {
                // No source resolved dirty → the cache is trustworthy after all.
                if let Some(n) = rt.nodes.get_mut(id) {
                    n.flags.set_color(NodeFlags::CLEAN);
                }
                stack.pop();
            }
        }
    }
}

/// Recomputes a dirty compute node (SOUL §3.1). The node keeps its source list
/// while the compute runs: reads confirm or replace edges in place
/// (`track_read`) and `end_run` sweeps the unread tail, so unchanged dependencies
/// do no edge work. The compute + cell are lifted out and the lock released
/// before user code runs; the result is written back and — only on a real change —
/// versions bump and observers are marked.
pub(crate) fn recompute(id: NodeId) {
    let mut grave = Graveyard::new();
    let taken = {
        let mut rt = RUNTIME.lock();
        let (running, has_compute, is_subscription) = match rt.nodes.get(id) {
            Some(n) => (
                n.flags.contains(NodeFlags::RUNNING),
                n.compute.is_some(),
                n.subscription.is_some(),
            ),
            None => return,
        };
        if running {
            return;
        }
        if !has_compute {
            if is_subscription {
                if let Some(n) = rt.nodes.get_mut(id) {
                    n.flags.set_color(NodeFlags::CLEAN);
                }
                queue_subscription_ready(&mut rt, id);
                return;
            }
            // A plain signal can never be dirty legitimately; treat as clean.
            if let Some(n) = rt.nodes.get_mut(id) {
                n.flags.set_color(NodeFlags::CLEAN);
            }
            return;
        }

        // Owner cleanup: nodes created by the previous run are owned by this node
        // (`attach_to_current_scope`); dispose them so each run starts fresh. The
        // values drop after the run (see below), not while this node observes.
        dispose_children(&mut rt, id, &mut grave);
        begin_run(&mut rt, id);

        let (compute, cell) = {
            let n = rt.nodes.get_mut(id).unwrap();
            n.flags.insert(NodeFlags::RUNNING);
            (n.compute.take(), n.value.take())
        };
        let prev = replace_current_observer(&mut rt, Some(id));
        (compute, cell, prev)
    };

    let (mut compute, mut cell, prev) = taken;

    // --- user code runs here, lock released, deps re-track via track_read ---
    // Caught so a panicking compute still leaves the node usable (RUNNING cleared,
    // observer restored) before the panic continues.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        match (compute.as_mut(), cell.as_mut()) {
            (Some(c), Some(v)) => c(&mut **v),
            _ => false,
        }
    }));
    let changed = matches!(outcome, Ok(true));

    let mut rt = RUNTIME.lock();
    replace_current_observer(&mut rt, prev);
    let existed = if let Some(n) = rt.nodes.get_mut(id) {
        n.compute = compute;
        n.value = cell;
        n.flags.remove(NodeFlags::RUNNING);
        // A panicked compute stays dirty so the next pull retries it.
        n.flags.set_color(if outcome.is_ok() {
            NodeFlags::CLEAN
        } else {
            NodeFlags::DIRTY
        });
        if changed {
            n.version = n.version.wrapping_add(1);
        }
        true
    } else {
        false // node was disposed mid-compute; drop compute + cell
    };
    if existed {
        end_run(&mut rt, id);
    }
    if existed && changed {
        rt.global_version = rt.global_version.wrapping_add(1);
        propagate_change(&mut rt, id);
    }
    drop(rt);
    // The previous run's children drop now: the node is no longer RUNNING and no
    // observer is installed, so a `Drop` that reads a signal adds no edge and a
    // panicking `Drop` cannot leave the node stuck.
    let dropped = catch_unwind(AssertUnwindSafe(|| drop_graveyard(grave)));
    match (outcome, dropped) {
        (Err(payload), _) | (_, Err(payload)) => resume_unwind(payload),
        _ => {}
    }
}

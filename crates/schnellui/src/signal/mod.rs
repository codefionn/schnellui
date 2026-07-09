//! # schnellui::signal
//!
//! The reactive core (SOUL §3.1): a **push-then-pull, lazily-evaluated,
//! mark-and-sweep coloring graph** stored in a single process-global generational
//! arena. Handles ([`Signal`], [`Memo`], [`Effect`]) are `Copy` and carry only a
//! [`NodeId`] slotmap key — no `T: Copy` bound, no refcount bump on read.
//! Callback-free [`Subscription`] handles additionally bridge tracked `!Send`
//! producers to owner-local ready queues without storing those producers here.
//!
//! ## Contract for implementers (SOUL §3.1)
//! - The runtime is process-global, `static`, lock-guarded (`parking_lot::Mutex`)
//!   so it stays `Send + Sync` (Directive #7 — never a thread-local store).
//! - **Never hold the lock across user code.** Acquire → `mem::take` the compute
//!   fn / copy inputs → release → run → re-acquire to write back.
//! - **PUSH** (`set`): mark direct observers `Dirty`, deeper descendants `Check`,
//!   *queue* effects/subscriptions — no recomputation.
//! - **PULL** (`Runtime::flush`, or on read): `update_if_necessary` walks sources
//!   in read order, stops at the first `Dirty`, recomputes, runs the opt-in
//!   equality gate, and only re-marks observers if the value actually changed;
//!   settled subscriptions append their opaque key to their owner's ready queue.
//! - Edge lists are stable per-node [`smallvec::SmallVec`]s. A recompute keeps
//!   its old source list and walks it with a cursor: a read of the same source
//!   at the same position confirms the existing edge, a different source
//!   replaces the stale slot, and only the unread tail is swept at the end of the
//!   run. Each edge records its index in the peer's list, so unlinking is an O(1)
//!   `swap_remove` that patches the moved entry. Same deps every run ⇒ no edge
//!   work, zero alloc, zero free (§3.1, §4).

use std::any::Any;
use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use once_cell::sync::Lazy;
use parking_lot::{Mutex, MutexGuard, ReentrantMutex};
use slotmap::{SecondaryMap, SlotMap};
use smallvec::SmallVec;

slotmap::new_key_type! {
    /// A `Copy` handle into the reactive arena. Every [`Signal`], [`Memo`] and
    /// [`Effect`] is just one of these keys (SOUL §3.1).
    pub struct NodeId;
    /// Internal identity for a [`SubscriptionGroup`].
    pub(crate) struct SubscriptionGroupId;
}

mod arena;
mod handles;
mod owner;
mod subscription;
pub use arena::*;
pub use handles::*;
pub use owner::*;
pub use subscription::*;

#[cfg(test)]
mod tests;

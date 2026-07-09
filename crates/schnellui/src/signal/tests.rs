use super::*;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering::SeqCst};
use std::sync::Arc;

/// The reactive arena is process-global; serialize tests that create effects,
/// memos, or flush so cross-test queued work never pollutes another test's
/// pull phase (or its allocation measurement).
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// A `fn`-pointer equality gate over `i32` for the opt-in comparators.
fn eq_i32(a: &dyn Any, b: &dyn Any) -> bool {
    a.downcast_ref::<i32>() == b.downcast_ref::<i32>()
}

#[test]
fn node_flags_color_pack() {
    let mut f = NodeFlags::default();
    assert_eq!(f.color(), NodeFlags::CLEAN);
    f.insert(NodeFlags::TRACKING);
    f.set_color(NodeFlags::DIRTY);
    // color and status bits are orthogonal
    assert_eq!(f.color(), NodeFlags::DIRTY);
    assert!(f.contains(NodeFlags::TRACKING));
    f.set_color(NodeFlags::CHECK);
    assert_eq!(f.color(), NodeFlags::CHECK);
    assert!(f.contains(NodeFlags::TRACKING));
    f.remove(NodeFlags::TRACKING);
    assert!(!f.contains(NodeFlags::TRACKING));
    assert_eq!(f.color(), NodeFlags::CHECK);
}

#[test]
fn signal_get_set_update() {
    let s = create_signal(1i32);
    assert_eq!(s.get(), 1);
    s.set(41);
    assert_eq!(s.get(), 41);
    s.update(|v| *v += 1);
    assert_eq!(s.get(), 42);
}

#[test]
fn signal_with_reads_by_ref() {
    let s = create_signal(String::from("hi"));
    let len = s.with(|v| v.len());
    assert_eq!(len, 2);
    // value is restored after `with` lifts it out and back.
    assert_eq!(s.get(), "hi");
}

#[test]
fn signal_transaction_returns_value_and_restores_after_panic() {
    let s = create_signal(vec![1]);
    assert_eq!(
        s.transaction(|values| {
            values.push(2);
            values.len()
        }),
        2
    );

    assert!(catch_unwind(AssertUnwindSafe(|| {
        s.transaction(|values| {
            values.push(3);
            panic!("expected transaction panic");
        });
    }))
    .is_err());
    assert_eq!(s.get(), vec![1, 2, 3]);
    assert_eq!(
        s.transaction(|values| {
            values.push(4);
            values.len()
        }),
        4
    );
}

#[test]
fn transaction_and_peek_report_disposal_without_running_closures() {
    let scope = create_scope();
    let signal = run_in_scope(scope, || create_signal(7i32));
    scope.dispose();
    let calls = AtomicUsize::new(0);
    assert_eq!(
        signal.try_transaction(|value| {
            calls.fetch_add(1, SeqCst);
            *value += 1;
        }),
        None
    );
    assert_eq!(
        signal.try_peek(|value| {
            calls.fetch_add(1, SeqCst);
            *value
        }),
        None
    );
    assert_eq!(calls.load(SeqCst), 0);
}

#[test]
fn wide_scope_disposal_is_linear() {
    let _g = TEST_LOCK.lock();
    const N: usize = 20_000;
    let scope = create_scope();
    run_in_scope(scope, || {
        for i in 0..N {
            let _ = create_signal(i);
        }
    });
    let before = OWNER_SCAN_STEPS.with(|steps| steps.get());
    scope.dispose();
    let steps = OWNER_SCAN_STEPS.with(|steps| steps.get()) - before;
    // Children leave with their parent's list, so none scans a sibling list.
    // The old per-child unlink scan would inspect ~N^2/2 positions here.
    assert!(
        steps <= 2 * N,
        "quadratic scope disposal: {steps} scan steps"
    );
}

#[test]
fn run_in_scope_restores_the_calling_threads_owner_after_a_panic() {
    let _g = TEST_LOCK.lock();
    let scope = create_scope();
    let child = std::sync::Mutex::new(None);
    assert!(catch_unwind(AssertUnwindSafe(|| {
        run_in_scope(scope, || {
            *child.lock().unwrap() = Some(create_signal(1i32));
            panic!("expected scope panic");
        });
    }))
    .is_err());
    let unrelated = create_signal(2i32);
    scope.dispose();
    assert_eq!(child.lock().unwrap().unwrap().try_get(), None);
    assert_eq!(unrelated.try_get(), Some(2));
}

#[test]
fn parallel_scopes_never_cross_own_or_dispose_each_others_signals() {
    let _g = TEST_LOCK.lock();
    let (first_installed_tx, first_installed_rx) = std::sync::mpsc::channel();
    let (second_installed_tx, second_installed_rx) = std::sync::mpsc::channel();
    let (first_created_tx, first_created_rx) = std::sync::mpsc::channel();

    let first = std::thread::spawn(move || {
        let scope = create_scope();
        let signal = run_in_scope(scope, || {
            first_installed_tx.send(()).unwrap();
            second_installed_rx.recv().unwrap();
            let signal = create_signal(10i32);
            first_created_tx.send(()).unwrap();
            signal
        });
        (scope, signal)
    });
    let second = std::thread::spawn(move || {
        first_installed_rx.recv().unwrap();
        let scope = create_scope();
        let signal = run_in_scope(scope, || {
            second_installed_tx.send(()).unwrap();
            first_created_rx.recv().unwrap();
            create_signal(20i32)
        });
        (scope, signal)
    });

    let (first_scope, first_signal) = first.join().unwrap();
    let (second_scope, second_signal) = second.join().unwrap();
    first_scope.dispose();
    assert_eq!(first_signal.try_get(), None);
    assert_eq!(second_signal.try_get(), Some(20));
    second_scope.dispose();
    assert_eq!(second_signal.try_get(), None);
}

#[test]
fn peek_and_untrack_do_not_create_dependencies() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(1i32);
    let via_peek = create_memo(move || source.peek(|value| *value));
    assert_eq!(via_peek.get(), 1);
    source.set(2);
    assert_eq!(via_peek.get(), 1);

    let tracked = create_signal(10i32);
    let untracked_source = create_signal(20i32);
    let mixed = create_memo(move || tracked.get() + untrack(|| untracked_source.get()));
    assert_eq!(mixed.get(), 30);
    untracked_source.set(30);
    assert_eq!(mixed.get(), 30);
    tracked.set(11);
    assert_eq!(mixed.get(), 41);
}

#[test]
fn set_bumps_global_version() {
    let before = Runtime::global_version();
    let s = create_signal(0u8);
    s.set(5);
    assert!(Runtime::global_version() > before);
}

#[test]
fn dispose_makes_try_get_none() {
    let scope = create_scope();
    // manually park a node under the scope to prove disposal semantics
    {
        let mut rt = RUNTIME.lock();
        let id = rt.nodes.insert(Node::signal(Box::new(7i32), None));
        rt.scopes.get_mut(scope.id).unwrap().push(id);
    }
    scope.dispose();
    // scope node itself is gone
    assert!(RUNTIME.lock().scopes.get(scope.id).is_none());
}

#[test]
fn memo_recomputes_on_dep_change() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(2i32);
    let m = create_memo(move || s.get() * 10);
    assert_eq!(m.get(), 20);
    s.set(3);
    assert_eq!(m.get(), 30);
}

#[test]
fn effect_queued_on_push_drained_on_flush() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(0i32);
    let seen = Arc::new(AtomicUsize::new(999));
    let _e = {
        let seen = seen.clone();
        create_effect(move || seen.store(s.get() as usize, SeqCst))
    };
    // first run is synchronous on creation
    assert_eq!(seen.load(SeqCst), 0);
    s.set(7);
    // queued, NOT yet run — the pull happens at flush (§3.1 / §7.5)
    assert_eq!(seen.load(SeqCst), 0);
    Runtime::flush();
    assert_eq!(seen.load(SeqCst), 7);
}

#[test]
fn diamond_glitch_free_each_recomputed_once() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(1i32);
    let (ca, cb, cd) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let a = {
        let ca = ca.clone();
        create_memo(move || {
            ca.fetch_add(1, SeqCst);
            s.get() + 1
        })
    };
    let b = {
        let cb = cb.clone();
        create_memo(move || {
            cb.fetch_add(1, SeqCst);
            s.get() + 10
        })
    };
    let d = {
        let cd = cd.clone();
        create_memo(move || {
            cd.fetch_add(1, SeqCst);
            a.get() + b.get()
        })
    };

    assert_eq!(d.get(), (1 + 1) + (1 + 10));
    assert_eq!(
        (ca.load(SeqCst), cb.load(SeqCst), cd.load(SeqCst)),
        (1, 1, 1)
    );

    s.set(2);
    assert_eq!(d.get(), (2 + 1) + (2 + 10));
    // each node recomputed exactly once more — glitch-free diamond (§3.1)
    assert_eq!(
        (ca.load(SeqCst), cb.load(SeqCst), cd.load(SeqCst)),
        (2, 2, 2)
    );
}

#[test]
fn signal_equals_prunes_propagation() {
    let _g = TEST_LOCK.lock();
    let s = create_signal_equals(5i32, eq_i32);
    let runs = Arc::new(AtomicUsize::new(0));
    let _e = {
        let runs = runs.clone();
        create_effect(move || {
            let _ = s.get();
            runs.fetch_add(1, SeqCst);
        })
    };
    assert_eq!(runs.load(SeqCst), 1); // first run on creation
    s.set(5); // equal → gate suppresses the whole push
    Runtime::flush();
    assert_eq!(runs.load(SeqCst), 1);
    s.set(6); // real change fires
    Runtime::flush();
    assert_eq!(runs.load(SeqCst), 2);
}

#[test]
fn memo_equals_stops_propagation() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(0i32);
    let parity = create_memo_equals(move || s.get() % 2, eq_i32);
    let runs = Arc::new(AtomicUsize::new(0));
    let _e = {
        let runs = runs.clone();
        create_effect(move || {
            let _ = parity.get();
            runs.fetch_add(1, SeqCst);
        })
    };
    assert_eq!(runs.load(SeqCst), 1);

    s.set(2); // 0 → still even: memo recomputes but value is unchanged
    Runtime::flush();
    // the memo boundary prunes: the effect must NOT re-run (§3.1)
    assert_eq!(runs.load(SeqCst), 1);

    s.set(3); // parity flips → propagation resumes
    Runtime::flush();
    assert_eq!(runs.load(SeqCst), 2);
}

#[test]
fn dynamic_dependencies_resubscribe() {
    let _g = TEST_LOCK.lock();
    let cond = create_signal(true);
    let x = create_signal(10i32);
    let y = create_signal(20i32);
    let count = Arc::new(AtomicUsize::new(0));
    let m = {
        let count = count.clone();
        create_memo(move || {
            count.fetch_add(1, SeqCst);
            if cond.get() {
                x.get()
            } else {
                y.get()
            }
        })
    };

    assert_eq!(m.get(), 10);
    assert_eq!(count.load(SeqCst), 1);

    // y is not a dependency yet → changing it must not recompute m.
    y.set(21);
    assert_eq!(m.get(), 10);
    assert_eq!(count.load(SeqCst), 1);

    // x IS a dependency → recompute.
    x.set(11);
    assert_eq!(m.get(), 11);
    assert_eq!(count.load(SeqCst), 2);

    // flip the branch: m now reads y and unsubscribes x.
    cond.set(false);
    assert_eq!(m.get(), 21);
    assert_eq!(count.load(SeqCst), 3);

    // x is no longer a dependency → no recompute.
    x.set(100);
    assert_eq!(m.get(), 21);
    assert_eq!(count.load(SeqCst), 3);

    // y is a dependency now.
    y.set(22);
    assert_eq!(m.get(), 22);
    assert_eq!(count.load(SeqCst), 4);
}

#[test]
fn disposal_safety() {
    let _g = TEST_LOCK.lock();

    // A disposed signal reads back as None, never panics.
    let scope = create_scope();
    let s = run_in_scope(scope, || create_signal(5i32));
    assert_eq!(s.try_get(), Some(5));
    scope.dispose();
    assert_eq!(s.try_get(), None);

    // Disposing a queued effect, then flushing, is safe and does not re-run it.
    let s2 = create_signal(0i32);
    let scope2 = create_scope();
    let runs = Arc::new(AtomicUsize::new(0));
    let _e = {
        let runs = runs.clone();
        run_in_scope(scope2, || {
            create_effect(move || {
                let _ = s2.try_get();
                runs.fetch_add(1, SeqCst);
            })
        })
    };
    assert_eq!(runs.load(SeqCst), 1);
    s2.set(1); // queues the effect
    scope2.dispose(); // removes it from the arena and the queue
    Runtime::flush(); // must not panic on the dangling id
    assert_eq!(runs.load(SeqCst), 1); // and must not re-run the disposed effect
}

#[test]
fn subscription_direct_dependencies_are_isolated() {
    let _g = TEST_LOCK.lock();
    let left = create_signal(0i32);
    let right = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let left_sub = group.subscribe(10);
    let right_sub = group.subscribe(20);
    left_sub.track(|| left.get());
    right_sub.track(|| right.get());

    left.set(1);
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![10]);

    right.set(1);
    Runtime::flush();
    ready.clear();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![20]);
}

#[test]
fn subscriptions_track_independently_on_parallel_threads() {
    let _g = TEST_LOCK.lock();
    let left = create_signal(0i32);
    let right = create_signal(0i32);
    let overlap = Arc::new(std::sync::Barrier::new(2));

    let run = |source: Signal<i32>, key: u64, overlap: Arc<std::sync::Barrier>| {
        std::thread::spawn(move || {
            let group = SubscriptionGroup::new();
            let subscription = group.subscribe(key);
            subscription.track(|| {
                // Both observer contexts are installed before either read.
                overlap.wait();
                source.get()
            });
            source.set(1);
            Runtime::flush();
            let mut ready = Vec::new();
            group.drain_ready_into(&mut ready);
            ready
        })
    };

    let left_thread = run(left, 10, overlap.clone());
    let right_thread = run(right, 20, overlap);
    assert_eq!(left_thread.join().unwrap(), vec![10]);
    assert_eq!(right_thread.join().unwrap(), vec![20]);
}

#[test]
fn concurrent_flush_waits_for_already_drained_work() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let blocker = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let subscription = group.subscribe(77);
    subscription.track(|| source.get());

    let entered = Arc::new(std::sync::Barrier::new(2));
    let release = Arc::new(std::sync::Barrier::new(2));
    let runs = Arc::new(AtomicUsize::new(0));
    let scope = create_scope();
    let _effect = run_in_scope(scope, || {
        let entered = entered.clone();
        let release = release.clone();
        let runs = runs.clone();
        create_effect(move || {
            blocker.get();
            if runs.fetch_add(1, SeqCst) > 0 {
                entered.wait();
                release.wait();
            }
        })
    });

    blocker.set(1);
    source.set(1);
    let first = std::thread::spawn(Runtime::flush);
    entered.wait();

    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let second = std::thread::spawn(move || {
        Runtime::flush();
        finished_tx.send(()).unwrap();
    });
    assert!(finished_rx
        .recv_timeout(std::time::Duration::from_millis(20))
        .is_err());

    release.wait();
    first.join().unwrap();
    second.join().unwrap();
    finished_rx.recv().unwrap();

    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![77]);
    scope.dispose();
}

#[test]
fn subscription_memo_equality_prunes_ready_key() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let parity = create_memo_equals(move || source.get() % 2, eq_i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(7);
    sub.track(|| parity.get());

    source.set(2); // memo is rechecked, but retains the equal value
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());

    source.set(3);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![7]);
}

#[test]
fn subscription_retracks_dynamic_dependencies() {
    let _g = TEST_LOCK.lock();
    let condition = create_signal(true);
    let left = create_signal(1i32);
    let right = create_signal(2i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(42);
    let track = || {
        sub.track(|| {
            if condition.get() {
                left.get()
            } else {
                right.get()
            }
        });
    };
    track();

    right.set(3);
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());

    left.set(4);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![42]);

    // Consume and rebuild the producer: its source switches from left to right.
    track();
    condition.set(false);
    Runtime::flush();
    ready.clear();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![42]);
    track();

    left.set(5);
    Runtime::flush();
    ready.clear();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());

    right.set(6);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![42]);
}

#[test]
fn nested_track_joins_the_outer_run() {
    let _g = TEST_LOCK.lock();
    let a = create_signal(1i32);
    let b = create_signal(2i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(1);
    sub.track(|| {
        a.get();
        sub.track(|| b.get());
    });
    // The inner run must not have swept `a`: both sources still notify.
    let mut ready = Vec::new();
    a.set(10);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![1]);
    sub.track(|| {
        a.get();
        sub.track(|| b.get());
    });
    ready.clear();
    b.set(20);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![1]);
}

#[test]
fn group_retrack_and_removal_do_constant_work_per_member() {
    let _g = TEST_LOCK.lock();
    const N: usize = 2000;
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let subs: Vec<_> = (0..N).map(|k| group.subscribe(k as u64)).collect();
    for sub in &subs {
        sub.track(|| source.get());
    }
    source.set(1);
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready.len(), N);

    // Drained: retracking touches no queue element.
    test_work::take();
    for sub in &subs {
        sub.track(|| source.get());
    }
    assert_eq!(test_work::take(), 0, "retrack after drain scans nothing");

    // Undrained: every member is queued, then retracked out of the queue.
    source.set(2);
    Runtime::flush();
    for sub in &subs {
        sub.track(|| source.get());
    }
    let work = test_work::take();
    assert!(
        work <= 2 * N,
        "undrained retrack work {work} is not O(1) each"
    );
    ready.clear();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty(), "retracked members leave the queue");

    // Undrained removal: all members queued, then every one unsubscribed.
    source.set(3);
    Runtime::flush();
    test_work::take();
    for sub in subs.iter().rev().step_by(2) {
        sub.unsubscribe();
    }
    for sub in &subs {
        sub.unsubscribe();
    }
    let work = test_work::take();
    assert!(
        work <= 4 * N,
        "undrained removal work {work} must be O(1) per member"
    );
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty(), "removed members leave the queue");
}

#[test]
fn subscription_coalesces_until_retracked() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(9);
    sub.track(|| source.get());
    let mut ready = Vec::new();

    source.set(1);
    Runtime::flush();
    source.set(2);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![9]);

    // Even after draining, this producer remains coalesced until its owner
    // has read it again.
    source.set(3);
    Runtime::flush();
    ready.clear();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());

    sub.track(|| source.get());
    source.set(4);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![9]);
}

#[test]
fn subscription_disposal_removes_edges_and_ready_work() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(11);
    sub.track(|| source.get());
    source.set(1);
    Runtime::flush();
    drop(sub); // removes both the source edge and the undrained key

    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());
    source.set(2);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());
}

#[test]
fn subscription_group_unsubscribe_invalidates_clones_and_clear_reuses_group() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(11);
    let clone = sub.clone();
    sub.track(|| source.get());
    group.unsubscribe(sub);

    source.set(1);
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    assert!(ready.is_empty());
    // A stale clone is harmless and cannot reinstall graph edges.
    assert_eq!(clone.track(|| source.get()), 1);

    let replacement = group.subscribe(12);
    replacement.track(|| source.get());
    group.clear();
    let reused = group.subscribe(13);
    reused.track(|| source.get());
    source.set(2);
    Runtime::flush();
    group.drain_ready_into(&mut ready);
    assert_eq!(ready, vec![13]);
}

#[cfg(feature = "count-allocations")]
#[test]
fn second_update_allocates_nothing() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(0i32);
    let out = Arc::new(AtomicI32::new(-1));
    let _e = {
        let out = out.clone();
        create_effect(move || out.store(s.get(), SeqCst))
    };

    // Warm: the first set + flush is allowed to grow pools (§4.1).
    s.set(1);
    Runtime::flush();
    assert_eq!(out.load(SeqCst), 1);

    // Measure the SECOND set + flush: the steady-state re-render path.
    let info = allocation_counter::measure(|| {
        s.set(2);
        Runtime::flush();
    });
    assert_eq!(info.count_total, 0, "allocations on steady-state update");
    assert_eq!(info.bytes_total, 0, "bytes on steady-state update");
    assert_eq!(out.load(SeqCst), 2);
}

// Silence unused import when the alloc feature is off.
#[cfg(not(feature = "count-allocations"))]
#[allow(dead_code)]
fn _uses_atomic_i32(_: AtomicI32) {}

/// Counts the child ids a node currently lists.
fn child_count(id: NodeId) -> usize {
    RUNTIME.lock().scopes.get(id).map_or(0, |list| list.len())
}

#[test]
fn dispose_runs_value_drops_outside_the_runtime_lock() {
    let _g = TEST_LOCK.lock();
    struct ScopeGuard(Scope);
    impl Drop for ScopeGuard {
        fn drop(&mut self) {
            self.0.dispose(); // locks RUNTIME
        }
    }
    let inner = create_scope();
    let outer = create_scope();
    let guard = ScopeGuard(inner);
    run_in_scope(outer, || {
        create_effect(move || {
            let _held = &guard;
        });
    });
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        outer.dispose();
        tx.send(()).unwrap();
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("dispose deadlocked on a re-entrant Drop");
}

#[test]
fn set_drops_the_old_value_outside_the_runtime_lock() {
    let _g = TEST_LOCK.lock();
    struct Reenter;
    impl Drop for Reenter {
        fn drop(&mut self) {
            let _ = Runtime::node_count(); // locks RUNTIME
        }
    }
    let s = create_signal(Reenter);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        s.set(Reenter);
        tx.send(()).unwrap();
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("Signal::set deadlocked dropping the old value");
}

#[test]
fn effect_rerun_disposes_nodes_created_by_the_previous_run() {
    let _g = TEST_LOCK.lock();
    let trigger = create_signal(0i32);
    let last_child = Arc::new(Mutex::new(None::<NodeId>));
    // Other tests may create nodes concurrently, so count the effect's own
    // owned nodes (the process-wide count is not stable under `cargo test`).
    let effect = {
        let last_child = last_child.clone();
        create_effect(move || {
            let t = trigger.get();
            *last_child.lock() = Some(create_signal(t).id());
        })
    };
    let owned = |e: Effect| {
        let rt = RUNTIME.lock();
        let list = rt.scopes.get(e.id).unwrap();
        assert!(list.iter().all(|&c| rt.nodes.contains_key(c)));
        list.len()
    };
    assert_eq!(owned(effect), 1);
    for i in 1..=100 {
        let previous = last_child.lock().unwrap();
        trigger.set(i);
        Runtime::flush();
        let current = last_child.lock().unwrap();
        assert_ne!(previous, current);
        assert!(
            !RUNTIME.lock().nodes.contains_key(previous),
            "previous run's child is still in the arena"
        );
        assert_eq!(owned(effect), 1);
    }
}

#[test]
fn disposed_child_scopes_unlink_from_the_parent() {
    let _g = TEST_LOCK.lock();
    let parent = create_scope();
    let kids: Vec<Scope> = run_in_scope(parent, || (0..1000).map(|_| create_scope()).collect());
    assert_eq!(child_count(parent.id), 1000);
    for kid in kids {
        kid.dispose();
    }
    assert_eq!(child_count(parent.id), 0);
    parent.dispose();
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "reactive cycle")]
fn mutually_writing_effects_are_cut_off() {
    let _g = TEST_LOCK.lock();
    let a = create_signal(0i32);
    let b = create_signal(0i32);
    create_effect(move || b.set(a.get() + 1));
    create_effect(move || a.set(b.get() + 1));
    Runtime::flush();
}

#[cfg(not(debug_assertions))]
#[test]
fn mutually_writing_effects_are_cut_off_in_release() {
    let _g = TEST_LOCK.lock();
    let a = create_signal(0i32);
    let b = create_signal(0i32);
    create_effect(move || b.set(a.get() + 1));
    create_effect(move || a.set(b.get() + 1));
    // Release builds log the cycle and return instead of panicking or hanging.
    Runtime::flush();
}

/// Checks the mirror invariant of every edge in the arena.
fn assert_edges_consistent() {
    let rt = RUNTIME.lock();
    for (id, node) in rt.nodes.iter() {
        for (i, edge) in node.sources.iter().enumerate() {
            let source = rt.nodes.get(edge.node).expect("source edge to a dead node");
            let mirror = source.observers[edge.back as usize];
            assert!(
                mirror.node == id && mirror.back as usize == i,
                "source mirror"
            );
        }
        for (j, edge) in node.observers.iter().enumerate() {
            let observer = rt
                .nodes
                .get(edge.node)
                .expect("observer edge to a dead node");
            let mirror = observer.sources[edge.back as usize];
            assert!(
                mirror.node == id && mirror.back as usize == j,
                "observer mirror"
            );
        }
    }
}

fn edge_changes() -> usize {
    EDGE_CHANGES.with(|count| count.get())
}

#[test]
fn unchanged_dependencies_do_no_edge_work() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(0i64);
    let others: Vec<_> = (0..4).map(create_signal).collect();
    let effects: Vec<_> = (0..1000)
        .map(|_| {
            let others = others.clone();
            create_effect(move || {
                let _ = s.get() + others.iter().map(|o| o.get()).sum::<i64>();
            })
        })
        .collect();
    let runs = |n: usize| {
        let before = edge_changes();
        for round in 0..n {
            s.set(round as i64 + 1);
            Runtime::flush();
        }
        edge_changes() - before
    };
    // 1000 observers re-run per set; every edge is confirmed in place.
    assert_eq!(runs(3), 0);
    assert_edges_consistent();
    for effect in effects {
        RUNTIME.lock().nodes.get(effect.id).expect("effect alive");
        let mut grave = Graveyard::new();
        dispose_node(&mut RUNTIME.lock(), effect.id, &mut grave);
    }
    assert_edges_consistent();
}

#[test]
fn dependency_changes_keep_edges_consistent() {
    let _g = TEST_LOCK.lock();
    let selector = create_signal(0usize);
    let inputs: Vec<Signal<i64>> = (0..6).map(|i| create_signal(i as i64)).collect();
    let reads = {
        let inputs = inputs.clone();
        create_memo(move || {
            // Reorders, drops, adds and repeats sources depending on the selector.
            let order: &[usize] = match selector.get() {
                0 => &[0, 1, 2, 3],
                1 => &[3, 2, 1, 0],
                2 => &[1, 1, 4, 1],
                3 => &[5],
                _ => &[],
            };
            order.iter().map(|&i| inputs[i].get()).sum::<i64>()
        })
    };
    let expected = |sel: usize, values: &[i64]| -> i64 {
        let order: &[usize] = match sel {
            0 => &[0, 1, 2, 3],
            1 => &[3, 2, 1, 0],
            2 => &[1, 1, 4, 1],
            3 => &[5],
            _ => &[],
        };
        order.iter().map(|&i| values[i]).sum()
    };
    let mut values: Vec<i64> = (0..6).collect();
    for step in 0..60usize {
        let sel = (step * 7 + step / 3) % 5;
        selector.set(sel);
        let i = step % 6;
        values[i] += 10;
        inputs[i].set(values[i]);
        assert_eq!(reads.get(), expected(sel, &values));
        assert_edges_consistent();
    }
    // A source that is no longer read has no edge to the memo.
    selector.set(4);
    assert_eq!(reads.get(), 0);
    for input in &inputs {
        let rt = RUNTIME.lock();
        assert!(rt.nodes[input.id].observers.is_empty());
    }
}

#[test]
fn rereading_a_source_adds_one_edge() {
    let _g = TEST_LOCK.lock();
    let s = create_signal(1i64);
    let m = create_memo(move || (0..50).map(|_| s.get()).sum::<i64>());
    assert_eq!(m.get(), 50);
    let rt = RUNTIME.lock();
    assert_eq!(rt.nodes[s.id].observers.len(), 1);
    assert_eq!(rt.nodes[m.id].sources.len(), 1);
}

#[test]
fn memo_chain_100k_deep_does_not_overflow_the_stack() {
    let _g = TEST_LOCK.lock();
    const DEPTH: i64 = 100_000;
    // A fresh thread has the default 2 MiB stack, like a test thread.
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let scope = create_scope();
            let head = create_signal(0i64);
            let tail = run_in_scope(scope, || {
                let mut prev = create_memo(move || head.get() + 1);
                // Settle each link as it is built so no read nests user code.
                prev.get();
                for _ in 1..DEPTH {
                    let upstream = prev;
                    prev = create_memo(move || upstream.get() + 1);
                    prev.get();
                }
                prev
            });
            assert_eq!(tail.get(), DEPTH);
            head.set(10);
            // The check walk descends the whole chain without recursing.
            assert_eq!(tail.get(), DEPTH + 10);
            scope.dispose();
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn owner_tree_100k_deep_disposes_without_recursion() {
    let _g = TEST_LOCK.lock();
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            let root = create_scope();
            let mut current = root;
            for _ in 0..100_000 {
                current = run_in_scope(current, create_scope);
            }
            let leaf = run_in_scope(current, || create_signal(1i32));
            root.dispose();
            assert_eq!(leaf.try_get(), None);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn broadcast_to_10k_effects_queues_and_runs_each_once() {
    let _g = TEST_LOCK.lock();
    const N: usize = 10_000;
    let s = create_signal(0i32);
    let runs: Arc<Vec<AtomicUsize>> = Arc::new((0..N).map(|_| AtomicUsize::new(0)).collect());
    let scope = create_scope();
    run_in_scope(scope, || {
        for i in 0..N {
            let runs = runs.clone();
            create_effect(move || {
                let _ = s.get();
                runs[i].fetch_add(1, SeqCst);
            });
        }
    });
    Runtime::flush();
    assert!(runs.iter().all(|r| r.load(SeqCst) == 1));
    s.set(1);
    s.set(2); // a second write before the flush must not enqueue anything again
    {
        let rt = RUNTIME.lock();
        let mut queued: Vec<NodeId> = rt.pending_effects.clone();
        assert_eq!(queued.len(), N);
        queued.sort();
        queued.dedup();
        assert_eq!(queued.len(), N, "an effect was queued twice");
    }
    Runtime::flush();
    assert!(runs.iter().all(|r| r.load(SeqCst) == 2));
    assert!(RUNTIME.lock().pending_effects.is_empty());
    scope.dispose();
}

/// A value whose `Drop` reads a signal, like a widget handle that peeks state.
struct ReadsOnDrop {
    probe: Signal<i32>,
    panic: Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for ReadsOnDrop {
    fn drop(&mut self) {
        let _ = self.probe.try_get();
        if self.panic.load(SeqCst) {
            panic!("expected panic in Drop");
        }
    }
}

#[test]
fn drop_of_previous_run_child_is_not_a_dependency() {
    let _g = TEST_LOCK.lock();
    let trigger = create_signal(0i32);
    let probe = create_signal(0i32);
    let runs = Arc::new(AtomicUsize::new(0));
    let panic = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let effect = {
        let runs = runs.clone();
        let panic = panic.clone();
        create_effect(move || {
            let _ = trigger.get();
            runs.fetch_add(1, SeqCst);
            let _owned = create_signal(ReadsOnDrop {
                probe,
                panic: panic.clone(),
            });
        })
    };
    trigger.set(1);
    Runtime::flush(); // the first child drops here and reads `probe`
    assert_eq!(runs.load(SeqCst), 2);
    probe.set(5);
    Runtime::flush();
    assert_eq!(runs.load(SeqCst), 2, "Drop read became a dependency");
    assert!(RUNTIME.lock().nodes[probe.id].observers.is_empty());

    // A panicking Drop surfaces, but the effect is not left RUNNING.
    panic.store(true, SeqCst);
    trigger.set(2);
    assert!(catch_unwind(Runtime::flush).is_err());
    panic.store(false, SeqCst);
    assert!(!RUNTIME.lock().nodes[effect.id]
        .flags
        .contains(NodeFlags::RUNNING));
    trigger.set(3);
    Runtime::flush();
    assert_eq!(runs.load(SeqCst), 4);
    assert_edges_consistent();
}

#[test]
fn subscription_track_disposes_nodes_created_by_the_previous_run() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let sub = group.subscribe(1);
    let created = Arc::new(Mutex::new(Vec::<NodeId>::new()));
    for _ in 0..3 {
        sub.track(|| {
            source.get();
            created.lock().push(create_signal(7i32).id());
        });
    }
    let created = created.lock().clone();
    assert_eq!(created.len(), 3);
    let rt = RUNTIME.lock();
    assert!(!rt.nodes.contains_key(created[0]));
    assert!(!rt.nodes.contains_key(created[1]));
    assert!(rt.nodes.contains_key(created[2]));
    assert_eq!(rt.scopes.get(sub.handle.id).map(|l| l.len()), Some(1));
}

#[test]
fn subscription_group_removal_patches_member_indexes() {
    let _g = TEST_LOCK.lock();
    let source = create_signal(0i32);
    let group = SubscriptionGroup::new();
    let subs: Vec<_> = (0..50u64).map(|k| group.subscribe(k)).collect();
    for sub in &subs {
        sub.track(|| source.get());
    }
    // Remove from the middle and the front; moved members must stay removable.
    for sub in subs.iter().step_by(3) {
        sub.unsubscribe();
    }
    source.set(1);
    Runtime::flush();
    let mut ready = Vec::new();
    group.drain_ready_into(&mut ready);
    ready.sort();
    let expected: Vec<u64> = (0..50).filter(|k| k % 3 != 0).collect();
    assert_eq!(ready, expected);
    drop(subs);
    group.drain_ready_into(&mut ready);
    assert_edges_consistent();
}

//! Randomized simulation of the reactive graph against a plain model.
//!
//! Each seed builds a random graph of signals, memos (static, dynamic and
//! equality-gated dependencies), effects and any number of effects that write
//! signals, then applies a random-length program of writes, reads, flushes and
//! scope disposals. After every
//! step the graph must agree with a model that recomputes everything eagerly.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use std::sync::{Arc, Mutex};

use schnellui::signal::{create_effect, create_scope, dispose, run_in_scope, Runtime};
use schnellui::testing::for_each_seed;
use schnellui::Store;

#[allow(dead_code)]
mod common;
use common::signals::{run, GraphSpec};

#[test]
fn random_reactive_graphs_match_an_eager_model() {
    for_each_seed("signals/random-graph", 200, |rng| {
        run(&GraphSpec::random(rng))
    });
}

#[test]
fn store_selectors_only_wake_effects_for_their_slice() {
    for_each_seed("signals/store-selectors", 100, |rng| {
        let scope = create_scope();
        run_in_scope(scope, || {
            let store = Store::new((0i64, 0i64));
            let left = store.select(|s| s.0);
            let right = store.select(|s| s.1);
            let sum = store.select(|s| s.0 + s.1);
            let seen: Arc<Mutex<[Vec<i64>; 3]>> = Arc::default();
            for (k, sel) in [left.clone(), right.clone(), sum.clone()]
                .into_iter()
                .enumerate()
            {
                let seen = seen.clone();
                create_effect(move || seen.lock().unwrap()[k].push(sel.get()));
            }
            let mut model = (0i64, 0i64);
            for _ in 0..10 + rng.index(61) {
                let before = model;
                match rng.below(3) {
                    0 => {
                        let v = rng.range_i64(-4, 4);
                        store.update(|s| s.0 = v);
                        model.0 = v;
                    }
                    1 => {
                        let v = rng.range_i64(-4, 4);
                        store.update(|s| s.1 = v);
                        model.1 = v;
                    }
                    _ => {
                        let (a, b) = (rng.range_i64(-4, 4), rng.range_i64(-4, 4));
                        store.set((a, b));
                        model = (a, b);
                    }
                }
                let counts: Vec<usize> = seen.lock().unwrap().iter().map(Vec::len).collect();
                Runtime::flush();
                let seen = seen.lock().unwrap();
                let slices = [
                    (before.0, model.0),
                    (before.1, model.1),
                    (before.0 + before.1, model.0 + model.1),
                ];
                for (k, (old, new)) in slices.into_iter().enumerate() {
                    let ran = seen[k].len() - counts[k];
                    assert_eq!(ran, usize::from(old != new), "selector {k}: {old} -> {new}");
                    assert_eq!(*seen[k].last().unwrap(), new);
                }
                assert_eq!(left.get(), model.0);
                assert_eq!(right.get(), model.1);
                assert_eq!(sum.get(), model.0 + model.1);
            }
        });
        dispose(scope);
    });
}

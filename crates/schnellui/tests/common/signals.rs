//! A reactive-graph program and the eager model it must agree with. Shared by
//! the seeded sweep in `sim_signals.rs` and the shrinking property test in
//! `prop_signals.rs`.
//!
//! Node references are raw numbers taken modulo the number of earlier nodes,
//! so any spec is valid and a shrunk spec stays valid.

use std::sync::{Arc, Mutex};

use schnellui::signal::{
    create_effect, create_memo, create_memo_equals, create_scope, create_signal, dispose,
    run_in_scope, Memo, Runtime, Scope, Signal,
};
use schnellui::testing::SimRng;

/// How a memo derives its value from earlier nodes.
#[derive(Clone, Copy, Debug)]
pub enum Rule {
    Sum(usize, usize),
    Scaled(usize, i64),
    /// Reads `a` when `cond` is even, else `b`: the dependency set changes.
    Choose {
        cond: usize,
        a: usize,
        b: usize,
    },
    /// `value % 3` behind an equality gate, so equal results stop propagation.
    Gated(usize),
}

impl Rule {
    fn deps(self) -> Vec<usize> {
        match self {
            Rule::Sum(a, b) => vec![a, b],
            Rule::Scaled(a, _) | Rule::Gated(a) => vec![a],
            Rule::Choose { cond, a, b } => vec![cond, a, b],
        }
    }

    /// Resolves raw references against the `n` nodes built so far.
    fn resolve(self, n: usize) -> Rule {
        match self {
            Rule::Sum(a, b) => Rule::Sum(a % n, b % n),
            Rule::Scaled(a, k) => Rule::Scaled(a % n, k),
            Rule::Choose { cond, a, b } => Rule::Choose {
                cond: cond % n,
                a: a % n,
                b: b % n,
            },
            Rule::Gated(a) => Rule::Gated(a % n),
        }
    }
}

fn eval(rule: Rule, v: &impl Fn(usize) -> i64) -> i64 {
    match rule {
        Rule::Sum(a, b) => v(a).wrapping_add(v(b)),
        Rule::Scaled(a, k) => v(a).wrapping_mul(k),
        Rule::Choose { cond, a, b } => {
            if v(cond).rem_euclid(2) == 0 {
                v(a)
            } else {
                v(b)
            }
        }
        Rule::Gated(a) => v(a).rem_euclid(3),
    }
}

/// One node of the graph, in creation order.
#[derive(Clone, Copy, Debug)]
pub enum NodeSpec {
    /// A signal the program may write.
    Input(i64),
    Memo(Rule),
    /// A signal that an effect keeps equal to `source * factor`. Writes made
    /// inside a flush must reach everything downstream of it.
    Mirror {
        source: usize,
        factor: i64,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum Op {
    /// Writes an input, chosen modulo the number of inputs.
    Set {
        input: usize,
        value: i64,
    },
    Add {
        input: usize,
        delta: i64,
    },
    /// Reads a node between flushes.
    Pull(usize),
    /// Disposes the scope of a watching effect.
    Dispose(usize),
    Flush,
}

#[derive(Clone, Debug)]
pub struct GraphSpec {
    pub nodes: Vec<NodeSpec>,
    /// Nodes watched by an effect that records every value it sees.
    pub watches: Vec<usize>,
    pub ops: Vec<Op>,
}

impl GraphSpec {
    /// A random program, sized like a small app's reactive state.
    pub fn random(rng: &mut SimRng) -> GraphSpec {
        let mut nodes = Vec::new();
        for _ in 0..2 + rng.index(4) {
            nodes.push(NodeSpec::Input(rng.range_i64(-5, 5)));
        }
        for _ in 0..2 + rng.index(14) {
            // Mirrors land anywhere, so some read memos and some chain.
            if rng.chance(1, 5) {
                nodes.push(NodeSpec::Mirror {
                    source: rng.index(64),
                    factor: rng.range_i64(-2, 2),
                });
            }
            let rule = match rng.below(4) {
                0 => Rule::Sum(rng.index(64), rng.index(64)),
                1 => Rule::Scaled(rng.index(64), rng.range_i64(-3, 3)),
                2 => Rule::Choose {
                    cond: rng.index(64),
                    a: rng.index(64),
                    b: rng.index(64),
                },
                _ => Rule::Gated(rng.index(64)),
            };
            nodes.push(NodeSpec::Memo(rule));
        }
        // Late inputs are only read by memos created after them.
        for _ in 0..rng.index(3) {
            let at = 1 + rng.index(nodes.len());
            nodes.insert(at, NodeSpec::Input(rng.range_i64(-5, 5)));
        }
        let watches = (0..rng.index(8)).map(|_| rng.index(64)).collect();
        let mut ops: Vec<Op> = (0..10 + rng.index(111))
            .map(|_| match rng.below(10) {
                0..=3 => Op::Set {
                    input: rng.index(8),
                    value: rng.range_i64(-6, 6),
                },
                4 => Op::Add {
                    input: rng.index(8),
                    delta: rng.range_i64(-2, 2),
                },
                5 | 6 => Op::Pull(rng.index(64)),
                7 => Op::Dispose(rng.index(8)),
                _ => Op::Flush,
            })
            .collect();
        // End on a flush so every program checks its effects at least once.
        ops.push(Op::Flush);
        GraphSpec {
            nodes,
            watches,
            ops,
        }
    }
}

#[derive(Clone, Copy)]
enum Handle {
    Signal(Signal<i64>),
    Memo(Memo<i64>),
}

impl Handle {
    fn get(self) -> i64 {
        match self {
            Handle::Signal(s) => s.get(),
            Handle::Memo(m) => m.get(),
        }
    }
}

fn eq_i64(a: &dyn std::any::Any, b: &dyn std::any::Any) -> bool {
    a.downcast_ref::<i64>() == b.downcast_ref::<i64>()
}

/// What each node should hold. Inputs and mirrors are stored; memos recompute.
struct Model {
    nodes: Vec<NodeSpec>,
    stored: Vec<i64>,
}

impl Model {
    fn value(&self, i: usize) -> i64 {
        match self.nodes[i] {
            NodeSpec::Memo(rule) => eval(rule, &|j| self.value(j)),
            _ => self.stored[i],
        }
    }

    /// The distinct mirrors upstream of node `i`, through memo reads and the
    /// effects that copy a mirror's source into it. Each one can add a flush
    /// round in which effects behind it run again.
    fn mirrors_upstream(&self, i: usize) -> usize {
        fn walk(model: &Model, i: usize, seen: &mut Vec<bool>) {
            if std::mem::replace(&mut seen[i], true) {
                return;
            }
            let deps = match model.nodes[i] {
                NodeSpec::Memo(rule) => rule.deps(),
                NodeSpec::Mirror { source, .. } => vec![source],
                NodeSpec::Input(_) => Vec::new(),
            };
            for d in deps {
                walk(model, d, seen);
            }
        }
        let mut seen = vec![false; self.nodes.len()];
        walk(self, i, &mut seen);
        (0..self.nodes.len())
            .filter(|&n| seen[n] && matches!(self.nodes[n], NodeSpec::Mirror { .. }))
            .count()
    }

    /// Mirrors catch up at a flush. Sources always come earlier, so one pass
    /// in creation order reaches the fixed point, even for chained mirrors.
    fn flush(&mut self) {
        for i in 0..self.nodes.len() {
            if let NodeSpec::Mirror { source, factor } = self.nodes[i] {
                self.stored[i] = self.value(source).wrapping_mul(factor);
            }
        }
    }
}

struct Watch {
    node: usize,
    gated: bool,
    scope: Scope,
    disposed: bool,
    runs: Arc<Mutex<Vec<i64>>>,
}

/// Builds the graph in a fresh scope, runs the program, checks every step
/// against the model, then disposes everything.
pub fn run(spec: &GraphSpec) {
    let scope = create_scope();
    run_in_scope(scope, || run_in(spec));
    dispose(scope);
}

fn run_in(spec: &GraphSpec) {
    let mut handles: Vec<Handle> = Vec::new();
    let mut model = Model {
        nodes: Vec::new(),
        stored: Vec::new(),
    };
    let mut inputs = Vec::new();
    for (i, node) in spec.nodes.iter().enumerate() {
        // The first node is always an input, so every reference has a target.
        let node = match (i, *node) {
            (0, NodeSpec::Input(v)) => NodeSpec::Input(v),
            (0, _) => NodeSpec::Input(0),
            (n, NodeSpec::Memo(rule)) => NodeSpec::Memo(rule.resolve(n)),
            (n, NodeSpec::Mirror { source, factor }) => NodeSpec::Mirror {
                source: source % n,
                factor,
            },
            (_, input) => input,
        };
        model.nodes.push(node);
        match node {
            NodeSpec::Input(v) => {
                inputs.push(i);
                model.stored.push(v);
                handles.push(Handle::Signal(create_signal(v)));
            }
            NodeSpec::Memo(rule) => {
                model.stored.push(0);
                let deps = handles.clone();
                let compute = move || eval(rule, &|j| deps[j].get());
                handles.push(Handle::Memo(match rule {
                    Rule::Gated(_) => create_memo_equals(compute, eq_i64),
                    _ => create_memo(compute),
                }));
            }
            NodeSpec::Mirror { source, factor } => {
                let initial = model.value(source).wrapping_mul(factor);
                model.stored.push(initial);
                let mirror = create_signal(initial);
                let source = handles[source];
                create_effect(move || mirror.set(source.get().wrapping_mul(factor)));
                handles.push(Handle::Signal(mirror));
            }
        }
    }

    let mut watches: Vec<Watch> = Vec::new();
    for &raw in &spec.watches {
        let node = raw % handles.len();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let scope = create_scope();
        let (handle, sink) = (handles[node], runs.clone());
        run_in_scope(scope, || {
            create_effect(move || sink.lock().unwrap().push(handle.get()));
        });
        watches.push(Watch {
            node,
            gated: matches!(model.nodes[node], NodeSpec::Memo(Rule::Gated(_))),
            scope,
            disposed: false,
            runs,
        });
    }

    Runtime::flush();
    for w in &watches {
        let runs = w.runs.lock().unwrap();
        assert_eq!(runs.len(), 1, "an effect runs once when created");
        assert_eq!(runs[0], model.value(w.node));
    }

    let mut flushed_values: Vec<i64> = watches.iter().map(|w| model.value(w.node)).collect();
    // A pull between flushes can compute a gated memo at an intermediate value
    // (A -> B, then back to A by the flush). The effect then legitimately sees
    // a change from B, so pruning is only checked when nothing pulled.
    let mut pulled_since_flush = false;
    for (step, op) in spec.ops.iter().enumerate() {
        match *op {
            Op::Set { input, value } => {
                let i = inputs[input % inputs.len()];
                let Handle::Signal(s) = handles[i] else {
                    unreachable!()
                };
                s.set(value);
                model.stored[i] = value;
            }
            Op::Add { input, delta } => {
                let i = inputs[input % inputs.len()];
                let Handle::Signal(s) = handles[i] else {
                    unreachable!()
                };
                s.update(|v| *v = v.wrapping_add(delta));
                model.stored[i] = model.stored[i].wrapping_add(delta);
            }
            Op::Pull(raw) => {
                let i = raw % handles.len();
                pulled_since_flush = true;
                assert_eq!(
                    handles[i].get(),
                    model.value(i),
                    "step {step}: pull of node {i} ({:?})",
                    model.nodes[i]
                );
            }
            Op::Dispose(raw) => {
                let live: Vec<usize> = (0..watches.len())
                    .filter(|&k| !watches[k].disposed)
                    .collect();
                if !live.is_empty() {
                    let k = live[raw % live.len()];
                    dispose(watches[k].scope);
                    watches[k].disposed = true;
                }
            }
            Op::Flush => {
                let before: Vec<usize> = watches
                    .iter()
                    .map(|w| w.runs.lock().unwrap().len())
                    .collect();
                Runtime::flush();
                model.flush();
                for (k, w) in watches.iter().enumerate() {
                    let runs = w.runs.lock().unwrap();
                    let expected = model.value(w.node);
                    if w.disposed {
                        assert_eq!(runs.len(), before[k], "step {step}: disposed effect ran");
                        continue;
                    }
                    // Each mirror upstream writes a signal mid-flush, so an
                    // effect behind it may run once more per mirror.
                    let rounds = 1 + model.mirrors_upstream(w.node);
                    assert!(
                        runs.len() <= before[k] + rounds,
                        "step {step}: effect on node {} ran {} times in one flush",
                        w.node,
                        runs.len() - before[k]
                    );
                    assert_eq!(
                        *runs.last().unwrap(),
                        expected,
                        "step {step}: effect on node {} ({:?}) missed an update",
                        w.node,
                        model.nodes[w.node]
                    );
                    if w.gated
                        && rounds == 1
                        && !pulled_since_flush
                        && expected == flushed_values[k]
                    {
                        assert_eq!(
                            runs.len(),
                            before[k],
                            "step {step}: equal gated value still re-ran its effect"
                        );
                    }
                    flushed_values[k] = expected;
                }
                pulled_since_flush = false;
            }
        }
    }
}

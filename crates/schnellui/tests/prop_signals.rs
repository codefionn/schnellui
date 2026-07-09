//! Property tests for the reactive graph. Unlike the seeded sweep in
//! `sim_signals.rs`, graphs here are larger, chain several effect-written
//! signals, and a failure shrinks to the smallest graph and program that
//! still breaks the property.
//!
//! `PROPTEST_CASES` scales the run. Failing inputs are saved next to this
//! file as `prop_signals.regressions` and replayed first on the next run.
//! The signal runtime is process-global; run with `--test-threads=1`.

use proptest::prelude::*;

#[allow(dead_code)]
mod common;
use common::signals::{run, GraphSpec, NodeSpec, Op, Rule};

fn rule() -> impl Strategy<Value = Rule> {
    let node = 0usize..64;
    prop_oneof![
        (node.clone(), node.clone()).prop_map(|(a, b)| Rule::Sum(a, b)),
        (node.clone(), -3i64..=3).prop_map(|(a, k)| Rule::Scaled(a, k)),
        (node.clone(), node.clone(), node.clone()).prop_map(|(cond, a, b)| Rule::Choose {
            cond,
            a,
            b
        }),
        node.prop_map(Rule::Gated),
    ]
}

fn node() -> impl Strategy<Value = NodeSpec> {
    prop_oneof![
        2 => (-5i64..=5).prop_map(NodeSpec::Input),
        6 => rule().prop_map(NodeSpec::Memo),
        1 => (0usize..64, -2i64..=2)
            .prop_map(|(source, factor)| NodeSpec::Mirror { source, factor }),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0usize..8, -6i64..=6).prop_map(|(input, value)| Op::Set { input, value }),
        1 => (0usize..8, -2i64..=2).prop_map(|(input, delta)| Op::Add { input, delta }),
        2 => (0usize..64).prop_map(Op::Pull),
        1 => (0usize..8).prop_map(Op::Dispose),
        2 => Just(Op::Flush),
    ]
}

fn graph() -> impl Strategy<Value = GraphSpec> {
    (
        prop::collection::vec(node(), 1..40),
        prop::collection::vec(0usize..64, 0..8),
        prop::collection::vec(op(), 0..120),
    )
        .prop_map(|(nodes, watches, mut ops)| {
            // End on a flush so every program checks its effects at least once.
            ops.push(Op::Flush);
            GraphSpec {
                nodes,
                watches,
                ops,
            }
        })
}

proptest! {
    #![proptest_config(common::cases(1000))]

    #[test]
    fn any_graph_and_program_agrees_with_the_eager_model(spec in graph()) {
        run(&spec);
    }
}

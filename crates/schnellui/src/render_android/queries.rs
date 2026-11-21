//! Size queries that name an ancestor. The target id is known only after the
//! diff has assigned ids, so SHOW_TARGET is resolved and diffed here.

use std::collections::HashMap;

use schnellui::template::ComponentRef;

use super::ops::{prop, Op};
use super::tree::{AndroidNode, QueryTarget, Value};

/// Sets SHOW_TARGET on every node with a query and appends the ops that bring
/// the host in line with `old`, the previous resolved tree.
///
/// A parent query is target 0. A component query is the id of the nearest
/// ancestor with that reference, or no query when there is none.
pub(crate) fn resolve(old: Option<&AndroidNode>, new: &mut AndroidNode, ops: &mut Vec<Op>) {
    let mut before = HashMap::new();
    if let Some(old) = old {
        collect(old, &mut before);
    }
    let mut ancestors = Vec::new();
    walk(new, &mut ancestors, &before, ops);
}

fn collect(node: &AndroidNode, out: &mut HashMap<u32, i32>) {
    if let Some(Value::I32(target)) = node.props.get(&prop::SHOW_TARGET) {
        out.insert(node.id, *target);
    }
    for child in &node.children {
        collect(child, out);
    }
}

fn walk(
    node: &mut AndroidNode,
    ancestors: &mut Vec<(ComponentRef, u32)>,
    before: &HashMap<u32, i32>,
    ops: &mut Vec<Op>,
) {
    let target = match node.query {
        None => None,
        Some(QueryTarget::Parent) => Some(0),
        Some(QueryTarget::Component(reference)) => ancestors
            .iter()
            .rev()
            .find(|(r, _)| *r == reference)
            .map(|(_, id)| *id as i32),
    };
    match (before.get(&node.id).copied(), target) {
        (old, Some(new)) => {
            node.props.insert(prop::SHOW_TARGET, Value::I32(new));
            if old != Some(new) {
                ops.push(Op::SetI32 {
                    id: node.id,
                    prop: prop::SHOW_TARGET,
                    value: new,
                });
            }
        }
        (Some(_), None) => ops.push(Op::Clear {
            id: node.id,
            prop: prop::SHOW_TARGET,
        }),
        (None, None) => {}
    }
    let pushed = node.component_ref.map(|r| ancestors.push((r, node.id)));
    for child in &mut node.children {
        walk(child, ancestors, before, ops);
    }
    if pushed.is_some() {
        ancestors.pop();
    }
}

//! The tree diff. It turns two node trees into ops and carries ids over.

use std::collections::HashMap;

use super::ops::{prop, Op};
use super::tree::AndroidNode;

/// Hands out ids. Ids start at 1 and are never reused within one mount.
pub(crate) struct IdAlloc(pub u32);

impl IdAlloc {
    fn next(&mut self) -> u32 {
        self.0 += 1;
        self.0
    }
}

/// Diffs the new root against the old one and appends ops. Matched nodes keep
/// their ids, new nodes get fresh ones.
pub(crate) fn diff_root(
    old: Option<&AndroidNode>,
    new: &mut AndroidNode,
    ids: &mut IdAlloc,
    ops: &mut Vec<Op>,
) {
    match old {
        Some(old) if same_view(old, new) => {
            new.id = old.id;
            diff_node(old, new, ids, ops);
        }
        Some(old) => {
            create_subtree(new, ids, ops);
            ops.push(Op::SetRoot { id: new.id });
            ops.push(Op::Remove { id: old.id });
        }
        None => {
            create_subtree(new, ids, ops);
            ops.push(Op::SetRoot { id: new.id });
        }
    }
}

/// Whether the host can keep the view: a kind or theme change needs a new one.
fn same_view(old: &AndroidNode, new: &AndroidNode) -> bool {
    old.kind == new.kind && old.theme == new.theme
}

fn create_op(node: &AndroidNode) -> Op {
    if node.theme == 0 {
        Op::Create {
            id: node.id,
            kind: node.kind,
        }
    } else {
        Op::CreateThemed {
            id: node.id,
            kind: node.kind,
            theme: node.theme,
        }
    }
}

/// Appends ops that build `root` and everything below it from nothing, ending
/// in SET_ROOT. Every node must already have an id.
pub(crate) fn emit_full(root: &AndroidNode, ops: &mut Vec<Op>) {
    emit_node(root, ops);
    ops.push(Op::SetRoot { id: root.id });
}

fn emit_node(node: &AndroidNode, ops: &mut Vec<Op>) {
    ops.push(create_op(node));
    for (prop, value) in &node.props {
        ops.push(value.set_op(node.id, *prop));
    }
    for (index, child) in node.children.iter().enumerate() {
        emit_node(child, ops);
        ops.push(Op::Insert {
            parent: node.id,
            child: child.id,
            index: index as u32,
        });
    }
}

fn create_subtree(node: &mut AndroidNode, ids: &mut IdAlloc, ops: &mut Vec<Op>) {
    node.id = ids.next();
    ops.push(create_op(node));
    for (prop, value) in &node.props {
        ops.push(value.set_op(node.id, *prop));
    }
    let parent = node.id;
    for (index, child) in node.children.iter_mut().enumerate() {
        create_subtree(child, ids, ops);
        ops.push(Op::Insert {
            parent,
            child: child.id,
            index: index as u32,
        });
    }
}

fn diff_node(old: &AndroidNode, new: &mut AndroidNode, ids: &mut IdAlloc, ops: &mut Vec<Op>) {
    diff_props(old, new, ops);
    diff_children(old, new, ids, ops);
}

/// SHOW_TARGET is resolved after the diff, so the diff leaves it alone.
fn diff_props(old: &AndroidNode, new: &AndroidNode, ops: &mut Vec<Op>) {
    let id = new.id;
    let mut old_iter = old
        .props
        .iter()
        .filter(|(p, _)| **p != prop::SHOW_TARGET)
        .peekable();
    let mut new_iter = new
        .props
        .iter()
        .filter(|(p, _)| **p != prop::SHOW_TARGET)
        .peekable();
    loop {
        match (old_iter.peek(), new_iter.peek()) {
            (None, None) => break,
            (Some(&(prop, _)), None) => {
                ops.push(Op::Clear { id, prop: *prop });
                old_iter.next();
            }
            (None, Some(&(prop, value))) => {
                ops.push(value.set_op(id, *prop));
                new_iter.next();
            }
            (Some(&(old_prop, old_value)), Some(&(new_prop, new_value))) => {
                if old_prop < new_prop {
                    ops.push(Op::Clear {
                        id,
                        prop: *old_prop,
                    });
                    old_iter.next();
                } else if new_prop < old_prop {
                    ops.push(new_value.set_op(id, *new_prop));
                    new_iter.next();
                } else {
                    if old_value != new_value {
                        ops.push(new_value.set_op(id, *new_prop));
                    }
                    old_iter.next();
                    new_iter.next();
                }
            }
        }
    }
}

/// For each new child, the index of the old child it continues, if any.
/// Keyed children match by key, unkeyed ones by position among the unkeyed
/// siblings. A kind change is no match.
fn match_children(old: &[AndroidNode], new: &[AndroidNode]) -> Vec<Option<usize>> {
    let mut by_key: HashMap<&str, usize> = HashMap::new();
    let mut unkeyed = Vec::new();
    for (i, child) in old.iter().enumerate() {
        match &child.key {
            Some(key) => {
                by_key.entry(key.as_str()).or_insert(i);
            }
            None => unkeyed.push(i),
        }
    }
    let mut used = vec![false; old.len()];
    let mut next_unkeyed = 0;
    new.iter()
        .map(|child| {
            let candidate = match &child.key {
                Some(key) => by_key.get(key.as_str()).copied(),
                None => {
                    let c = unkeyed.get(next_unkeyed).copied();
                    next_unkeyed += 1;
                    c
                }
            };
            candidate
                .filter(|&j| !used[j] && same_view(&old[j], child))
                .inspect(|&j| used[j] = true)
        })
        .collect()
}

/// Indices into `seq` of one longest strictly increasing subsequence.
fn longest_increasing(seq: &[usize]) -> Vec<usize> {
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; seq.len()];
    for (i, &v) in seq.iter().enumerate() {
        let at = tails.partition_point(|&t| seq[t] < v);
        if at > 0 {
            prev[i] = tails[at - 1];
        }
        if at == tails.len() {
            tails.push(i);
        } else {
            tails[at] = i;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut at = tails.last().copied();
    while let Some(i) = at {
        out.push(i);
        at = (prev[i] != usize::MAX).then_some(prev[i]);
    }
    out.reverse();
    out
}

fn diff_children(old: &AndroidNode, new: &mut AndroidNode, ids: &mut IdAlloc, ops: &mut Vec<Op>) {
    let parent = new.id;
    let matches = match_children(&old.children, &new.children);

    let mut kept = vec![false; old.children.len()];
    for j in matches.iter().flatten() {
        kept[*j] = true;
    }
    for (j, child) in old.children.iter().enumerate() {
        if !kept[j] {
            ops.push(Op::Remove { id: child.id });
        }
    }

    // Children that stay put are a longest increasing run of old positions.
    let matched_at: Vec<usize> = (0..matches.len())
        .filter(|&i| matches[i].is_some())
        .collect();
    let old_positions: Vec<usize> = matched_at.iter().map(|&i| matches[i].unwrap()).collect();
    let mut stays = vec![false; matches.len()];
    for k in longest_increasing(&old_positions) {
        stays[matched_at[k]] = true;
    }

    // The host's child order while we go: surviving old children in old order.
    let mut current: Vec<u32> = old
        .children
        .iter()
        .enumerate()
        .filter(|(j, _)| kept[*j])
        .map(|(_, c)| c.id)
        .collect();
    let mut previous: Option<u32> = None;
    for (i, child) in new.children.iter_mut().enumerate() {
        match matches[i] {
            Some(j) => {
                let old_child = &old.children[j];
                child.id = old_child.id;
                diff_node(old_child, child, ids, ops);
                if !stays[i] {
                    current.retain(|&id| id != child.id);
                    let index = insert_index(&current, previous);
                    current.insert(index, child.id);
                    ops.push(Op::Insert {
                        parent,
                        child: child.id,
                        index: index as u32,
                    });
                }
            }
            None => {
                create_subtree(child, ids, ops);
                let index = insert_index(&current, previous);
                current.insert(index, child.id);
                ops.push(Op::Insert {
                    parent,
                    child: child.id,
                    index: index as u32,
                });
            }
        }
        previous = Some(child.id);
    }
}

/// The index right after `previous` in `current`, or 0 without one.
fn insert_index(current: &[u32], previous: Option<u32>) -> usize {
    match previous {
        None => 0,
        Some(prev) if current.last() == Some(&prev) => current.len(),
        Some(prev) => current
            .iter()
            .position(|&id| id == prev)
            .map_or(0, |p| p + 1),
    }
}

//! Indexed state pairing for structural remounts.
//!
//! A remount is a deliberately coarse operation, but restoring user-owned state
//! must not multiply that cost by the number of editors or host bindings. This
//! module walks each scene once, reserves explicit component references first,
//! then pairs the remaining exact semantic peers in tree order. The resulting
//! map is bijective: a fallback match can never steal a reference-owned target.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use schnellui_scene::{Scene, WidgetId, WidgetKind};
use slotmap::SecondaryMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SemanticHash {
    kind: WidgetKind,
    role: u16,
    name_hash: u64,
}

#[derive(Default)]
struct SemanticGroup {
    name: Option<String>,
    previous: Vec<WidgetId>,
    replacement: Vec<WidgetId>,
}

/// Precomputed old-node → new-node counterparts for one replacement.
pub(crate) struct CounterpartMap {
    counterparts: SecondaryMap<WidgetId, WidgetId>,
}

impl CounterpartMap {
    pub(crate) fn new(
        previous: &Scene,
        replacement: &Scene,
        candidates: impl IntoIterator<Item = WidgetId>,
    ) -> Self {
        let mut candidate_ids = SecondaryMap::<WidgetId, ()>::new();
        let mut groups: HashMap<SemanticHash, Vec<SemanticGroup>> = HashMap::new();

        for id in candidates {
            if candidate_ids.insert(id, ()).is_some() {
                continue;
            }
            let Some((key, name)) = semantic_identity(previous, id) else {
                continue;
            };
            let bucket = groups.entry(key).or_default();
            if !bucket.iter().any(|group| group.name.as_deref() == name) {
                bucket.push(SemanticGroup {
                    name: name.map(ToOwned::to_owned),
                    ..SemanticGroup::default()
                });
            }
        }

        let mut counterparts = SecondaryMap::new();
        if candidate_ids.is_empty() {
            return Self { counterparts };
        }
        let mut reserved_replacements = SecondaryMap::<WidgetId, ()>::new();

        // Explicit application identity is authoritative. Reserve those targets
        // before occurrence matching so fallback peers cannot claim them.
        for previous_id in candidate_ids.keys() {
            let Some(reference) = previous.component_ref(previous_id) else {
                continue;
            };
            let Some(replacement_id) = replacement.resolve_ref(reference) else {
                continue;
            };
            let same_kind = previous
                .node(previous_id)
                .zip(replacement.node(replacement_id))
                .is_some_and(|(old, new)| old.kind == new.kind);
            if same_kind {
                counterparts.insert(previous_id, replacement_id);
                reserved_replacements.insert(replacement_id, ());
            }
        }

        if groups.is_empty() {
            return Self { counterparts };
        }

        collect_semantic_peers(previous, &mut groups, true);
        collect_semantic_peers(replacement, &mut groups, false);

        for bucket in groups.values() {
            for group in bucket {
                let mut replacement_index = 0;
                for &previous_id in &group.previous {
                    // Explicit counterparts and their targets are omitted from
                    // fallback occurrence matching. Every other semantic peer
                    // consumes one unreserved replacement occurrence, even if
                    // that previous peer does not own state being restored.
                    if counterparts.contains_key(previous_id) {
                        continue;
                    }
                    while group
                        .replacement
                        .get(replacement_index)
                        .is_some_and(|id| reserved_replacements.contains_key(*id))
                    {
                        replacement_index += 1;
                    }
                    let Some(&replacement_id) = group.replacement.get(replacement_index) else {
                        break;
                    };
                    replacement_index += 1;
                    if candidate_ids.contains_key(previous_id) {
                        counterparts.insert(previous_id, replacement_id);
                        reserved_replacements.insert(replacement_id, ());
                    }
                }
            }
        }

        Self { counterparts }
    }

    pub(crate) fn get(&self, previous: WidgetId) -> Option<WidgetId> {
        self.counterparts.get(previous).copied()
    }
}

fn semantic_identity(scene: &Scene, id: WidgetId) -> Option<(SemanticHash, Option<&str>)> {
    let kind = scene.node(id)?.kind;
    let semantics = scene.a11y(id)?;
    let name = semantics.name.as_deref();
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    Some((
        SemanticHash {
            kind,
            role: semantics.role,
            name_hash: hasher.finish(),
        },
        name,
    ))
}

fn collect_semantic_peers(
    scene: &Scene,
    groups: &mut HashMap<SemanticHash, Vec<SemanticGroup>>,
    is_previous: bool,
) {
    for id in scene.preorder() {
        let Some((key, name)) = semantic_identity(scene, id) else {
            continue;
        };
        let Some(bucket) = groups.get_mut(&key) else {
            continue;
        };
        // The hash selects a tiny bucket; exact comparison keeps collisions safe.
        let Some(group) = bucket
            .iter_mut()
            .find(|group| group.name.as_deref() == name)
        else {
            continue;
        };
        if is_previous {
            group.previous.push(id);
        } else {
            group.replacement.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schnellui_scene::ComponentRef;

    fn named(
        scene: &mut Scene,
        kind: WidgetKind,
        parent: Option<WidgetId>,
        name: &str,
    ) -> WidgetId {
        let id = scene.insert(kind, parent);
        let semantics = scene.a11y_mut(id);
        semantics.role = 1;
        semantics.name = Some(name.to_owned());
        id
    }

    #[test]
    fn explicit_refs_reserve_targets_from_semantic_fallback() {
        let reference = ComponentRef::new();
        let mut previous = Scene::new();
        let old_root = named(&mut previous, WidgetKind::Column, None, "root");
        previous.set_root(old_root);
        let old_referenced = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );
        previous.set_component_ref(old_referenced, reference);
        let old_fallback = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );

        let mut replacement = Scene::new();
        let new_root = named(&mut replacement, WidgetKind::Column, None, "root");
        replacement.set_root(new_root);
        let new_fallback = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        let new_referenced = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        replacement.set_component_ref(new_referenced, reference);

        let map = CounterpartMap::new(&previous, &replacement, [old_referenced, old_fallback]);
        assert_eq!(map.get(old_referenced), Some(new_referenced));
        assert_eq!(map.get(old_fallback), Some(new_fallback));
    }

    #[test]
    fn semantic_fallback_preserves_occurrence_order() {
        let mut previous = Scene::new();
        let old_root = named(&mut previous, WidgetKind::Column, None, "root");
        previous.set_root(old_root);
        let old_first = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );
        let old_second = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );

        let mut replacement = Scene::new();
        let new_root = named(&mut replacement, WidgetKind::Column, None, "root");
        replacement.set_root(new_root);
        let new_first = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        let new_second = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );

        let map = CounterpartMap::new(&previous, &replacement, [old_first, old_second]);
        assert_eq!(map.get(old_first), Some(new_first));
        assert_eq!(map.get(old_second), Some(new_second));
    }

    #[test]
    fn semantic_fallback_keeps_occurrence_when_only_later_peer_is_a_candidate() {
        let mut previous = Scene::new();
        let old_root = named(&mut previous, WidgetKind::Column, None, "root");
        previous.set_root(old_root);
        let _old_first = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );
        let old_second = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );

        let mut replacement = Scene::new();
        let new_root = named(&mut replacement, WidgetKind::Column, None, "root");
        replacement.set_root(new_root);
        let _new_first = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        let new_second = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );

        let map = CounterpartMap::new(&previous, &replacement, [old_second]);
        assert_eq!(map.get(old_second), Some(new_second));
    }

    #[test]
    fn explicit_refs_do_not_distort_later_fallback_occurrences() {
        let reference = ComponentRef::new();
        let mut previous = Scene::new();
        let old_root = named(&mut previous, WidgetKind::Column, None, "root");
        previous.set_root(old_root);
        let _old_first = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );
        let old_referenced = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );
        previous.set_component_ref(old_referenced, reference);
        let old_last = named(
            &mut previous,
            WidgetKind::TextInput,
            Some(old_root),
            "field",
        );

        let mut replacement = Scene::new();
        let new_root = named(&mut replacement, WidgetKind::Column, None, "root");
        replacement.set_root(new_root);
        let new_referenced = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        replacement.set_component_ref(new_referenced, reference);
        let _new_first = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );
        let new_last = named(
            &mut replacement,
            WidgetKind::TextInput,
            Some(new_root),
            "field",
        );

        let map = CounterpartMap::new(&previous, &replacement, [old_referenced, old_last]);
        assert_eq!(map.get(old_referenced), Some(new_referenced));
        assert_eq!(map.get(old_last), Some(new_last));
    }
}

//! Ephemeral structural classes for comparing freshly minted and persisted dependency IDs.
use std::collections::{BTreeMap, BTreeSet};

/// `(node id, generation signature with detour cleared, optional target id)`.
/// Classes are comparable only within one call. Graph namespaces stay separate, even
/// when imported IDs collide with persisted IDs. Missing/duplicate IDs and cycles get
/// no class, so callers never merge them or guess which dependency was intended.
pub fn dependency_identity_classes(
    graphs: &[Vec<(String, String, Option<String>)>],
) -> Vec<Vec<Option<usize>>> {
    let mut intern = BTreeMap::<(String, Option<usize>), usize>::new();
    graphs
        .iter()
        .map(|nodes| {
            let mut indices = BTreeMap::new();
            let mut duplicates = BTreeSet::new();
            for (i, (id, _, _)) in nodes.iter().enumerate() {
                if indices.insert(id.as_str(), i).is_some() {
                    duplicates.insert(id.as_str());
                }
            }
            let mut done = vec![false; nodes.len()];
            let mut classes = vec![None; nodes.len()];
            for start in 0..nodes.len() {
                if done[start] {
                    continue;
                }
                let mut path = Vec::new();
                let mut visiting = BTreeSet::new();
                let mut current = start;
                let mut target_class = loop {
                    if done[current] {
                        break classes[current];
                    }
                    if duplicates.contains(nodes[current].0.as_str()) || !visiting.insert(current) {
                        break None;
                    }
                    path.push(current);
                    match nodes[current].2.as_deref() {
                        None => {
                            let key = (nodes[current].1.clone(), None);
                            let next = intern.len();
                            let class = *intern.entry(key).or_insert(next);
                            classes[current] = Some(class);
                            done[current] = true;
                            path.pop();
                            break Some(class);
                        }
                        Some(id) => match indices.get(id).filter(|_| !duplicates.contains(id)) {
                            Some(index) => current = *index,
                            None => break None,
                        },
                    }
                };
                while let Some(index) = path.pop() {
                    if let Some(target) = target_class {
                        let key = (nodes[index].1.clone(), Some(target));
                        let next = intern.len();
                        target_class = Some(*intern.entry(key).or_insert(next));
                    }
                    classes[index] = target_class;
                    done[index] = true;
                }
                // Duplicate-ID starts have no path but must still remain unmatchable.
                done[start] = true;
            }
            classes
        })
        .collect()
}

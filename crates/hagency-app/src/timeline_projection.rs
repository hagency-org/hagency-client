//! A replayed SDK snapshot may contain multiple handles for one Matrix event.
//! Keep the SDK vector intact for its index-based diffs; publish one stable row
//! per event/transaction identity to every native room and thread consumer.
use imbl::Vector;
use std::{collections::HashMap, sync::Arc};

pub(crate) fn unique<T>(
    raw: &Vector<Arc<T>>,
    identity: impl Fn(&T) -> Option<(String, u8)>,
) -> (Vector<Arc<T>>, bool) {
    let mut positions = HashMap::new();
    let mut rows = Vector::new();
    let mut aliases = false;
    for item in raw {
        if let Some((key, priority)) = identity(item) {
            if let Some(&(index, previous_priority)) = positions.get(&key) {
                aliases = true;
                if priority >= previous_priority {
                    rows.set(index, item.clone());
                    positions.insert(key, (index, priority));
                }
                continue;
            }
            positions.insert(key, (rows.len(), priority));
        }
        rows.push_back(item.clone());
    }
    (rows, aliases)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Debug, PartialEq)]
    struct Item(&'static str, &'static str, u8);
    fn project(raw: &Vector<Arc<Item>>) -> (Vector<Arc<Item>>, bool) {
        unique(raw, |i| (!i.0.is_empty()).then(|| (i.0.into(), i.2)))
    }
    #[test]
    fn replay_identity_preserves_order_and_raw_diff_indices_and_prefers_remote() {
        let mut raw: Vector<_> = [
            Item("a", "file", 2),
            Item("b", "same body", 2),
            Item("a", "old local", 1),
            Item("c", "same body", 2),
            Item("", "divider", 0),
        ]
        .into_iter()
        .map(Arc::new)
        .collect();
        let (rows, aliased) = project(&raw);
        assert!(aliased);
        assert_eq!(
            rows.iter().map(|i| i.0).collect::<Vec<_>>(),
            ["a", "b", "c", ""]
        );
        assert_eq!(rows[0].1, "file");
        assert_eq!(raw.len(), 5);
        // A later SDK Set still addresses raw index 2; its remote edit replaces
        // the canonical first row without repeating the file or losing content.
        raw.set(2, Arc::new(Item("a", "edited file", 2)));
        assert_eq!(project(&raw).0[0].1, "edited file");
    }
    #[test]
    fn distinct_identical_messages_and_unidentified_items_are_never_collapsed() {
        let raw: Vector<_> = [
            Item("a", "same", 2),
            Item("b", "same", 2),
            Item("", "same", 0),
            Item("", "same", 0),
        ]
        .into_iter()
        .map(Arc::new)
        .collect();
        let (rows, aliased) = project(&raw);
        assert!(!aliased);
        assert_eq!(rows, raw);
    }
}

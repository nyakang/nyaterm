//! Ordered placements with direct lookup for Kitty placement replacement.

use std::collections::{BTreeMap, HashMap};

use super::{GraphicsPlacement, GraphicsScreenKind};

type KittyKey = (GraphicsScreenKind, u32, u32);

#[derive(Debug, Default)]
pub(super) struct Placements {
    // Internal ids are monotonic, so this preserves insertion/painting order.
    ordered: BTreeMap<u64, GraphicsPlacement>,
    kitty: HashMap<KittyKey, u64>,
}

fn kitty_key(placement: &GraphicsPlacement) -> Option<KittyKey> {
    Some((
        placement.screen,
        placement.kitty_id?,
        placement.placement_id?,
    ))
}

impl Placements {
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &GraphicsPlacement> {
        self.ordered.values()
    }

    pub(super) fn shift_lines(&mut self, delta: i64) {
        for placement in self.ordered.values_mut() {
            placement.line = placement.line.saturating_sub(delta);
        }
    }

    pub(super) fn last(&self) -> Option<&GraphicsPlacement> {
        self.ordered
            .last_key_value()
            .map(|(_, placement)| placement)
    }

    pub(super) fn len(&self) -> usize {
        self.ordered.len()
    }

    pub(super) fn push(&mut self, placement: GraphicsPlacement) {
        if let Some(key) = kitty_key(&placement)
            && let Some(old) = self.kitty.insert(key, placement.id)
        {
            self.ordered.remove(&old);
        }
        self.ordered.insert(placement.id, placement);
    }

    pub(super) fn remove(&mut self, id: u64) -> Option<GraphicsPlacement> {
        let placement = self.ordered.remove(&id)?;
        if let Some(key) = kitty_key(&placement) {
            self.kitty.remove(&key);
        }
        Some(placement)
    }

    pub(super) fn retain(&mut self, mut keep: impl FnMut(&GraphicsPlacement) -> bool) {
        self.ordered.retain(|_, placement| {
            if keep(placement) {
                true
            } else {
                if let Some(key) = kitty_key(placement) {
                    self.kitty.remove(&key);
                }
                false
            }
        });
    }

    pub(super) fn trim_oldest(&mut self, limit: usize) {
        while self.len() > limit {
            let id = *self
                .ordered
                .first_key_value()
                .expect("nonempty placements")
                .0;
            self.remove(id);
        }
    }

    pub(super) fn clear(&mut self) {
        self.ordered.clear();
        self.kitty.clear();
    }
}

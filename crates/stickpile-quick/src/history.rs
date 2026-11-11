//! Per-window history: which items were last copied while a
//! given window class was focused. The popup lists those first among the
//! recently used items, so Telegram opens on what you send in Telegram and
//! Discord on what you send in Discord.
//!
//! Keyed by window class, not title: titles carry unread counters
//! (`Element [1] | Room`) and change with every chat. The shell persists the
//! history as JSON next to the cache.

use serde::{Deserialize, Serialize};
use stickpile_api::ItemSummary;

/// Item ids kept per window class.
pub const PER_WINDOW: usize = 32;
/// Window classes kept (least recently used dropped).
pub const MAX_WINDOWS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowHistory {
    /// Most recently used window first.
    #[serde(default)]
    windows: Vec<WindowItems>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct WindowItems {
    class: String,
    /// Newest first.
    items: Vec<String>,
}

fn key(class: &str) -> String {
    class.trim().to_lowercase()
}

impl WindowHistory {
    pub fn from_json(text: &str) -> Result<WindowHistory, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("plain data serialises")
    }

    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Remember that `item_id` was copied while `class` was focused.
    pub fn record(&mut self, class: &str, item_id: &str) {
        let k = key(class);
        if k.is_empty() {
            return;
        }
        let mut entry = match self.windows.iter().position(|w| w.class == k) {
            Some(i) => self.windows.remove(i),
            None => WindowItems {
                class: k,
                items: Vec::new(),
            },
        };
        entry.items.retain(|id| id != item_id);
        entry.items.insert(0, item_id.to_string());
        entry.items.truncate(PER_WINDOW);
        self.windows.insert(0, entry);
        self.windows.truncate(MAX_WINDOWS);
    }

    /// Item ids used in `class`, newest first.
    pub fn ids(&self, class: &str) -> &[String] {
        let k = key(class);
        self.windows
            .iter()
            .find(|w| w.class == k)
            .map(|w| w.items.as_slice())
            .unwrap_or(&[])
    }

    /// Reorder `recent`: items used in `class` first (in that window's order),
    /// then everything else in its original order.
    pub fn order(&self, class: &str, recent: Vec<ItemSummary>) -> Vec<ItemSummary> {
        let ids = self.ids(class);
        if ids.is_empty() {
            return recent;
        }
        let rank = |item: &ItemSummary| ids.iter().position(|id| *id == item.id).unwrap_or(usize::MAX);
        let mut indexed: Vec<(usize, usize, ItemSummary)> = recent
            .into_iter()
            .enumerate()
            .map(|(i, item)| (rank(&item), i, item))
            .collect();
        indexed.sort_by_key(|(r, i, _)| (*r, *i));
        indexed.into_iter().map(|(_, _, item)| item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::item;

    #[test]
    fn record_moves_to_front_and_caps() {
        let mut h = WindowHistory::default();
        h.record("Vesktop", "a");
        h.record("vesktop", "b");
        h.record("vesktop", "a");
        assert_eq!(h.ids("VESKTOP"), ["a", "b"]);
        for i in 0..(PER_WINDOW + 5) {
            h.record("vesktop", &format!("x{i}"));
        }
        assert_eq!(h.ids("vesktop").len(), PER_WINDOW);
        assert_eq!(h.ids("vesktop")[0], format!("x{}", PER_WINDOW + 4));
        h.record("  ", "ignored");
        assert_eq!(h.ids(""), [] as [String; 0]);
    }

    #[test]
    fn windows_are_capped_lru() {
        let mut h = WindowHistory::default();
        for i in 0..(MAX_WINDOWS + 1) {
            h.record(&format!("app{i}"), "a");
        }
        assert!(h.ids("app0").is_empty(), "oldest window dropped");
        assert_eq!(h.ids(&format!("app{MAX_WINDOWS}")), ["a"]);
    }

    #[test]
    fn order_puts_window_items_first() {
        let mut h = WindowHistory::default();
        h.record("com.ayugram.desktop", "c");
        h.record("com.ayugram.desktop", "d");
        let recent = vec![item("a"), item("b"), item("c"), item("d")];
        let ids = |v: Vec<ItemSummary>| v.into_iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!(
            ids(h.order("com.ayugram.desktop", recent.clone())),
            ["d", "c", "a", "b"]
        );
        assert_eq!(ids(h.order("firefox", recent)), ["a", "b", "c", "d"]);
    }

    #[test]
    fn json_round_trip() {
        let mut h = WindowHistory::default();
        h.record("vesktop", "a");
        let back = WindowHistory::from_json(&h.to_json()).unwrap();
        assert_eq!(back, h);
        assert!(WindowHistory::from_json("{}").unwrap().is_empty());
        assert!(WindowHistory::from_json("nope").is_err());
    }
}

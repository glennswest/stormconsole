//! Navigation contributions. Plugins declare sections and items; the host
//! merges same-named sections across plugins and sorts by order, so the SPA
//! renders whatever it is given and a new plugin appears with no frontend
//! change.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavItem {
    pub label: String,
    /// SPA hash route, e.g. "#/grid?id=k8s:pods".
    pub href: String,
    #[serde(default)]
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NavSection {
    pub label: String,
    #[serde(default)]
    pub order: i32,
    pub items: Vec<NavItem>,
}

impl NavSection {
    pub fn new(label: &str, order: i32) -> Self {
        Self { label: label.to_string(), order, items: Vec::new() }
    }

    pub fn item(mut self, label: &str, href: impl Into<String>) -> Self {
        let order = self.items.len() as i32;
        self.items.push(NavItem { label: label.to_string(), href: href.into(), order });
        self
    }
}

/// Merge sections from all plugins: same label folds into one section (the
/// lowest order wins), items sort by order then label.
pub(crate) fn merge(sections: Vec<NavSection>) -> Vec<NavSection> {
    let mut merged: Vec<NavSection> = Vec::new();
    for s in sections {
        match merged.iter_mut().find(|m| m.label == s.label) {
            Some(m) => {
                m.order = m.order.min(s.order);
                m.items.extend(s.items);
            }
            None => merged.push(s),
        }
    }
    for m in &mut merged {
        m.items.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.label.cmp(&b.label)));
    }
    merged.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.label.cmp(&b.label)));
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_merge_by_label_and_sort_by_order() {
        let merged = merge(vec![
            NavSection::new("Storage", 40).item("Volumes", "#/grid?id=sb:volumes"),
            NavSection::new("Home", 0).item("Overview", "#/"),
            NavSection::new("Storage", 40).item("Drives", "#/grid?id=drive:all"),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].label, "Home");
        assert_eq!(merged[1].items.len(), 2);
    }
}

//! Navigation contributions. Plugins declare sections and items; the host
//! merges same-named sections across plugins and sorts by order, so the SPA
//! renders whatever it is given and a new plugin appears with no frontend
//! change.

use serde::{Deserialize, Serialize};

/// What a section is *for*, which is what decides whether it starts open.
///
/// Not "basic" and "advanced" — that framing ages badly and is faintly
/// insulting. The split is **work** against **administration**: somebody
/// creating a virtual machine and somebody deciding whether a drive is
/// failing are doing different jobs, and the second one knows where to
/// look. So the sections people come to *work* in stay open and the ones
/// they come to *diagnose* in start shut, with their total beside them so
/// a shut section still says whether it is worth opening (#16).
///
/// Declared by the plugin that contributes the section, because the
/// plugin is the thing that knows. `Work` is the default, so a section
/// nobody has classified stays open: this hides nothing that was not
/// deliberately classified as administration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavKind {
    #[default]
    Work,
    Admin,
}

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
    #[serde(default)]
    pub kind: NavKind,
    pub items: Vec<NavItem>,
}

impl NavSection {
    pub fn new(label: &str, order: i32) -> Self {
        Self { label: label.to_string(), order, kind: NavKind::Work, items: Vec::new() }
    }

    /// A section people come to diagnose in rather than work in: engine
    /// internals, slabs, drives, goldens, the registry, fleet plumbing.
    pub fn admin(mut self) -> Self {
        self.kind = NavKind::Admin;
        self
    }

    pub fn item(mut self, label: &str, href: impl Into<String>) -> Self {
        let order = self.items.len() as i32;
        self.items.push(NavItem { label: label.to_string(), href: href.into(), order });
        self
    }

    /// An item at a stated position, for a section two plugins build.
    ///
    /// Sequential positions are fine while one plugin owns a section, and
    /// useless the moment another has to slot something between two of
    /// them — `.item()` numbers 0, 1, 2, and there is no integer between
    /// 0 and 1. Sections built by more than one plugin leave gaps.
    pub fn item_at(mut self, label: &str, href: impl Into<String>, order: i32) -> Self {
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
                // Work wins. Storage is contributed by two plugins — the
                // PVCs somebody asked for, and stormblock's engine
                // internals — and a section holding one thing a person
                // works in is a section that opens. The shut ones are the
                // sections that are administration all the way through.
                if s.kind == NavKind::Work {
                    m.kind = NavKind::Work;
                }
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
    fn a_section_is_work_unless_every_contributor_calls_it_administration() {
        let merged = merge(vec![
            NavSection::new("Storage", 40).admin().item("Slabs", "#/slabs"),
            NavSection::new("Storage", 40).item("PVCs", "#/k8s/pvc"),
            NavSection::new("Hardware", 70).admin().item("Drives", "#/drives"),
        ]);
        let by = |l: &str| merged.iter().find(|s| s.label == l).unwrap().kind;
        assert_eq!(by("Storage"), NavKind::Work, "PVCs are work, so the section opens");
        assert_eq!(by("Hardware"), NavKind::Admin);
    }

    #[test]
    fn a_section_nobody_classified_stays_open() {
        let merged = merge(vec![NavSection::new("Something new", 90).item("x", "#/x")]);
        assert_eq!(merged[0].kind, NavKind::Work);
    }

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

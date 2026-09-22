//! Events for one object, asked of whichever plugin owns it.
//!
//! An event is the only record of what *happened* to a thing, as opposed
//! to what it is now. A VM stuck in `Scheduling`, a pod that will not
//! pull, a volume that failed to attach — the object's own fields say the
//! state and the events say the story, and a console that shows the first
//! without the second leaves somebody reading a phase and guessing.
//!
//! The console had events in exactly two places: a cluster-wide list under
//! Observe, and a namespace's tab. Neither answers "what happened to
//! *this*", which is the question actually being asked, and the
//! cluster-wide list is the worst possible place to answer it from.
//!
//! So it is a plugin contract rather than a view: a plugin is asked for
//! one component id and answers `None` if the id is not its own. The host
//! takes the first plugin that claims it, which means a new plugin with an
//! event source of its own needs no change anywhere else — and one without
//! needs no change at all.
//!
//! **"No events" and "nothing records events" are different answers**, and
//! a box that renders them the same way teaches people to distrust it.
//! stormblock, stormdrive and the registry emit nothing; a volume with no
//! events is not a volume nothing has happened to.

use serde::{Deserialize, Serialize};

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// When it last happened, as the source wrote it (RFC 3339). Rendered
    /// as an age by the view, which is the form people read.
    pub time: String,
    /// `Normal` or `Warning` — Kubernetes' own two, because every source
    /// here either is Kubernetes or is describing something that is.
    #[serde(rename = "type")]
    pub kind: String,
    /// The short machine-readable cause: `FailedScheduling`, `Pulled`.
    pub reason: String,
    pub message: String,
    /// What reported it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// How many times, for a source that collapses repeats. A `×47` is
    /// most of the diagnosis in a crash loop.
    #[serde(default = "one")]
    pub count: i64,
}

fn one() -> i64 {
    1
}

impl Event {
    pub fn is_warning(&self) -> bool {
        self.kind == "Warning"
    }
}

/// What a plugin knows about one object's history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Events {
    /// Whether anything records events for this object at all. `false`
    /// with an empty list means "nobody is writing these down"; `true`
    /// with an empty list means "nothing has happened".
    pub available: bool,
    /// Why not, when it is not — named upstream rather than "unavailable".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub items: Vec<Event>,
}

impl Events {
    pub fn of(items: Vec<Event>) -> Self {
        Self { available: true, reason: String::new(), items }
    }

    /// Nothing records events for this object, and here is what does not.
    pub fn none(reason: impl Into<String>) -> Self {
        Self { available: false, reason: reason.into(), items: Vec::new() }
    }

    /// Newest first, and bounded. A box in an opened row is for the last
    /// thing that happened, not an archive — the Events page is the
    /// archive, and an unbounded list in a table row is a table nobody can
    /// scroll past.
    pub fn newest(mut self, cap: usize) -> Self {
        self.items.sort_by(|a, b| b.time.cmp(&a.time));
        self.items.truncate(cap);
        self
    }
}

/// The answer when no plugin claims the id.
///
/// Said in terms of where events come from on this platform, because
/// "none" in front of a volume that has just failed to attach is a
/// statement somebody will act on.
pub fn unclaimed(id: &str) -> Events {
    Events::none(format!(
        "nothing on this console records events for {id}. Events here come from the cluster's \
         apiserver; the storage engine, the drive service and the registry do not write any."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t: &str) -> Event {
        Event {
            time: t.into(),
            kind: "Normal".into(),
            reason: "Started".into(),
            message: String::new(),
            source: String::new(),
            count: 1,
        }
    }

    #[test]
    fn newest_first_and_bounded() {
        let e = Events::of(vec![
            at("2026-09-22T10:00:00Z"),
            at("2026-09-22T12:00:00Z"),
            at("2026-09-22T11:00:00Z"),
        ])
        .newest(2);
        assert_eq!(e.items.len(), 2);
        assert_eq!(e.items[0].time, "2026-09-22T12:00:00Z");
        assert_eq!(e.items[1].time, "2026-09-22T11:00:00Z");
    }

    /// The distinction the whole module exists for.
    #[test]
    fn nothing_happened_and_nobody_is_writing_are_different_answers() {
        let quiet = Events::of(vec![]);
        assert!(quiet.available && quiet.items.is_empty());

        let deaf = unclaimed("sb:volume:1f4c");
        assert!(!deaf.available);
        assert!(deaf.reason.contains("do not write any"), "{}", deaf.reason);
    }
}

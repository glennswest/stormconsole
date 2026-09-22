//! Who is asking, and what they may see.
//!
//! The console is the broadest read surface on the platform — kubernetes,
//! fleet, logs, drives, volumes, registry, all in one feed — so "everyone
//! sees everything" stops being defensible the moment it runs a fleet
//! (issue #7).
//!
//! The seam is deliberately thin. A [`Viewer`] is an identity carried on
//! the request; a plugin answers [`ConsolePlugin::access`] with what that
//! identity may see of *its* slice, and the [`Registry`] applies the answer
//! when it serves a snapshot. Two properties matter:
//!
//! - **It is an authorization result, not a display choice.** The filter
//!   runs on the server, before the feed leaves the process, so a viewer
//!   cannot reach a hidden object by URL, by websocket, or by reading the
//!   JSON. A plugin that cannot determine an answer says
//!   [`Access::Unrestricted`] and the console says *that* rather than
//!   implying an enforcement it is not doing.
//! - **It says what is hidden.** A short list with no explanation reads as
//!   a broken console; `hidden` and `note` travel to the UI so it can say
//!   "3 namespaces you cannot view".
//!
//! [`ConsolePlugin::access`]: crate::ConsolePlugin::access
//! [`Registry`]: crate::Registry

use std::sync::Arc;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

/// The identity behind one request. Anonymous when the console serves
/// without authentication, which is the single-node default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Viewer {
    /// The console user, once signed in.
    pub user: Option<String>,
    /// A credential to act as this viewer against an upstream — for
    /// kubernetes, the bearer token whose RBAC decides what they may read.
    pub token: Option<String>,
    /// What this viewer may do.
    ///
    /// Named for what the console actually offers rather than for
    /// Kubernetes verbs: "may open a console", "may delete a volume" do not
    /// line up with get/list/watch, and pretending they do produces a role
    /// model nobody can reason about. Empty means `viewer`.
    pub roles: Vec<String>,
    /// This viewer's SSH public keys, so a machine they create is one they
    /// can log into without pasting a key — or, worse, without the habit
    /// that grows in its place, which is a password in the cloud-init seed.
    pub ssh_keys: Vec<String>,
}

/// A plugin's routes get the viewer the host put on the request, so a
/// plugin can refuse to answer for something this identity may not see —
/// and can act *as* them upstream, which is what makes a write subject to
/// the same authorization as the read that showed it.
impl<S: Send + Sync> FromRequestParts<S> for Viewer {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(parts.extensions.get::<Viewer>().cloned().unwrap_or_default())
    }
}

impl Viewer {
    pub fn anonymous() -> Self {
        Self::default()
    }

    /// Nobody signed in, or no credential to carry upstream: there is no
    /// identity to authorize against.
    pub fn is_anonymous(&self) -> bool {
        self.user.is_none() && self.token.is_none()
    }

    /// Does this viewer hold a role?
    ///
    /// `admin` holds every role, which is the one special case worth having:
    /// the alternative is every check listing `admin` beside it and one of
    /// them eventually not doing so.
    pub fn has_role(&self, role: &str) -> bool {
        self.roles.iter().any(|r| r == role || r == "admin")
    }

    /// May this viewer change things, as opposed to look at them?
    ///
    /// The single distinction worth drawing before a full capability model
    /// exists: a console that cannot tell a reader from an operator is one
    /// where every reader is an operator, which is where this started.
    pub fn may_write(&self) -> bool {
        self.has_role("operator") || self.has_role("admin")
    }
}

/// What one viewer may see of one plugin's components.
#[derive(Clone)]
pub enum Access {
    /// Nothing is withheld — the plugin has nothing to authorize, or no
    /// identity was presented to authorize against.
    Unrestricted,
    /// Only components this predicate accepts, plus a count and a line
    /// naming what was withheld.
    Limited(Scope),
}

impl Access {
    /// Accept everything this plugin owns; `hidden` and `note` carry the
    /// short explanation the UI shows.
    pub fn limited(
        allow: impl Fn(&str) -> bool + Send + Sync + 'static,
        hidden: usize,
        note: impl Into<String>,
    ) -> Self {
        Access::Limited(Scope { allow: Arc::new(allow), hidden, note: note.into() })
    }

    pub fn allows(&self, id: &str) -> bool {
        match self {
            Access::Unrestricted => true,
            Access::Limited(s) => (s.allow)(id),
        }
    }

    pub fn hidden(&self) -> usize {
        match self {
            Access::Unrestricted => 0,
            Access::Limited(s) => s.hidden,
        }
    }

    pub fn note(&self) -> &str {
        match self {
            Access::Unrestricted => "",
            Access::Limited(s) => &s.note,
        }
    }

    pub fn is_unrestricted(&self) -> bool {
        matches!(self, Access::Unrestricted)
    }
}

#[derive(Clone)]
pub struct Scope {
    /// Given a component id, may this viewer see it?
    pub allow: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// How many objects (or namespaces, or whatever the plugin counts in)
    /// were withheld — so the UI can say so instead of showing a short
    /// list that reads like a bug.
    pub hidden: usize,
    /// One line: "3 namespaces you cannot view".
    pub note: String,
}

impl std::fmt::Debug for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scope").field("hidden", &self.hidden).field("note", &self.note).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrestricted_allows_everything_and_hides_nothing() {
        let a = Access::Unrestricted;
        assert!(a.allows("k8s:pod:kube-system/x"));
        assert_eq!(a.hidden(), 0);
        assert!(a.is_unrestricted());
    }

    #[test]
    fn limited_carries_its_predicate_and_its_explanation() {
        let a = Access::limited(|id| id.contains(":default/"), 2, "2 namespaces you cannot view");
        assert!(a.allows("k8s:pod:default/web"));
        assert!(!a.allows("k8s:pod:kube-system/dns"));
        assert_eq!(a.hidden(), 2);
        assert_eq!(a.note(), "2 namespaces you cannot view");
    }

    #[test]
    fn an_anonymous_viewer_is_the_one_with_neither_half() {
        assert!(Viewer::anonymous().is_anonymous());
        // Spread the default rather than listing every field: a Viewer
        // that grows one should not break four constructors in tests.
        assert!(!Viewer { user: Some("gw".into()), ..Default::default() }.is_anonymous());
        assert!(!Viewer { token: Some("t".into()), ..Default::default() }.is_anonymous());
    }
}

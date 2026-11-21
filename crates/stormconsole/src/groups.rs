//! Groups of users (#334). The owner, 2026-11-20: "Make groups now. We need
//! that for drive and hardware management."
//!
//! Beside sc#27's users in the users namespace (`storm-users`):
//!
//! - **A group**: ConfigMap `<name>-group`, label `storm.io/group: <name>`,
//!   `data.description` and `data.members`, the user names one to a line,
//!   sorted. The label says what it is, not the name.
//! - **A group's role**: ClusterRoleBinding `storm-group:<group>:<role>` to
//!   ClusterRole `<role>`, the same label, its subjects the members'
//!   ServiceAccounts. It is rewritten on every change of members.
//!
//! Why the members are the subjects and not a `Group`: a person signs in as
//! their ServiceAccount, and a ServiceAccount's token carries only
//! `system:serviceaccounts` and `system:serviceaccounts:<ns>` (rustkube
//! `auth.rs`). A `Group: drives` subject would match nobody. Naming the
//! account means the apiserver's RBAC grants a member the group's roles as
//! it grants their own, and the console reads them the same way
//! ([`crate::users::roles_in`]: any binding naming the account), so
//! sign-in, the sweep and storage-admin's SelfSubjectAccessReview need
//! nothing new.
//!
//! A member is a user: labelled for its name, never admin (stormcos's own
//! account, `cluster-admin` already). A user deleted leaves every group
//! first, so a user made again under the name inherits nothing. Every change
//! is made as the signed-in administrator, as the Users page's are.

use serde_json::{json, Value};

use crate::users::{check_name, items, label, Fail, Users, RBAC};

pub const GROUP_LABEL: &str = "storm.io/group";
const PREFIX: &str = "storm-group:";

/// One group, as the page shows it.
#[derive(Clone, Debug, serde::Serialize, PartialEq, Eq)]
pub struct Group {
    pub name: String,
    pub description: String,
    pub members: Vec<String>,
    /// The ClusterRoles its bindings grant.
    pub roles: Vec<String>,
    pub created: Option<String>,
}

/// `data.members` as a list: one name a line, blanks dropped, sorted, once each.
pub fn members_of(cm: &Value) -> Vec<String> {
    let mut m: Vec<String> = cm
        .pointer("/data/members")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    m.sort();
    m.dedup();
    m
}

/// A group's ConfigMap: a ConfigMap labelled for its own name, `<name>-group`.
pub fn group_name(cm: &Value) -> Option<&str> {
    let g = label(cm, GROUP_LABEL)?;
    let n = cm.pointer("/metadata/name").and_then(Value::as_str)?;
    (n.strip_suffix("-group") == Some(g)).then_some(g)
}

/// The ClusterRoles of a group's own bindings: labelled for it and named for it.
pub fn group_roles(bindings: &Value, g: &str) -> Vec<String> {
    let prefix = format!("{PREFIX}{g}:");
    let mut r: Vec<String> = items(bindings)
        .filter(|b| label(b, GROUP_LABEL) == Some(g))
        .filter_map(|b| b.pointer("/metadata/name").and_then(Value::as_str)?.strip_prefix(&prefix).map(str::to_string))
        .collect();
    r.sort();
    r
}

/// Every group in a ConfigMap list, with its roles from the bindings.
pub fn rows(configmaps: &Value, bindings: &Value) -> Vec<Group> {
    let mut out: Vec<Group> = items(configmaps)
        .filter_map(|cm| {
            let name = group_name(cm)?;
            Some(Group {
                name: name.to_string(),
                description: cm.pointer("/data/description").and_then(Value::as_str).unwrap_or_default().to_string(),
                members: members_of(cm),
                roles: group_roles(bindings, name),
                created: cm.pointer("/metadata/creationTimestamp").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The bindings, less the groups': what a user holds in their own right.
pub fn without_groups(bindings: &Value) -> Value {
    let kept: Vec<Value> = items(bindings).filter(|b| label(b, GROUP_LABEL).is_none()).cloned().collect();
    json!({ "items": kept })
}

/// A group's binding for a role, naming every member's account.
pub fn binding(ns: &str, g: &str, role: &str, members: &[String]) -> Value {
    let subjects: Vec<Value> =
        members.iter().map(|m| json!({"kind": "ServiceAccount", "name": m, "namespace": ns})).collect();
    json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "ClusterRoleBinding",
        "metadata": {"name": format!("{PREFIX}{g}:{role}"), "labels": {GROUP_LABEL: g}},
        "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": "ClusterRole", "name": role},
        "subjects": subjects,
    })
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v: Vec<String> = v.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    v.sort();
    v.dedup();
    v
}

fn check_group(name: &str) -> Result<(), Fail> {
    check_name(name).map_err(|_| {
        Fail::Bad(format!("{name:?} is not a group name: lower-case letters, digits and '-', at most 57"))
    })?;
    // `<name>-group` must still be a name.
    if name.len() > 57 {
        return Err(Fail::Bad(format!("{name:?} is too long for a group name: at most 57")));
    }
    Ok(())
}

impl Users {
    fn group_path(&self, g: &str) -> String {
        format!("/api/v1/namespaces/{}/configmaps/{g}-group", self.ns)
    }

    /// A group's ConfigMap, or why there is none.
    pub(crate) async fn group(&self, as_: &str, g: &str) -> Result<Value, Fail> {
        check_group(g)?;
        match self.get_as(&self.group_path(g), as_).await {
            Ok(cm) if group_name(&cm) == Some(g) => Ok(cm),
            Ok(_) => Err(Fail::Bad(format!("{}/{g}-group is a ConfigMap, not a group", self.ns))),
            Err(Fail::Missing(_)) => Err(Fail::Bad(format!("no group {g}"))),
            Err(e) => Err(e),
        }
    }

    /// Every group, with its members and roles.
    pub async fn groups(&self, as_: &str) -> Result<Vec<Group>, Fail> {
        let cms = self.get_as(&format!("/api/v1/namespaces/{}/configmaps", self.ns), as_).await?;
        let bindings = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        Ok(rows(&cms, &bindings))
    }

    /// The groups a user is in.
    pub(crate) async fn groups_of(&self, as_: &str, user: &str) -> Result<Vec<String>, Fail> {
        Ok(self.groups(as_).await?.into_iter().filter(|g| g.members.iter().any(|m| m == user)).map(|g| g.name).collect())
    }

    /// Members must be users, and not admin.
    async fn check_members(&self, as_: &str, members: &[String]) -> Result<(), Fail> {
        for m in members {
            check_name(m)?;
            self.not_admin(m, "put in a group")?;
            match self.is_user(as_, m).await {
                Ok(_) => {}
                Err(Fail::Missing(_)) => return Err(Fail::Bad(format!("no user {m}"))),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Make a group: its ConfigMap, then a binding per role. Every member
    /// and role is checked before anything is written.
    pub async fn create_group(
        &self,
        as_: &str,
        g: &str,
        description: &str,
        members: &[String],
        roles: &[String],
    ) -> Result<(), Fail> {
        check_group(g)?;
        let members = sorted(members);
        let roles = sorted(roles);
        self.check_members(as_, &members).await?;
        self.known_roles(as_, &roles).await?;
        let cm = json!({
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": format!("{g}-group"), "namespace": self.ns, "labels": {GROUP_LABEL: g}},
            "data": {"description": description.trim(), "members": members.join("\n")},
        });
        self.write(false, &format!("/api/v1/namespaces/{}/configmaps", self.ns), &cm, as_).await?;
        for r in &roles {
            self.write(false, &format!("{RBAC}/clusterrolebindings"), &binding(&self.ns, g, r, &members), as_).await?;
        }
        Ok(())
    }

    /// Delete a group: its bindings, then its ConfigMap. Returns who was in
    /// it, whose roles change.
    pub async fn delete_group(&self, as_: &str, g: &str) -> Result<Vec<String>, Fail> {
        let cm = self.group(as_, g).await?;
        let bindings = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        for r in group_roles(&bindings, g) {
            self.remove(&format!("{RBAC}/clusterrolebindings/{PREFIX}{g}:{r}"), as_).await?;
        }
        self.remove(&self.group_path(g), as_).await?;
        Ok(members_of(&cm))
    }

    /// Make a group's members exactly these, and its bindings name them.
    /// Returns everyone who was or now is in it.
    pub async fn set_members(&self, as_: &str, g: &str, members: &[String]) -> Result<Vec<String>, Fail> {
        let mut cm = self.group(as_, g).await?;
        let members = sorted(members);
        self.check_members(as_, &members).await?;
        let was = members_of(&cm);
        self.write_members(as_, g, &mut cm, &members).await?;
        Ok(sorted(&[was, members].concat()))
    }

    /// Make a group's bindings exactly these roles. Returns its members.
    pub async fn set_group_roles(&self, as_: &str, g: &str, roles: &[String]) -> Result<Vec<String>, Fail> {
        let cm = self.group(as_, g).await?;
        let roles = sorted(roles);
        self.known_roles(as_, &roles).await?;
        let members = members_of(&cm);
        let bindings = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        let have = group_roles(&bindings, g);
        for r in roles.iter().filter(|r| !have.contains(r)) {
            self.write(false, &format!("{RBAC}/clusterrolebindings"), &binding(&self.ns, g, r, &members), as_).await?;
        }
        for r in have.iter().filter(|r| !roles.contains(r)) {
            self.remove(&format!("{RBAC}/clusterrolebindings/{PREFIX}{g}:{r}"), as_).await?;
        }
        Ok(members)
    }

    /// Add one user to a group (a user being made).
    pub(crate) async fn join(&self, as_: &str, g: &str, user: &str) -> Result<(), Fail> {
        let mut cm = self.group(as_, g).await?;
        let mut members = members_of(&cm);
        members.push(user.to_string());
        let members = sorted(&members);
        self.write_members(as_, g, &mut cm, &members).await
    }

    /// Take a user out of every group they are in (a user being deleted).
    pub(crate) async fn leave_every_group(&self, as_: &str, user: &str) -> Result<(), Fail> {
        for g in self.groups_of(as_, user).await? {
            let mut cm = self.group(as_, &g).await?;
            let members: Vec<String> = members_of(&cm).into_iter().filter(|m| m != user).collect();
            self.write_members(as_, &g, &mut cm, &members).await?;
        }
        Ok(())
    }

    /// The ConfigMap's members, then every binding of the group rewritten
    /// to name exactly them. The ConfigMap is written as read (its
    /// resourceVersion), so two administrators changing one group at once
    /// cannot both win: the second is a 409.
    async fn write_members(&self, as_: &str, g: &str, cm: &mut Value, members: &[String]) -> Result<(), Fail> {
        cm["data"]["members"] = json!(members.join("\n"));
        self.write(true, &self.group_path(g), cm, as_).await?;
        let bindings = self.get_as(&format!("{RBAC}/clusterrolebindings"), as_).await?;
        for r in group_roles(&bindings, g) {
            let path = format!("{RBAC}/clusterrolebindings/{PREFIX}{g}:{r}");
            let mut b = self.get_as(&path, as_).await?;
            b["subjects"] = binding(&self.ns, g, &r, members)["subjects"].clone();
            self.write(true, &path, &b, as_).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cm(name: &str, g: Option<&str>, members: &str) -> Value {
        let mut c = json!({"metadata": {"name": name, "creationTimestamp": "2026-11-20T00:00:00Z"},
                           "data": {"description": "the drive team", "members": members}});
        if let Some(g) = g {
            c["metadata"]["labels"] = json!({GROUP_LABEL: g});
        }
        c
    }

    #[test]
    fn members_are_one_a_line_sorted_once() {
        assert_eq!(members_of(&cm("x-group", Some("x"), "bob\n\n alice \nbob\n")), ["alice", "bob"]);
        assert!(members_of(&json!({})).is_empty());
    }

    #[test]
    fn a_group_is_a_configmap_labelled_for_its_own_name() {
        assert_eq!(group_name(&cm("drives-group", Some("drives"), "")), Some("drives"));
        assert_eq!(group_name(&cm("drives-group", None, "")), None, "no label: some other ConfigMap");
        assert_eq!(group_name(&cm("drives", Some("drives"), "")), None, "not <name>-group");
        assert_eq!(group_name(&cm("other-group", Some("drives"), "")), None);
    }

    #[test]
    fn a_groups_binding_names_each_members_account() {
        let b = binding("storm-users", "drives", "storage-admin", &["alice".into(), "bob".into()]);
        assert_eq!(b["metadata"]["name"], "storm-group:drives:storage-admin");
        assert_eq!(b["metadata"]["labels"][GROUP_LABEL], "drives");
        assert_eq!(b["roleRef"]["name"], "storage-admin");
        assert_eq!(b["subjects"][1], json!({"kind": "ServiceAccount", "name": "bob", "namespace": "storm-users"}));
        // So a member's roles, read as sign-in reads them, carry the group's.
        let all = json!({"items": [b,
            {"metadata": {"name": "storm-user:alice:view", "labels": {"storm.io/user": "alice"}}, "roleRef": {"kind": "ClusterRole", "name": "view"},
             "subjects": [{"kind": "ServiceAccount", "name": "alice", "namespace": "storm-users"}]}]});
        assert_eq!(crate::users::roles_in(&all, "storm-users", "alice"), ["storage-admin", "view"]);
        assert_eq!(crate::users::roles_in(&all, "storm-users", "bob"), ["storage-admin"]);
        assert_eq!(crate::users::roles_in(&without_groups(&all), "storm-users", "alice"), ["view"], "own roles");
        assert!(crate::users::roles_in(&without_groups(&all), "storm-users", "bob").is_empty());
        assert!(binding("n", "g", "r", &[])["subjects"].as_array().unwrap().is_empty(), "an empty group names nobody");
    }

    #[test]
    fn rows_are_the_groups_with_their_roles() {
        let cms = json!({"items": [cm("ops-group", Some("ops"), "carol"), cm("drives-group", Some("drives"), "bob\nalice"), cm("kube-root-ca.crt", None, "")]});
        let bindings = json!({"items": [
            binding("storm-users", "drives", "storage-admin", &[]),
            binding("storm-users", "drives", "view", &[]),
            // Named like one but not labelled: not the group's.
            {"metadata": {"name": "storm-group:drives:edit"}, "roleRef": {"kind": "ClusterRole", "name": "edit"}},
        ]});
        let r = rows(&cms, &bindings);
        assert_eq!(r.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["drives", "ops"]);
        assert_eq!(r[0].members, ["alice", "bob"]);
        assert_eq!(r[0].roles, ["storage-admin", "view"]);
        assert_eq!(r[0].description, "the drive team");
        assert!(r[1].roles.is_empty());
    }

    #[test]
    fn group_names_leave_room_for_the_suffix() {
        assert!(check_group("drives").is_ok());
        assert!(check_group(&"a".repeat(57)).is_ok());
        assert!(check_group(&"a".repeat(58)).is_err());
        assert!(check_group("Drives").is_err());
    }
}

//! Creation, the OpenShift way: every list view has a "+ Create", the top
//! bar has an "Import YAML", and what each one does is declared by the
//! plugin that owns the resource — a YAML editor seeded with a template,
//! or a small form — posting to a path the plugin serves. The UI renders
//! whatever it is given; nothing about pods or volumes lives in the SPA.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub name: String,
    pub label: String,
    /// text | number | select | textarea
    pub kind: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

impl Field {
    pub fn text(name: &str, label: &str) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            kind: "text".into(),
            required: false,
            options: vec![],
            default: String::new(),
            hint: String::new(),
        }
    }

    pub fn select(name: &str, label: &str, options: &[&str]) -> Self {
        Self {
            kind: "select".into(),
            options: options.iter().map(|s| s.to_string()).collect(),
            default: options.first().map(|s| s.to_string()).unwrap_or_default(),
            ..Self::text(name, label)
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.into();
        self
    }

    pub fn default(mut self, d: &str) -> Self {
        self.default = d.into();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Creator {
    /// Stable id, unique across the console: "k8s:yaml", "sb:volume".
    pub id: String,
    /// Owning plugin; the host fills it in.
    #[serde(default)]
    pub plugin: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Hash routes this creator is offered on, by prefix match; "*" means
    /// everywhere (the top-bar menu lists every creator regardless).
    #[serde(default)]
    pub at: Vec<String>,
    /// yaml | form. A yaml creator sends the editor text as
    /// `application/yaml`; a form creator sends its fields as one JSON
    /// object.
    pub mode: String,
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub template: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Field>,
}

impl Creator {
    pub fn yaml(id: &str, label: &str, path: &str, template: &str) -> Self {
        Self {
            id: id.into(),
            plugin: String::new(),
            label: label.into(),
            description: String::new(),
            at: vec![],
            mode: "yaml".into(),
            method: "POST".into(),
            path: path.into(),
            template: template.into(),
            fields: vec![],
        }
    }

    pub fn form(id: &str, label: &str, path: &str, fields: Vec<Field>) -> Self {
        Self {
            id: id.into(),
            plugin: String::new(),
            label: label.into(),
            description: String::new(),
            at: vec![],
            mode: "form".into(),
            method: "POST".into(),
            path: path.into(),
            template: String::new(),
            fields,
        }
    }

    pub fn at(mut self, routes: &[&str]) -> Self {
        self.at = routes.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn describe(mut self, d: &str) -> Self {
        self.description = d.into();
        self
    }
}

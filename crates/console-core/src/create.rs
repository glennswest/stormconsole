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
    pub options: Vec<FieldOption>,
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

    /// A select whose options are their own labels.
    pub fn select(name: &str, label: &str, options: &[&str]) -> Self {
        Self::choices(name, label, options.iter().map(|o| FieldOption::plain(o)).collect())
    }

    /// A select whose options say one thing and submit another.
    ///
    /// The form posts the **value**; the label is only ever displayed. That
    /// distinction is the whole point of this existing: the VM root-disk
    /// picker showed "alma 10 x86_64 — not goldened yet, will be built" and
    /// submitted it verbatim, because the option carried one string and the
    /// server mapped the label back to a value afterwards. The moment the two
    /// could drift — a catalogue refresh between rendering the form and
    /// submitting it — the mapping missed and a machine was created with a
    /// sentence as the name of its disk.
    pub fn choices(name: &str, label: &str, options: Vec<FieldOption>) -> Self {
        Self {
            kind: "select".into(),
            default: options.first().map(|o| o.value.clone()).unwrap_or_default(),
            options,
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

/// One choice in a select: what is submitted, and what is read.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldOption {
    pub value: String,
    /// What a person sees. Defaults to the value, so a plain list of strings
    /// still means what it used to.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

impl FieldOption {
    /// A choice that reads as what it submits.
    pub fn plain(value: &str) -> Self {
        FieldOption { value: value.to_string(), label: String::new() }
    }

    /// A choice that says one thing and submits another.
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        FieldOption { value: value.into(), label: label.into() }
    }

    /// What to display.
    pub fn text(&self) -> &str {
        if self.label.is_empty() { &self.value } else { &self.label }
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

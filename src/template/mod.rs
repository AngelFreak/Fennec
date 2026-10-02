//! Report templates: TOML files with header fields and layout options.
//!
//! ```toml
//! name = "Notat"
//! heading = "NOTAT"
//! footer = "Afdeling for byggesager"
//! logo = "logo.png"            # relative to the template file
//! body_font = "Source Serif 4"
//! body_size_pt = 11
//! summary_heading = "Resumé"   # where an included AI summary goes
//!
//! [[fields]]
//! key = "sagsnr"
//! label = "Sagsnr."
//! kind = "text"                # text | date | list | multiline
//! default = ""                 # may use {today} {user} {duration} {model}
//! required = true
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum TemplateError {
    #[error("{path}: {message}")]
    Invalid { path: PathBuf, message: String },
    #[error("could not read {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FieldKind {
    #[default]
    Text,
    Date,
    List,
    Multiline,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Field {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub kind: FieldKind,
    #[serde(default)]
    pub default: String,
    #[serde(default)]
    pub required: bool,
    /// Choices for `kind = "list"`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// File stem; not part of the TOML.
    #[serde(skip)]
    pub id: String,
    /// Directory the template was loaded from, for resolving `logo`.
    #[serde(skip)]
    pub dir: Option<PathBuf>,
    pub name: String,
    #[serde(default)]
    pub heading: String,
    #[serde(default)]
    pub footer: String,
    #[serde(default)]
    pub logo: Option<PathBuf>,
    #[serde(default = "default_font")]
    pub body_font: String,
    #[serde(default = "default_size")]
    pub body_size_pt: f64,
    #[serde(default = "default_summary_heading")]
    pub summary_heading: String,
    #[serde(default)]
    pub fields: Vec<Field>,
}

fn default_font() -> String {
    "Source Serif 4".into()
}
fn default_size() -> f64 {
    11.0
}
fn default_summary_heading() -> String {
    "Resumé".into()
}

/// Values available to `{placeholder}` defaults.
#[derive(Debug, Clone, Default)]
pub struct PlaceholderContext {
    pub today: String,
    pub user: String,
    pub duration: String,
    pub model: String,
}

impl Template {
    pub fn parse(id: &str, toml_text: &str, path: &Path) -> Result<Self, TemplateError> {
        let mut t: Template = toml::from_str(toml_text).map_err(|e| TemplateError::Invalid {
            path: path.to_path_buf(),
            message: e.message().to_string(),
        })?;
        t.id = id.to_string();
        t.dir = path.parent().map(Path::to_path_buf);
        t.validate().map_err(|message| TemplateError::Invalid {
            path: path.to_path_buf(),
            message,
        })?;
        Ok(t)
    }

    pub fn load(path: &Path) -> Result<Self, TemplateError> {
        let text = std::fs::read_to_string(path).map_err(|source| TemplateError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let id = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self::parse(&id, &text, path)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("templates always serialize")
    }

    pub fn save(&self, dir: &Path) -> Result<PathBuf, TemplateError> {
        let path = dir.join(format!("{}.toml", self.id));
        std::fs::create_dir_all(dir)
            .and_then(|_| std::fs::write(&path, self.to_toml()))
            .map_err(|source| TemplateError::Io {
                path: path.clone(),
                source,
            })?;
        Ok(path)
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("name must not be empty".into());
        }
        let mut seen = BTreeSet::new();
        for f in &self.fields {
            let valid_key = !f.key.is_empty()
                && f.key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            if !valid_key {
                return Err(format!("field key {:?} must use a-z, 0-9 and _", f.key));
            }
            if !seen.insert(&f.key) {
                return Err(format!("field key {:?} is used twice", f.key));
            }
            if f.label.trim().is_empty() {
                return Err(format!("field {:?} needs a label", f.key));
            }
            if f.kind == FieldKind::List && f.options.is_empty() {
                return Err(format!("list field {:?} needs options", f.key));
            }
        }
        if !(6.0..=24.0).contains(&self.body_size_pt) {
            return Err(format!(
                "body_size_pt {} must be between 6 and 24",
                self.body_size_pt
            ));
        }
        Ok(())
    }

    pub fn logo_path(&self) -> Option<PathBuf> {
        let logo = self.logo.as_ref()?;
        Some(match &self.dir {
            Some(dir) if logo.is_relative() => dir.join(logo),
            _ => logo.clone(),
        })
    }

    /// Initial field values for a new document, with placeholders expanded.
    pub fn initial_values(&self, ctx: &PlaceholderContext) -> BTreeMap<String, String> {
        self.fields
            .iter()
            .map(|f| (f.key.clone(), expand(&f.default, ctx)))
            .collect()
    }

    /// Labels of required fields that are empty in `values`.
    pub fn missing_required(&self, values: &BTreeMap<String, String>) -> Vec<String> {
        self.fields
            .iter()
            .filter(|f| f.required && values.get(&f.key).is_none_or(|v| v.trim().is_empty()))
            .map(|f| f.label.clone())
            .collect()
    }

    /// The plain built-in template used when none is chosen.
    pub fn blank() -> Self {
        Template {
            id: "blank".into(),
            dir: None,
            name: "Blank".into(),
            heading: String::new(),
            footer: String::new(),
            logo: None,
            body_font: default_font(),
            body_size_pt: default_size(),
            summary_heading: default_summary_heading(),
            fields: Vec::new(),
        }
    }
}

/// A field key from its label: "Udarbejdet af" → "udarbejdet_af".
pub fn key_from_label(label: &str) -> String {
    let mut out = String::new();
    for c in label.trim().to_lowercase().chars() {
        match c {
            'æ' => out.push_str("ae"),
            'ø' => out.push_str("oe"),
            'å' => out.push_str("aa"),
            c if c.is_ascii_alphanumeric() => out.push(c),
            _ if !out.ends_with('_') && !out.is_empty() => out.push('_'),
            _ => {}
        }
    }
    let out = out.trim_end_matches('_').to_string();
    if out.is_empty() { "felt".into() } else { out }
}

/// Replaces `{today}`, `{user}`, `{duration}` and `{model}`; other text is kept.
pub fn expand(text: &str, ctx: &PlaceholderContext) -> String {
    text.replace("{today}", &ctx.today)
        .replace("{user}", &ctx.user)
        .replace("{duration}", &ctx.duration)
        .replace("{model}", &ctx.model)
}

/// Loads every `*.toml` in `dir`, sorted by name. Broken files are returned
/// as errors next to the good ones so the UI can show them on their card.
pub fn load_dir(dir: &Path) -> Vec<Result<Template, TemplateError>> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    paths.iter().map(|p| Template::load(p)).collect()
}

/// Writes the built-in templates into `dir` if it has none yet.
pub fn install_defaults(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let has_any = std::fs::read_dir(dir)?
        .filter_map(Result::ok)
        .any(|e| e.path().extension().is_some_and(|x| x == "toml"));
    if has_any {
        return Ok(());
    }
    for (name, body) in DEFAULTS {
        std::fs::write(dir.join(name), body)?;
    }
    Ok(())
}

const DEFAULTS: &[(&str, &str)] = &[
    (
        "notat.toml",
        r#"name = "Notat"
heading = "NOTAT"

[[fields]]
key = "sagsnr"
label = "Sagsnr."
required = true

[[fields]]
key = "dato"
label = "Dato"
kind = "date"
default = "{today}"
required = true

[[fields]]
key = "udarbejdet_af"
label = "Udarbejdet af"
default = "{user}"
required = true

[[fields]]
key = "emne"
label = "Emne"
"#,
    ),
    (
        "moedereferat.toml",
        r#"name = "Mødereferat"
heading = "MØDEREFERAT"

[[fields]]
key = "dato"
label = "Dato"
kind = "date"
default = "{today}"
required = true

[[fields]]
key = "deltagere"
label = "Deltagere"
kind = "multiline"

[[fields]]
key = "referent"
label = "Referent"
default = "{user}"

[[fields]]
key = "emne"
label = "Emne"
required = true
"#,
    ),
    (
        "afhoeringsrapport.toml",
        r#"name = "Afhøringsrapport"
heading = "AFHØRINGSRAPPORT"

[[fields]]
key = "sagsnr"
label = "Sagsnr."
required = true

[[fields]]
key = "dato"
label = "Dato"
kind = "date"
default = "{today}"
required = true

[[fields]]
key = "afhoert"
label = "Afhørt"
required = true

[[fields]]
key = "afhoerer"
label = "Afhører"
default = "{user}"
required = true

[[fields]]
key = "sted"
label = "Sted"
"#,
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Template, TemplateError> {
        Template::parse("t", text, Path::new("/tpl/t.toml"))
    }

    #[test]
    fn keys_come_from_labels() {
        assert_eq!(key_from_label("Udarbejdet af"), "udarbejdet_af");
        assert_eq!(key_from_label("Sagsnr."), "sagsnr");
        assert_eq!(key_from_label("Afhørt"), "afhoert");
        assert_eq!(key_from_label("!!"), "felt");
    }

    #[test]
    fn defaults_fill_in_optional_settings() {
        let t = parse("name = \"Notat\"").unwrap();
        assert_eq!(t.body_size_pt, 11.0);
        assert_eq!(t.summary_heading, "Resumé");
        assert!(t.fields.is_empty());
    }

    #[test]
    fn duplicate_field_keys_are_rejected_with_the_path() {
        let err = parse(
            "name = \"x\"\n[[fields]]\nkey = \"a\"\nlabel = \"A\"\n[[fields]]\nkey = \"a\"\nlabel = \"B\"",
        )
        .unwrap_err();
        assert!(err.to_string().contains("/tpl/t.toml"), "{err}");
        assert!(err.to_string().contains("used twice"), "{err}");
    }

    #[test]
    fn list_fields_need_options() {
        let err = parse("name = \"x\"\n[[fields]]\nkey = \"k\"\nlabel = \"K\"\nkind = \"list\"").unwrap_err();
        assert!(err.to_string().contains("needs options"), "{err}");
    }

    #[test]
    fn keys_must_be_simple_identifiers() {
        assert!(parse("name = \"x\"\n[[fields]]\nkey = \"Sags nr\"\nlabel = \"K\"").is_err());
    }

    #[test]
    fn toml_syntax_errors_name_the_file() {
        let err = parse("name = ").unwrap_err();
        assert!(err.to_string().starts_with("/tpl/t.toml"), "{err}");
    }

    #[test]
    fn placeholders_expand_in_initial_values() {
        let t = parse(
            "name = \"x\"\n[[fields]]\nkey = \"d\"\nlabel = \"Dato\"\ndefault = \"{today}\"\n\
             [[fields]]\nkey = \"u\"\nlabel = \"Af\"\ndefault = \"{user} ({model})\"",
        )
        .unwrap();
        let ctx = PlaceholderContext {
            today: "2. oktober 2026".into(),
            user: "Ane".into(),
            model: "Edda".into(),
            ..Default::default()
        };
        let v = t.initial_values(&ctx);
        assert_eq!(v["d"], "2. oktober 2026");
        assert_eq!(v["u"], "Ane (Edda)");
    }

    #[test]
    fn missing_required_lists_empty_and_absent_fields_by_label() {
        let t = parse(
            "name = \"x\"\n[[fields]]\nkey = \"a\"\nlabel = \"Sagsnr.\"\nrequired = true\n\
             [[fields]]\nkey = \"b\"\nlabel = \"Af\"\nrequired = true\n[[fields]]\nkey = \"c\"\nlabel = \"Emne\"",
        )
        .unwrap();
        let mut v = BTreeMap::new();
        v.insert("a".to_string(), "  ".to_string());
        assert_eq!(t.missing_required(&v), ["Sagsnr.", "Af"]);
    }

    #[test]
    fn relative_logo_resolves_next_to_the_template() {
        let t = parse("name = \"x\"\nlogo = \"logo.png\"").unwrap();
        assert_eq!(t.logo_path().unwrap(), Path::new("/tpl/logo.png"));
    }

    #[test]
    fn built_in_templates_are_valid_and_round_trip_through_toml() {
        for (name, body) in DEFAULTS {
            let t = Template::parse(name, body, Path::new(name)).unwrap();
            let again = Template::parse(name, &t.to_toml(), Path::new(name)).unwrap();
            assert_eq!(t.fields, again.fields, "{name}");
        }
    }

    #[test]
    fn install_defaults_only_writes_into_an_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        install_defaults(dir.path()).unwrap();
        let loaded: Vec<_> = load_dir(dir.path())
            .into_iter()
            .map(|t| t.unwrap().name)
            .collect();
        assert_eq!(loaded, ["Afhøringsrapport", "Mødereferat", "Notat"]);
        std::fs::remove_file(dir.path().join("notat.toml")).unwrap();
        install_defaults(dir.path()).unwrap();
        assert_eq!(load_dir(dir.path()).len(), 2, "user deletions are respected");
    }

    #[test]
    fn load_dir_reports_broken_files_alongside_good_ones() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.toml"), "name = \"A\"").unwrap();
        std::fs::write(dir.path().join("b.toml"), "name = ").unwrap();
        let r = load_dir(dir.path());
        assert!(r[0].is_ok() && r[1].is_err());
    }
}

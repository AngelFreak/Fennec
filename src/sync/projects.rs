//! What the phone learns about Fennec's projects and templates, for filing
//! its recordings.

use std::path::Path;

use serde_json::{Value, json};

use crate::store::Project;

/// What `POST /v1/hello` says about a project.
pub fn project_json(p: &Project) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "color": p.color,
        "default_template": p.default_template,
        "documents": p.document_count,
    })
}

/// Fennec's templates, with their header fields, for the phone's pickers.
pub fn templates_json(dir: &Path) -> Vec<Value> {
    crate::template::load_dir(dir)
        .into_iter()
        .flatten()
        .map(|t| {
            json!({
                "id": t.id,
                "name": t.name,
                "fields": t.fields.iter().map(|f| json!({
                    "key": f.key,
                    "label": f.label,
                    "kind": f.kind,
                    "required": f.required,
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

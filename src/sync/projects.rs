//! Projects from the phone: Fennec Recorder can add them and change their
//! name, colour and default template. Deleting stays on the computer, since
//! it moves documents to Unsorted.

use std::path::Path;

use serde_json::{Value, json};

use super::http::Response;
use crate::store::{PROJECT_COLORS, Project, ProjectId, Store};

/// What `GET /v1/info` says about a project.
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

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("phone project change: {e}");
    Response::error(500, "internal", "Fennec could not save the project.")
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A name the phone sent: trimmed, 1–80 characters, not another project's.
fn check_name(store: &Store, name: &str, except: Option<ProjectId>) -> Result<String, Response> {
    let name: String = name.trim().chars().take(80).collect();
    if name.is_empty() {
        return Err(Response::error(400, "bad_name", "Give the project a name."));
    }
    let taken = store
        .projects()
        .map_err(internal)?
        .iter()
        .any(|p| Some(p.id) != except && p.name.to_lowercase() == name.to_lowercase());
    if taken {
        return Err(Response::error(
            409,
            "exists",
            "Fennec already has a project with that name.",
        ));
    }
    Ok(name)
}

fn check_template(templates: &Path, id: &str) -> Result<(), Response> {
    let known = crate::template::load_dir(templates)
        .into_iter()
        .flatten()
        .any(|t| t.id == id);
    if known {
        Ok(())
    } else {
        Err(Response::error(
            400,
            "bad_template",
            "Fennec has no template with that id.",
        ))
    }
}

fn find(store: &Store, id: ProjectId) -> Result<Option<Project>, Response> {
    Ok(store
        .projects()
        .map_err(internal)?
        .into_iter()
        .find(|p| p.id == id))
}

/// `POST /v1/projects {name, color?, default_template?}`.
pub fn create(store: &Store, templates: &Path, body: &[u8]) -> (Response, bool) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return (
            Response::error(400, "bad_request", "Send the project as JSON."),
            false,
        );
    };
    let name = match check_name(store, v["name"].as_str().unwrap_or_default(), None) {
        Ok(n) => n,
        Err(r) => return (r, false),
    };
    let count = store.projects().map(|p| p.len()).unwrap_or(0);
    let color = match v["color"].as_str() {
        Some(c) if valid_color(c) => c.to_string(),
        Some(_) => {
            return (
                Response::error(400, "bad_color", "Colours are written #RRGGBB."),
                false,
            );
        }
        None => PROJECT_COLORS[count % PROJECT_COLORS.len()].to_string(),
    };
    let template = v["default_template"].as_str().filter(|t| !t.is_empty());
    if let Some(t) = template
        && let Err(r) = check_template(templates, t)
    {
        return (r, false);
    }
    let created = store
        .create_project(&name, &color)
        .and_then(|id| store.update_project(id, &name, &color, template).map(|()| id));
    match created.map(|id| find(store, id)) {
        Ok(Ok(Some(p))) => (Response::ok(project_json(&p)), true),
        Ok(Ok(None)) => (internal("the new project vanished"), false),
        Ok(Err(r)) => (r, false),
        Err(e) => (internal(e), false),
    }
}

/// `PUT /v1/projects/{id}`: any of `name`, `color`, `default_template`
/// (`null` clears it). Fields left out stay as they are.
pub fn update(store: &Store, templates: &Path, id: ProjectId, body: &[u8]) -> (Response, bool) {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return (
            Response::error(400, "bad_request", "Send the changes as JSON."),
            false,
        );
    };
    let p = match find(store, id) {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                Response::error(404, "unknown_project", "Fennec has no such project."),
                false,
            );
        }
        Err(r) => return (r, false),
    };
    let name = match v.get("name").and_then(Value::as_str) {
        Some(n) => match check_name(store, n, Some(id)) {
            Ok(n) => n,
            Err(r) => return (r, false),
        },
        None => p.name.clone(),
    };
    let color = match v.get("color") {
        None => p.color.clone(),
        Some(Value::String(c)) if valid_color(c) => c.clone(),
        Some(_) => {
            return (
                Response::error(400, "bad_color", "Colours are written #RRGGBB."),
                false,
            );
        }
    };
    let template = match v.get("default_template") {
        None => p.default_template.clone(),
        Some(Value::Null) => None,
        Some(Value::String(t)) if t.is_empty() => None,
        Some(Value::String(t)) => match check_template(templates, t) {
            Ok(()) => Some(t.clone()),
            Err(r) => return (r, false),
        },
        Some(_) => {
            return (
                Response::error(400, "bad_template", "default_template is an id or null."),
                false,
            );
        }
    };
    if let Err(e) = store.update_project(id, &name, &color, template.as_deref()) {
        return (internal(e), false);
    }
    match find(store, id) {
        Ok(Some(p)) => (Response::ok(project_json(&p)), true),
        Ok(None) => (internal("the project vanished"), false),
        Err(r) => (r, false),
    }
}

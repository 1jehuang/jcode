//! Model-facing names are resolved at the tool boundary. Storage and swarm keep IDs.
use super::*;

/// Keep the existing leniency for providers that stringify tool arguments.
pub(super) fn normalize_containers(input: &mut Value) {
    *input = normalize_todo_input(std::mem::take(input));
    for key in ["update", "add", "remove"] {
        let Some(value) = input.get_mut(key) else {
            continue;
        };
        if let Value::String(raw) = value {
            if raw.trim().is_empty() {
                *value = Value::Null;
            } else if let Ok(parsed) = serde_json::from_str::<Value>(raw.trim()) {
                *value = parsed;
            }
        }
        if value.is_object() {
            *value = Value::Array(vec![std::mem::take(value)]);
        }
        if key != "remove"
            && let Some(items) = value.as_array_mut()
        {
            for item in items {
                if let Value::String(raw) = item
                    && let Ok(parsed @ Value::Object(_)) = serde_json::from_str::<Value>(raw.trim())
                {
                    *item = parsed;
                }
            }
        }
    }
}

pub(super) fn uses_names(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.contains_key("name") || fields.values().any(uses_names),
        Value::Array(items) => items.iter().any(uses_names),
        _ => false,
    }
}

fn task_name(todo: &Value) -> Option<&str> {
    todo.get("content").and_then(Value::as_str)
}

fn resolve_name(todos: &[Value], name: &str) -> Result<String> {
    let mut matches = todos.iter().filter(|todo| task_name(todo) == Some(name));
    let todo = matches
        .next()
        .ok_or_else(|| anyhow::anyhow!("unknown todo name {name:?}"))?;
    if matches.next().is_some() {
        bail!("ambiguous todo name {name:?}; task names must be unique within the plan");
    }
    todo.get("id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("stored todo {name:?} has no internal ID"))
}

fn normalize_item(item: &mut Value, previous: &[Value]) -> Result<bool> {
    let Some(fields) = item.as_object_mut() else {
        return Ok(false);
    };
    let Some(name) = fields.remove("name") else {
        return Ok(false);
    };
    let name = name
        .as_str()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow::anyhow!("todo name must be a nonempty string"))?;
    if fields.contains_key("id") || fields.contains_key("content") || fields.contains_key("details")
    {
        bail!("use name/description without id/content/details for named todos");
    }
    let id = if previous.iter().any(|todo| task_name(todo) == Some(name)) {
        resolve_name(previous, name)?
    } else {
        format!("todo_{}", uuid::Uuid::new_v4().simple())
    };
    fields.insert("id".into(), json!(id));
    fields.insert("content".into(), json!(name));
    if let Some(description) = fields.remove("description") {
        fields.insert("details".into(), description);
    }
    Ok(true)
}

fn resolve_dependencies(value: &mut Value, catalog: &[Value]) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    let names = value
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("todo prerequisites must be an array of names"))?;
    for name in names {
        let id = resolve_name(
            catalog,
            name.as_str()
                .ok_or_else(|| anyhow::anyhow!("todo prerequisite must be a name string"))?,
        )?;
        *name = json!(id);
    }
    Ok(())
}

/// Accept legacy calls unchanged, but never expose legacy keys in the schema.
/// Name-based references are resolved before the existing atomic patch pipeline.
pub(super) fn normalize_public_input(input: &mut Value, previous: &[Value]) -> Result<()> {
    normalize_containers(input);
    // Public dependency names differ from the persisted legacy representation.
    fn rename_field(value: &mut Value, public: &str, internal: &str) -> Result<()> {
        if let Some(fields) = value.as_object_mut() {
            if fields.contains_key(public) && fields.contains_key(internal) {
                bail!("use {public}, not both {public} and {internal}");
            }
            if let Some(dependencies) = fields.remove(public) {
                fields.insert(internal.into(), dependencies);
            }
        }
        Ok(())
    }
    if let Some(phases) = input.get_mut("phases").and_then(Value::as_array_mut) {
        for phase in phases {
            if let Some(lanes) = phase.get_mut("lanes").and_then(Value::as_array_mut) {
                for lane in lanes {
                    rename_field(lane, "dependencies", "after")?;
                }
            }
        }
    }
    for key in ["todos", "add", "update"] {
        if let Some(items) = input.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                rename_field(item, "lane_dependencies", "lane_after")?;
            }
        }
    }
    let mut catalog = previous.to_vec();
    let mut named = false;
    let mut replacement = Vec::new();
    if let Some(phases) = input.get_mut("phases").and_then(Value::as_array_mut) {
        for phase in phases {
            if let Some(lanes) = phase.get_mut("lanes").and_then(Value::as_array_mut) {
                for lane in lanes {
                    if let Some(items) = lane.get_mut("items").and_then(Value::as_array_mut) {
                        for item in items {
                            named |= normalize_item(item, previous)?;
                            replacement.push(item.clone());
                        }
                    }
                }
            }
        }
        catalog = replacement;
    } else if let Some(items) = input.get_mut("todos").and_then(Value::as_array_mut) {
        for item in items.iter_mut() {
            named |= normalize_item(item, previous)?;
        }
        catalog = items.clone();
    }
    if let Some(adds) = input.get_mut("add").and_then(Value::as_array_mut) {
        for item in adds {
            named |= normalize_item(item, &[])?;
            catalog.push(item.clone());
        }
    }
    if let Some(updates) = input.get("update").and_then(Value::as_array) {
        named |= updates.iter().any(|item| item.get("name").is_some());
    }
    // remove has no item object, so distinguish the public form from legacy IDs.
    if let Some(removes) = input.get("remove").and_then(Value::as_array) {
        named |= removes.iter().filter_map(Value::as_str).any(|name| {
            previous.iter().any(|todo| task_name(todo) == Some(name))
                || !previous
                    .iter()
                    .any(|todo| todo.get("id").and_then(Value::as_str) == Some(name))
        });
    }
    if !named {
        return Ok(());
    }
    validate_unique_names(&catalog)?;
    if let Some(phases) = input.get_mut("phases").and_then(Value::as_array_mut) {
        for phase in phases {
            if let Some(lanes) = phase.get_mut("lanes").and_then(Value::as_array_mut) {
                for lane in lanes {
                    if let Some(after) = lane.get_mut("after") {
                        resolve_dependencies(after, &catalog)?;
                    }
                }
            }
        }
    }
    for key in ["todos", "add"] {
        if let Some(items) = input.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                if let Some(after) = item.get_mut("lane_after") {
                    resolve_dependencies(after, &catalog)?;
                }
                if key == "add"
                    && let Some(after) = item.get_mut("after").filter(|value| !value.is_null())
                {
                    *after = json!(resolve_name(
                        &catalog,
                        after
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("after must be a todo name"))?
                    )?);
                }
            }
        }
    }
    // All references use names at the beginning of the call, plus additions.
    // Renaming a prerequisite never changes its stored dependency identity.
    let reference_catalog = catalog.clone();
    if let Some(updates) = input.get_mut("update").and_then(Value::as_array_mut) {
        for update in updates {
            let Some(fields) = update.as_object_mut() else {
                continue;
            };
            if let Some(name) = fields.remove("name") {
                if fields.contains_key("id")
                    || fields.contains_key("content")
                    || fields.contains_key("details")
                {
                    bail!("named updates must not include id/content/details");
                }
                let name = name
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("update name must be a string"))?;
                let id = resolve_name(previous, name)?;
                fields.insert("id".into(), json!(id));
                if let Some(description) = fields.remove("description") {
                    fields.insert("details".into(), description);
                }
                if let Some(rename) = fields.remove("rename_to").filter(|value| !value.is_null()) {
                    let rename = rename
                        .as_str()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .ok_or_else(|| anyhow::anyhow!("rename_to must be a nonempty name"))?;
                    let target = catalog
                        .iter_mut()
                        .find(|todo| todo.get("id").and_then(Value::as_str) == Some(&id))
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "send a replacement plan or incremental edits, not both"
                            )
                        })?;
                    target["content"] = json!(rename);
                    fields.insert("content".into(), json!(rename));
                }
                if let Some(after) = fields.get_mut("lane_after") {
                    resolve_dependencies(after, &reference_catalog)?;
                }
            }
        }
    }
    if let Some(removes) = input.get_mut("remove").and_then(Value::as_array_mut) {
        for name in removes {
            *name = json!(resolve_name(
                previous,
                name.as_str()
                    .ok_or_else(|| anyhow::anyhow!("remove entries must be todo names"))?
            )?);
        }
    }
    validate_unique_names(&catalog)?;
    Ok(())
}

fn validate_unique_names(todos: &[Value]) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for todo in todos {
        let name = task_name(todo)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("todo name must be nonempty"))?;
        if !names.insert(name) {
            bail!("duplicate todo name {name:?}; names must be unique within the plan")
        }
    }
    Ok(())
}

pub(super) fn public_todos(todos: &[TodoItem]) -> Result<Value> {
    let mut value = serde_json::to_value(todos)?;
    if let Some(items) = value.as_array_mut() {
        for item in items {
            let fields = item
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("Serialized todo must be an object"))?;
            fields.remove("id");
            if let Some(name) = fields.remove("content") {
                fields.insert("name".into(), name);
            }
            if let Some(description) = fields.remove("details") {
                fields.insert("description".into(), description);
            }
            if let Some(after) = fields.get_mut("lane_after").and_then(Value::as_array_mut) {
                for id in after {
                    if let Some(todo) = todos
                        .iter()
                        .find(|todo| Some(todo.id.as_str()) == id.as_str())
                    {
                        *id = json!(todo.content);
                    }
                }
            }
            if let Some(dependencies) = fields.remove("lane_after") {
                fields.insert("lane_dependencies".into(), dependencies);
            }
        }
    }
    Ok(value)
}

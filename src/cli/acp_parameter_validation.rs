use super::*;

pub(super) fn cwd_from_params(params: &Value) -> std::result::Result<PathBuf, String> {
    let cwd = match params.get("cwd").and_then(Value::as_str) {
        Some(cwd) if !cwd.trim().is_empty() => PathBuf::from(cwd),
        _ => std::env::current_dir().map_err(|err| err.to_string())?,
    };
    if !cwd.is_absolute() {
        return Err(format!("ACP cwd must be absolute: {}", cwd.display()));
    }
    Ok(cwd)
}

pub(super) fn required_session_id(params: &Value) -> std::result::Result<String, String> {
    params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "Missing required sessionId".to_string())
}

pub(super) fn validate_acp_mcp_servers(params: &Value) -> std::result::Result<(), String> {
    match params.get("mcpServers") {
        None | Some(Value::Null) => Ok(()),
        // Session-scoped MCP is not supported yet, but rejecting this required
        // ACP field prevents hosts with MCP servers from creating a session.
        Some(Value::Array(_)) => Ok(()),
        Some(_) => Err("ACP mcpServers must be an array".to_string()),
    }
}

/// Read an explicit skill selection from ACP prompt metadata. Accepts
/// `_meta.jcode.skill`, `_meta.jcode.activeSkill`, `_meta.skill`, and
/// `_meta.activeSkill`.
pub(super) fn explicit_skill_from_params(params: &Value) -> Option<String> {
    let meta = params.get("_meta")?;
    [meta.get("jcode"), Some(meta)]
        .into_iter()
        .flatten()
        .flat_map(|object| ["skill", "activeSkill", "active_skill"].map(|key| object.get(key)))
        .flatten()
        .filter_map(Value::as_str)
        .map(|name| name.trim().trim_start_matches(['/', '$']).trim())
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

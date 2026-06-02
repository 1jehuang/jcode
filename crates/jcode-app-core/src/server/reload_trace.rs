use anyhow::Result;
use serde_json::{Map, Value};
use std::io::Write;

fn sanitize_file_component(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    }
}

fn trace_dir() -> Result<std::path::PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("reload-traces"))
}

pub(super) fn trace_path(reload_id: &str) -> Result<std::path::PathBuf> {
    Ok(trace_dir()?.join(format!("{}.jsonl", sanitize_file_component(reload_id))))
}

pub(super) fn record(reload_id: &str, phase: &str, mut fields: Map<String, Value>) {
    let path = match trace_path(reload_id) {
        Ok(path) => path,
        Err(error) => {
            crate::logging::warn(&format!(
                "reload trace: failed to resolve trace path reload_id={} phase={}: {}",
                reload_id, phase, error
            ));
            return;
        }
    };

    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        crate::logging::warn(&format!(
            "reload trace: failed to create trace dir reload_id={} phase={} path={}: {}",
            reload_id,
            phase,
            parent.display(),
            error
        ));
        return;
    }

    fields.insert("schema_version".to_string(), Value::from(1));
    fields.insert(
        "timestamp".to_string(),
        Value::from(chrono::Utc::now().to_rfc3339()),
    );
    fields.insert(
        "timestamp_ms".to_string(),
        Value::from(chrono::Utc::now().timestamp_millis()),
    );
    fields.insert("pid".to_string(), Value::from(std::process::id()));
    fields.insert("reload_id".to_string(), Value::from(reload_id.to_string()));
    fields.insert("phase".to_string(), Value::from(phase.to_string()));

    let line = match serde_json::to_string(&Value::Object(fields)) {
        Ok(line) => line,
        Err(error) => {
            crate::logging::warn(&format!(
                "reload trace: failed to encode event reload_id={} phase={}: {}",
                reload_id, phase, error
            ));
            return;
        }
    };

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut file) => {
            if let Err(error) = writeln!(file, "{}", line) {
                crate::logging::warn(&format!(
                    "reload trace: failed to append event reload_id={} phase={} path={}: {}",
                    reload_id,
                    phase,
                    path.display(),
                    error
                ));
            }
        }
        Err(error) => crate::logging::warn(&format!(
            "reload trace: failed to open trace reload_id={} phase={} path={}: {}",
            reload_id,
            phase,
            path.display(),
            error
        )),
    }
}

pub(super) fn record_value(reload_id: &str, phase: &str, fields: Value) {
    let map = match fields {
        Value::Object(map) => map,
        other => {
            let mut map = Map::new();
            map.insert("detail".to_string(), other);
            map
        }
    };
    record(reload_id, phase, map);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn record_value_appends_jsonl_trace_event() -> anyhow::Result<()> {
        let _guard = crate::storage::lock_test_env();
        let temp_home = tempfile::TempDir::new()?;
        let prev_home = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", temp_home.path());

        record_value(
            "reload/id:with/slashes",
            "unit_phase",
            json!({"session_id": "session-1", "ok": true}),
        );

        let path = trace_path("reload/id:with/slashes")?;
        let content = std::fs::read_to_string(path)?;
        let line = content
            .lines()
            .next()
            .expect("trace should contain one line");
        let event: serde_json::Value = serde_json::from_str(line)?;
        assert_eq!(event["reload_id"], "reload/id:with/slashes");
        assert_eq!(event["phase"], "unit_phase");
        assert_eq!(event["session_id"], "session-1");
        assert_eq!(event["ok"], true);
        assert_eq!(event["schema_version"], 1);

        if let Some(prev_home) = prev_home {
            crate::env::set_var("JCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
        Ok(())
    }
}

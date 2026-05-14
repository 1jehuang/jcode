use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::process::Stdio;
use tokio::process::Command as TokioCommand;

const DB_EXECUTE_DESCRIPTION: &str = "Execute a SQL statement against the agent's local Postgres database. The statement is scoped to the agent's own schema. Use for CREATE TABLE, INSERT, UPDATE, DELETE, SELECT, DROP TABLE, etc. For queries that may return large results, limit with SQL clauses.";

pub struct DbExecuteTool;

impl DbExecuteTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Deserialize)]
struct DbExecuteInput {
    sql: String,
}

/// Build a db-execute tool that scopes SQL to the agent's session schema.
/// The container and credentials are well-known localhost defaults.
fn agent_schema_name(session_id: &str) -> String {
    // Sanitize: schema names must start with a letter or underscore,
    // contain only lowercase letters, digits, and underscores, and be <= 63 chars.
    let sanitized: String = session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    // Ensure it starts with a letter
    let prefixed = if sanitized.starts_with(|c: char| c.is_ascii_alphabetic()) {
        sanitized
    } else {
        format!("a_{}", sanitized)
    };
    // Truncate to 30 chars, then add "agent_" prefix (fits within 63-char limit)
    let short: String = prefixed.chars().take(30).collect();
    format!("agent_{}", short)
}

fn provision_schema_sql(schema: &str) -> String {
    format!(
        "CREATE SCHEMA IF NOT EXISTS {schema};\nSET search_path TO {schema};"
    )
}

#[async_trait]
impl Tool for DbExecuteTool {
    fn name(&self) -> &str {
        "db-execute"
    }

    fn description(&self) -> &str {
        DB_EXECUTE_DESCRIPTION
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["sql"],
            "properties": {
                "intent": super::intent_schema_property(),
                "sql": {
                    "type": "string",
                    "description": "SQL statement to execute. Scoped to agent's schema."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: DbExecuteInput = serde_json::from_value(input)?;
        let schema = agent_schema_name(&ctx.session_id);

        let full_sql = format!(
            "{}\n{}",
            provision_schema_sql(&schema),
            params.sql.trim()
        );

        let result = run_psql(&full_sql).await?;
        Ok(ToolOutput::new(result))
    }
}

async fn run_psql(sql: &str) -> Result<String> {
    let mut child = TokioCommand::new("docker")
        .args([
            "exec",
            "-i",
            "jcode-agent-db",
            "psql",
            "-U",
            "jcode_agent",
            "-d",
            "jcode_agent_workspace",
            "-v",
            "ON_ERROR_STOP=1",
            "-A",  // unaligned output
            "-t",  // tuples only (no headers)
            "-q",  // quiet
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    // Write SQL to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(sql.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        // stdin is dropped here, closing the pipe
    }

    let output = child.wait_with_output().await?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if output.status.success() {
        if stdout.is_empty() && stderr.is_empty() {
            Ok("OK".to_string())
        } else if stdout.is_empty() {
            Ok(stderr)
        } else {
            Ok(stdout)
        }
    } else {
        Err(anyhow::anyhow!(
            "psql error (exit {}): {}\n{}",
            output.status.code().unwrap_or(-1),
            stderr,
            stdout
        ))
    }
}

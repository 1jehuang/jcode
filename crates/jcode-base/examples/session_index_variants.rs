//! Compare SQLite index variants on a bounded sample of real persisted rows.
//! Arguments: source database, NEW output directory, private query JSON.
use anyhow::{Context, Result, ensure};
use jcode_session_types::SessionSearchQueryProfile;
use rusqlite::{Connection, OpenFlags, params, types::Value};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Instant,
};

fn connect(path: &Path) -> Result<Connection> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")?;
    Ok(connection)
}
fn milliseconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}
fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
fn query(connection: &Connection, mode: &str, text: &str) -> Result<BTreeSet<String>> {
    let profile = SessionSearchQueryProfile::new(text);
    ensure!(
        profile.is_actionable()
            && profile
                .terms
                .iter()
                .all(|t| t.is_ascii() && t.bytes().all(|b| b.is_ascii_alphanumeric())),
        "benchmark requires ASCII alphanumeric query terms"
    );
    let mut selects = Vec::new();
    let mut parameters = Vec::<Value>::new();
    for term in &profile.terms {
        let number = parameters.len() + 1;
        if mode == "word" || (mode == "trigram_full" && term.len() >= 3) {
            parameters.push(format!("\"{term}\"").into());
            selects.push(format!("SELECT DISTINCT d.sid FROM search JOIN docs d ON d.id=search.rowid WHERE search MATCH ?{number}"));
        } else if mode.starts_with("trigram") && term.len() >= 3 {
            parameters.push(format!("%{term}%").into());
            selects.push(format!("SELECT DISTINCT d.sid FROM search JOIN docs d ON d.id=search.rowid WHERE search.body LIKE ?{number}"));
        } else {
            parameters.push(format!("%{term}%").into());
            selects.push(format!(
                "SELECT DISTINCT sid FROM docs WHERE body LIKE ?{number}"
            ));
        }
    }
    parameters.push((profile.min_term_matches as i64).into());
    let sql = format!(
        "SELECT sid FROM ({}) GROUP BY sid HAVING count(*)>=?{}",
        selects.join(" UNION ALL "),
        parameters.len()
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(parameters), |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4,
        "usage: session_index_variants SOURCE_DB NEW_OUTPUT_DIR PRIVATE_QUERIES_JSON"
    );
    let output = Path::new(&args[2]);
    ensure!(!output.exists(), "output must not exist");
    let queries: BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(&args[3])?)?;
    std::fs::create_dir(output)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(output, std::fs::Permissions::from_mode(0o700))?;
    }
    let source = Connection::open_with_flags(&args[1], OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let candidates = {
        let mut statement = source.prepare(
            "SELECT id,size_bytes FROM sessions WHERE size_bytes<=33554432 ORDER BY size_bytes,id",
        )?;
        statement
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    ensure!(!candidates.is_empty(), "empty corpus");
    let sample_path = output.join("sample.sqlite3");
    let mut sample = connect(&sample_path)?;
    sample.execute_batch(
        "CREATE TABLE docs(id INTEGER PRIMARY KEY,sid TEXT NOT NULL,body TEXT NOT NULL);",
    )?;
    let mut selected = BTreeSet::new();
    let mut bytes = 0_u64;
    let mut rows = 0_u64;
    let transaction = sample.transaction()?;
    for position in 0..64 {
        let (sid, _) = &candidates[position * (candidates.len() - 1) / 63];
        if !selected.insert(sid.clone()) {
            continue;
        }
        for (table, column, key) in [
            ("sessions", "metadata", "id"),
            ("session_messages", "data", "session_id"),
            ("session_replay", "data", "session_id"),
            ("session_environment", "data", "session_id"),
            ("session_memory", "data", "session_id"),
            ("session_compaction", "data", "session_id"),
        ] {
            let mut statement =
                source.prepare(&format!("SELECT {column} FROM {table} WHERE {key}=?1"))?;
            for body in statement.query_map([sid], |r| r.get::<_, String>(0))? {
                let body = body?;
                bytes += body.len() as u64;
                ensure!(bytes <= 268435456, "sample exceeded 256 MiB hard limit");
                transaction.execute(
                    "INSERT INTO docs(sid,body) VALUES(?1,?2)",
                    params![sid, body],
                )?;
                rows += 1;
            }
        }
    }
    transaction.commit()?;
    sample.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    let base_bytes = std::fs::metadata(&sample_path)?.len();
    let mut truth = BTreeMap::new();
    for (label, text) in &queries {
        truth.insert(label.clone(), query(&sample, "scan", text)?);
    }
    drop(sample);
    let mut reports = Vec::<Json>::new();
    for mode in [
        "scan",
        "btree",
        "word",
        "trigram_none",
        "trigram_column",
        "trigram_full",
    ] {
        eprintln!("building {mode}");
        let path = output.join(format!("{mode}.sqlite3"));
        std::fs::copy(&sample_path, &path)?;
        let connection = connect(&path)?;
        let started = Instant::now();
        if mode == "btree" {
            connection.execute_batch("CREATE INDEX body_nocase ON docs(body COLLATE NOCASE);")?;
        } else if mode != "scan" {
            let (tokenizer, detail) = match mode {
                "word" => ("unicode61", "none"),
                "trigram_none" => ("trigram", "none"),
                "trigram_column" => ("trigram", "column"),
                _ => ("trigram", "full"),
            };
            connection.execute_batch(&format!("BEGIN IMMEDIATE;
                CREATE VIRTUAL TABLE search USING fts5(body,content='docs',content_rowid='id',tokenize='{tokenizer}',detail={detail},columnsize=0);
                CREATE TRIGGER docs_insert AFTER INSERT ON docs BEGIN INSERT INTO search(rowid,body) VALUES(new.id,new.body); END;
                CREATE TRIGGER docs_delete AFTER DELETE ON docs BEGIN INSERT INTO search(search,rowid,body) VALUES('delete',old.id,old.body); END;
                CREATE TRIGGER docs_update AFTER UPDATE OF body ON docs BEGIN INSERT INTO search(search,rowid,body) VALUES('delete',old.id,old.body); INSERT INTO search(rowid,body) VALUES(new.id,new.body); END;
                INSERT INTO search(search) VALUES('rebuild'); COMMIT;"))?;
        }
        connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let build_ms = milliseconds(started);
        let database_bytes = std::fs::metadata(&path)?.len();
        let mut result = BTreeMap::new();
        for (label, text) in &queries {
            let mut timings = Vec::new();
            let mut found = BTreeSet::new();
            for _ in 0..3 {
                let start = Instant::now();
                found = query(&connection, mode, text)
                    .with_context(|| format!("query class {label}, mode {mode}"))?;
                timings.push(milliseconds(start));
            }
            let expected = &truth[label];
            result.insert(label.clone(),json!({"median_ms":median(timings),"matched_sessions":found.len(),"equal_to_raw":found==*expected,"false_negatives":expected.difference(&found).count(),"false_positives":found.difference(expected).count()}));
        }
        let (id, body): (i64, String) = connection.query_row(
            "SELECT id,body FROM docs WHERE length(body) BETWEEN 256 AND 4096 ORDER BY id LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut commits = Vec::new();
        for iteration in 0..3 {
            let replacement = format!("{body} mutationprobe{iteration}");
            let start = Instant::now();
            connection.execute(
                "UPDATE docs SET body=?1 WHERE id=?2",
                params![replacement, id],
            )?;
            commits.push(milliseconds(start));
            let stored: String =
                connection.query_row("SELECT body FROM docs WHERE id=?1", [id], |r| r.get(0))?;
            ensure!(stored == replacement, "mutation readback differs");
        }
        connection.execute("UPDATE docs SET body=?1 WHERE id=?2", params![body, id])?;
        reports.push(json!({"mode":mode,"build_ms":build_ms,"database_bytes":database_bytes,"index_bytes":database_bytes.saturating_sub(base_bytes),"query":result,"small_row_update_median_ms":median(commits)}));
        eprintln!("finished {mode}");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"sqlite_version":rusqlite::version(),"sample_sessions":selected.len(),"sample_rows":rows,"sample_payload_bytes":bytes,"source_eligible_sessions":candidates.len(),"excluded_session_size_over_bytes":33554432,"base_database_bytes":base_bytes,"query_repeats":3,"scope":"raw persisted-row candidate search, not full SessionSearchTool or picker","variants":reports})
        )?
    );
    Ok(())
}

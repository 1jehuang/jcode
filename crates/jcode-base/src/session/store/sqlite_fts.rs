//! Transactional substring indexes over persisted session rows.
use anyhow::{Result, ensure};
use rusqlite::{Connection, TransactionBehavior};
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "sqlite_fts_tests.rs"]
mod tests;

const SOURCES: [(&str, &str, &str); 6] = [
    ("sessions", "metadata", "id"),
    ("session_messages", "data", "session_id"),
    ("session_replay", "data", "session_id"),
    ("session_environment", "data", "session_id"),
    ("session_memory", "data", "session_id"),
    ("session_compaction", "data", "session_id"),
];

fn normalized(column: &str) -> String {
    // These Unicode lowercase mappings contain ASCII letters. Other non-ASCII
    // queries use the conservative fallback instead of SQLite's case folding.
    format!("replace(replace({column},'K','k'),'İ','i̇')")
}

pub(super) fn migrate(connection: &mut Connection) -> Result<()> {
    let current: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current >= 5 {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version >= 5 {
        transaction.commit()?;
        return Ok(());
    }
    ensure!(version == 4, "FTS migration requires session schema 4");
    for (table, column, _) in SOURCES {
        let index = format!("{table}_search_fts");
        let view = format!("{table}_search_content");
        let text = normalized(column);
        let new_text = normalized(&format!("new.{column}"));
        let old_text = normalized(&format!("old.{column}"));
        transaction.execute_batch(&format!(
            "CREATE VIEW {view} AS SELECT rowid AS rowid,{text} AS text FROM {table};
             CREATE VIRTUAL TABLE {index} USING fts5(text,content='{view}',content_rowid='rowid',tokenize='trigram',detail=none);
             CREATE TRIGGER {index}_insert AFTER INSERT ON {table} BEGIN
               INSERT INTO {index}(rowid,text) VALUES(new.rowid,{new_text});
             END;
             CREATE TRIGGER {index}_delete AFTER DELETE ON {table} BEGIN
               INSERT INTO {index}({index},rowid,text) VALUES('delete',old.rowid,{old_text});
             END;
             CREATE TRIGGER {index}_update AFTER UPDATE OF {column} ON {table}
             WHEN old.{column} IS NOT new.{column} BEGIN
               INSERT INTO {index}({index},rowid,text) VALUES('delete',old.rowid,{old_text});
               INSERT INTO {index}(rowid,text) VALUES(new.rowid,{new_text});
             END;
             INSERT INTO {index}({index}) VALUES('rebuild');"
        ))?;
    }
    transaction.execute_batch("PRAGMA user_version=5;")?;
    transaction.commit()?;
    Ok(())
}

pub(super) fn candidates(connection: &Connection, query: &str) -> Result<Vec<String>> {
    let profile = jcode_session_types::SessionSearchQueryProfile::new(query);
    let all_ids = || -> Result<Vec<String>> {
        let mut statement = connection.prepare_cached("SELECT id FROM sessions")?;
        let rows = statement.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    // Trigrams cannot narrow short terms. Preserve existing substring behavior
    // for Unicode too, rather than silently imposing SQLite's case semantics.
    if !profile.is_actionable()
        || profile.terms.len() > 64
        || profile
            .terms
            .iter()
            .any(|term| !term.is_ascii() || term.len() < 3)
    {
        return all_ids();
    }
    // Each term's trigrams must occur in one persisted row. Positions are not
    // stored, so the existing result scorer verifies the remaining candidates.
    let mut selects = Vec::new();
    let mut parameters: Vec<rusqlite::types::Value> = Vec::new();
    for (term_number, term) in profile.terms.iter().enumerate() {
        // Individual three-character MATCH tokens work with detail=none and
        // do not read external content to recheck a LIKE predicate.
        let grams: BTreeSet<_> = term
            .as_bytes()
            .windows(3)
            .take(64)
            .map(|gram| format!("\"{}\"", std::str::from_utf8(gram).expect("ASCII term")))
            .collect();
        parameters.push(grams.into_iter().collect::<Vec<_>>().join(" AND ").into());
        for (table, _, id_column) in SOURCES {
            selects.push(format!(
                "SELECT source.{id_column} AS id,{term_number} AS term
                 FROM {table}_search_fts JOIN {table} AS source ON source.rowid={table}_search_fts.rowid
                 WHERE {table}_search_fts MATCH ?{}", term_number + 1
            ));
        }
    }
    parameters.push((profile.min_term_matches as i64).into());
    let sql = format!(
        "WITH hits AS ({}) SELECT id FROM hits GROUP BY id HAVING count(DISTINCT term)>=?{}",
        selects.join(" UNION ALL "),
        parameters.len()
    );
    let mut statement = connection.prepare_cached(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(parameters), |row| row.get(0))?;
    let mut ids: BTreeSet<String> = rows.collect::<rusqlite::Result<_>>()?;
    // Session identifiers are a supported search surface, even without a body hit.
    for id in all_ids()? {
        if jcode_session_types::session_search_path_matches_query(&id, &profile) {
            ids.insert(id);
        }
    }
    Ok(ids.into_iter().collect())
}

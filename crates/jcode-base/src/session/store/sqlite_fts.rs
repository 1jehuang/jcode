//! Transactional substring indexes over persisted session rows.
use anyhow::Result;
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

pub(super) fn ready(connection: &Connection) -> Result<bool> {
    let count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'
             AND name IN ('sessions_search_fts','session_messages_search_fts','session_replay_search_fts',
                          'session_environment_search_fts','session_memory_search_fts','session_compaction_search_fts')
             AND sql LIKE '%detail=full%'",
            [], |row| row.get(0),
        )?;
    Ok(count == SOURCES.len() as i64)
}

pub(super) fn migrate(connection: &mut Connection) -> Result<()> {
    if ready(connection)? {
        return Ok(());
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if ready(&transaction)? {
        transaction.commit()?;
        return Ok(());
    }
    for (table, column, _) in SOURCES {
        transaction.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {table}_search_fts_insert;
             DROP TRIGGER IF EXISTS {table}_search_fts_delete;
             DROP TRIGGER IF EXISTS {table}_search_fts_update;
             DROP TABLE IF EXISTS {table}_search_fts;
             DROP VIEW IF EXISTS {table}_search_content;"
        ))?;
        let index = format!("{table}_search_fts");
        let view = format!("{table}_search_content");
        let text = normalized(column);
        let new_text = normalized(&format!("new.{column}"));
        let old_text = normalized(&format!("old.{column}"));
        transaction.execute_batch(&format!(
            "CREATE VIEW {view} AS SELECT rowid AS rowid,{text} AS text FROM {table};
             CREATE VIRTUAL TABLE {index} USING fts5(text,content='{view}',content_rowid='rowid',tokenize='trigram',detail=full);
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
    transaction.commit()?;
    // Avoid replaying a bulk index-build WAL on every later process startup.
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
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
    // Positional trigram phrases verify contiguous substrings using postings,
    // without reading external transcript content before bounded scoring.
    let mut selects = Vec::new();
    let mut parameters: Vec<rusqlite::types::Value> = Vec::new();
    for (term_number, term) in profile.terms.iter().enumerate() {
        parameters.push(format!("\"{term}\"").into());
        let match_parameter = parameters.len();
        for (table, _, id_column) in SOURCES {
            selects.push(format!(
                "SELECT source.{id_column} AS id,{term_number} AS term
                 FROM {table}_search_fts JOIN {table} AS source ON source.rowid={table}_search_fts.rowid
                 WHERE {table}_search_fts MATCH ?{match_parameter}"
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

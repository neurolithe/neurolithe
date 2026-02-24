use rusqlite::Connection;

/// Initialize the database schema for NeuroLithe memory service.
/// This creates the required `episodes`, `nodes`, and `edges` tables if they don't exist.
pub fn init_schema(conn: &Connection, vector_dimension: usize) -> rusqlite::Result<()> {
    // 1. The Ground-Truth Episodic Logs
    conn.execute(
        "CREATE TABLE IF NOT EXISTS episodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tenant_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            raw_dialogue TEXT NOT NULL,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // 2. The Graph Nodes (The Facts)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tenant_id TEXT NOT NULL,
            source_episode_id INTEGER,
            payload JSON NOT NULL,
            status TEXT DEFAULT 'active',
            is_explicit BOOLEAN DEFAULT 0,
            support_count INTEGER DEFAULT 1,
            relevance_score REAL DEFAULT 1.0,
            last_accessed_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY(source_episode_id) REFERENCES episodes(id)
        )",
        [],
    )?;

    // 3. The Graph Edges (The Relationships with Temporal Bounds)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS edges (
            source_id INTEGER,
            target_id INTEGER,
            relation TEXT NOT NULL,
            valid_from DATETIME,
            valid_until DATETIME,
            weight REAL DEFAULT 1.0,
            FOREIGN KEY(source_id) REFERENCES nodes(id),
            FOREIGN KEY(target_id) REFERENCES nodes(id)
        )",
        [],
    )?;
    // 4. The Vector Index (Semantic Search via sqlite-vec)
    // We use vec0 to store our configurable dimensional embeddings
    let vec_query = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_nodes USING vec0(
            node_id INTEGER PRIMARY KEY,
            embedding float[{}]
        )",
        vector_dimension
    );
    conn.execute(&vec_query, [])?;

    // 5. The FTS5 Index (Full-Text Keyword Search)
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS fts_nodes USING fts5(
            payload,
            content='nodes',
            content_rowid='id'
        )",
        [],
    )?;

    // FTS sync triggers
    conn.execute_batch(
        "
        CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
            INSERT INTO fts_nodes(rowid, payload) VALUES (new.id, new.payload);
        END;
        CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
            INSERT INTO fts_nodes(fts_nodes, rowid, payload) VALUES ('delete', old.id, old.payload);
        END;
        CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
            INSERT INTO fts_nodes(fts_nodes, rowid, payload) VALUES ('delete', old.id, old.payload);
            INSERT INTO fts_nodes(rowid, payload) VALUES (new.id, new.payload);
        END;
        "
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::database::init_db;

    #[test]
    fn test_init_schema() {
        let conn = init_db(None as Option<&String>).expect("Failed to open DB");
        init_schema(&conn, 1536).expect("Failed to initialize schema");

        // Verify tables exist
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        
        let table_names: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();

        assert!(table_names.contains(&"episodes".to_string()));
        assert!(table_names.contains(&"nodes".to_string()));
        assert!(table_names.contains(&"edges".to_string()));
        assert!(table_names.contains(&"vec_nodes".to_string()));
        assert!(table_names.contains(&"fts_nodes".to_string()));
    }
}

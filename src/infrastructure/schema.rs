use rusqlite::Connection;

/// Initialize the database schema for NeuroLithe memory service.
/// This creates the required `episodes`, `nodes`, and `edges` tables if they don't exist.
pub fn init_schema(conn: &Connection, vector_dimension: usize) -> rusqlite::Result<()> {
    // 0. The CCL Registry
    conn.execute(
        "CREATE TABLE IF NOT EXISTS ccl_registry (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tenant_id TEXT NOT NULL,
            name TEXT NOT NULL,
            description TEXT NOT NULL,
            UNIQUE(tenant_id, name)
        )",
        [],
    )?;

    // 1. The Ground-Truth Episodic Logs
    conn.execute(
        "CREATE TABLE IF NOT EXISTS episodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tenant_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            raw_dialogue TEXT NOT NULL,
            ccl TEXT DEFAULT 'reality',
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
            ccl TEXT DEFAULT 'reality',
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
            ccl TEXT DEFAULT 'reality',
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
        ",
    )?;

    // 6. Idempotency ledger for `memory.command` writes. A `commandId` is
    // recorded after a remember/forget succeeds; a duplicate delivery is
    // skipped. Old rows are swept periodically (the ids only guard against
    // at-least-once redelivery, which is bounded in time).
    conn.execute(
        "CREATE TABLE IF NOT EXISTS processed_commands (
            command_id TEXT PRIMARY KEY,
            processed_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    Ok(())
}

/// Initialize the Long-Term Memory schema (the permanent knowledge tree) in the
/// LTM database. Creates `tree_nodes`, `tree_edges` (multi-parent DAG),
/// `leaves` (`dataId` references), the `vec_ltm` sqlite-vec index at the LTM
/// dimension, and `fts_ltm` over node summaries with sync triggers. Idempotent.
pub fn init_ltm_schema(conn: &Connection, vector_dimension: usize) -> rusqlite::Result<()> {
    // 0. Concept / leaf nodes. `permanent` is always true — LTM never decays.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tree_nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL,
            summary TEXT NOT NULL DEFAULT '',
            kind TEXT NOT NULL DEFAULT 'grown',
            permanent BOOLEAN NOT NULL DEFAULT 1,
            created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
        )",
        [],
    )?;

    // 1. Parent -> child edges. The composite PK allows a node to sit under
    //    many parents (DAG) while preventing duplicate same-parent edges.
    conn.execute(
        "CREATE TABLE IF NOT EXISTS tree_edges (
            parent_id INTEGER NOT NULL,
            child_id INTEGER NOT NULL,
            weight REAL NOT NULL DEFAULT 1.0,
            PRIMARY KEY (parent_id, child_id),
            FOREIGN KEY (parent_id) REFERENCES tree_nodes(id),
            FOREIGN KEY (child_id) REFERENCES tree_nodes(id)
        )",
        [],
    )?;
    // The (parent_id, child_id) PK indexes child-lookups by parent (get_children).
    // Add the reverse index for parent-lookups by child (get_parents / ancestor
    // walks in LTM retrieval).
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_tree_edges_child ON tree_edges(child_id)",
        [],
    )?;

    // 2. Document leaves — a tree node points at a dataId (Ledger/Pithos).
    conn.execute(
        "CREATE TABLE IF NOT EXISTS leaves (
            tree_node_id INTEGER NOT NULL,
            data_id TEXT NOT NULL,
            provenance TEXT NOT NULL DEFAULT '{}',
            PRIMARY KEY (tree_node_id, data_id),
            FOREIGN KEY (tree_node_id) REFERENCES tree_nodes(id)
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_leaves_data_id ON leaves(data_id)",
        [],
    )?;

    // 3. Vector index over CONCEPT node embeddings (sqlite-vec, LTM dimension).
    // Concepts and leaves live in separate indexes: placement searches concepts
    // only, so a document's embedding can never crowd out a concept match (the
    // vec0 `k` cut is applied before any kind filter — a shared index would let
    // a near leaf displace the real concept and misroute the doc to the inbox).
    let vec_query = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_ltm USING vec0(
            node_id INTEGER PRIMARY KEY,
            embedding float[{vector_dimension}]
        )"
    );
    conn.execute(&vec_query, [])?;

    // 3b. Vector index over DOCUMENT LEAF embeddings — so recall can land on a
    // document directly by meaning (while it still sits under the inbox).
    let vec_leaves_query = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS vec_leaves USING vec0(
            node_id INTEGER PRIMARY KEY,
            embedding float[{vector_dimension}]
        )"
    );
    conn.execute(&vec_leaves_query, [])?;

    // 4. FTS5 over node summaries (content table = tree_nodes).
    conn.execute(
        "CREATE VIRTUAL TABLE IF NOT EXISTS fts_ltm USING fts5(
            summary,
            content='tree_nodes',
            content_rowid='id'
        )",
        [],
    )?;

    conn.execute_batch(
        "
        CREATE TRIGGER IF NOT EXISTS tree_nodes_ai AFTER INSERT ON tree_nodes BEGIN
            INSERT INTO fts_ltm(rowid, summary) VALUES (new.id, new.summary);
        END;
        CREATE TRIGGER IF NOT EXISTS tree_nodes_ad AFTER DELETE ON tree_nodes BEGIN
            INSERT INTO fts_ltm(fts_ltm, rowid, summary) VALUES ('delete', old.id, old.summary);
        END;
        CREATE TRIGGER IF NOT EXISTS tree_nodes_au AFTER UPDATE ON tree_nodes BEGIN
            INSERT INTO fts_ltm(fts_ltm, rowid, summary) VALUES ('delete', old.id, old.summary);
            INSERT INTO fts_ltm(rowid, summary) VALUES (new.id, new.summary);
        END;
        ",
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
        assert!(table_names.contains(&"ccl_registry".to_string()));
        assert!(table_names.contains(&"vec_nodes".to_string()));
        assert!(table_names.contains(&"fts_nodes".to_string()));
    }
}

use crate::domain::models::{Episode, MemoryNode, TenantId};
use crate::domain::ports::MemoryRepository;
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct SqliteMemoryRepository {
    conn: Connection,
}

impl SqliteMemoryRepository {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }
}

impl MemoryRepository for SqliteMemoryRepository {
    fn store_ccl_definition(&self, definition: &crate::domain::models::CclDefinition) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO ccl_registry (tenant_id, name, description) VALUES (?1, ?2, ?3)",
            params![
                definition.tenant_id.0,
                definition.name,
                definition.description
            ],
        )?;
        Ok(())
    }

    fn get_ccl_definitions(&self, tenant_id: &TenantId) -> Result<Vec<crate::domain::models::CclDefinition>> {
        let mut stmt = self.conn.prepare("SELECT id, name, description FROM ccl_registry WHERE tenant_id = ?1")?;
        let def_iter = stmt.query_map(params![tenant_id.0], |row| {
            Ok(crate::domain::models::CclDefinition {
                id: Some(row.get(0)?),
                tenant_id: tenant_id.clone(),
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        let mut results = Vec::new();
        for def in def_iter {
            results.push(def?);
        }
        Ok(results)
    }

    fn store_episode(&self, episode: &Episode) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO episodes (tenant_id, session_id, raw_dialogue, ccl) VALUES (?1, ?2, ?3, ?4)",
            params![
                episode.tenant_id.0,
                episode.session_id.0,
                episode.raw_dialogue,
                episode.ccl
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    fn store_node(&self, node: &MemoryNode, embedding: &[f32]) -> Result<i64> {
        let payload_json = serde_json::to_string(&node.payload)?;

        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "INSERT INTO nodes (tenant_id, source_episode_id, payload, status, ccl, is_explicit, support_count, relevance_score) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                node.tenant_id.0,
                node.source_episode_id,
                payload_json,
                node.status,
                node.ccl,
                node.is_explicit,
                node.support_count,
                node.relevance_score
            ],
        )?;
        let node_id = tx.last_insert_rowid();

        let embedding_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embedding.as_ptr() as *const u8,
                std::mem::size_of_val(embedding),
            )
        };

        tx.execute(
            "INSERT INTO vec_nodes(node_id, embedding) VALUES (?1, ?2)",
            params![node_id, embedding_bytes],
        )?;

        tx.commit()?;
        Ok(node_id)
    }

    fn store_edge(&self, edge: &crate::domain::models::Edge) -> Result<()> {
        self.conn.execute(
            "INSERT INTO edges (source_id, target_id, relation, ccl, valid_from, valid_until, weight) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                edge.source_id,
                edge.target_id,
                edge.relation,
                edge.ccl,
                edge.valid_from,
                edge.valid_until,
                edge.weight
            ],
        )?;
        Ok(())
    }

    fn hybrid_search(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        tenant_id: &TenantId,
        limit: usize,
    ) -> Result<Vec<MemoryNode>> {
        let embedding_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                query_embedding.as_ptr() as *const u8,
                std::mem::size_of_val(query_embedding),
            )
        };

        let query = "
            WITH hybrid_matches AS (
                -- Semantic
                SELECT node_id, distance as score FROM vec_nodes WHERE embedding MATCH ?1 AND k = 10
                UNION ALL
                -- Keyword
                SELECT rowid as node_id, rank as score FROM fts_nodes WHERE fts_nodes MATCH ?2
            ),
            ranked_matches AS (
                SELECT node_id, SUM(score) as combined_score FROM hybrid_matches GROUP BY node_id ORDER BY combined_score LIMIT ?3
            )
            SELECT 
                n.id, n.tenant_id, n.source_episode_id, n.payload, n.status, n.ccl, n.is_explicit, n.support_count, n.relevance_score
            FROM nodes n
            JOIN ranked_matches rm ON n.id = rm.node_id
            WHERE n.tenant_id = ?4 AND n.status = 'active'
            ORDER BY rm.combined_score ASC;
        ";

        let mut stmt = self.conn.prepare(query)?;

        let node_iter = stmt.query_map(
            params![embedding_bytes, query_text, limit as i64, tenant_id.0],
            |row| {
                let payload_str: String = row.get(3)?;
                Ok(MemoryNode {
                    id: Some(row.get(0)?),
                    tenant_id: TenantId(row.get(1)?),
                    source_episode_id: row.get::<_, Option<i64>>(2)?,
                    payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
                    status: row.get(4)?,
                    ccl: row.get(5)?,
                    is_explicit: row.get(6)?,
                    support_count: row.get(7)?,
                    relevance_score: row.get(8)?,
                })
            },
        )?;

        let mut results = Vec::new();
        for node in node_iter {
            results.push(node?);
        }

        Ok(results)
    }

    fn query_with_graph(
        &self,
        query_text: &str,
        query_embedding: &[f32],
        tenant_id: &crate::domain::models::TenantId,
        time_filter: &crate::domain::models::TimeFilter,
        ccl_filter: &[String],
        limit: usize,
    ) -> Result<Vec<crate::domain::models::MemoryResult>> {
        let ccl_json = serde_json::to_string(ccl_filter)?;
        
        let embedding_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                query_embedding.as_ptr() as *const u8,
                std::mem::size_of_val(query_embedding),
            )
        };

        // Blueprint section 2.5: Hybrid + Graph + Temporal query
        let query = "
            WITH hybrid_matches AS (
                SELECT node_id, distance as score FROM vec_nodes WHERE embedding MATCH ?1 AND k = 10
                UNION ALL
                SELECT rowid as node_id, rank as score FROM fts_nodes WHERE fts_nodes MATCH ?2
            ),
            ranked_matches AS (
                SELECT node_id, SUM(score) as combined_score FROM hybrid_matches GROUP BY node_id ORDER BY combined_score LIMIT 5
            ),
            graph_context AS (
                SELECT node_id FROM ranked_matches
                UNION
                SELECT target_id AS node_id FROM edges
                WHERE source_id IN (SELECT node_id FROM ranked_matches)
                  AND (valid_until IS NULL OR ?5 IS NULL OR valid_until >= ?5)
                  AND (valid_from IS NULL OR ?6 IS NULL OR valid_from <= ?6)
                  AND ccl IN (SELECT value FROM json_each(?7))
                UNION
                SELECT source_id AS node_id FROM edges
                WHERE target_id IN (SELECT node_id FROM ranked_matches)
                  AND (valid_until IS NULL OR ?5 IS NULL OR valid_until >= ?5)
                  AND (valid_from IS NULL OR ?6 IS NULL OR valid_from <= ?6)
                  AND ccl IN (SELECT value FROM json_each(?7))
            )
            SELECT
                n.id, n.payload, n.ccl, n.relevance_score, n.updated_at
            FROM nodes n
            JOIN graph_context gc ON n.id = gc.node_id
            WHERE n.tenant_id = ?3 AND n.status = 'active'
              AND (n.created_at >= ?5 OR ?5 IS NULL)
              AND (n.created_at <= ?6 OR ?6 IS NULL)
              AND n.ccl IN (SELECT value FROM json_each(?7))
            ORDER BY n.relevance_score DESC
            LIMIT ?4;
        ";

        let mut stmt = self.conn.prepare(query)?;

        let rows: Vec<(i64, String, String, f64, String)> = stmt
            .query_map(
                params![
                    embedding_bytes,
                    query_text,
                    tenant_id.0,
                    limit as i64,
                    time_filter.after,
                    time_filter.before,
                    &ccl_json,
                ],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Collect node IDs for relevance boost
        let node_ids: Vec<i64> = rows.iter().map(|(id, _, _, _, _)| *id).collect();
        if !node_ids.is_empty() {
            self.boost_relevance(&node_ids)?;
        }

        // Build token-optimized output with 1-hop connections
        let mut results = Vec::new();
        for (node_id, payload_str, ccl, _, updated_at) in rows {
            let payload: serde_json::Value =
                serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null);
            let fact = payload
                .get("fact")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string();

            // Get 1-hop connections for this node
            let mut edge_stmt = self.conn.prepare(
                "SELECT e.relation, e.ccl, e.valid_from, e.valid_until, n2.payload
                 FROM edges e
                 JOIN nodes n2 ON n2.id = e.target_id
                 WHERE e.source_id = ?1
                   AND e.ccl IN (SELECT value FROM json_each(?2))
                 UNION ALL
                 SELECT e.relation, e.ccl, e.valid_from, e.valid_until, n2.payload
                 FROM edges e
                 JOIN nodes n2 ON n2.id = e.source_id
                 WHERE e.target_id = ?1
                   AND e.ccl IN (SELECT value FROM json_each(?2))",
            )?;

            let connections: Vec<crate::domain::models::MemoryConnection> = edge_stmt
                .query_map(params![node_id, &ccl_json], |row| {
                    let rel: String = row.get(0)?;
                    let edge_ccl: String = row.get(1)?;
                    let vf: Option<String> = row.get(2)?;
                    let vu: Option<String> = row.get(3)?;
                    let entity_payload: String = row.get(4)?;
                    let ep: serde_json::Value =
                        serde_json::from_str(&entity_payload).unwrap_or(serde_json::Value::Null);
                    let entity = ep
                        .get("fact")
                        .and_then(|f| f.as_str())
                        .unwrap_or("")
                        .to_string();
                    Ok(crate::domain::models::MemoryConnection {
                        relation: rel,
                        entity,
                        ccl: edge_ccl,
                        valid_from: vf,
                        valid_until: vu,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            results.push(crate::domain::models::MemoryResult {
                fact,
                ccl,
                last_updated: updated_at,
                connections,
            });
        }

        Ok(results)
    }

    fn boost_relevance(&self, node_ids: &[i64]) -> Result<()> {
        let placeholders: Vec<String> = node_ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect();
        let query = format!(
            "UPDATE nodes SET relevance_score = 1.0, last_accessed_at = CURRENT_TIMESTAMP WHERE id IN ({})",
            placeholders.join(", ")
        );
        let params: Vec<Box<dyn rusqlite::types::ToSql>> = node_ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::types::ToSql>)
            .collect();
        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();
        self.conn.execute(&query, param_refs.as_slice())?;
        Ok(())
    }

    fn find_similar_nodes(
        &self,
        embedding: &[f32],
        tenant_id: &TenantId,
        threshold: f64,
        limit: usize,
    ) -> Result<Vec<MemoryNode>> {
        let embedding_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embedding.as_ptr() as *const u8,
                std::mem::size_of_val(embedding),
            )
        };

        let query = "
            SELECT n.id, n.tenant_id, n.source_episode_id, n.payload, n.status, n.ccl, n.is_explicit, n.support_count, n.relevance_score, v.distance
            FROM vec_nodes v
            JOIN nodes n ON n.id = v.node_id
            WHERE v.embedding MATCH ?1 AND k = ?2
              AND n.tenant_id = ?3 AND n.status = 'active'
              AND v.distance <= ?4
            ORDER BY v.distance ASC;
        ";

        let mut stmt = self.conn.prepare(query)?;

        let node_iter = stmt.query_map(
            params![embedding_bytes, limit as i64, tenant_id.0, threshold],
            |row| {
                let payload_str: String = row.get(3)?;
                Ok(MemoryNode {
                    id: Some(row.get(0)?),
                    tenant_id: TenantId(row.get(1)?),
                    source_episode_id: row.get::<_, Option<i64>>(2)?,
                    payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
                    status: row.get(4)?,
                    ccl: row.get(5)?,
                    is_explicit: row.get(6)?,
                    support_count: row.get(7)?,
                    relevance_score: row.get(8)?,
                })
            },
        )?;

        let mut results = Vec::new();
        for node in node_iter {
            results.push(node?);
        }

        Ok(results)
    }

    fn update_node_support(
        &self,
        node_id: i64,
        new_payload: Option<&serde_json::Value>,
    ) -> Result<()> {
        if let Some(payload) = new_payload {
            let payload_json = serde_json::to_string(payload)?;
            self.conn.execute(
                "UPDATE nodes SET support_count = support_count + 1, relevance_score = 1.0, updated_at = CURRENT_TIMESTAMP, payload = ?1 WHERE id = ?2",
                params![payload_json, node_id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE nodes SET support_count = support_count + 1, relevance_score = 1.0, updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![node_id],
            )?;
        }
        Ok(())
    }

    fn delete_tenant(&self, tenant_id: &TenantId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;

        tx.execute(
            "DELETE FROM vec_nodes WHERE node_id IN (SELECT id FROM nodes WHERE tenant_id = ?1)",
            params![tenant_id.0],
        )?;

        tx.execute(
            "DELETE FROM nodes WHERE tenant_id = ?1",
            params![tenant_id.0],
        )?;
        tx.execute(
            "DELETE FROM episodes WHERE tenant_id = ?1",
            params![tenant_id.0],
        )?;

        tx.commit()?;
        Ok(())
    }

    fn export_tenant(&self, tenant_id: &TenantId) -> Result<String> {
        let mut stmt = self
            .conn
            .prepare("SELECT payload FROM nodes WHERE tenant_id = ?1")?;
        let payload_iter = stmt.query_map(params![tenant_id.0], |row| {
            let p: String = row.get(0)?;
            Ok(p)
        })?;

        let mut all_facts = Vec::new();
        for p in payload_iter {
            let val: serde_json::Value = serde_json::from_str(&p?)?;
            all_facts.push(val);
        }

        let export_json = serde_json::json!({
            "tenant_id": tenant_id.0,
            "extracted_facts": all_facts
        });

        Ok(serde_json::to_string_pretty(&export_json)?)
    }

    fn sweep_decay(&self, engine: &crate::domain::decay::DecayEngine) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, relevance_score, status FROM nodes WHERE status = 'active'")?;

        let nodes_to_update: Result<Vec<(i64, f64, String)>> = stmt
            .query_map([], |row| {
                let id: i64 = row.get(0)?;
                let current_score: f64 = row.get(1)?;
                let status: String = row.get(2)?;
                Ok((id, current_score, status))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into);

        let nodes = nodes_to_update?;

        let days_elapsed = 1.0;

        let tx = self.conn.unchecked_transaction()?;
        for (id, current_score, _) in nodes {
            let new_score = engine.calculate_decay(current_score, days_elapsed);
            let new_status = if new_score < 0.1 {
                "archived"
            } else {
                "active"
            };

            tx.execute(
                "UPDATE nodes SET relevance_score = ?1, status = ?2 WHERE id = ?3",
                params![new_score, new_status, id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::SessionId;
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::schema::init_schema;
    use serde_json::json;

    fn setup_mem_repo() -> Box<dyn MemoryRepository> {
        let conn = init_db(None as Option<&String>).unwrap();
        init_schema(&conn, 1536).unwrap();
        Box::new(SqliteMemoryRepository::new(conn))
    }

    #[test]
    fn test_store_episode_and_node() {
        let repo = setup_mem_repo();

        let episode = Episode {
            id: None,
            tenant_id: TenantId("test-tenant".into()),
            session_id: SessionId("session-1".into()),
            raw_dialogue: "I live in Berlin".into(),
            ccl: "reality".into(),
            created_at: None,
        };

        let episode_id = repo.store_episode(&episode).unwrap();
        assert!(episode_id > 0);

        let node = MemoryNode {
            id: None,
            tenant_id: TenantId("test-tenant".into()),
            source_episode_id: Some(episode_id),
            payload: json!({"fact": "User lives in Berlin"}),
            status: "active".into(),
            ccl: "reality".into(),
            is_explicit: true,
            support_count: 1,
            relevance_score: 1.0,
        };

        let dummy_embedding = vec![0.1f32; 1536]; // fake 1536d vector
        let node_id = repo.store_node(&node, &dummy_embedding).unwrap();
        assert!(node_id > 0);

        // Since repo is now a Box<dyn MemoryRepository>, we cannot directly access repo.conn.
        // For testing the internal state (trigger effects), we need another connection or a specialized test method.
        // For now, testing the repository's returned behavior is sufficient since FTS trigger setups are tested
        // separately in schema tests.
    }

    #[test]
    fn test_hybrid_search() {
        let repo = setup_mem_repo();

        // Let's create an episode
        let episode = Episode {
            id: None,
            tenant_id: TenantId("tenant-X".into()),
            session_id: SessionId("session-1".into()),
            raw_dialogue: "I have a dog named Rust.".into(),
            ccl: "reality".into(),
            created_at: None,
        };
        let ep_id = repo.store_episode(&episode).unwrap();

        // Node 1: Contains the keyword "dog" explicitly
        let node1 = MemoryNode {
            id: None,
            tenant_id: TenantId("tenant-X".into()),
            source_episode_id: Some(ep_id),
            payload: json!({"fact": "User owns a dog"}),
            status: "active".into(),
            ccl: "reality".into(),
            is_explicit: true,
            support_count: 1,
            relevance_score: 1.0,
        };
        // Node 2: Contains another keyword but vector might be close
        let node2 = MemoryNode {
            id: None,
            tenant_id: TenantId("tenant-X".into()),
            source_episode_id: Some(ep_id),
            payload: json!({"fact": "User is a programmer"}),
            status: "active".into(),
            ccl: "reality".into(),
            is_explicit: true,
            support_count: 1,
            relevance_score: 0.8,
        };

        let emb1 = vec![0.9f32; 1536];
        let emb2 = vec![0.1f32; 1536]; // different embedding

        repo.store_node(&node1, &emb1).unwrap();
        repo.store_node(&node2, &emb2).unwrap();

        // Query combining text and an embedding close to emb1
        let query_emb = vec![0.85f32; 1536];

        let results = repo
            .hybrid_search("dog", &query_emb, &TenantId("tenant-X".into()), 5)
            .unwrap();
        assert!(!results.is_empty());

        // Should rank node1 highest because it matches FTS "dog" AND vector distance is closer
        let ranked_top = &results[0];
        assert_eq!(
            ranked_top.payload.get("fact").unwrap().as_str().unwrap(),
            "User owns a dog"
        );
    }

    #[test]
    fn test_tenant_isolation_delete_and_export() {
        let repo = setup_mem_repo();

        let t1 = TenantId("tenant-A".into());
        let t2 = TenantId("tenant-B".into());

        // Setup tenant A
        let ep1 = repo
            .store_episode(&Episode {
                id: None,
                tenant_id: t1.clone(),
                session_id: SessionId("s1".into()),
                raw_dialogue: "secret A".into(),
                ccl: "reality".into(),
                created_at: None,
            })
            .unwrap();

        repo.store_node(
            &MemoryNode {
                id: None,
                tenant_id: t1.clone(),
                source_episode_id: Some(ep1),
                payload: json!({"fact": "A fact"}),
                status: "active".into(),
                ccl: "reality".into(),
                is_explicit: false,
                support_count: 1,
                relevance_score: 1.0,
            },
            &vec![0.1; 1536],
        )
        .unwrap();

        // Setup tenant B
        let ep2 = repo
            .store_episode(&Episode {
                id: None,
                tenant_id: t2.clone(),
                session_id: SessionId("s2".into()),
                raw_dialogue: "secret B".into(),
                ccl: "reality".into(),
                created_at: None,
            })
            .unwrap();

        repo.store_node(
            &MemoryNode {
                id: None,
                tenant_id: t2.clone(),
                source_episode_id: Some(ep2),
                payload: json!({"fact": "B fact"}),
                status: "active".into(),
                ccl: "reality".into(),
                is_explicit: false,
                support_count: 1,
                relevance_score: 1.0,
            },
            &vec![0.2; 1536],
        )
        .unwrap();

        // Test Export
        let export_a = repo.export_tenant(&t1).unwrap();
        assert!(export_a.contains("A fact"));
        assert!(!export_a.contains("B fact"));

        // Test Deletion
        repo.delete_tenant(&t1).unwrap();

        let after_delete_a = repo.export_tenant(&t1).unwrap();
        assert!(!after_delete_a.contains("A fact"));

        let export_b = repo.export_tenant(&t2).unwrap();
        assert!(export_b.contains("B fact")); // B untouched
    }
}

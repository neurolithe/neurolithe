//! SQLite implementation of the LTM knowledge-tree repository.
//!
//! Mirrors `SqliteMemoryRepository`: owns one `rusqlite::Connection` (to the
//! LTM database) and serializes access. Concept-node embeddings go into the
//! `vec_ltm` sqlite-vec index; summaries are kept in `fts_ltm` via triggers.

use crate::domain::ltm::{Leaf, LtmRepository, TreeEdge, TreeNode, TreeNodeKind};
use anyhow::Result;
use rusqlite::{Connection, params};

pub struct SqliteLtmRepository {
    conn: Connection,
}

impl SqliteLtmRepository {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Map a `tree_nodes` row (id, name, summary, kind, permanent, created_at,
    /// updated_at) to a [`TreeNode`].
    fn row_to_node(row: &rusqlite::Row) -> rusqlite::Result<TreeNode> {
        let kind_str: String = row.get(3)?;
        let kind = TreeNodeKind::from_db_str(&kind_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(3, rusqlite::types::Type::Text, e.into())
        })?;
        Ok(TreeNode {
            id: Some(row.get(0)?),
            name: row.get(1)?,
            summary: row.get(2)?,
            kind,
            permanent: row.get(4)?,
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    }

    /// Seed the curated spine (Reza's main branches + an inbox) under a root,
    /// once. Idempotent: a no-op if the tree already has any nodes.
    ///
    /// Spine nodes are seeded without embeddings — vectors are added when the
    /// embedding model is wired (feeder, slice 6). Called by the daemon on
    /// startup (slice 11).
    pub fn seed_spine(&self) -> Result<()> {
        let existing: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM tree_nodes", [], |r| r.get(0))?;
        if existing > 0 {
            return Ok(());
        }

        let root = self.create_node(
            &TreeNode::new(
                "Reza",
                "Root of the personal knowledge tree.",
                TreeNodeKind::Spine,
            ),
            None,
        )?;

        // Curated main branches (the spine). The AI grows leaves below these.
        let branches = [
            ("job", "Work, employer, projects, and career."),
            ("learning", "Study, courses, skills, and reading."),
            ("investment", "Finances, investing, and assets."),
            (
                "self-improvement",
                "Health, habits, mindset, and personal growth.",
            ),
        ];
        for (name, summary) in branches {
            let child =
                self.create_node(&TreeNode::new(name, summary, TreeNodeKind::Spine), None)?;
            self.add_edge(&TreeEdge::new(root, child))?;
        }

        // The holding area for documents that find no good concept match.
        let inbox = self.create_node(
            &TreeNode::new(
                "inbox",
                "Unsorted documents awaiting placement.",
                TreeNodeKind::Inbox,
            ),
            None,
        )?;
        self.add_edge(&TreeEdge::new(root, inbox))?;

        Ok(())
    }
}

impl LtmRepository for SqliteLtmRepository {
    fn create_node(&self, node: &TreeNode, embedding: Option<&[f32]>) -> Result<i64> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tree_nodes (name, summary, kind, permanent) VALUES (?1, ?2, ?3, ?4)",
            params![node.name, node.summary, node.kind.as_str(), node.permanent],
        )?;
        let node_id = tx.last_insert_rowid();

        if let Some(embedding) = embedding {
            let embedding_bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    embedding.as_ptr() as *const u8,
                    std::mem::size_of_val(embedding),
                )
            };
            tx.execute(
                "INSERT INTO vec_ltm(node_id, embedding) VALUES (?1, ?2)",
                params![node_id, embedding_bytes],
            )?;
        }

        tx.commit()?;
        Ok(node_id)
    }

    fn add_edge(&self, edge: &TreeEdge) -> Result<()> {
        // INSERT OR IGNORE: re-adding the same parent->child pair is a no-op.
        self.conn.execute(
            "INSERT OR IGNORE INTO tree_edges (parent_id, child_id, weight) VALUES (?1, ?2, ?3)",
            params![edge.parent_id, edge.child_id, edge.weight],
        )?;
        Ok(())
    }

    fn create_leaf(&self, leaf: &Leaf) -> Result<()> {
        let provenance_json = serde_json::to_string(&leaf.provenance)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO leaves (tree_node_id, data_id, provenance) VALUES (?1, ?2, ?3)",
            params![leaf.tree_node_id, leaf.data_id, provenance_json],
        )?;
        Ok(())
    }

    fn get_node(&self, id: i64) -> Result<Option<TreeNode>> {
        let node = self
            .conn
            .query_row(
                "SELECT id, name, summary, kind, permanent, created_at, updated_at
                 FROM tree_nodes WHERE id = ?1",
                params![id],
                Self::row_to_node,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(node)
    }

    fn get_children(&self, parent_id: i64) -> Result<Vec<TreeNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.name, n.summary, n.kind, n.permanent, n.created_at, n.updated_at
             FROM tree_nodes n
             JOIN tree_edges e ON n.id = e.child_id
             WHERE e.parent_id = ?1
             ORDER BY n.id",
        )?;
        let rows = stmt.query_map(params![parent_id], Self::row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn get_parents(&self, child_id: i64) -> Result<Vec<TreeNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT n.id, n.name, n.summary, n.kind, n.permanent, n.created_at, n.updated_at
             FROM tree_nodes n
             JOIN tree_edges e ON n.id = e.parent_id
             WHERE e.child_id = ?1
             ORDER BY n.id",
        )?;
        let rows = stmt.query_map(params![child_id], Self::row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn get_node_by_data_id(&self, data_id: &str) -> Result<Option<TreeNode>> {
        let node = self
            .conn
            .query_row(
                "SELECT n.id, n.name, n.summary, n.kind, n.permanent, n.created_at, n.updated_at
                 FROM tree_nodes n
                 JOIN leaves l ON n.id = l.tree_node_id
                 WHERE l.data_id = ?1
                 LIMIT 1",
                params![data_id],
                Self::row_to_node,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        Ok(node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ltm::Provenance;
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::schema::init_ltm_schema;

    const DIM: usize = 4;

    fn setup() -> SqliteLtmRepository {
        let conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&conn, DIM).unwrap();
        SqliteLtmRepository::new(conn)
    }

    /// A node can sit under several parents (DAG), and children/parents read
    /// back correctly from either direction.
    #[test]
    fn test_node_with_two_parents() {
        let repo = setup();

        let p1 = repo
            .create_node(&TreeNode::new("job", "", TreeNodeKind::Spine), None)
            .unwrap();
        let p2 = repo
            .create_node(
                &TreeNode::new("self-improvement", "", TreeNodeKind::Spine),
                None,
            )
            .unwrap();
        let child = repo
            .create_node(&TreeNode::new("leadership", "", TreeNodeKind::Grown), None)
            .unwrap();

        repo.add_edge(&TreeEdge::new(p1, child)).unwrap();
        repo.add_edge(&TreeEdge::new(p2, child)).unwrap();
        // Re-adding an existing edge is a no-op.
        repo.add_edge(&TreeEdge::new(p1, child)).unwrap();

        let parents = repo.get_parents(child).unwrap();
        assert_eq!(parents.len(), 2, "child has two parents");
        let parent_names: Vec<_> = parents.iter().map(|n| n.name.as_str()).collect();
        assert!(parent_names.contains(&"job"));
        assert!(parent_names.contains(&"self-improvement"));

        let p1_children = repo.get_children(p1).unwrap();
        assert_eq!(p1_children.len(), 1);
        assert_eq!(p1_children[0].name, "leadership");
        assert_eq!(p1_children[0].kind, TreeNodeKind::Grown);
    }

    /// A leaf links a tree node to a `dataId`, found via get-by-dataId with its
    /// provenance round-tripped.
    #[test]
    fn test_leaf_links_to_data_id() {
        let repo = setup();

        let leaf_node = repo
            .create_node(
                &TreeNode::new(
                    "2019 PharmaCare letter",
                    "health insurance",
                    TreeNodeKind::Leaf,
                ),
                Some(&[0.1_f32; DIM]),
            )
            .unwrap();

        repo.create_leaf(&Leaf {
            tree_node_id: leaf_node,
            data_id: "doc_01HX".into(),
            provenance: Provenance {
                source: "scanner".into(),
                ingested_at: Some("2026-06-28T00:00:00Z".into()),
                confidence: 0.9,
            },
        })
        .unwrap();

        let found = repo.get_node_by_data_id("doc_01HX").unwrap();
        assert!(found.is_some(), "leaf is found by dataId");
        let found = found.unwrap();
        assert_eq!(found.id, Some(leaf_node));
        assert_eq!(found.kind, TreeNodeKind::Leaf);

        // An unknown dataId yields nothing.
        assert!(repo.get_node_by_data_id("doc_missing").unwrap().is_none());
    }

    /// `init_ltm_schema` is safe to run repeatedly (idempotent migrations).
    #[test]
    fn test_schema_idempotent() {
        let conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&conn, DIM).unwrap();
        // A second run must not error (CREATE ... IF NOT EXISTS / triggers).
        init_ltm_schema(&conn, DIM).unwrap();

        let tables: Vec<String> = {
            let mut stmt = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        for t in ["tree_nodes", "tree_edges", "leaves", "vec_ltm", "fts_ltm"] {
            assert!(tables.contains(&t.to_string()), "{t} should exist");
        }
    }

    /// The spine seeds a root + branches + inbox exactly once.
    #[test]
    fn test_spine_seeded_once() {
        let repo = setup();

        repo.seed_spine().unwrap();
        let count_after_first: i64 = repo
            .conn
            .query_row("SELECT COUNT(*) FROM tree_nodes", [], |r| r.get(0))
            .unwrap();
        // root + 4 branches + inbox = 6.
        assert_eq!(count_after_first, 6);

        // Seeding again is a no-op.
        repo.seed_spine().unwrap();
        let count_after_second: i64 = repo
            .conn
            .query_row("SELECT COUNT(*) FROM tree_nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count_after_second, 6, "spine is seeded only once");

        // The root has the four spine branches plus the inbox as children.
        let root = repo.get_node(1).unwrap().expect("root exists");
        assert_eq!(root.name, "Reza");
        assert_eq!(root.kind, TreeNodeKind::Spine);
        let children = repo.get_children(1).unwrap();
        assert_eq!(children.len(), 5);
        assert!(children.iter().any(|n| n.kind == TreeNodeKind::Inbox));
    }
}

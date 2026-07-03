//! LTM placement — files a new document into the knowledge tree and keeps
//! rolling summaries in sync.
//!
//! V2 is deliberately **simple**: best-match-or-inbox placement and a
//! concatenate-and-truncate roll-up. The hard "grow the right way" logic
//! (split fat nodes, merge similar branches, spawn new branches) is deferred —
//! see `V2-DESIGN.md` §3.2 and `JARVIS-MEMORY-TREE.md`.

use crate::domain::ltm::{Leaf, LtmRepository, Provenance, TreeEdge, TreeNode, TreeNodeKind};
use anyhow::{Result, anyhow};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// Max L2 distance for a document to count as matching a concept node. Beyond
/// this it falls to the inbox. (Embeddings are expected roughly normalized;
/// matches `find_similar_nodes`' distance convention in the STM engine.)
const DEFAULT_MAX_DISTANCE: f64 = 0.5;

/// Cap on a rolled summary's length (characters) before truncation.
const DEFAULT_MAX_SUMMARY_LEN: usize = 600;

/// A document ready to be filed: its distilled meaning + its archive pointer.
pub struct DocumentToPlace {
    /// Short human-facing label for the leaf node.
    pub name: String,
    /// Distilled summary (becomes the leaf's summary + roll-up input).
    pub summary: String,
    /// Embedding of the summary (LTM dimension). Used both to locate the best
    /// concept (placement) and — stored on the leaf — to make the document
    /// recallable by meaning.
    pub embedding: Vec<f32>,
    /// Archive reference (Ledger/Pithos `dataId`).
    pub data_id: String,
    pub provenance: Provenance,
}

/// Where a document landed.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub leaf_node_id: i64,
    pub parent_id: i64,
    /// True if matched a concept; false if it fell to the inbox.
    pub matched: bool,
}

/// Files documents into the LTM tree and rolls summaries up the affected
/// ancestors.
pub struct LtmPlacement {
    repo: Arc<dyn LtmRepository>,
    max_distance: f64,
    max_summary_len: usize,
}

impl LtmPlacement {
    pub fn new(repo: Arc<dyn LtmRepository>) -> Self {
        Self {
            repo,
            max_distance: DEFAULT_MAX_DISTANCE,
            max_summary_len: DEFAULT_MAX_SUMMARY_LEN,
        }
    }

    /// File a document: attach it under the best-matching concept (vector within
    /// threshold) or, failing that, under the inbox; then roll its summary up
    /// into the ancestors. Returns where it landed.
    pub fn place(&self, doc: &DocumentToPlace) -> Result<Placement> {
        // 1. Best concept match, or fall back to the inbox.
        let best = self
            .repo
            .find_similar_concepts(&doc.embedding, self.max_distance, 1)?
            .into_iter()
            .next();
        let (parent_id, matched) = match best {
            Some((node, _dist)) => (node.id.expect("stored node has id"), true),
            None => {
                let inbox = self
                    .repo
                    .get_inbox()?
                    .ok_or_else(|| anyhow!("no inbox node — spine not seeded"))?;
                (inbox.id.expect("stored node has id"), false)
            }
        };

        // 2. Create the leaf node and its dataId link. The leaf IS vector-indexed
        //    (its summary embedding), so `recall` can land on the document
        //    directly by meaning — essential while docs pile in the inbox before
        //    the tree grows real concepts around them. This does NOT affect
        //    placement: step 1 uses the concept-only `find_similar_concepts`, so
        //    an embedded leaf can never crowd out a concept match and misroute a
        //    new document to the inbox.
        let leaf_node_id = self.repo.create_node(
            &TreeNode::new(doc.name.clone(), doc.summary.clone(), TreeNodeKind::Leaf),
            Some(&doc.embedding),
        )?;
        self.repo.create_leaf(&Leaf {
            tree_node_id: leaf_node_id,
            data_id: doc.data_id.clone(),
            provenance: doc.provenance.clone(),
        })?;
        self.repo
            .add_edge(&TreeEdge::new(parent_id, leaf_node_id))?;

        // 3. Roll the new summary up into the parent and its ancestors.
        self.roll_up_from(parent_id)?;

        Ok(Placement {
            leaf_node_id,
            parent_id,
            matched,
        })
    }

    /// Tombstone: forget a `dataId` — remove its leaf and re-roll the affected
    /// ancestor summaries. Returns false if the document was not in the tree.
    pub fn forget(&self, data_id: &str) -> Result<bool> {
        let Some(leaf) = self.repo.get_node_by_data_id(data_id)? else {
            return Ok(false);
        };
        let leaf_id = leaf.id.expect("stored node has id");
        // Capture the parents before deletion so we know what to re-roll.
        let parents = self.repo.get_parents(leaf_id)?;
        self.repo.delete_node(leaf_id)?;
        for parent in parents {
            self.roll_up_from(parent.id.expect("stored node has id"))?;
        }
        Ok(true)
    }

    /// Recompute `start`'s summary and every ancestor's, children-before-parents
    /// (so each parent sees its children's fresh summaries). Multi-parent safe.
    fn roll_up_from(&self, start: i64) -> Result<()> {
        // BFS upward, tracking each node's min distance from `start`.
        let mut dist: HashMap<i64, usize> = HashMap::new();
        let mut queue: VecDeque<i64> = VecDeque::new();
        dist.insert(start, 0);
        queue.push_back(start);
        while let Some(node_id) = queue.pop_front() {
            let d = dist[&node_id];
            for parent in self.repo.get_parents(node_id)? {
                let pid = parent.id.expect("stored node has id");
                let nd = d + 1;
                if dist.get(&pid).is_none_or(|&old| nd < old) {
                    dist.insert(pid, nd);
                    queue.push_back(pid);
                }
            }
        }

        // Recompute in increasing distance order: children before parents.
        let mut ordered: Vec<(i64, usize)> = dist.into_iter().collect();
        ordered.sort_by_key(|&(_, d)| d);
        for (node_id, _) in ordered {
            self.recompute_summary(node_id)?;
        }
        Ok(())
    }

    /// A node's rolling summary is the condensed concatenation of its children's
    /// summaries (leaf children contribute document summaries; concept children
    /// contribute their own rolled summaries). No children → empty.
    fn recompute_summary(&self, node_id: i64) -> Result<()> {
        let children = self.repo.get_children(node_id)?;
        let parts: Vec<String> = children
            .into_iter()
            .map(|c| c.summary)
            .filter(|s| !s.trim().is_empty())
            .collect();
        let new_summary = if parts.is_empty() {
            String::new()
        } else {
            condense(&parts.join("; "), self.max_summary_len)
        };

        if let Some(node) = self.repo.get_node(node_id)?
            && node.summary != new_summary
        {
            self.repo.update_summary(node_id, &new_summary)?;
        }
        Ok(())
    }
}

/// Truncate `text` to at most `max_len` characters on a char boundary, marking
/// the cut with an ellipsis.
fn condense(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let head: String = text.chars().take(max_len.saturating_sub(1)).collect();
    format!("{head}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::database::init_db;
    use crate::infrastructure::ltm_repository::SqliteLtmRepository;
    use crate::infrastructure::schema::init_ltm_schema;

    const DIM: usize = 4;

    fn provenance() -> Provenance {
        Provenance {
            source: "test".into(),
            ingested_at: None,
            confidence: 1.0,
        }
    }

    /// A tree with root -> "job" (a concept embedded at [1,0,0,0]) and an inbox.
    /// Returns the service plus the concept/root/inbox ids.
    struct Fixture {
        svc: LtmPlacement,
        repo: Arc<SqliteLtmRepository>,
        root: i64,
        concept: i64,
        inbox: i64,
    }

    fn fixture() -> Fixture {
        let conn = init_db(None as Option<&String>).unwrap();
        init_ltm_schema(&conn, DIM).unwrap();
        let repo = Arc::new(SqliteLtmRepository::new(conn));

        let root = repo
            .create_node(
                &TreeNode::new("Reza", "root seed", TreeNodeKind::Spine),
                None,
            )
            .unwrap();
        let concept = repo
            .create_node(
                &TreeNode::new("job", "job seed", TreeNodeKind::Spine),
                Some(&[1.0, 0.0, 0.0, 0.0]),
            )
            .unwrap();
        repo.add_edge(&TreeEdge::new(root, concept)).unwrap();
        let inbox = repo
            .create_node(&TreeNode::new("inbox", "", TreeNodeKind::Inbox), None)
            .unwrap();
        repo.add_edge(&TreeEdge::new(root, inbox)).unwrap();

        let svc = LtmPlacement::new(repo.clone() as Arc<dyn LtmRepository>);
        Fixture {
            svc,
            repo,
            root,
            concept,
            inbox,
        }
    }

    /// A document whose embedding matches a concept attaches under that concept.
    #[test]
    fn test_high_similarity_attaches_under_match() {
        let f = fixture();
        let doc = DocumentToPlace {
            name: "metro project".into(),
            summary: "BestBuy project Metro notes".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0], // distance 0 to the concept
            data_id: "doc_metro".into(),
            provenance: provenance(),
        };

        let placement = f.svc.place(&doc).unwrap();
        assert!(placement.matched, "should match the concept");
        assert_eq!(placement.parent_id, f.concept);

        let parents = f.repo.get_parents(placement.leaf_node_id).unwrap();
        assert_eq!(parents.len(), 1);
        assert_eq!(parents[0].id, Some(f.concept));
    }

    /// A document with no nearby concept falls to the inbox.
    #[test]
    fn test_low_similarity_falls_to_inbox() {
        let f = fixture();
        let doc = DocumentToPlace {
            name: "stray".into(),
            summary: "unrelated content".into(),
            embedding: vec![0.0, 0.0, 0.0, 1.0], // distance sqrt(2) > threshold
            data_id: "doc_stray".into(),
            provenance: provenance(),
        };

        let placement = f.svc.place(&doc).unwrap();
        assert!(!placement.matched, "should not match");
        assert_eq!(placement.parent_id, f.inbox);
    }

    /// After attaching, the parent concept and its ancestor (root) summaries
    /// roll up to include the new document.
    #[test]
    fn test_ancestor_summaries_roll_up_on_attach() {
        let f = fixture();
        let doc = DocumentToPlace {
            name: "metro".into(),
            summary: "Metro rollout".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            data_id: "doc_metro".into(),
            provenance: provenance(),
        };
        f.svc.place(&doc).unwrap();

        let concept = f.repo.get_node(f.concept).unwrap().unwrap();
        assert!(
            concept.summary.contains("Metro rollout"),
            "concept rolled up: {:?}",
            concept.summary
        );
        let root = f.repo.get_node(f.root).unwrap().unwrap();
        assert!(
            root.summary.contains("Metro rollout"),
            "root rolled up: {:?}",
            root.summary
        );
    }

    /// Forgetting a dataId removes its leaf and re-rolls the ancestors so the
    /// document no longer appears in their summaries.
    #[test]
    fn test_forget_removes_leaf_and_rerolls() {
        let f = fixture();
        let doc = DocumentToPlace {
            name: "metro".into(),
            summary: "Metro rollout".into(),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            data_id: "doc_metro".into(),
            provenance: provenance(),
        };
        let placement = f.svc.place(&doc).unwrap();

        let removed = f.svc.forget("doc_metro").unwrap();
        assert!(removed, "document was present");

        // Leaf is gone from the tree and from the dataId index.
        assert!(f.repo.get_node(placement.leaf_node_id).unwrap().is_none());
        assert!(f.repo.get_node_by_data_id("doc_metro").unwrap().is_none());

        // Ancestors re-rolled: the document's summary no longer appears.
        let concept = f.repo.get_node(f.concept).unwrap().unwrap();
        assert!(
            !concept.summary.contains("Metro rollout"),
            "concept re-rolled: {:?}",
            concept.summary
        );
        let root = f.repo.get_node(f.root).unwrap().unwrap();
        assert!(!root.summary.contains("Metro rollout"));

        // Forgetting an unknown dataId is a no-op false.
        assert!(!f.svc.forget("doc_unknown").unwrap());
    }

    /// A second document identical to the first still routes to the concept,
    /// not the inbox — because leaves are not vector-indexed, the first doc's
    /// leaf can't crowd out the concept match.
    #[test]
    fn test_second_similar_doc_still_matches_concept() {
        let f = fixture();
        let make = |data_id: &str| DocumentToPlace {
            name: "metro".into(),
            summary: format!("Metro rollout {data_id}"),
            embedding: vec![1.0, 0.0, 0.0, 0.0],
            data_id: data_id.into(),
            provenance: provenance(),
        };

        let first = f.svc.place(&make("doc_1")).unwrap();
        assert!(first.matched);
        assert_eq!(first.parent_id, f.concept);

        let second = f.svc.place(&make("doc_2")).unwrap();
        assert!(second.matched, "second similar doc must still match");
        assert_eq!(
            second.parent_id, f.concept,
            "second doc must not fall to the inbox"
        );

        // Both leaves now hang under the concept.
        let children = f.repo.get_children(f.concept).unwrap();
        assert_eq!(children.len(), 2);
    }
}

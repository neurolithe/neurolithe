use crate::infrastructure::config::AppConfig;
use crate::infrastructure::schema::{init_ltm_schema, init_schema};
use rusqlite::Connection;
use std::path::Path;

pub fn init_db(path: Option<&impl AsRef<Path>>) -> rusqlite::Result<Connection> {
    // Load sqlite-vec extension automatically for all connections
    unsafe {
        #[allow(clippy::missing_transmute_annotations)]
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite_vec::sqlite3_vec_init as *const (),
        )));
    }

    let conn = match path {
        Some(p) => Connection::open(p)?,
        None => Connection::open_in_memory()?,
    };

    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;

    Ok(conn)
}

/// On-disk size of a SQLite database in bytes (`page_count * page_size`). For
/// an in-memory DB this is the in-memory footprint. Used by the metrics CT scan.
pub fn db_size_bytes(conn: &Connection) -> rusqlite::Result<i64> {
    let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    Ok(page_count * page_size)
}

/// The two independent SQLite memory stores that make up NeuroLithe V2.
///
/// `stm` is today's decaying fact engine; `ltm` is the permanent knowledge
/// tree. They live in separate files so each can be reset and evolve
/// independently — STM is wiped by soft/hard reset, LTM only by hard reset, and
/// the decay path never touches LTM. One process owns both connections and
/// serializes access (as the repository does today).
pub struct MemoryStores {
    pub stm: Connection,
    pub ltm: Connection,
}

/// Open both memory stores from config and apply each store's schema at its own
/// vector dimension. The stores' dimensions are independent, so building one
/// never affects the other. Spine seeding is the LTM repository's job (called
/// by the daemon, slice 11), not the schema's.
pub fn init_stores(config: &AppConfig) -> rusqlite::Result<MemoryStores> {
    let stm = init_db(config.stm.path.as_ref())?;
    init_schema(&stm, config.stm.vector_dimension)?;

    let ltm = init_db(config.ltm.path.as_ref())?;
    init_ltm_schema(&ltm, config.ltm.vector_dimension)?;

    Ok(MemoryStores { stm, ltm })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::config::{
        FeederConfig, KafkaConfig, LlmConfig, LlmProvider, MetricsConfig, PithosConfig,
        StoreConfig, SweepConfig,
    };

    #[test]
    fn test_init_db_in_memory() {
        let _conn = init_db(None as Option<&String>).expect("Failed to init in-memory db");
    }

    /// A test config pointing the two stores at distinct paths with distinct
    /// vector dimensions.
    fn test_config(stm_path: String, ltm_path: String) -> AppConfig {
        AppConfig {
            llm: LlmConfig {
                provider: LlmProvider::Custom,
                model: "m".into(),
                embedding_model: "e".into(),
                base_url: None,
            },
            stm: StoreConfig {
                vector_dimension: 1536,
                path: Some(stm_path),
            },
            ltm: StoreConfig {
                vector_dimension: 768,
                path: Some(ltm_path),
            },
            kafka: KafkaConfig {
                brokers: "localhost:9092".into(),
                group_id: "neurolithe".into(),
            },
            pithos: PithosConfig {
                base_url: "http://localhost:8080".into(),
                token: String::new(),
            },
            sweep: SweepConfig {
                interval_secs: 86_400,
            },
            metrics: MetricsConfig { interval_secs: 60 },
            feeder: FeederConfig { enabled: true },
        }
    }

    /// Both stores init into their own files independently, and their vector
    /// dimensions don't interfere: STM's `vec_nodes` is built at the STM
    /// dimension while LTM carries a different one.
    #[test]
    fn test_init_stores_independent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stm_path = dir.path().join("stm.sqlite");
        let ltm_path = dir.path().join("ltm.sqlite");

        let config = test_config(
            stm_path.to_string_lossy().into_owned(),
            ltm_path.to_string_lossy().into_owned(),
        );
        // Sanity: the two stores carry different dimensions.
        assert_ne!(config.stm.vector_dimension, config.ltm.vector_dimension);

        let stores = init_stores(&config).expect("init_stores should succeed");

        // Both files were created on disk — the stores are distinct.
        assert!(stm_path.exists(), "STM file should exist");
        assert!(ltm_path.exists(), "LTM file should exist");

        // STM ran its schema: the sqlite-vec virtual table is present and was
        // built at the STM dimension (a node-shaped embedding fits).
        let vec_table_exists: bool = stores
            .stm
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='vec_nodes'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        assert!(vec_table_exists, "STM vec_nodes should be created");

        // The STM vec table accepts a vector of the STM dimension — proof the
        // dimension applied to STM and was not crossed with LTM's.
        let embedding = vec![0.0_f32; config.stm.vector_dimension];
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                embedding.as_ptr() as *const u8,
                std::mem::size_of_val(embedding.as_slice()),
            )
        };
        stores
            .stm
            .execute(
                "INSERT INTO vec_nodes(node_id, embedding) VALUES (1, ?1)",
                rusqlite::params![bytes],
            )
            .expect("STM vec insert at STM dimension should succeed");
    }
}

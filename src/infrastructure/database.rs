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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_db_in_memory() {
        let _conn = init_db(None as Option<&String>).expect("Failed to init in-memory db");
    }
}

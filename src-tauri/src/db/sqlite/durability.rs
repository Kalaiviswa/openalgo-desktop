//! How far each SQLite store pushes a commit to disk before it returns
//! (`PRAGMA synchronous`), chosen per store (ARCH-02).
//!
//! Every store runs in WAL mode, where an app crash never loses a committed
//! transaction at either level; the levels differ only on a power cut or an
//! operating-system crash, where `NORMAL` can roll back the last commits.
//!
//! | Store | Level | Why |
//! | --- | --- | --- |
//! | `openalgo.db` ([`Durability::MAIN`]) | `FULL` | The order and obligation journal: strategy runs, orders and the position book, OpenScript and scalping rows, credentials and the key vault. A committed order intent or fill must survive a power cut, or recovery acts on a past that did not happen. |
//! | `logs.db` ([`Durability::LOGS`]) | `NORMAL` | Request logs, latency and traffic samples, health history: a power cut loses only the last diagnostics. |
//! | `sandbox.db` ([`Durability::SANDBOX`]) | `NORMAL` | Simulated orders and funds; the sandbox engine catches up on start. |
//!
//! `FULL` costs one log sync per commit, small at one trader's write rate; a
//! master contract replacement is one transaction, so it pays one sync. The
//! web runs every database at `NORMAL`: `openalgo.db` at `FULL` is a
//! deliberate deviation (CLAUDE.md, "Data and migrations").

/// A store's `PRAGMA synchronous` level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// `synchronous=FULL`: the log is synced on every commit.
    Full,
    /// `synchronous=NORMAL`: the log is synced at checkpoints.
    Normal,
}

impl Durability {
    /// `openalgo.db`, the order and obligation journal.
    pub const MAIN: Durability = Durability::Full;
    /// `logs.db`.
    pub const LOGS: Durability = Durability::Normal;
    /// `sandbox.db`.
    pub const SANDBOX: Durability = Durability::Normal;

    /// The statement that applies this level to one connection.
    pub const fn pragma(self) -> &'static str {
        match self {
            Durability::Full => "PRAGMA synchronous=FULL;",
            Durability::Normal => "PRAGMA synchronous=NORMAL;",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{logs::LogsDb, SqliteDb};

    /// `PRAGMA synchronous` as SQLite reports it: 1 is NORMAL, 2 is FULL.
    fn synchronous(c: &rusqlite::Connection) -> rusqlite::Result<i64> {
        c.query_row("PRAGMA synchronous", [], |r| r.get(0))
    }

    /// ARCH-02: the order and obligation journal syncs every commit; logs
    /// and the sandbox stay at NORMAL. Read back from live connections.
    #[test]
    fn each_store_runs_at_its_own_durability() {
        let dir = tempfile::tempdir().unwrap();
        let main = SqliteDb::new(&dir.path().join("openalgo.db")).unwrap();
        // Every pooled connection, not only the first one opened.
        let (a, b) = (main.conn().unwrap(), main.conn().unwrap());
        assert_eq!(synchronous(&a).unwrap(), 2, "openalgo.db runs at FULL");
        assert_eq!(synchronous(&b).unwrap(), 2, "openalgo.db runs at FULL");

        let logs = LogsDb::new(&dir.path().join("logs.db")).unwrap();
        assert_eq!(
            synchronous(&logs.conn().unwrap()).unwrap(),
            1,
            "logs.db stays at NORMAL"
        );

        let sandbox = crate::sandbox::SandboxDb::open(&dir.path().join("sandbox.db")).unwrap();
        assert_eq!(
            sandbox.with_conn(synchronous).unwrap(),
            1,
            "sandbox.db stays at NORMAL"
        );
    }
}

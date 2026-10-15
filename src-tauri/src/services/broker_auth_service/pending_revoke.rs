//! What a broker revoke whose database write failed still has to write
//! (SEC-05b), and how it is kept in the marker file
//! (`BrokerAuthService`'s `REVOKE_MARKER`) across a restart.

use crate::brokers::catalog;
use crate::db::sqlite::auth;
use crate::error::Result;

/// What a broker revoke whose database write failed still has to write
/// (SEC-05b), kept in memory and in the marker file (`REVOKE_MARKER`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingRevoke {
    /// A logout, a broker switch or any other explicit revoke: every stored
    /// session. An empty or unreadable marker means this (fail closed; it
    /// is also what a marker written before scopes existed holds).
    All,
    /// The daily boundary (SES-02): the stored sessions authenticated
    /// before it, except continuous (crypto) ones (SES-01). A session
    /// signed in after the boundary is never touched by its retry.
    Before(chrono::DateTime<chrono::Utc>),
}

impl PendingRevoke {
    /// Two revokes both still to be written: the wider one.
    pub(super) fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Before(a), Self::Before(b)) => Self::Before(a.max(b)),
            _ => Self::All,
        }
    }

    pub(super) fn write(self, conn: &rusqlite::Connection) -> Result<()> {
        match self {
            Self::All => auth::revoke_all(conn).map(|_| ()),
            Self::Before(boundary) => auth::revoke_before(conn, boundary, |broker| {
                !catalog::has_daily_boundary(broker)
            })
            .map(|_| ()),
        }
    }

    /// The marker's contents: empty for `All`, the boundary (RFC 3339) for
    /// `Before`.
    pub(super) fn encode(self) -> String {
        match self {
            Self::All => String::new(),
            Self::Before(boundary) => boundary.to_rfc3339(),
        }
    }

    /// Read a marker; anything but a boundary means every session.
    pub(super) fn decode(text: &str) -> Self {
        chrono::DateTime::parse_from_rfc3339(text.trim())
            .map(|b| Self::Before(b.with_timezone(&chrono::Utc)))
            .unwrap_or(Self::All)
    }
}

#[cfg(test)]
mod tests {
    use super::PendingRevoke;
    use chrono::TimeZone;

    /// The marker's scope: empty (as written before scopes existed) or
    /// unreadable means every session; a boundary reads back as written;
    /// two pending revokes merge to the wider.
    #[test]
    fn a_marker_reads_back_its_scope_and_merges_to_the_wider() {
        let b1 = chrono::Utc
            .with_ymd_and_hms(2026, 10, 4, 21, 30, 0)
            .unwrap();
        let b2 = chrono::Utc
            .with_ymd_and_hms(2026, 10, 5, 21, 30, 0)
            .unwrap();
        let (all, before1, before2) = (
            PendingRevoke::All,
            PendingRevoke::Before(b1),
            PendingRevoke::Before(b2),
        );
        for unreadable in ["", "\n", "2026-10-0", "garbage"] {
            assert_eq!(PendingRevoke::decode(unreadable), all, "{:?}", unreadable);
        }
        assert_eq!(all.encode(), "");
        assert_eq!(PendingRevoke::decode(&before1.encode()), before1);
        assert_eq!(before1.merge(before2), before2);
        assert_eq!(before2.merge(before1), before2);
        assert_eq!(before1.merge(all), all);
        assert_eq!(all.merge(before1), all);
    }
}

//! Broker app credentials (what the web keeps in `.env` as BROKER_API_KEY,
//! BROKER_API_SECRET and the market-data pair), one set per broker.
//! Encrypted per column with AAD bound to the broker id. Never returned to
//! the UI in plaintext; the masked form mirrors the web's `mask_secret`, and
//! the Broker Configuration page shows [`hint`]. The broker's redirect URL,
//! when the trader set one, is kept with its keys (not a secret).

use crate::error::Result;
use crate::security::crypto::Aad;
use crate::security::{Secret, SecurityManager};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

#[derive(Debug, Clone, Default)]
pub struct BrokerCredentialSet {
    pub api_key: Secret,
    pub api_secret: Option<Secret>,
    pub api_key_market: Option<Secret>,
    pub api_secret_market: Option<Secret>,
    pub client_id: Option<String>,
    /// The redirect URL the trader set for this broker; `None` means the
    /// automatic one (`ServerConfig::redirect_url_for`).
    pub redirect_url: Option<String>,
}

/// Fields to change; `None` keeps the stored value (web: empty means keep).
#[derive(Debug, Default)]
pub struct CredentialUpdate {
    pub api_key: Option<Secret>,
    pub api_secret: Option<Secret>,
    pub api_key_market: Option<Secret>,
    pub api_secret_market: Option<Secret>,
    pub client_id: Option<String>,
}

fn aad(column: &str, broker: &str) -> Aad {
    Aad::new("broker_credentials", column, broker)
}

/// What the Broker Configuration page shows of a saved value: the first and
/// last three characters around `***` for a value of 12 characters or more,
/// otherwise only `***`. At most six characters, and never more than half
/// the value, are ever shown; the length is not.
pub fn hint(value: &str) -> String {
    let n = value.chars().count();
    if n >= 12 {
        let head: String = value.chars().take(3).collect();
        let tail: String = value.chars().skip(n - 3).collect();
        format!("{}***{}", head, tail)
    } else {
        "***".to_string()
    }
}

/// Same rule as the web's `mask_secret`: prefix plus a fixed eight asterisks.
pub fn mask(value: &str, show: usize) -> String {
    if value.is_empty() {
        String::new()
    } else if value.chars().count() <= show {
        "*".repeat(8)
    } else {
        format!(
            "{}{}",
            value.chars().take(show).collect::<String>(),
            "*".repeat(8)
        )
    }
}

fn enc(
    security: &SecurityManager,
    value: &Option<Secret>,
    column: &str,
    broker: &str,
) -> Result<(Option<String>, Option<String>)> {
    match value {
        Some(v) if !v.is_empty() => {
            let (c, n) = security.encrypt(v.expose(), &aad(column, broker))?;
            Ok((Some(c), Some(n)))
        }
        _ => Ok((None, None)),
    }
}

fn dec(
    security: &SecurityManager,
    ct: Option<String>,
    nonce: Option<String>,
    column: &str,
    broker: &str,
) -> Result<Option<Secret>> {
    match (ct, nonce) {
        (Some(c), Some(n)) if !c.is_empty() => {
            Ok(Some(security.decrypt(&c, &n, &aad(column, broker))?))
        }
        _ => Ok(None),
    }
}

pub fn load(
    conn: &Connection,
    security: &SecurityManager,
    broker: &str,
) -> Result<Option<BrokerCredentialSet>> {
    type Raw = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<Raw> = conn
        .query_row(
            "SELECT api_key_encrypted, api_key_nonce, api_secret_encrypted, api_secret_nonce,
                    api_key_market_encrypted, api_key_market_nonce,
                    api_secret_market_encrypted, api_secret_market_nonce, client_id,
                    redirect_url
             FROM broker_credentials WHERE broker_id = ?1",
            [broker],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()?;
    let Some((k, kn, s, sn, km, kmn, sm, smn, client_id, redirect_url)) = row else {
        return Ok(None);
    };
    Ok(Some(BrokerCredentialSet {
        api_key: dec(security, k, kn, "api_key", broker)?.unwrap_or_default(),
        api_secret: dec(security, s, sn, "api_secret", broker)?,
        api_key_market: dec(security, km, kmn, "api_key_market", broker)?,
        api_secret_market: dec(security, sm, smn, "api_secret_market", broker)?,
        client_id,
        redirect_url: redirect_url.filter(|u: &String| !u.trim().is_empty()),
    }))
}

/// Merge `update` into what is stored for `broker`.
pub fn save(
    conn: &Connection,
    security: &SecurityManager,
    broker: &str,
    update: CredentialUpdate,
) -> Result<()> {
    let current = load(conn, security, broker)?.unwrap_or_default();
    let merged = BrokerCredentialSet {
        api_key: update.api_key.unwrap_or(current.api_key),
        api_secret: update.api_secret.or(current.api_secret),
        api_key_market: update.api_key_market.or(current.api_key_market),
        api_secret_market: update.api_secret_market.or(current.api_secret_market),
        client_id: update.client_id.or(current.client_id),
        redirect_url: current.redirect_url,
    };
    let (k, kn) = enc(security, &Some(merged.api_key.clone()), "api_key", broker)?;
    let (s, sn) = enc(security, &merged.api_secret, "api_secret", broker)?;
    let (km, kmn) = enc(security, &merged.api_key_market, "api_key_market", broker)?;
    let (sm, smn) = enc(
        security,
        &merged.api_secret_market,
        "api_secret_market",
        broker,
    )?;
    conn.execute(
        "INSERT INTO broker_credentials (broker_id, api_key_encrypted, api_key_nonce,
             api_secret_encrypted, api_secret_nonce, api_key_market_encrypted, api_key_market_nonce,
             api_secret_market_encrypted, api_secret_market_nonce, client_id, updated_at)
         VALUES (?1, COALESCE(?2, ''), COALESCE(?3, ''), ?4, ?5, ?6, ?7, ?8, ?9, ?10, datetime('now'))
         ON CONFLICT(broker_id) DO UPDATE SET
             api_key_encrypted = excluded.api_key_encrypted,
             api_key_nonce = excluded.api_key_nonce,
             api_secret_encrypted = excluded.api_secret_encrypted,
             api_secret_nonce = excluded.api_secret_nonce,
             api_key_market_encrypted = excluded.api_key_market_encrypted,
             api_key_market_nonce = excluded.api_key_market_nonce,
             api_secret_market_encrypted = excluded.api_secret_market_encrypted,
             api_secret_market_nonce = excluded.api_secret_market_nonce,
             client_id = excluded.client_id,
             updated_at = datetime('now')",
        params![broker, k, kn, s, sn, km, kmn, sm, smn, merged.client_id],
    )?;
    conn.execute(
        "INSERT OR REPLACE INTO configured_brokers (broker_id) VALUES (?1)",
        [broker],
    )?;
    Ok(())
}

/// Brokers that have an API key saved, sorted by name. Credentials are kept
/// per broker, so switching back to one needs no re-entry.
pub fn list_configured(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT broker_id FROM broker_credentials
         WHERE api_key_encrypted IS NOT NULL AND api_key_encrypted <> ''
         ORDER BY broker_id",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|r| r.ok()).collect())
}

/// Set the redirect URL kept with a broker's keys; `None` (or empty) goes
/// back to the automatic one. The broker's keys must already be saved.
pub fn set_redirect_url(conn: &Connection, broker: &str, url: Option<&str>) -> Result<()> {
    let url = url.map(str::trim).filter(|u| !u.is_empty());
    conn.execute(
        "UPDATE broker_credentials SET redirect_url = ?2, updated_at = datetime('now')
         WHERE broker_id = ?1",
        params![broker, url],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, broker: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM broker_credentials WHERE broker_id = ?1",
        [broker],
    )?;
    conn.execute(
        "DELETE FROM configured_brokers WHERE broker_id = ?1",
        [broker],
    )?;
    Ok(())
}

pub fn delete_all(conn: &Connection) -> Result<()> {
    conn.execute("DELETE FROM broker_credentials", [])?;
    conn.execute("DELETE FROM configured_brokers", [])?;
    Ok(())
}

/// Masked view for the Profile page (web `/api/broker/credentials` GET).
#[derive(Debug, Serialize, Default)]
pub struct MaskedCredentials {
    pub broker_api_key: String,
    pub broker_api_key_raw_length: usize,
    pub broker_api_secret: String,
    pub broker_api_secret_raw_length: usize,
    pub broker_api_key_market: String,
    pub broker_api_key_market_raw_length: usize,
    pub broker_api_secret_market: String,
    pub broker_api_secret_market_raw_length: usize,
}

impl BrokerCredentialSet {
    pub fn masked(&self) -> MaskedCredentials {
        let m = |v: &Option<Secret>, show| {
            v.as_ref()
                .map(|s| (mask(s.expose(), show), s.len()))
                .unwrap_or_default()
        };
        let (k, kl) = (mask(self.api_key.expose(), 6), self.api_key.len());
        let (s, sl) = m(&self.api_secret, 4);
        let (km, kml) = m(&self.api_key_market, 6);
        let (sm, sml) = m(&self.api_secret_market, 4);
        MaskedCredentials {
            broker_api_key: k,
            broker_api_key_raw_length: kl,
            broker_api_secret: s,
            broker_api_secret_raw_length: sl,
            broker_api_key_market: km,
            broker_api_key_market_raw_length: kml,
            broker_api_secret_market: sm,
            broker_api_secret_market_raw_length: sml,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_shows_at_most_six_characters_and_never_half() {
        assert_eq!(hint(""), "***");
        assert_eq!(hint("abcd1234"), "***");
        assert_eq!(hint("abcdefghijk"), "***");
        assert_eq!(hint("abcdefghijkl"), "abc***jkl");
        let long = "kiteapikey1234567890";
        let h = hint(long);
        assert_eq!(h, "kit***890");
        assert!(!h.contains(long));
    }

    #[test]
    fn mask_matches_web() {
        assert_eq!(mask("", 6), "");
        assert_eq!(mask("abc", 6), "********");
        assert_eq!(mask("abcdefghij", 6), "abcdef********");
        assert_eq!(mask("abcdefghijklmnopqrstuvwxyz", 4), "abcd********");
    }
}

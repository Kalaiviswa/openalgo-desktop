//! The broker account a session trades through (NEW-TR-01).
//!
//! A Live obligation is bound to the broker and the account it was opened
//! on (`services::live_owner`). Most brokers name the account at login, but
//! not all: Paytm Money names none, Groww and Tradejini have no client id to
//! save, and several answer with nothing when a reply field is missing, or
//! with a placeholder: the broker's own name (Samco, Delta Exchange), or a
//! value of the saved keys (the Fyers app id, the IIFL Capital API key, the
//! Nubra phone number). The audit is in PR #165.
//!
//! For those, the account is the identity of the saved key set the session
//! was made from: a keyed digest (`SecurityManager::broker_keys_identity`),
//! never the keys themselves. The desktop holds one key set per broker, so
//! the same keys are the same owner across restarts, and saving other keys
//! makes another owner: rows of the old keys are never sent, read or closed
//! through a session of the new ones. When neither the broker's account nor
//! the keys can be read, the account is blank, and a blank account owns
//! nothing (`LiveOwner::same_session`).

use crate::db::sqlite::credentials;
use crate::state::{AppState, BrokerSession};

/// How an account derived from the saved keys is written.
pub const KEYS_ACCOUNT: &str = "keys:";
/// The digest characters kept: 64 bits, ample to tell key sets apart.
const KEYS_DIGEST_LEN: usize = 16;

/// The account `session` trades through: the broker's own account id, or
/// the identity of its saved keys where the broker names none, or only a
/// placeholder. Blank when neither can be read.
pub fn account_of(ctx: &AppState, session: &BrokerSession) -> String {
    let named = session.user_id.trim();
    let saved = ctx
        .sqlite
        .conn()
        .and_then(|c| credentials::load(&c, &ctx.security, &session.broker_id));
    let keys = match saved {
        Ok(Some(keys)) if !keys.api_key.is_empty() => keys,
        // No saved keys to tell by (a test session, a locked vault): the
        // broker's own name for the account, unless it is none.
        _ => return real_account(named, &session.broker_id, None).unwrap_or_default(),
    };
    let client_id = keys.client_id.clone().unwrap_or_default();
    if let Some(account) = real_account(named, &session.broker_id, Some((&keys, &client_id))) {
        return account;
    }
    match ctx.security.broker_keys_identity(&[
        &session.broker_id.to_ascii_lowercase(),
        keys.api_key.expose(),
        &client_id,
    ]) {
        Ok(digest) if digest.len() >= KEYS_DIGEST_LEN => {
            format!("{}{}", KEYS_ACCOUNT, &digest[..KEYS_DIGEST_LEN])
        }
        _ => String::new(),
    }
}

/// `named` when it is an account id: not blank, not the broker's own name,
/// and not a saved key value the adapter fell back to (unless the saved
/// configuration names that value as the account: Dhan's client id, the
/// Noren family's, Nubra's mobile number; `catalog::configured_account`).
fn real_account(
    named: &str,
    broker: &str,
    keys: Option<(&credentials::BrokerCredentialSet, &str)>,
) -> Option<String> {
    if named.is_empty() || named.eq_ignore_ascii_case(broker) {
        return None;
    }
    if let Some((keys, client_id)) = keys {
        let api_key = keys.api_key.expose();
        let configured = crate::brokers::catalog::configured_account(
            broker,
            api_key,
            Some(client_id).filter(|c| !c.is_empty()),
        );
        if configured.as_deref() == Some(named) {
            return Some(named.to_string());
        }
        if named == api_key || (!client_id.is_empty() && named == client_id) {
            return None;
        }
    }
    Some(named.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(api_key: &str) -> credentials::BrokerCredentialSet {
        credentials::BrokerCredentialSet {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// The audit's cases: what a login names is the account unless it is
    /// nothing, the broker's own name, or a saved key value the adapter fell
    /// back to; a value the configuration names as the account stays one.
    #[test]
    fn a_placeholder_is_not_an_account() {
        let real = |named: &str, broker: &str, api_key: &str, client: &str| {
            real_account(named, broker, Some((&keys(api_key), client)))
        };
        assert_eq!(real("", "paytm", "k", ""), None);
        assert_eq!(real("samco", "samco", "k", ""), None);
        assert_eq!(real("DELTAEXCHANGE", "deltaexchange", "k", ""), None);
        assert_eq!(real("APPID-100", "fyers", "APPID-100", "APPID-100"), None);
        assert_eq!(real("IIFLKEY", "iiflcapital", "IIFLKEY", ""), None);
        assert_eq!(
            real("XA1234", "fyers", "APPID-100", "APPID-100"),
            Some("XA1234".into())
        );
        assert_eq!(real("1100", "dhan", "token", "1100"), Some("1100".into()));
        assert_eq!(
            real("9876543210", "nubra", "9876543210", ""),
            Some("9876543210".into())
        );
        assert_eq!(
            real("AB1234", "zerodha", "kitekey", ""),
            Some("AB1234".into())
        );
        assert_eq!(
            real_account("AB1234", "zerodha", None),
            Some("AB1234".into())
        );
        assert_eq!(real_account("", "paytm", None), None);
    }
}

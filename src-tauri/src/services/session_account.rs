//! The broker account a session trades through (NEW-TR-01).
//!
//! A Live obligation is bound to the broker and the account it was opened
//! on (`services::live_owner`). Most brokers name the account at login, but
//! not all: Paytm Money names none, Groww and Tradejini have no client id to
//! save, and several answer with nothing when a reply field is missing, or
//! with a placeholder: the broker's own name (Samco, Delta Exchange), or a
//! value of the saved keys (the Fyers app id before its `-`, the IIFL
//! Capital API key or its part before `:::`). The XTS family names the
//! market login's account when market keys are saved and the interactive
//! one otherwise, so its name changes when market keys are added later. The
//! audit of all 37 brokers is in PR #165.
//!
//! For those, the account is the identity of the saved key set the session
//! was made from: a keyed digest (`SecurityManager::broker_keys_identity`),
//! never the keys themselves. The desktop holds one key set per broker, so
//! the same keys are the same owner across restarts, and saving other keys
//! (which ends the session) makes another owner. **A limit, disclosed:**
//! two logins of different accounts made with the same saved keys are one
//! owner when the broker names neither account.
//!
//! The account is worked out once per session ([`AccountCache`]): keys that
//! cannot be read at that moment (a locked vault, a busy database) give a
//! blank account, never a placeholder, and a blank account is never cached,
//! so the next look tries again. A blank account owns nothing
//! (`LiveOwner::same_session`).

use crate::db::sqlite::credentials::{self, BrokerCredentialSet};
use crate::state::{AppState, BrokerSession};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use std::sync::OnceLock;

/// How an account derived from the saved keys is written.
pub const KEYS_ACCOUNT: &str = "keys:";
/// The digest characters kept: 64 bits, ample to tell key sets apart.
const KEYS_DIGEST_LEN: usize = 16;
/// Brokers whose login names the account of whichever key set answered (the
/// XTS family): always told by their saved interactive keys.
const ALWAYS_BY_KEYS: &[&str] = &[
    "compositedge",
    "fivepaisaxts",
    "ibulls",
    "iifl",
    "jainamxts",
    "rmoney",
    "wisdom",
];

/// The session an account was worked out for.
type Stamp = (String, String, DateTime<Utc>);

/// What a session's account was worked out to be. For a session told by its
/// saved keys, `placeholders` are what its login could have named instead
/// (the broker's name, a saved key value), as digests under a key of this
/// process ([`placeholder_digest`]): an obligation recorded with one of
/// them before accounts were derived is this session's
/// (`LiveOwner::refusal`). Never shown or stored.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Told {
    pub account: String,
    pub placeholders: Vec<String>,
}

/// The account of the session connected now, worked out once for it.
#[derive(Default)]
pub struct AccountCache(Mutex<Option<(Stamp, Told)>>);

/// The account `session` trades through: the broker's own account id, or
/// the identity of its saved keys where the broker names none, or only a
/// placeholder. Blank when neither can be told.
pub fn account_of(ctx: &AppState, session: &BrokerSession) -> String {
    told(ctx, session).account
}

/// [`account_of`] with the placeholders of a session told by its keys.
pub fn told(ctx: &AppState, session: &BrokerSession) -> Told {
    let stamp: Stamp = (
        session.broker_id.clone(),
        session.user_id.clone(),
        session.authenticated_at,
    );
    if let Some((at, told)) = ctx.session_account.0.lock().as_ref() {
        if *at == stamp {
            return told.clone();
        }
    }
    let saved = ctx
        .sqlite
        .conn()
        .and_then(|c| credentials::load(&c, &ctx.security, &session.broker_id));
    let told = match saved {
        Ok(Some(keys)) if !keys.api_key.is_empty() => by_keys(ctx, session, &keys),
        // No saved keys at all (a session installed by a test): the broker's
        // own name for the account, unless it is none or a placeholder.
        Ok(_) => Told {
            account: real_account(session.user_id.trim(), &session.broker_id, None)
                .unwrap_or_default(),
            placeholders: Vec::new(),
        },
        // The keys cannot be read now: whether the name is a placeholder
        // cannot be told, so nothing is owned until it can.
        Err(e) => {
            tracing::warn!(
                "The broker keys could not be read to tell the account: {}",
                e
            );
            Told::default()
        }
    };
    // A blank account is looked for again next time, never kept.
    if !told.account.is_empty() {
        *ctx.session_account.0.lock() = Some((stamp, told.clone()));
    }
    told
}

/// The account of a session whose saved keys could be read.
fn by_keys(ctx: &AppState, session: &BrokerSession, keys: &BrokerCredentialSet) -> Told {
    let broker = session.broker_id.to_ascii_lowercase();
    let client_id = keys.client_id.clone().unwrap_or_default();
    if !ALWAYS_BY_KEYS.contains(&broker.as_str()) {
        if let Some(account) =
            real_account(session.user_id.trim(), &broker, Some((keys, &client_id)))
        {
            return Told {
                account,
                placeholders: Vec::new(),
            };
        }
    }
    let account =
        match ctx
            .security
            .broker_keys_identity(&[&broker, keys.api_key.expose(), &client_id])
        {
            Ok(digest) if digest.len() >= KEYS_DIGEST_LEN => {
                format!("{}{}", KEYS_ACCOUNT, &digest[..KEYS_DIGEST_LEN])
            }
            _ => return Told::default(),
        };
    let mut placeholders = vec![placeholder_digest(&broker)];
    placeholders.extend(
        key_values(keys.api_key.expose(), &client_id)
            .iter()
            .map(|v| placeholder_digest(v)),
    );
    Told {
        account,
        placeholders,
    }
}

/// A placeholder as a session keeps it: HMAC-SHA256 under a random key of
/// this process, so a saved key value is compared without being kept.
pub(crate) fn placeholder_digest(value: &str) -> String {
    use hmac::{Hmac, Mac};
    use rand::RngCore;
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        k
    });
    let mut mac = match <Hmac<sha2::Sha256> as Mac>::new_from_slice(key) {
        Ok(m) => m,
        Err(_) => return String::new(),
    };
    mac.update(value.trim().as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Whether `value`, recorded on an obligation, is one of `placeholders`.
pub fn is_placeholder(placeholders: &[String], value: &str) -> bool {
    let digest = placeholder_digest(value);
    !digest.is_empty() && placeholders.contains(&digest)
}

/// The values an adapter falls back to when the broker names no account:
/// the API key, its part before `:::` or `-`, and the client id.
fn key_values(api_key: &str, client_id: &str) -> Vec<String> {
    let mut out = vec![api_key.to_string(), client_id.to_string()];
    for sep in [":::", "-"] {
        if let Some((head, _)) = api_key.split_once(sep) {
            out.push(head.to_string());
        }
    }
    out.into_iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// `named` when it is an account id: not blank, not the broker's own name,
/// and not a saved key value the adapter fell back to (unless the saved
/// configuration names that value as the account: Dhan's client id, the
/// Noren family's, Nubra's mobile number; `catalog::configured_account`).
fn real_account(
    named: &str,
    broker: &str,
    keys: Option<(&BrokerCredentialSet, &str)>,
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
        if key_values(api_key, client_id).iter().any(|v| v == named) {
            return None;
        }
    }
    Some(named.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(api_key: &str) -> BrokerCredentialSet {
        BrokerCredentialSet {
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
        // Fyers without `fy_id`: the saved client id (its app id), else the
        // API key's part before `-` (fyers/auth.rs).
        assert_eq!(real("XB0123", "fyers", "XB0123-100", ""), None);
        assert_eq!(real("XB0123-100", "fyers", "key", "XB0123-100"), None);
        // IIFL Capital: the client id, else the API key's part before
        // `:::`, else the whole key (iiflcapital/auth.rs).
        assert_eq!(real("C123", "iiflcapital", "C123:::secret", ""), None);
        assert_eq!(real("IIFLKEY", "iiflcapital", "IIFLKEY", ""), None);
        assert_eq!(
            real("XA1234", "fyers", "XB0123-100", ""),
            Some("XA1234".into())
        );
        // Configured as the account, and confirmed by the broker.
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

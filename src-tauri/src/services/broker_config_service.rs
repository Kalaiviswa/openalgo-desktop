//! Broker keys, one set per broker, for as many brokers as the trader
//! likes, and the one broker that is logged in (desktop; an approved
//! deviation from the web, which reads one broker's keys from `.env`; see
//! CLAUDE.md, "The compatibility contract").
//!
//! The Broker Configuration and Select Broker pages read [`list`]: every
//! broker with its sign-in kind, exchanges, the keys it needs
//! (`setup_guide`) and a masked view of what is saved. Nothing saved is
//! ever returned in full ([`credentials::hint`]). [`save`] keeps every key
//! left blank; [`forget`] removes one broker's keys, signing out of it
//! first when it is the one logged in; [`end_session`] ends the live broker
//! session without signing out of OpenAlgo.
//!
//! [`list`]: BrokerConfigService::list
//! [`save`]: BrokerConfigService::save
//! [`forget`]: BrokerConfigService::forget
//! [`end_session`]: BrokerConfigService::end_session

use crate::brokers::catalog;
use crate::brokers::setup_guide::{self, KeyName};
use crate::db::sqlite::credentials::{self, BrokerCredentialSet, CredentialUpdate};
use crate::error::{AppError, Result};
use crate::events::SessionEndReason;
use crate::security::{Secret, SecurityManager};
use crate::services::broker_auth_service::BrokerAuthService;
use crate::state::AppState;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{json, Value};

pub struct BrokerConfigService;

/// A save from the Broker Configuration page. Every `None` key keeps what
/// is saved; `redirect_url` is `None` to keep, `Some("")` for the
/// automatic address, or the address to use.
#[derive(Debug, Default)]
pub struct KeySave {
    pub update: CredentialUpdate,
    pub redirect_url: Option<String>,
}

/// What a save did.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Saved. `signed_out` when the saved keys were the logged-in broker's,
    /// whose session then ended (FLOW-04: new keys are a new session).
    Saved { message: String, signed_out: bool },
    /// New keys for the broker that is logged in, and the trader has not
    /// confirmed ending its session: nothing was saved.
    LoggedIn,
}

/// What a forget did.
#[derive(Debug, PartialEq, Eq)]
pub enum ForgetOutcome {
    /// The broker's keys are gone; `signed_out` when its session ended.
    Forgotten { signed_out: bool },
    /// The broker is logged in and the trader has not confirmed signing out.
    LoggedIn,
}

fn valid_broker(broker: &str) -> Result<()> {
    if catalog::ALL_BROKERS.contains(&broker) {
        Ok(())
    } else {
        Err(AppError::Validation(
            "Choose a broker from the list.".into(),
        ))
    }
}

/// A full `http`/`https` address with a host (`url_with_scheme` in the
/// broker routes applies the same rule to the Host Server URL).
fn http_url(v: &str) -> bool {
    match url::Url::parse(v) {
        Ok(u) => {
            matches!(u.scheme(), "http" | "https")
                && u.host_str().is_some_and(|h| !h.is_empty())
                && v.to_ascii_lowercase()
                    .starts_with(&format!("{}://", u.scheme()))
        }
        Err(_) => false,
    }
}

impl BrokerConfigService {
    /// The API key formats the web checks on save (`utils/env_check.py`):
    /// 5paisa `User_Key:::User_ID:::client_id`, Flattrade and Dhan
    /// `client_id:::api_key`. Shared by both save paths.
    pub fn check_api_key_format(broker: &str, key: &str) -> std::result::Result<(), &'static str> {
        let parts = key.matches(":::").count();
        match broker {
            "fivepaisa" if parts != 2 => {
                Err("5paisa API key must be in format: 'User_Key:::User_ID:::client_id'")
            }
            "flattrade" if parts != 1 => {
                Err("Flattrade API key must be in format: 'client_id:::api_key'")
            }
            "dhan" if parts != 1 => Err("Dhan API key must be in format: 'client_id:::api_key'"),
            _ => Ok(()),
        }
    }

    /// Save a broker's keys inside the caller's transaction: what `update`
    /// carries replaces the stored value, the rest is kept. Saving a
    /// broker's keys again is how a trader switches accounts, so the account
    /// its last ended session was bound to is forgotten. The one save of
    /// broker keys, for this page and the web-shaped
    /// `POST /api/broker/credentials`.
    pub fn save_keys(
        conn: &Connection,
        security: &SecurityManager,
        broker: &str,
        update: CredentialUpdate,
    ) -> Result<()> {
        credentials::save(conn, security, broker, update)?;
        crate::db::sqlite::auth::forget_account(conn, broker)?;
        Ok(())
    }

    /// The redirect URL a sign-in to `broker` uses: the one saved with its
    /// keys, else the single address the web-shaped settings keep (when it
    /// names this broker), else the automatic one.
    pub fn redirect_url(state: &AppState, broker: &str) -> Result<String> {
        let saved = {
            let conn = state.sqlite.conn()?;
            credentials::load(&conn, &state.security, broker)?.and_then(|c| c.redirect_url)
        };
        Ok(saved.unwrap_or_else(|| state.server_config().redirect_url_for(broker)))
    }

    /// Every broker, as the broker pages show it.
    pub fn list(state: &AppState) -> Result<Value> {
        let cfg = state.server_config();
        let live = state.get_broker_session().map(|s| s.broker_id);
        let conn = state.sqlite.conn()?;
        let mut brokers = Vec::with_capacity(catalog::ALL_BROKERS.len());
        for id in catalog::ALL_BROKERS {
            let saved = credentials::load(&conn, &state.security, id)?;
            let configured = saved.as_ref().is_some_and(|c| !c.api_key.is_empty());
            let guide = setup_guide::guide(id);
            let fields: Vec<Value> = guide
                .fields
                .iter()
                .map(|f| {
                    json!({
                        "key": f.key.as_str(),
                        "label": f.label,
                        "hint": f.hint,
                        "secret": f.secret,
                        "required": f.required,
                        "saved": saved.as_ref().and_then(|c| saved_view(c, f.key)),
                    })
                })
                .collect();
            let (redirect, automatic) = if setup_guide::uses_redirect_url(id) {
                let automatic = cfg.automatic_redirect_url(id);
                let effective = saved
                    .as_ref()
                    .and_then(|c| c.redirect_url.clone())
                    .unwrap_or_else(|| cfg.redirect_url_for(id));
                (Some(effective), Some(automatic))
            } else {
                (None, None)
            };
            let exchanges: Vec<&str> = state
                .brokers
                .get(id)
                .map(|a| a.supported_exchanges().iter().map(|e| e.as_str()).collect())
                .unwrap_or_default();
            brokers.push(json!({
                "id": id,
                "name": setup_guide::display_name(id),
                "configured": configured,
                "active": live.as_deref() == Some(*id),
                "sign_in": catalog::sign_in(id),
                "exchanges": exchanges,
                "banner": guide.banner,
                "fields": fields,
                "redirect_url_is_default": redirect.is_some() && redirect == automatic,
                "redirect_url": redirect,
                "redirect_url_automatic": automatic,
                "redirect_hint": setup_guide::uses_redirect_url(id).then_some(guide.redirect_hint),
            }));
        }
        Ok(json!({ "logged_in": live, "brokers": brokers }))
    }

    /// Save one broker's keys (and its redirect URL). Keys left out keep
    /// their saved value; a broker's first save needs every required key.
    ///
    /// New keys for the broker that is logged in end its session (FLOW-04:
    /// the session belongs to the keys it signed in with), only when the
    /// trader confirmed (`sign_out`); a redirect URL alone changes nothing
    /// live. The save and the sign-out run under the session lock.
    pub async fn save(
        state: &AppState,
        broker: &str,
        req: KeySave,
        sign_out: bool,
    ) -> Result<SaveOutcome> {
        valid_broker(broker)?;
        let guide = setup_guide::guide(broker);
        let name = setup_guide::display_name(broker);
        // Only the keys this broker uses are saved.
        let mut update = req.update;
        let uses = |k: KeyName| guide.fields.iter().any(|f| f.key == k);
        if !uses(KeyName::ApiSecret) {
            update.api_secret = None;
        }
        if !uses(KeyName::ClientId) {
            update.client_id = None;
        }
        if !uses(KeyName::ApiKeyMarket) {
            update.api_key_market = None;
        }
        if !uses(KeyName::ApiSecretMarket) {
            update.api_secret_market = None;
        }
        if let Some(k) = &update.api_key {
            Self::check_api_key_format(broker, k.expose())
                .map_err(|m| AppError::Validation(m.into()))?;
        }
        let redirect = match req.redirect_url.map(|u| u.trim().to_string()) {
            None => None,
            Some(_) if !setup_guide::uses_redirect_url(broker) => {
                return Err(AppError::Validation(format!(
                    "{} does not use a redirect URL.",
                    name
                )))
            }
            Some(u) if u.is_empty() => Some(None),
            Some(u) => {
                let suffix = format!("/{}/callback", broker);
                if !http_url(&u) || !u.trim_end_matches('/').ends_with(&suffix) {
                    return Err(AppError::Validation(format!(
                        "Enter the redirect URL as a full address ending in {}, or use the automatic one.",
                        suffix
                    )));
                }
                Some(Some(u.trim_end_matches('/').to_string()))
            }
        };
        let keys_changed = update.api_key.is_some()
            || update.api_secret.is_some()
            || update.client_id.is_some()
            || update.api_key_market.is_some()
            || update.api_secret_market.is_some();
        if !keys_changed && redirect.is_none() {
            return Err(AppError::Validation(
                "Nothing to save. Enter a new value to replace a saved one.".into(),
            ));
        }
        let held = BrokerAuthService::session_lock(state).await;
        let live = state
            .get_broker_session()
            .is_some_and(|s| s.broker_id == broker);
        if live && keys_changed && !sign_out {
            return Ok(SaveOutcome::LoggedIn);
        }
        let global_named_broker;
        {
            let mut conn = state.sqlite.conn()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let current = credentials::load(&tx, &state.security, broker)?;
            let configured = current.as_ref().is_some_and(|c| !c.api_key.is_empty());
            if !configured {
                if let Some(f) = guide.fields.iter().find(|f| {
                    f.required
                        && !match f.key {
                            KeyName::ApiKey => update.api_key.is_some(),
                            KeyName::ApiSecret => update.api_secret.is_some(),
                            KeyName::ClientId => update.client_id.is_some(),
                            KeyName::ApiKeyMarket => update.api_key_market.is_some(),
                            KeyName::ApiSecretMarket => update.api_secret_market.is_some(),
                        }
                }) {
                    return Err(AppError::Validation(format!(
                        "Enter the {} to save {}.",
                        f.label, name
                    )));
                }
            }
            if keys_changed {
                Self::save_keys(&tx, &state.security, broker, update)?;
            }
            // The URL kept with this broker's keys is the one source for it
            // from now on: the single web-shaped setting stops naming it.
            global_named_broker = state
                .server_config()
                .redirect_url
                .is_some_and(|u| u.ends_with(&format!("/{}/callback", broker)));
            if let Some(url) = &redirect {
                credentials::set_redirect_url(&tx, broker, url.as_deref())?;
                if global_named_broker {
                    crate::config::save(
                        &tx,
                        &crate::config::ServerConfigUpdate {
                            redirect_url: Some(String::new()),
                            ..Default::default()
                        },
                    )?;
                }
            }
            tx.commit()?;
        }
        if redirect.is_some() && global_named_broker {
            state.reload_config()?;
        }
        let signed_out = live && keys_changed;
        if signed_out {
            if let Err(e) =
                BrokerAuthService::revoke_held(state, SessionEndReason::Logout, &held).await
            {
                // The session is over in memory; its row is retried.
                tracing::error!("The ended broker session is still to be revoked: {}", e);
            }
        }
        drop(held);
        tracing::info!("Broker keys saved for {}", broker);
        Ok(SaveOutcome::Saved {
            message: if signed_out {
                format!(
                    "Saved your {} keys and ended its session. Log in again to use them.",
                    name
                )
            } else {
                format!("Saved your {} settings.", name)
            },
            signed_out,
        })
    }

    /// Forget one broker: its keys, its redirect URL and its stored session
    /// row. The broker that is logged in is signed out first, and only when
    /// the trader confirmed (`sign_out`).
    pub async fn forget(state: &AppState, broker: &str, sign_out: bool) -> Result<ForgetOutcome> {
        valid_broker(broker)?;
        // Under the session lock, so no sign-in to this broker completes
        // between the sign-out and the removal of its keys.
        let held = BrokerAuthService::session_lock(state).await;
        let live = state
            .get_broker_session()
            .is_some_and(|s| s.broker_id == broker);
        if live && !sign_out {
            return Ok(ForgetOutcome::LoggedIn);
        }
        if live {
            if let Err(e) =
                BrokerAuthService::revoke_held(state, SessionEndReason::Logout, &held).await
            {
                // The session is over in memory; its row is retried.
                tracing::error!("The ended broker session is still to be revoked: {}", e);
            }
        }
        let cfg = state.server_config();
        {
            let mut conn = state.sqlite.conn()?;
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            credentials::delete(&tx, broker)?;
            crate::db::sqlite::auth::delete(&tx, broker)?;
            let clear_active = cfg.active_broker.as_deref() == Some(broker);
            let clear_redirect = cfg
                .redirect_url
                .as_deref()
                .is_some_and(|u| u.ends_with(&format!("/{}/callback", broker)));
            if clear_active || clear_redirect {
                crate::config::save(
                    &tx,
                    &crate::config::ServerConfigUpdate {
                        active_broker: clear_active.then(String::new),
                        redirect_url: clear_redirect.then(String::new),
                        ..Default::default()
                    },
                )?;
            }
            tx.commit()?;
        }
        state.reload_config()?;
        drop(held);
        tracing::info!("Broker keys forgotten for {}", broker);
        Ok(ForgetOutcome::Forgotten { signed_out: live })
    }

    /// End the live broker session (not the OpenAlgo sign-in): what logging
    /// in to another broker does first. Returns the broker that was logged
    /// in, if any.
    pub async fn end_session(state: &AppState) -> Result<Option<String>> {
        let held = BrokerAuthService::session_lock(state).await;
        let Some(live) = state.get_broker_session() else {
            return Ok(None);
        };
        if let Err(e) = BrokerAuthService::revoke_held(state, SessionEndReason::Logout, &held).await
        {
            // The session is over in memory; its row is retried.
            tracing::error!("The ended broker session is still to be revoked: {}", e);
        }
        tracing::info!("Broker session ended for {}", live.broker_id);
        Ok(Some(live.broker_id))
    }
}

/// The masked view of one saved key, or `None` when nothing is saved. The
/// client id is an account id, not a secret, and is shown as saved: a
/// mistyped one makes every sign-in fail with "different account".
fn saved_view(c: &BrokerCredentialSet, key: KeyName) -> Option<String> {
    let hint = |s: &Secret| (!s.is_empty()).then(|| credentials::hint(s.expose()));
    match key {
        KeyName::ApiKey => hint(&c.api_key),
        KeyName::ApiSecret => c.api_secret.as_ref().and_then(hint),
        KeyName::ApiKeyMarket => c.api_key_market.as_ref().and_then(hint),
        KeyName::ApiSecretMarket => c.api_secret_market.as_ref().and_then(hint),
        KeyName::ClientId => c
            .client_id
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
    }
}

//! The WhatsApp tasks: pairing on a fresh session store, and the bot's
//! run (restore the sealed session, stay connected, save the session back,
//! and hand owner commands to its worker).

use super::store::SnapshotStore;
use super::{commands, db};
use super::{
    jid_to_phone, route_command, PairEvent, Raw, WaRoute, LOGGED_OUT_MESSAGE, PAIR_TIMEOUT_MESSAGE,
    STOP_JOIN,
};
use crate::messaging::format::redact_phone;
use crate::state::AppState;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use whatsapp_rust::Client;

pub(super) async fn pair_task(
    ctx: Weak<AppState>,
    phone: Option<String>,
    owner_user_id: Option<i64>,
    owner_username: Option<String>,
    cancel: CancellationToken,
) {
    let Some(c) = ctx.upgrade() else { return };
    let svc = &c.messaging.whatsapp;
    let store = Arc::new(SnapshotStore::new());
    let (tx, mut rx) = mpsc::channel::<Raw>(64);
    let bot = match svc.build(&c, store.clone(), tx, phone).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("WhatsApp pairing could not start: {}", e);
            svc.pair_event(
                &c,
                PairEvent::Failed("WhatsApp pairing could not start. Try again.".into()),
            )
            .await;
            return;
        }
    };
    let timeout = svc.settings.read().pair_timeout;
    drop(c);
    let mut handle = bot.spawn();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let outcome: Option<PairEvent> = loop {
        tokio::select! {
            _ = cancel.cancelled() => break None,
            _ = &mut deadline => break Some(PairEvent::Failed(PAIR_TIMEOUT_MESSAGE.into())),
            _ = &mut handle => break Some(PairEvent::Failed(
                "WhatsApp closed the pairing connection. Try again.".into())),
            ev = rx.recv() => {
                let Some(ev) = ev else { continue };
                let Some(c) = ctx.upgrade() else { break None };
                match ev {
                    Raw::Qr(code) => c.messaging.whatsapp.pair_event(&c, PairEvent::Qr(code)).await,
                    Raw::Code(code) => c.messaging.whatsapp.pair_event(&c, PairEvent::Code(code)).await,
                    Raw::Connected => {
                        let client = handle.client();
                        if client.is_logged_in() || store.has_paired_device() {
                            let own_jid = client.get_pn().map(|j| j.to_non_ad_string());
                            let own_phone = own_jid.as_deref().map(jid_to_phone).filter(|p| !p.is_empty());
                            break Some(PairEvent::Paired { own_jid, own_phone });
                        }
                    }
                    _ => {}
                }
            }
        }
    };
    // Close the pairing client (flushes its device state into the store).
    if tokio::time::timeout(STOP_JOIN, handle.client().disconnect())
        .await
        .is_err()
    {
        tracing::warn!("WhatsApp pairing client did not close in time");
    }
    handle.abort();
    let Some(outcome) = outcome else { return };
    let Some(c) = ctx.upgrade() else { return };
    let svc = &c.messaging.whatsapp;
    let PairEvent::Paired { own_jid, own_phone } = outcome else {
        svc.pair_event(&c, outcome).await;
        return;
    };
    let saved = store.export().map_err(|e| e.to_string()).and_then(|snap| {
        let conn = c.sqlite.conn().map_err(|e| e.to_string())?;
        db::save_session(
            &conn,
            &c.security,
            &snap,
            &db::Owner {
                own_jid: own_jid.as_deref(),
                own_phone: own_phone.as_deref(),
                owner_user_id,
                owner_username: owner_username.as_deref(),
            },
            c.now(),
        )
        .map_err(|e| e.to_string())
    });
    if let Err(e) = saved {
        tracing::error!("Saving the WhatsApp session failed: {}", e);
        svc.pair_event(
            &c,
            PairEvent::Failed("The WhatsApp session could not be saved. Try pairing again.".into()),
        )
        .await;
        return;
    }
    svc.logged_out.store(false, Ordering::SeqCst);
    tracing::info!(
        "WhatsApp device paired ({})",
        own_phone.as_deref().map(redact_phone).unwrap_or_default()
    );
    svc.pair_event(&c, PairEvent::Paired { own_jid, own_phone })
        .await;
    // Auto-start the bot, on its own task so this pairing task can finish.
    let weak = Arc::downgrade(&c);
    c.spawn(async move {
        if let Some(c) = weak.upgrade() {
            let (ok, msg) = c.messaging.whatsapp.start_bot(&c).await;
            tracing::info!("WhatsApp bot auto-start after pairing: {} ({})", ok, msg);
        }
    });
}

enum RunEnd {
    Stop,
    LoggedOut,
    Dropped,
}

struct Saver {
    store: Arc<SnapshotStore>,
    stored: String,
    version: u64,
    refused: bool,
}

impl Saver {
    /// Write the live session back when it changed (web `_save_session`).
    async fn save(&mut self, ctx: &AppState, require_login: bool, client: &Client) {
        if self.refused || self.store.version() == self.version {
            return;
        }
        if require_login && !client.is_logged_in() {
            return;
        }
        if !self.store.has_paired_device() {
            return;
        }
        let v = self.store.version();
        let store = self.store.clone();
        let snap = match tokio::task::spawn_blocking(move || store.export()).await {
            Ok(Ok(s)) => s,
            _ => {
                tracing::error!("Could not save the WhatsApp session; trying again later");
                return;
            }
        };
        let res = ctx
            .sqlite
            .conn()
            .and_then(|c| db::refresh_session(&c, &ctx.security, &snap, &self.stored));
        match res {
            Ok(Some(ct)) => {
                self.stored = ct;
                self.version = v;
                tracing::debug!("WhatsApp session saved");
            }
            Ok(None) => {
                tracing::info!(
                    "WhatsApp session not saved: the device is no longer paired with this session"
                );
                self.refused = true;
            }
            Err(e) => tracing::error!("Could not save the WhatsApp session: {}", e),
        }
    }
}

pub(super) async fn bot_task(
    ctx: Weak<AppState>,
    snapshot: Vec<u8>,
    stored: String,
    cancel: CancellationToken,
    ready: oneshot::Sender<bool>,
) {
    let mut ready = Some(ready);
    let store = match SnapshotStore::import(&snapshot) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            tracing::error!("The stored WhatsApp session could not be read: {}", e);
            if let Some(r) = ready.take() {
                let _ = r.send(false);
            }
            return;
        }
    };
    drop(snapshot);
    let mut saver = Saver {
        version: store.version(),
        store: store.clone(),
        stored,
        refused: false,
    };
    let base = ctx
        .upgrade()
        .map(|c| c.messaging.whatsapp.settings.read().clone())
        .unwrap_or_default();
    let mut backoff = base.reconnect_base;
    loop {
        let Some(c) = ctx.upgrade() else { return };
        let svc = &c.messaging.whatsapp;
        let (tx, mut rx) = mpsc::channel::<Raw>(256);
        let bot = match svc.build(&c, store.clone(), tx, None).await {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("WhatsApp client could not be built: {}", e);
                if let Some(r) = ready.take() {
                    let _ = r.send(false);
                    return;
                }
                drop(c);
                if sleep_or_stop(&cancel, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(base.reconnect_max);
                continue;
            }
        };
        let mut handle = bot.spawn();
        let client = handle.client();
        *svc.client.write() = Some(client.clone());
        svc.running.store(true, Ordering::SeqCst);
        svc.logged_out.store(false, Ordering::SeqCst);
        let _ = c.sqlite.conn().and_then(|cn| db::set_active(&cn, true));
        svc.emit_status(&c).await;
        if let Some(r) = ready.take() {
            let _ = r.send(true);
        }
        tracing::info!("WhatsApp bot connecting");

        // Slash commands run one at a time on a worker owned by this run.
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<(String, String, String)>(32);
        let wctx = ctx.clone();
        let worker: JoinHandle<()> = tokio::spawn(async move {
            while let Some((chat, sender, text)) = cmd_rx.recv().await {
                let Some(c) = wctx.upgrade() else { return };
                commands::dispatch(&c, &chat, &sender, &text).await;
            }
        });
        drop(c);

        let s = base.clone();
        let mut next_save = tokio::time::Instant::now() + s.first_save;
        let end = loop {
            tokio::select! {
                _ = cancel.cancelled() => break RunEnd::Stop,
                _ = &mut handle => break RunEnd::Dropped,
                ev = rx.recv() => {
                    let Some(ev) = ev else { continue };
                    let Some(c) = ctx.upgrade() else { break RunEnd::Stop };
                    match ev {
                        Raw::Connected => {
                            backoff = s.reconnect_base;
                            next_save = next_save.min(tokio::time::Instant::now() + s.first_save);
                            if let Some(own) = client.get_pn() {
                                let jid = own.to_non_ad_string();
                                let _ = c.sqlite.conn().and_then(|cn| {
                                    db::persist_owner_identity(&cn, &jid, &jid_to_phone(&jid))
                                });
                            }
                            let restored = !c.messaging.whatsapp.running.swap(true, Ordering::SeqCst);
                            if restored {
                                let _ = c.sqlite.conn().and_then(|cn| db::set_active(&cn, true));
                                tracing::info!("WhatsApp connection is back");
                                c.messaging.whatsapp.emit_status(&c).await;
                            }
                        }
                        Raw::LoggedOut => break RunEnd::LoggedOut,
                        Raw::Message { is_from_me, is_group, sender, chat, text } => {
                            let text = text.trim().to_string();
                            if !text.starts_with('/') {
                                continue;
                            }
                            let own: Vec<String> = [client.get_pn(), client.get_lid()]
                                .into_iter()
                                .flatten()
                                .map(|j| j.to_non_ad_string())
                                .collect();
                            match route_command(is_from_me, is_group, &chat, &own, &c.messaging.whatsapp_replies, c.now()) {
                                WaRoute::Dispatch => {}
                                WaRoute::Drop => continue,
                                WaRoute::Reply(m) => {
                                    tracing::debug!("WhatsApp command from someone other than the owner refused");
                                    let to = vec![chat.clone()];
                                    let text = m.to_string();
                                    let weak = ctx.clone();
                                    c.spawn(async move {
                                        if let Some(c) = weak.upgrade() {
                                            c.messaging.whatsapp.send(&c, &to, &text).await;
                                        }
                                    });
                                    continue;
                                }
                            }
                            if cmd_tx.try_send((chat, sender, text)).is_err() {
                                tracing::warn!("WhatsApp command dropped: too many waiting");
                            }
                        }
                        _ => {}
                    }
                }
                _ = tokio::time::sleep_until(next_save) => {
                    if let Some(c) = ctx.upgrade() {
                        saver.save(&c, true, &client).await;
                    }
                    next_save = tokio::time::Instant::now() + s.save_interval;
                }
            }
        };
        drop(cmd_tx);
        worker.abort();
        let _ = worker.await;
        let Some(c) = ctx.upgrade() else {
            handle.abort();
            return;
        };
        let svc = &c.messaging.whatsapp;
        *svc.client.write() = None;
        match end {
            RunEnd::Stop => {
                // Save while connected, then close (which flushes the device
                // state), then save what the close flushed.
                saver.save(&c, true, &client).await;
                if tokio::time::timeout(STOP_JOIN, client.disconnect())
                    .await
                    .is_err()
                {
                    tracing::warn!("WhatsApp client did not close in time");
                }
                handle.abort();
                saver.save(&c, false, &client).await;
                svc.running.store(false, Ordering::SeqCst);
                let _ = c.sqlite.conn().and_then(|cn| db::set_active(&cn, false));
                svc.emit_status(&c).await;
                return;
            }
            RunEnd::LoggedOut => {
                handle.abort();
                svc.running.store(false, Ordering::SeqCst);
                let cleared = c
                    .sqlite
                    .conn()
                    .and_then(|cn| db::clear_rejected(&cn, &saver.stored))
                    .unwrap_or(false);
                if cleared || !saver.refused {
                    svc.logged_out.store(true, Ordering::SeqCst);
                    tracing::warn!("{}", LOGGED_OUT_MESSAGE);
                } else {
                    tracing::info!("WhatsApp logged out a session that a new pairing has replaced");
                }
                let _ = c.sqlite.conn().and_then(|cn| db::set_active(&cn, false));
                svc.emit_status(&c).await;
                return;
            }
            RunEnd::Dropped => {
                handle.abort();
                svc.running.store(false, Ordering::SeqCst);
                svc.emit_status(&c).await;
                tracing::warn!("WhatsApp connection ended; reconnecting");
                drop(c);
                if sleep_or_stop(&cancel, backoff).await {
                    if let Some(c) = ctx.upgrade() {
                        let _ = c.sqlite.conn().and_then(|cn| db::set_active(&cn, false));
                    }
                    return;
                }
                backoff = (backoff * 2).min(base.reconnect_max);
            }
        }
    }
}

async fn sleep_or_stop(cancel: &CancellationToken, d: Duration) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => true,
        _ = tokio::time::sleep(d) => false,
    }
}

//! Which broker session a Live obligation belongs to (LOG-07, defence in
//! depth; docs/codex-evaluation 90-plus-roadmap, "Broker switching").
//!
//! Every Live obligation records the broker, and the account at that broker,
//! it was opened on: a strategy run (`sm_strategy_run.broker` and
//! `broker_account`), an OpenScript run (`RunInfo::owner`), a scalping stop
//! (`scalping_sl_state.broker` and `broker_account`) and an order a Live run
//! queued in the Action Center (`pending_orders.broker` and
//! `broker_account`). Before an order for it is sent, the dispatch layer
//! compares that with the session connected now, and refuses without
//! sending when they differ. Nothing was sent, so under LOG-08 it is a
//! definite refusal: the claim is released and the obligation stays managed
//! for when its own session is back.
//!
//! The broker switch guard (`services::broker_switch`) already refuses a
//! switch while obligations remain; this second layer holds when a session
//! changes some other way, and acknowledging a warning never changes it.
//! One key set per broker (several accounts of one broker are out of scope),
//! but the account is still compared, so a session that comes back on
//! another account is caught too.

use crate::state::AppState;
use crate::strategy::dispatch::{OrderGateway, RunMode};

/// The broker session a Live obligation was opened on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LiveOwner {
    /// The broker id (`zerodha`); blank when it was not recorded.
    pub broker: String,
    /// The account at the broker (its client id); blank when unknown.
    pub account: String,
}

impl LiveOwner {
    pub fn new(broker: impl Into<String>, account: impl Into<String>) -> Self {
        Self {
            broker: broker.into().trim().to_string(),
            account: account.into().trim().to_string(),
        }
    }

    /// The session connected now, if any.
    pub fn of_session(ctx: &AppState) -> Option<Self> {
        ctx.get_broker_session()
            .map(|s| Self::new(s.broker_id, s.user_id))
    }

    /// The session `gateway` sends `mode` orders through now: what a new
    /// obligation records (Sandbox: `sandbox`, no account).
    pub fn of_gateway(gateway: &dyn OrderGateway, mode: RunMode) -> Self {
        Self::new(gateway.broker_name(mode), gateway.broker_account(mode))
    }

    /// The live session `gateway` sends through now, if there is one.
    pub fn active(gateway: &dyn OrderGateway) -> Option<Self> {
        Some(Self::of_gateway(gateway, RunMode::Live)).filter(|o| !o.broker.is_empty())
    }

    /// The broker as a trader knows it, with the account when known:
    /// "Zerodha (AB1234)".
    pub fn describe(&self) -> String {
        let name = crate::brokers::setup_guide::display_name(&self.broker);
        if self.account.is_empty() {
            name
        } else {
            format!("{} ({})", name, self.account)
        }
    }

    /// Whether the broker was recorded. A blank one, or the mode name a
    /// signal run writes when there was no session, says nothing.
    fn knows_broker(&self) -> bool {
        !self.broker.is_empty()
            && !self.broker.eq_ignore_ascii_case("live")
            && !self.broker.eq_ignore_ascii_case("sandbox")
    }

    /// Why an order for an obligation opened on `self` must not go through
    /// `active`, the session connected now, in a trader's words; `None`
    /// when it may. Only what both sides know is compared: an obligation
    /// recorded before owners were (no broker) or an account a broker does
    /// not report is not a mismatch. No session at all is refused by the
    /// caller's own session check.
    pub fn refusal(&self, active: Option<&LiveOwner>) -> Option<String> {
        let active = active?;
        let other_broker = self.knows_broker() && !self.broker.eq_ignore_ascii_case(&active.broker);
        let other_account = !self.account.is_empty()
            && !active.account.is_empty()
            && !self.account.eq_ignore_ascii_case(&active.account);
        if !other_broker && !other_account {
            return None;
        }
        let mine = self.describe();
        Some(format!(
            "This was opened on {}, but OpenAlgo is signed in to {} now, so the order was not sent. Sign in to {} to manage it.",
            mine,
            active.describe(),
            mine
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_another_broker_or_account_is_refused() {
        let zerodha = LiveOwner::new("zerodha", "AB1234");
        assert_eq!(zerodha.refusal(Some(&zerodha)), None);
        assert_eq!(zerodha.refusal(None), None, "no session: the caller says");
        let why = zerodha
            .refusal(Some(&LiveOwner::new("angel", "XY9876")))
            .unwrap();
        assert_eq!(
            why,
            "This was opened on Zerodha (AB1234), but OpenAlgo is signed in to Angel One (XY9876) now, so the order was not sent. Sign in to Zerodha (AB1234) to manage it."
        );
        assert!(zerodha
            .refusal(Some(&LiveOwner::new("zerodha", "ZZ0001")))
            .is_some());
        // What was not recorded is not compared.
        let legacy = LiveOwner::new("zerodha", "");
        assert_eq!(legacy.refusal(Some(&zerodha)), None);
        assert!(legacy.refusal(Some(&LiveOwner::new("angel", ""))).is_some());
        for unknown in ["", "live", "sandbox"] {
            let o = LiveOwner::new(unknown, "");
            assert_eq!(o.refusal(Some(&zerodha)), None, "{:?}", unknown);
        }
        assert_eq!(
            zerodha.refusal(Some(&LiveOwner::new("ZERODHA", "ab1234"))),
            None
        );
    }
}

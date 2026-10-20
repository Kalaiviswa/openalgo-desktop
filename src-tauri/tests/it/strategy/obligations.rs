//! LOG-07: the open Live obligations a broker switch must not leave behind
//! (`services::obligations`), counted from the stores alone.

use super::*;
use openalgo_desktop_lib::services::obligations::open_live_obligations;

#[tokio::test]
async fn nothing_open_is_empty_and_sandbox_runs_do_not_count() {
    let a = app();
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert!(o.is_empty(), "{:?}", o);
    // A Sandbox run keeps running on the next broker's quotes.
    let cfg = config("Sandbox", json!([short_call_leg()]), json!({}));
    let (row, _) = a.ctx.strategy.store.create_strategy(USER, &cfg).unwrap();
    let r = a
        .ctx
        .strategy
        .start_run(row.id, USER, "sandbox", "manual", None)
        .await;
    assert!(r.ok, "{:?}", r);
    assert!(open_live_obligations(&a.ctx, "zerodha").unwrap().is_empty());
}

#[tokio::test]
async fn a_live_run_and_its_unconfirmed_exit_are_open_obligations() {
    use openalgo_desktop_lib::brokers::mock::AfterSend;
    let a = app();
    a.ctx.reconciler.stop();
    let cfg = config("Live", json!([short_call_leg()]), json!({}));
    let (row, _) = a.ctx.strategy.store.create_strategy(USER, &cfg).unwrap();
    a.ctx
        .strategy
        .store
        .set_live_enabled(row.id, USER, true)
        .unwrap();
    a.ctx.sqlite.set_analyze_mode(false).unwrap();
    let r = a
        .ctx
        .strategy
        .start_run(row.id, USER, "live", "manual", None)
        .await;
    assert!(r.ok, "{:?}", r);
    let run = r.run_id.unwrap();
    a.ctx
        .strategy
        .apply_fill(run, 1, Some(100.0), true, FillOpts::default())
        .await;
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.strategy_runs, 1, "{:?}", o);
    assert_eq!(o.uncertain_placements, 0);
    // Another broker's obligations are not this one's.
    assert_eq!(
        open_live_obligations(&a.ctx, "angel")
            .unwrap()
            .strategy_runs,
        0
    );
    // An exit whose answer was lost is an uncertain placement.
    a.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "open"));
    a.ctx.strategy.process_tick(ATM_CE, "NFO", 121.0).await;
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.uncertain_placements, 1, "{:?}", o);
    assert!(!o.is_empty());
}

#[tokio::test]
async fn live_scalping_stops_are_open_obligations_and_sandbox_ones_are_not() {
    use openalgo_desktop_lib::scalping::store::SlUpsert;
    let a = app();
    let stop = |mode: &str| SlUpsert {
        symbol: ATM_CE.into(),
        exchange: "NFO".into(),
        product: "NRML".into(),
        mode: mode.into(),
        side: Some("SELL".into()),
        entry_price: Some(100.0),
        quantity: Some(65),
        initial_sl: Some(120.0),
        is_active: Some(true),
        ..Default::default()
    };
    a.ctx.scalping.store.upsert_sl(&stop("analyze")).unwrap();
    assert!(open_live_obligations(&a.ctx, "zerodha").unwrap().is_empty());
    a.ctx.scalping.store.upsert_sl(&stop("live")).unwrap();
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.scalping_stops, 1, "{:?}", o);
    // Exit orders stored on the stop that cannot be read back count as
    // uncertain, never as settled.
    let id = a.ctx.scalping.store.active_sl(Some("live")).unwrap()[0].id;
    a.ctx
        .scalping
        .store
        .set_exit_attempt(id, Some("{not json"))
        .unwrap();
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.uncertain_placements, 1, "{:?}", o);
}

// ------------------------------------------- the switch guard, end to end
//
// Roadmap "Broker switching": one test per row, against the real guard
// (`services::obligations::OutcomesGuard`, installed by `AppState::new`)
// and the real stores. The switch is the Broker page's save that makes
// Angel One the active broker while Zerodha is logged in.

use axum::http::StatusCode;
use openalgo_desktop_lib::events::OrderUpdate;

async fn switch_to_angel(a: &App) -> (StatusCode, Value) {
    a.post("/api/broker/credentials", json!({"broker": "angel"}))
        .await
}

/// The switch was refused with a reason naming `says`, and nothing changed.
fn refused(a: &App, (s, v): (StatusCode, Value), says: &[&str]) -> String {
    assert_eq!(s, StatusCode::CONFLICT, "{}", v);
    let why = v["message"].as_str().unwrap_or_default().to_string();
    for part in says {
        assert!(why.contains(part), "{}", why);
    }
    assert!(why.contains("Nothing was changed."), "{}", why);
    assert_eq!(
        a.ctx.get_broker_session().map(|s| s.broker_id).as_deref(),
        Some("zerodha"),
        "the Zerodha session stays"
    );
    why
}

/// The switch went ahead: Angel One is the active broker and the Zerodha
/// session ended.
fn switched(a: &App, (s, v): (StatusCode, Value)) {
    assert_eq!(s, StatusCode::OK, "{}", v);
    assert_eq!(v["broker_switched"], true, "{}", v);
    assert!(
        a.ctx.get_broker_session().is_none(),
        "Zerodha's session ended"
    );
}

/// A Live run of one short call, its entry filled at 100.
async fn live_run(a: &App) -> i64 {
    let cfg = config("Live", json!([short_call_leg()]), json!({}));
    let (row, _) = a.ctx.strategy.store.create_strategy(USER, &cfg).unwrap();
    a.ctx
        .strategy
        .store
        .set_live_enabled(row.id, USER, true)
        .unwrap();
    a.ctx.sqlite.set_analyze_mode(false).unwrap();
    let r = a
        .ctx
        .strategy
        .start_run(row.id, USER, "live", "manual", None)
        .await;
    assert!(r.ok, "{:?}", r);
    let run = r.run_id.unwrap();
    a.ctx
        .strategy
        .apply_fill(run, 1, Some(100.0), true, FillOpts::default())
        .await;
    run
}

/// The broker's id for the exit the run's stop sent.
fn exit_of(a: &App, run: i64) -> String {
    a.ctx
        .strategy
        .store
        .list_orders(run)
        .unwrap()
        .into_iter()
        .find(|o| o.kind != "entry")
        .and_then(|o| o.broker_order_id)
        .expect("the exit was accepted")
}

/// The broker's update for the exit `exit`: `filled` of 65 at 95.
fn exit_frame(exit: &str, status: &str, filled: i64) -> OrderUpdate {
    OrderUpdate {
        orderid: exit.into(),
        order_status: status.into(),
        filled_quantity: filled,
        average_price: 95.0,
        ..Default::default()
    }
}

fn run_stopped(a: &App, run: i64) -> bool {
    a.ctx
        .strategy
        .store
        .get_run(run)
        .unwrap()
        .unwrap()
        .stopped_at
        .is_some()
}

/// Defence in depth (roadmap "Broker switching"): the dispatch layer
/// refuses an exit for a Live run opened on Zerodha (AB1234) while another
/// account's session is connected, however that session got there (here it
/// is installed directly, past the switch guard). Nothing is sent, the run
/// stays open and managed, and the trader is told once. Back on its own
/// session, the stop exits it.
#[tokio::test]
async fn dispatch_refuses_an_a_owned_exit_while_b_is_active() {
    use openalgo_desktop_lib::brokers::mock::MockCall;
    use openalgo_desktop_lib::state::BrokerSession;
    let a = app();
    a.ctx.reconciler.stop();
    let run = live_run(&a).await;
    let placed = || {
        a.mock
            .calls()
            .iter()
            .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
            .count()
    };
    let entry_only = placed();
    let own = a.ctx.get_broker_session().unwrap();
    a.ctx.set_broker_session(Some(BrokerSession {
        user_id: "XY9876".into(),
        ..own.clone()
    }));
    // The short call's stop is 20 points over its 100 entry.
    a.ctx.strategy.process_tick(ATM_CE, "NFO", 121.0).await;
    assert_eq!(placed(), entry_only, "no exit through XY9876");
    let out = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(!out.ok && out.stop_pending, "{:?}", out);
    assert!(
        out.error
            .as_deref()
            .is_some_and(|e| e.contains("opened on Zerodha (AB1234)")),
        "{:?}",
        out
    );
    assert_eq!(placed(), entry_only, "no stop exit through XY9876");
    assert!(!run_stopped(&a, run), "still open and managed");
    let sid = a
        .ctx
        .strategy
        .store
        .get_run(run)
        .unwrap()
        .unwrap()
        .strategy_id;
    let told = a
        .ctx
        .strategy
        .store
        .list_events(sid, Some(run), None, Some("critical"), 100)
        .unwrap();
    assert_eq!(
        told.iter()
            .filter(|e| e["message"]
                .as_str()
                .is_some_and(|m| m.contains("signed in to Zerodha (XY9876)")))
            .count(),
        1,
        "told once: {:?}",
        told
    );

    a.ctx.set_broker_session(Some(own));
    let out = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(out.ok, "{:?}", out);
    assert_eq!(placed(), entry_only + 1, "the exit went out through AB1234");
}

/// Lead's third review of #165, P2-2: a run recorded before accounts were
/// derived from the saved keys carries the broker's placeholder as its
/// account (Samco answered "samco"). The same broker's session, now told by
/// its saved keys, still manages it: the stop's exit goes out. Before, the
/// two different values refused it, with a message no sign-in could meet.
#[tokio::test]
async fn a_run_recorded_with_a_placeholder_account_is_still_managed_by_its_brokers_keys() {
    use openalgo_desktop_lib::brokers::mock::MockCall;
    use openalgo_desktop_lib::db::sqlite::credentials::{self, CredentialUpdate};
    use openalgo_desktop_lib::state::BrokerSession;
    let a = app();
    a.ctx.reconciler.stop();
    let run = live_run(&a).await;
    let conn = a.ctx.sqlite.conn().unwrap();
    conn.execute(
        "UPDATE sm_strategy_run SET broker_account = 'zerodha' WHERE id = ?1",
        [run],
    )
    .unwrap();
    credentials::save(
        &conn,
        &a.ctx.security,
        "zerodha",
        CredentialUpdate {
            api_key: Some("kitekey".into()),
            api_secret: Some("kitesecret".into()),
            ..Default::default()
        },
    )
    .unwrap();
    drop(conn);
    let own = a.ctx.get_broker_session().unwrap();
    a.ctx.set_broker_session(Some(BrokerSession {
        user_id: String::new(),
        ..own
    }));
    let placed = || {
        a.mock
            .calls()
            .iter()
            .filter(|c| matches!(c, MockCall::PlaceOrder(_)))
            .count()
    };
    let before = placed();
    let out = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(out.ok, "{:?}", out);
    assert_eq!(placed(), before + 1, "the exit went out");
}

/// Row 1: a running Live strategy refuses the switch, named, until it is
/// stopped.
#[tokio::test]
async fn switch_row1_a_live_strategy_run_refuses_the_switch_until_it_is_stopped() {
    let a = app();
    a.ctx.reconciler.stop();
    let run = live_run(&a).await;
    refused(
        &a,
        switch_to_angel(&a).await,
        &[
            "Zerodha still has 1 running Live strategy.",
            "stop the strategies and protection",
        ],
    );
    let out = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(out.ok, "{:?}", out);
    // Stopping: the run is open until its exit fills.
    refused(
        &a,
        switch_to_angel(&a).await,
        &["Zerodha still has 1 running Live strategy."],
    );
    let exit = exit_of(&a, run);
    a.ctx
        .strategy
        .apply_update(&exit, exit_frame(&exit, "complete", 65))
        .await;
    assert!(run_stopped(&a, run), "the exit's fill closed the run");
    switched(&a, switch_to_angel(&a).await);
}

/// Row 1: Live scalping protection refuses the switch, named, until the
/// stop is removed. A Sandbox stop never counts.
#[tokio::test]
async fn switch_row1_live_scalping_protection_refuses_the_switch_until_it_is_removed() {
    use openalgo_desktop_lib::scalping::store::SlUpsert;
    let a = app();
    let stop = |mode: &str| SlUpsert {
        symbol: ATM_CE.into(),
        exchange: "NFO".into(),
        product: "NRML".into(),
        mode: mode.into(),
        side: Some("SELL".into()),
        entry_price: Some(100.0),
        quantity: Some(65),
        initial_sl: Some(120.0),
        is_active: Some(true),
        ..Default::default()
    };
    a.ctx.scalping.store.upsert_sl(&stop("analyze")).unwrap();
    a.ctx.scalping.store.upsert_sl(&stop("live")).unwrap();
    refused(
        &a,
        switch_to_angel(&a).await,
        &["Zerodha still has 1 position under scalping protection"],
    );
    assert!(a
        .ctx
        .scalping
        .store
        .delete_sl(ATM_CE, "NFO", "NRML", Some("live"))
        .unwrap());
    switched(&a, switch_to_angel(&a).await);
}

/// Row 2: an exit whose answer was lost refuses the switch until the order
/// reconciler settles it from the broker's book.
#[tokio::test]
async fn switch_row2_an_order_with_an_unknown_outcome_refuses_the_switch_until_reconciled() {
    use openalgo_desktop_lib::brokers::mock::AfterSend;
    let a = app();
    a.ctx.reconciler.stop();
    let run = live_run(&a).await;
    // The broker takes the exit and fills it; the answer is lost.
    *a.mock.after_send_price.lock() = 121.0;
    *a.mock.after_send_time.lock() = "2026-10-07 10:00:05".into();
    a.mock
        .after_send
        .lock()
        .push_back((AfterSend::Dropped, "complete"));
    a.ctx.strategy.process_tick(ATM_CE, "NFO", 121.0).await;
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.uncertain_placements, 1, "{:?}", o);
    refused(
        &a,
        switch_to_angel(&a).await,
        &[
            "1 order whose outcome is not known yet",
            "check the order book until every order is settled",
        ],
    );
    // The reconciler finds the exit, filled, in the book: the leg and the
    // run close, and nothing is left to settle.
    a.ctx.reconciler.pass().await;
    assert!(run_stopped(&a, run), "the fill closed the run");
    assert!(open_live_obligations(&a.ctx, "zerodha").unwrap().is_empty());
    switched(&a, switch_to_angel(&a).await);
}

/// Row 2: an order from the scalping page whose answer was lost refuses
/// the switch until it is settled; a Sandbox one never counts (LOG-08).
#[tokio::test]
async fn switch_row2_a_scalping_page_order_with_an_unknown_outcome_refuses_the_switch() {
    use openalgo_desktop_lib::scalping::unconfirmed::{record, NewPending};
    let a = app();
    a.ctx.reconciler.stop();
    let order = |mode| NewPending {
        mode,
        kind: "entry",
        symbol: ATM_CE,
        exchange: "NFO",
        product: "NRML",
        action: "BUY",
        quantity: 65,
        client_tag: None,
    };
    record(&a.ctx.sqlite, &order("analyze"), a.ctx.now()).unwrap();
    assert!(open_live_obligations(&a.ctx, "zerodha").unwrap().is_empty());
    let id = record(&a.ctx.sqlite, &order("live"), a.ctx.now()).unwrap();
    refused(
        &a,
        switch_to_angel(&a).await,
        &[
            "Zerodha still has 1 order whose outcome is not known yet.",
            "check the order book until every order is settled",
        ],
    );
    // Settled (here: found in the book and bound to its order).
    a.ctx
        .sqlite
        .conn()
        .unwrap()
        .execute(
            "UPDATE scalping_unconfirmed SET status = 'placed', orderid = 'OID1' WHERE id = ?1",
            [id],
        )
        .unwrap();
    switched(&a, switch_to_angel(&a).await);
}

/// Row 3: an exit partly filled refuses the switch until it ends.
#[tokio::test]
async fn switch_row3_an_exit_still_filling_refuses_the_switch_until_it_ends() {
    let a = app();
    a.ctx.reconciler.stop();
    let run = live_run(&a).await;
    let out = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(out.ok, "{:?}", out);
    let exit = exit_of(&a, run);
    let frame = |status: &str, filled: i64| exit_frame(&exit, status, filled);
    a.ctx.strategy.apply_update(&exit, frame("open", 25)).await;
    let o = open_live_obligations(&a.ctx, "zerodha").unwrap();
    assert_eq!(o.partial_exits, 1, "{:?}", o);
    refused(
        &a,
        switch_to_angel(&a).await,
        &["1 exit still filling", "wait for the exits to finish"],
    );
    a.ctx
        .strategy
        .apply_update(&exit, frame("complete", 65))
        .await;
    assert!(run_stopped(&a, run), "the last fill closed the run");
    switched(&a, switch_to_angel(&a).await);
}

/// Row 4: a working order and a position placed by hand do not block the
/// switch; they stay at Zerodha, untouched.
#[tokio::test]
async fn switch_row4_manual_orders_and_positions_do_not_block_and_stay() {
    use openalgo_desktop_lib::brokers::mock::MockCall;
    let a = app();
    a.ctx.sqlite.set_analyze_mode(false).unwrap();
    for (pricetype, price) in [("MARKET", 0.0), ("LIMIT", 900.0)] {
        let (s, v) = a
            .api(
                "/api/v1/placeorder",
                json!({"strategy": "manual", "symbol": "SBIN", "exchange": "NSE",
                       "action": "BUY", "quantity": "1", "pricetype": pricetype,
                       "product": "CNC", "price": price.to_string()}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{}", v);
    }
    assert!(open_live_obligations(&a.ctx, "zerodha").unwrap().is_empty());
    let before = a.mock.calls().len();
    switched(&a, switch_to_angel(&a).await);
    assert!(
        !a.mock.calls()[before..].iter().any(|c| matches!(
            c,
            MockCall::PlaceOrder(_)
                | MockCall::CancelOrder(_)
                | MockCall::CancelAll
                | MockCall::CloseAll
        )),
        "nothing is placed, cancelled or closed at Zerodha"
    );
}

/// Row 5: a Sandbox run does not block the switch and keeps running.
#[tokio::test]
async fn switch_row5_sandbox_runs_do_not_block_and_keep_running() {
    let a = app();
    let cfg = config("Sandbox", json!([short_call_leg()]), json!({}));
    let (row, _) = a.ctx.strategy.store.create_strategy(USER, &cfg).unwrap();
    let r = a
        .ctx
        .strategy
        .start_run(row.id, USER, "sandbox", "manual", None)
        .await;
    assert!(r.ok, "{:?}", r);
    let run = r.run_id.unwrap();
    switched(&a, switch_to_angel(&a).await);
    assert!(!run_stopped(&a, run), "the Sandbox run keeps running");
}

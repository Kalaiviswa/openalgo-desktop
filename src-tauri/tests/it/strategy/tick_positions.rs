//! A tick path exit closes only the position it was decided on
//! (PROD-REL-01): acting comes after deciding, and a leg can be re-entered
//! in between.

use super::*;
use openalgo_desktop_lib::strategy::signals::SignalResult;
use std::sync::Arc;

async fn signal(t: &T, sid: i64, action: &str, leg: i64) -> SignalResult {
    let s = t.m.store.get_strategy(sid, USER).unwrap().unwrap();
    t.m.handle_signal(&s, action, Some(&json!(leg)), None, None)
        .await
}

/// The kinds of one leg's orders, in the order they were recorded.
fn leg_kinds(t: &T, run: i64, leg: i64) -> Vec<String> {
    t.orders(run)
        .into_iter()
        .filter(|o| o.leg_id == leg)
        .map(|o| o.kind)
        .collect()
}

/// Legs 1 and 2, both long RELIANCE at 1000 with a 20-point stop, breach
/// on one price. While leg 1's exit is with the broker, a webhook flips leg
/// 2 to short and both of the flip's orders fill. Leg 2's exit was decided
/// for the long it no longer holds, so it must not close the new short.
/// Before, leg 2's position was checked once, before leg 1's exit was
/// placed, and the leg was claimed by its id afterwards: the new short was
/// bought back as a stop exit.
#[tokio::test]
async fn a_leg_re_entered_while_its_exit_waits_keeps_its_new_position() {
    let t = t();
    let sid = t.make(signal_config(
        json!([
            signal_leg(1, "RELIANCE", "both"),
            signal_leg(2, "RELIANCE", "both")
        ]),
        json!({}),
    ));
    assert!(signal(&t, sid, "long_entry", 1).await.acted());
    assert!(signal(&t, sid, "long_entry", 2).await.acted());
    let run = t.row(sid).current_run_id.unwrap();
    for leg in [1, 2] {
        t.m.apply_fill(run, leg, Some(1000.0), true, FillOpts::default())
            .await;
    }
    let decided_on = t.leg(run, 2).position_ref;

    // Leg 1's exit is held at the broker; leg 2's waits behind it.
    let gate = Arc::new(Gate::default());
    *t.gw.place_gate.lock() = Some(gate.clone());
    let m = t.m.clone();
    let acting = tokio::spawn(async move { m.process_tick("RELIANCE", "NSE", 970.0).await });
    gate.entered().await;
    *t.gw.place_gate.lock() = None;

    let flip = signal(&t, sid, "short_entry", 2).await;
    assert!(flip.ok && flip.flipped, "{:?}", flip);
    let rows: Vec<_> = t
        .orders(run)
        .into_iter()
        .filter(|o| o.leg_id == 2)
        .collect();
    let square = rows.iter().find(|o| o.kind == "exit_signal").unwrap();
    let new_entry = rows.iter().rev().find(|o| o.kind == "entry").unwrap();
    for o in [new_entry, square] {
        t.frame(
            o.broker_order_id.as_deref().unwrap(),
            "complete",
            o.qty,
            975.0,
        )
        .await;
    }
    let short = t.leg(run, 2);
    assert_eq!(
        (short.position.as_str(), short.status.as_str()),
        ("S", "open")
    );
    assert!(short.superseded.is_none(), "the outgoing long settled");
    assert_ne!(short.position_ref, decided_on);

    gate.release();
    acting.await.unwrap();
    assert_eq!(leg_kinds(&t, run, 1), vec!["entry", "exit_sl"]);
    assert_eq!(
        leg_kinds(&t, run, 2),
        vec!["entry", "exit_signal", "entry"],
        "the new short was not closed by the long's stop"
    );
    let short = t.leg(run, 2);
    assert_eq!(short.status, "open");
    assert!(!short.exit_in_flight());
}

//! Claiming and firing GTT legs (web `sandbox/gtt_manager.py` trigger
//! handling): one caller claims a leg (OCO exclusivity), fires its order,
//! and the active legs are checked against each price.

use super::{active_legs, get_gtt, get_leg, leg_is_triggered_by, legs_of, GttRow, LegRow};
use crate::sandbox::clock::{gtt_ts, ts};
use crate::sandbox::config::SandboxConfig;
use crate::sandbox::core::{blocking, Core};
use crate::sandbox::events::{gtt_triggered, Outbox};
use crate::sandbox::funds;
use crate::sandbox::locks::PositionKey;
use crate::sandbox::orders::OrderRequest;
use crate::sandbox::placement;
use crate::sandbox::quotes::{quote_is_old, Quote};
use crate::sandbox::types::{dec_to_db, money, SandboxError, SbResult, SymbolKey};
use chrono::NaiveDateTime;
use rusqlite::{params, Connection};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::sync::Arc;

/// Claim a leg for firing: one conditional UPDATE, refused when the leg is
/// not pending, a sibling is claimed or fired (OCO exclusivity), or the
/// parent is no longer active. Exactly one caller wins.
pub fn try_claim(conn: &Connection, leg_id: i64, now: NaiveDateTime) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE sandbox_gtt_legs SET leg_status = 'triggering', claimed_at = ?1
         WHERE id = ?2 AND leg_status = 'pending'
           AND NOT EXISTS (
             SELECT 1 FROM sandbox_gtt_legs s
             WHERE s.gtt_id = (SELECT gtt_id FROM sandbox_gtt_legs WHERE id = ?2)
               AND s.id != ?2 AND s.leg_status IN ('triggering','triggered'))
           AND EXISTS (
             SELECT 1 FROM sandbox_gtt g
             WHERE g.gtt_id = (SELECT gtt_id FROM sandbox_gtt_legs WHERE id = ?2)
               AND g.gtt_status = 'active')",
        params![gtt_ts(now), leg_id],
    )?;
    Ok(n == 1)
}

fn revert_claim(conn: &Connection, leg_id: i64) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sandbox_gtt_legs SET leg_status = 'pending', claimed_at = NULL
         WHERE id = ?1 AND leg_status = 'triggering'",
        params![leg_id],
    )?;
    Ok(())
}

pub(crate) async fn claim(core: &Arc<Core>, leg_id: i64) -> SbResult<bool> {
    blocking(core, move |c| {
        let now = c.now();
        Ok(c.db.with_tx(|tx| try_claim(tx, leg_id, now))?)
    })
    .await
}

/// Take the parent and release its reservation in one transaction: the
/// margin released, or `None` (the claim handed back) when the parent is
/// no longer active or its margin could not be released.
async fn release_reservation(
    core: &Arc<Core>,
    gtt_id: String,
    leg_id: i64,
) -> SbResult<Option<Decimal>> {
    blocking(core, move |c| {
        let r: SbResult<Option<Decimal>> = c.db.with_tx(|tx| {
            let now = c.now();
            let n = tx.execute(
                "UPDATE sandbox_gtt SET gtt_status = 'triggered', updated_at = ?1
                 WHERE gtt_id = ?2 AND gtt_status = 'active'",
                params![gtt_ts(now), gtt_id],
            )?;
            if n != 1 {
                revert_claim(tx, leg_id)?;
                return Ok(None);
            }
            let Some(g) = get_gtt(tx, &gtt_id)? else {
                return Ok(None);
            };
            let released = g.margin_blocked;
            if released > Decimal::ZERO {
                let cfg = SandboxConfig::load(tx)?;
                let staged = funds::apply(tx, &g.user_id, cfg.starting_capital, &ts(now), |f| {
                    funds::margin_delta(f, -released)
                })?;
                if let Err(r) = staged {
                    return Err(SandboxError::internal(r.0));
                }
                tx.execute(
                    "UPDATE sandbox_gtt SET margin_blocked = '0' WHERE gtt_id = ?1",
                    params![gtt_id],
                )?;
            }
            Ok(Some(released))
        });
        match r {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!(
                    "GTT {} not fired: its margin could not be released ({})",
                    gtt_id,
                    e.message
                );
                c.db.with_tx(|tx| revert_claim(tx, leg_id))?;
                Ok(None)
            }
        }
    })
    .await
}

/// The order a fired leg places (strategy "GTT" when the GTT has none).
fn leg_order(gtt: &GttRow, leg: &LegRow) -> OrderRequest {
    OrderRequest {
        symbol: gtt.symbol.clone(),
        exchange: gtt.exchange.clone(),
        action: leg.action.clone(),
        quantity: leg.quantity,
        price: Some(leg.price),
        trigger_price: None,
        price_type: leg.pricetype.clone(),
        product: leg.product.clone(),
        strategy: gtt
            .strategy
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "GTT".to_string()),
    }
}

/// A fired leg's order was placed: the leg is marked triggered with it, the
/// other leg of a two-leg GTT is cancelled, and the triggered event goes
/// out once that commits.
async fn record_fired(
    core: &Arc<Core>,
    gtt: &GttRow,
    leg_id: i64,
    orderid: String,
    price: Decimal,
) -> SbResult<bool> {
    let gtt_id = gtt.gtt_id.clone();
    let two_leg = gtt.trigger_type == "two-leg";
    let g2 = gtt.clone();
    blocking(core, move |c| {
        let mut outbox = Outbox::new();
        c.db.with_tx(|tx| -> SbResult<()> {
            let now = gtt_ts(c.now());
            tx.execute(
                "UPDATE sandbox_gtt_legs SET leg_status = 'triggered', triggered_order_id = ?1,
                           claimed_at = NULL, updated_at = ?2 WHERE id = ?3",
                params![orderid, now, leg_id],
            )?;
            if two_leg {
                tx.execute(
                    "UPDATE sandbox_gtt_legs SET leg_status = 'cancelled', claimed_at = NULL, updated_at = ?1
                             WHERE gtt_id = ?2 AND id != ?3 AND leg_status IN ('pending','triggering')",
                    params![now, gtt_id, leg_id],
                )?;
            }
            let prices: Vec<f64> = legs_of(tx, &gtt_id)?
                .iter()
                .map(|l| money(l.trigger_price))
                .collect();
            outbox.push(gtt_triggered(
                &g2.gtt_id,
                &g2.symbol,
                &g2.exchange,
                g2.strategy.as_deref().unwrap_or(""),
                &prices,
                &orderid,
            ));
            Ok(())
        })?;
        c.publish(outbox);
        tracing::info!("Sandbox GTT {} fired at {}; order {}", gtt_id, price, orderid);
        Ok(true)
    })
    .await
}

/// A fired leg's order was refused: the margin goes back and the GTT is
/// active again (rejected when its margin cannot come back), and the
/// claim is handed back.
async fn restore_after_refusal(
    core: &Arc<Core>,
    gtt_id: String,
    leg_id: i64,
    released: Decimal,
) -> SbResult<bool> {
    blocking(core, move |c| {
        c.db.with_tx(|tx| -> SbResult<()> {
            let cfg = SandboxConfig::load(tx)?;
            let now = c.now();
            let mut restored = true;
            if released > Decimal::ZERO {
                let r = funds::apply(tx, c.user(), cfg.starting_capital, &ts(now), |f| {
                    funds::margin_delta(f, released)
                })?;
                restored = r.is_ok();
            }
            if restored {
                tx.execute(
                    "UPDATE sandbox_gtt SET gtt_status = 'active', margin_blocked = ?1, updated_at = ?2
                             WHERE gtt_id = ?3 AND gtt_status = 'triggered'",
                    params![dec_to_db(released), gtt_ts(now), gtt_id],
                )?;
            } else {
                tracing::warn!(
                    "GTT {} could not get its margin back after a refused order; marking it rejected",
                    gtt_id
                );
                tx.execute(
                    "UPDATE sandbox_gtt SET gtt_status = 'rejected', margin_blocked = '0', updated_at = ?1
                             WHERE gtt_id = ?2",
                    params![gtt_ts(now), gtt_id],
                )?;
            }
            revert_claim(tx, leg_id)?;
            Ok(())
        })?;
        Ok(false)
    })
    .await
}

/// Place the order for a leg this caller claimed. Returns `true` when the
/// order was placed.
pub(crate) async fn fire(core: &Arc<Core>, leg_id: i64, price: Decimal) -> SbResult<bool> {
    let loaded = blocking(core, move |c| {
        Ok(c.db.with_conn(|conn| -> rusqlite::Result<_> {
            let Some(leg) = get_leg(conn, leg_id)? else {
                return Ok(None);
            };
            let gtt = get_gtt(conn, &leg.gtt_id)?;
            Ok(gtt.map(|g| (leg, g)))
        })?)
    })
    .await?;
    let Some((leg, gtt)) = loaded else {
        return Ok(false);
    };
    if leg.leg_status != "triggering" {
        return Ok(false);
    }
    // Never wait on the tick path: if another order on this position is in
    // flight, hand the claim back and fire on the next price.
    let Some(guard) = core.locks.try_lock(PositionKey::new(
        &gtt.user_id,
        &gtt.exchange,
        &gtt.symbol,
        &leg.product,
    )) else {
        blocking(core, move |c| {
            Ok(c.db.with_tx(|tx| revert_claim(tx, leg_id))?)
        })
        .await?;
        return Ok(false);
    };

    let released = release_reservation(core, gtt.gtt_id.clone(), leg_id).await?;
    let Some(released) = released else {
        return Ok(false);
    };

    let req = leg_order(&gtt, &leg);
    let placed = placement::place_locked(core, &guard, req, None, Some(leg.id)).await;
    drop(guard);
    let gtt_id = gtt.gtt_id.clone();
    match placed {
        Ok(p) => record_fired(core, &gtt, leg_id, p.orderid.clone(), price).await,
        Err(e) => {
            tracing::warn!("Sandbox GTT {} leg order refused: {}", gtt_id, e.message);
            restore_after_refusal(core, gtt_id, leg_id, released).await
        }
    }
}

/// Evaluate a tick's price against the pending legs of one symbol. A price
/// received longer ago than `stale_quote_after` triggers nothing.
pub(crate) async fn on_price(core: &Arc<Core>, key: &SymbolKey, q: &Quote) -> SbResult<usize> {
    if quote_is_old(q, std::time::Instant::now(), core.opts.stale_quote_after) {
        return Ok(0);
    }
    let ltp = q.ltp;
    let k = key.clone();
    let legs = blocking(core, move |c| {
        Ok(c.db.with_conn(|conn| active_legs(conn, Some(&k)))?)
    })
    .await?;
    let mut fired = 0;
    for (leg, _g) in legs {
        if leg_is_triggered_by(&leg.trigger_direction, Some(leg.trigger_price), Some(ltp))
            && claim(core, leg.id).await?
            && fire(core, leg.id, ltp).await?
        {
            fired += 1;
        }
    }
    Ok(fired)
}

/// Evaluate every pending leg against one batch of quotes (polling engine
/// and catch-up).
pub(crate) async fn poll_active(core: &Arc<Core>) -> SbResult<usize> {
    poll_active_except(core, &std::collections::HashSet::new()).await
}

/// [`poll_active`] for the legs of every symbol not in `fresh` (symbols
/// whose own feed is live are left to their ticks).
pub(crate) async fn poll_active_except(
    core: &Arc<Core>,
    fresh: &std::collections::HashSet<SymbolKey>,
) -> SbResult<usize> {
    let legs: Vec<_> = blocking(core, |c| {
        Ok(c.db.with_conn(|conn| active_legs(conn, None))?)
    })
    .await?
    .into_iter()
    .filter(|(_, g)| !fresh.contains(&SymbolKey::new(g.symbol.clone(), g.exchange.clone())))
    .collect();
    if legs.is_empty() {
        return Ok(0);
    }
    let mut keys: Vec<SymbolKey> = legs
        .iter()
        .map(|(_, g)| SymbolKey::new(g.symbol.clone(), g.exchange.clone()))
        .collect();
    keys.sort();
    keys.dedup();
    let now = std::time::Instant::now();
    let quotes: HashMap<SymbolKey, Decimal> = core
        .quotes
        .quotes(&keys)
        .await
        .into_iter()
        .filter(|(_, q)| !quote_is_old(q, now, core.opts.stale_quote_after))
        .map(|(k, q)| (k, q.ltp))
        .collect();
    let mut fired = 0;
    for (leg, g) in legs {
        let Some(ltp) = quotes.get(&SymbolKey::new(g.symbol.clone(), g.exchange.clone())) else {
            continue;
        };
        if leg_is_triggered_by(&leg.trigger_direction, Some(leg.trigger_price), Some(*ltp))
            && claim(core, leg.id).await?
            && fire(core, leg.id, *ltp).await?
        {
            fired += 1;
        }
    }
    Ok(fired)
}

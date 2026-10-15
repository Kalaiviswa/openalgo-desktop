//! Order placement (web `sandbox/order_manager.py` `place_order`): the MIS
//! square-off gate, the CNC holdings check, pricing the margin, the
//! placement transaction, the immediate fill and the engine's watch.

use super::clock::ts;
use super::config::SandboxConfig;
use super::core::{blocking, Core, EngineCmd};
use super::db::OrderRow;
use super::events::{order_update, Outbox};
use super::execution;
use super::funds;
use super::locks::{PositionGuard, PositionKey};
use super::orders::{get_by_id, insert_order, normalize, NewOrder, Normalized, OrderRequest};
use super::positions;
use super::quotes::{quote_is_old, quote_looks_stale, Quote};
use super::replies::OrderPlaced;
use super::types::{Action, OrderStatus, PriceType, Product, SandboxError, SbResult, SymbolKey};
use chrono::{NaiveDateTime, NaiveTime};
use rusqlite::Connection;
use rust_decimal::Decimal;
use std::sync::Arc;

/// MIS orders are refused from the square-off time until 09:00 unless they
/// reduce an open MIS position.
fn mis_gate(
    cfg: &SandboxConfig,
    n: &Normalized,
    now: NaiveDateTime,
    position_qty: i64,
) -> Result<(), SandboxError> {
    if n.product != Product::Mis {
        return Ok(());
    }
    let Some(sq) = cfg.square_off_time(&n.exchange) else {
        return Ok(());
    };
    let t = now.time();
    let open = NaiveTime::from_hms_opt(9, 0, 0).unwrap_or(NaiveTime::MIN);
    if t < sq && t >= open {
        return Ok(());
    }
    let reducing = (n.action == Action::Buy && position_qty < 0)
        || (n.action == Action::Sell && position_qty > 0);
    if reducing {
        return Ok(());
    }
    Err(SandboxError::bad_request(format!(
        "MIS orders cannot be placed after square-off time ({} IST). Trading resumes at 09:00 AM IST.",
        sq.format("%H:%M")
    )))
}

/// Pre-placement reads made under the position lock.
struct Context {
    position_qty: i64,
    position_ltp: Option<Decimal>,
    holdings_qty: i64,
}

fn read_context(conn: &Connection, user: &str, n: &Normalized) -> rusqlite::Result<Context> {
    let pos = positions::get(conn, user, &n.symbol, &n.exchange, n.product.as_str())?;
    let holdings_qty = if n.action == Action::Sell && n.product == Product::Cnc {
        super::holdings::get(conn, user, &n.symbol, &n.exchange)?
            .map(|h| h.quantity.max(0))
            .unwrap_or(0)
    } else {
        0
    };
    Ok(Context {
        position_qty: pos.as_ref().map(|p| p.quantity).unwrap_or(0),
        position_ltp: pos.and_then(|p| p.ltp).filter(|l| *l > Decimal::ZERO),
        holdings_qty,
    })
}

/// A CNC sell's refusal when the position and the holdings cannot cover
/// it (CNC is delivery: it needs existing shares).
fn cnc_rejection(n: &Normalized, ctx: &Context) -> Option<String> {
    let mut cnc_rejection: Option<String> = None;
    if n.action == Action::Sell && n.product == Product::Cnc {
        let total = ctx.position_qty + ctx.holdings_qty;
        if total <= 0 {
            cnc_rejection = Some(format!(
                "Cannot sell {} in CNC. No positions or holdings available. CNC (delivery) requires existing shares. Use MIS for intraday short selling.",
                n.symbol
            ));
        } else if n.quantity > total {
            cnc_rejection = Some(format!(
                "Cannot sell {} shares of {} in CNC. Only {} shares available (Position: {}, Holdings: {})",
                n.quantity, n.symbol, total, ctx.position_qty, ctx.holdings_qty
            ));
        }
    }
    cnc_rejection
}

/// How an order is priced for its margin, and the quote to fill it with
/// at once when it can fill right away.
struct Pricing {
    margin_price: Decimal,
    cached: Option<Quote>,
    trigger_met: bool,
}

/// Price the margin (MARKET at the quote, else the position's last price;
/// LIMIT at its price; SL and SL-M at the trigger), and keep a quote when
/// the order can fill right away.
async fn price_order(
    core: &Core,
    n: &Normalized,
    ctx: &Context,
    prefetched: Option<Quote>,
) -> SbResult<Pricing> {
    let usable = |q: &Quote| q.ltp > Decimal::ZERO;
    let prefetched = prefetched.filter(usable);
    let margin_price: Option<Decimal>;
    let mut cached: Option<Quote> = None;
    let mut trigger_met = false;
    match n.price_type {
        PriceType::Market => {
            let q = match prefetched {
                Some(q) => Some(q),
                None => core.quote_with_retry(&n.symbol, &n.exchange).await,
            };
            if let Some(q) = q {
                margin_price = Some(q.ltp);
                cached = Some(q);
            } else if let Some(ltp) = ctx.position_ltp {
                tracing::warn!(
                    "No quote for {}; using the position's last price for the margin",
                    n.symbol
                );
                margin_price = Some(ltp);
            } else {
                return Err(SandboxError::bad_request(format!(
                    "Cannot place MARKET order for {} - unable to fetch current price. Please try again later or use LIMIT order with a specific price.",
                    n.symbol
                )));
            }
        }
        PriceType::Limit => {
            margin_price = n.price;
            let q = match prefetched {
                Some(q) => Some(q),
                None => core.quote_with_retry(&n.symbol, &n.exchange).await,
            };
            if let (Some(q), Some(limit)) = (q, n.price) {
                let marketable = match n.action {
                    Action::Buy => q.ltp <= limit,
                    Action::Sell => q.ltp >= limit,
                };
                if marketable {
                    cached = Some(q);
                }
            }
        }
        PriceType::Sl | PriceType::SlM => {
            margin_price = n.trigger_price;
            let q = match prefetched {
                Some(q) => Some(q),
                None => core.quote_with_retry(&n.symbol, &n.exchange).await,
            };
            if let (Some(q), Some(trig)) = (q, n.trigger_price) {
                let met = match n.action {
                    Action::Buy => q.ltp >= trig,
                    Action::Sell => q.ltp <= trig,
                };
                if met {
                    trigger_met = true;
                    if n.price_type == PriceType::SlM {
                        cached = Some(q);
                    } else if let Some(limit) = n.price {
                        let limit_ok = match n.action {
                            Action::Buy => q.ltp <= limit,
                            Action::Sell => q.ltp >= limit,
                        };
                        if limit_ok {
                            cached = Some(q);
                        }
                    }
                }
            }
        }
    }
    let margin_price = match margin_price {
        Some(p) if p > Decimal::ZERO => p,
        _ => {
            return Err(SandboxError::bad_request(format!(
                "Invalid price for margin calculation. Please provide valid price/trigger_price for {} order",
                n.price_type.as_str()
            )))
        }
    };
    Ok(Pricing {
        margin_price,
        cached,
        trigger_met,
    })
}

/// The margin to block for an order: what its notional needs, netted
/// against an opposite position (nothing when the position covers it),
/// and nothing for an order that blocks no margin.
fn margin_to_block(
    conn: &Connection,
    cfg: &SandboxConfig,
    user: &str,
    n: &Normalized,
    (margin_price, cv): (Decimal, Decimal),
) -> rusqlite::Result<Decimal> {
    let required = funds::margin_required(
        cfg,
        &n.symbol,
        &n.exchange,
        n.product,
        n.action,
        funds::notional(n.quantity, margin_price, cv),
    );
    let pos = positions::get(conn, user, &n.symbol, &n.exchange, n.product.as_str())?;
    let mut to_block = required;
    if let Some(p) = pos.filter(|p| p.quantity != 0) {
        let opposite = (p.quantity > 0 && n.action == Action::Sell)
            || (p.quantity < 0 && n.action == Action::Buy);
        if opposite {
            let existing = p.quantity.abs();
            to_block = if n.quantity <= existing {
                Decimal::ZERO
            } else {
                funds::margin_required(
                    cfg,
                    &n.symbol,
                    &n.exchange,
                    n.product,
                    n.action,
                    funds::notional(n.quantity - existing, margin_price, cv),
                )
            };
        }
    }
    if !funds::blocks_margin(&n.symbol, &n.exchange, n.product, n.action) {
        to_block = Decimal::ZERO;
    }
    Ok(to_block)
}

/// What the placement transaction records.
struct NewPlacement {
    n: Normalized,
    user: String,
    stored_price: Option<Decimal>,
    cnc_rejection: Option<String>,
    margin_price: Decimal,
    initial_status: OrderStatus,
    gtt_leg_id: Option<i64>,
}

/// One transaction: re-read the position, net the margin, block it and
/// record the order (or the CNC rejection). Its events are published once
/// it commits.
async fn record_order(core: &Arc<Core>, p: NewPlacement) -> SbResult<OrderRow> {
    let NewPlacement {
        n,
        user,
        stored_price,
        cnc_rejection,
        margin_price,
        initial_status,
        gtt_leg_id,
    } = p;
    blocking(core, move |c| {
        let mut outbox = Outbox::new();
        let r = c.db.with_tx(|tx| {
            let cfg = SandboxConfig::load(tx)?;
            let now = c.now();
            let now_s = ts(now);
            funds::ensure(tx, &user, cfg.starting_capital, &now_s)?;
            if let Some(reason) = &cnc_rejection {
                let row = insert_order(
                    tx,
                    &NewOrder {
                        user: &user,
                        n: &n,
                        price: stored_price,
                        status: OrderStatus::Rejected,
                        pending_quantity: 0,
                        rejection_reason: Some(reason),
                        margin: Decimal::ZERO,
                        gtt_leg_id: None,
                        now,
                    },
                )?;
                outbox.push(order_update(&row, OrderStatus::Rejected, reason));
                return Ok::<_, SandboxError>(Err(
                    SandboxError::bad_request(reason.clone()).with_orderid(row.orderid)
                ));
            }
            let cv = c.contract_value(&n.symbol, &n.exchange);
            let to_block = margin_to_block(tx, &cfg, &user, &n, (margin_price, cv))?;
            if to_block > Decimal::ZERO {
                let current = funds::ensure(tx, &user, cfg.starting_capital, &now_s)?;
                if let Err(r) = funds::check_available(&current, to_block) {
                    return Ok(Err(SandboxError::bad_request(r.0)));
                }
                let blocked = funds::apply(tx, &user, cfg.starting_capital, &now_s, |f| {
                    funds::block(f, to_block)
                })?;
                if let Err(r) = blocked {
                    return Err(SandboxError::bad_request(r.0));
                }
            }
            let row = insert_order(
                tx,
                &NewOrder {
                    user: &user,
                    n: &n,
                    price: stored_price,
                    status: initial_status,
                    pending_quantity: n.quantity,
                    rejection_reason: None,
                    margin: to_block,
                    gtt_leg_id,
                    now,
                },
            )?;
            outbox.push(order_update(&row, initial_status, ""));
            Ok(Ok(row))
        });
        match r {
            Ok(inner) => {
                c.publish(outbox);
                Ok(inner)
            }
            Err(e) => Err(e),
        }
    })
    .await?
}

/// Immediate execution: MARKET always, LIMIT when marketable, SL/SL-M
/// when the trigger (and for SL the limit) is already met; deferred to the
/// engine when the quote looks stale or is old.
async fn fill_now(
    core: &Arc<Core>,
    order: &OrderRow,
    price_type: PriceType,
    cached: Option<Quote>,
) -> SbResult<()> {
    if price_type == PriceType::Market || cached.is_some() {
        if let Some(q) = cached.filter(|q| {
            let stale = quote_looks_stale(q);
            if stale {
                tracing::warn!(
                    "Deferring immediate fill of {}: quote LTP {} is outside its day range",
                    order.orderid,
                    q.ltp
                );
            }
            let old = quote_is_old(q, std::time::Instant::now(), core.opts.stale_quote_after);
            if old {
                tracing::warn!(
                    "Deferring immediate fill of {}: its quote is older than {:?}",
                    order.orderid,
                    core.opts.stale_quote_after
                );
            }
            !stale && !old
        }) {
            match price_type {
                PriceType::Market => {
                    execution::process(core, order, &q).await?;
                }
                _ => {
                    if q.ltp > Decimal::ZERO {
                        execution::execute(core, order.id, q.ltp, Some(order.terms())).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Still resting: make sure the engine watches the symbol. Otherwise a
/// fill may have opened or closed a position: refresh what needs ticks.
async fn watch_or_rebuild(core: &Arc<Core>, order: &OrderRow) -> SbResult<()> {
    let still_pending = {
        let id = order.id;
        blocking(core, move |c| {
            Ok(c.db.with_conn(|conn| get_by_id(conn, id))?)
        })
        .await?
        .map(|o| o.order_status.is_pending())
        .unwrap_or(false)
    };
    if still_pending {
        core.notify_engine(EngineCmd::Watch(SymbolKey::new(
            order.symbol.clone(),
            order.exchange.clone(),
        )));
    } else {
        core.notify_engine(EngineCmd::Rebuild);
    }
    Ok(())
}

/// Place an order, taking the position's lock.
pub(crate) async fn place(
    core: &Arc<Core>,
    req: OrderRequest,
    prefetched: Option<Quote>,
) -> SbResult<OrderPlaced> {
    // Validation needs no lock; refusing early keeps the lock table quiet.
    let n = normalize(core, &req)?;
    let guard = core
        .locks
        .lock(PositionKey::new(
            core.user(),
            &n.exchange,
            &n.symbol,
            n.product.as_str(),
        ))
        .await;
    place_locked(core, &guard, req, prefetched, None).await
}

/// Place an order while the caller holds the position's lock (smart order,
/// close position, GTT fire).
pub(crate) async fn place_locked(
    core: &Arc<Core>,
    _guard: &PositionGuard,
    req: OrderRequest,
    prefetched: Option<Quote>,
    gtt_leg_id: Option<i64>,
) -> SbResult<OrderPlaced> {
    let n = normalize(core, &req)?;
    let user = core.user().to_string();

    let ctx = {
        let n = n.clone();
        let user = user.clone();
        blocking(core, move |c| {
            c.db.with_conn(|conn| {
                let cfg = SandboxConfig::load(conn)?;
                let ctx = read_context(conn, &user, &n)?;
                Ok::<_, SandboxError>((cfg, ctx))
            })
        })
        .await?
    };
    let (cfg, ctx) = ctx;
    let now = core.now();
    mis_gate(&cfg, &n, now, ctx.position_qty)?;

    let cnc_rejection = cnc_rejection(&n, &ctx);

    let Pricing {
        margin_price,
        cached,
        trigger_met,
    } = price_order(core, &n, &ctx, prefetched).await?;

    let initial_status = if n.price_type.has_trigger() && !trigger_met {
        OrderStatus::TriggerPending
    } else {
        OrderStatus::Open
    };
    let stored_price = if n.price_type == PriceType::Market {
        Some(margin_price)
    } else {
        n.price
    };
    let order = record_order(
        core,
        NewPlacement {
            n: n.clone(),
            user,
            stored_price,
            cnc_rejection,
            margin_price,
            initial_status,
            gtt_leg_id,
        },
    )
    .await?;
    tracing::info!(
        "Sandbox order {} placed: {} {} {} {}",
        order.orderid,
        order.symbol,
        order.action.as_str(),
        order.quantity,
        order.price_type.as_str()
    );

    fill_now(core, &order, n.price_type, cached).await?;
    watch_or_rebuild(core, &order).await?;
    Ok(OrderPlaced::new(order.orderid))
}

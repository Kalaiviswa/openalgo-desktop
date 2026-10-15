//! Positions: fill netting, the session-filtered position book with MTM,
//! closing a position and the trade book (web
//! `sandbox/position_manager.py`, `execution_engine._update_position`).
//! Expired contracts are settled in `expiry`.

use super::clock::{display_seconds, parse_ts, ts};
use super::config::SandboxConfig;
use super::core::{blocking, Core};
use super::db::{OrderRow, PositionRow, TradeRow};
use super::expiry::{contract_expiry, is_contract_expired_now, settle_expired};
use super::funds;
use super::locks::PositionKey;
use super::replies::{
    OrderMessage, PositionbookReply, PositionbookRow, TradebookReply, TradebookRow, ANALYZE,
    SUCCESS,
};
use super::session;
use super::types::{
    dec_to_db, float, money, pct, Action, PriceType, Product, SandboxError, SbResult, SymbolKey,
    AVG_DP, MARGIN_DP, PCT_DP,
};
use rusqlite::{params, Connection, OptionalExtension};
use rust_decimal::{Decimal, RoundingStrategy};
use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// SQL
// ---------------------------------------------------------------------------

pub fn get(
    conn: &Connection,
    user: &str,
    symbol: &str,
    exchange: &str,
    product: &str,
) -> rusqlite::Result<Option<PositionRow>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM sandbox_positions WHERE user_id = ?1 AND symbol = ?2 AND exchange = ?3 AND product = ?4",
            PositionRow::COLUMNS
        ),
        params![user, symbol, exchange, product],
        PositionRow::from_row,
    )
    .optional()
}

pub fn get_by_id(conn: &Connection, id: i64) -> rusqlite::Result<Option<PositionRow>> {
    conn.query_row(
        &format!(
            "SELECT {} FROM sandbox_positions WHERE id = ?1",
            PositionRow::COLUMNS
        ),
        params![id],
        PositionRow::from_row,
    )
    .optional()
}

pub fn list_user(conn: &Connection, user: &str) -> rusqlite::Result<Vec<PositionRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM sandbox_positions WHERE user_id = ?1 ORDER BY id",
        PositionRow::COLUMNS
    ))?;
    let rows = stmt.query_map(params![user], PositionRow::from_row)?;
    rows.collect()
}

/// Every open position (all users), optionally of one product.
pub fn list_open(
    conn: &Connection,
    product: Option<Product>,
) -> rusqlite::Result<Vec<PositionRow>> {
    let sql = match product {
        Some(_) => format!(
            "SELECT {} FROM sandbox_positions WHERE quantity != 0 AND product = ?1 ORDER BY id",
            PositionRow::COLUMNS
        ),
        None => format!(
            "SELECT {} FROM sandbox_positions WHERE quantity != 0 ORDER BY id",
            PositionRow::COLUMNS
        ),
    };
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = match product {
        Some(p) => stmt.query_map(params![p.as_str()], PositionRow::from_row)?,
        None => stmt.query_map([], PositionRow::from_row)?,
    };
    rows.collect()
}

fn insert(conn: &Connection, p: &PositionRow) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO sandbox_positions (user_id, symbol, exchange, product, quantity, average_price,
           ltp, pnl, pnl_percent, accumulated_realized_pnl, today_realized_pnl, margin_blocked,
           created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            p.user_id,
            p.symbol,
            p.exchange,
            p.product.as_str(),
            p.quantity,
            dec_to_db(p.average_price),
            p.ltp.map(dec_to_db),
            dec_to_db(p.pnl),
            dec_to_db(p.pnl_percent),
            dec_to_db(p.accumulated_realized_pnl),
            dec_to_db(p.today_realized_pnl),
            dec_to_db(p.margin_blocked),
            p.created_at,
            p.updated_at
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Write every mutable column of a position.
pub fn save(conn: &Connection, p: &PositionRow) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE sandbox_positions SET quantity = ?1, average_price = ?2, ltp = ?3, pnl = ?4,
           pnl_percent = ?5, accumulated_realized_pnl = ?6, today_realized_pnl = ?7,
           margin_blocked = ?8, updated_at = ?9
         WHERE id = ?10",
        params![
            p.quantity,
            dec_to_db(p.average_price),
            p.ltp.map(dec_to_db),
            dec_to_db(p.pnl),
            dec_to_db(p.pnl_percent),
            dec_to_db(p.accumulated_realized_pnl),
            dec_to_db(p.today_realized_pnl),
            dec_to_db(p.margin_blocked),
            p.updated_at,
            p.id
        ],
    )?;
    Ok(())
}

/// Claim a position row for settlement if it still holds `quantity` (web
/// `claim_position_for_settlement`). Inside the engine's transactions this is
/// a re-check; it is kept so settlement never acts on a row that moved.
pub fn claim_for_settlement(conn: &Connection, id: i64, quantity: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE sandbox_positions SET quantity = quantity WHERE id = ?1 AND quantity = ?2",
        params![id, quantity],
    )?;
    Ok(n == 1)
}

// ---------------------------------------------------------------------------
// P&L arithmetic
// ---------------------------------------------------------------------------

/// Realized P&L of closing `close_qty` of a position of sign `old_qty`.
pub fn realized_pnl(
    old_qty: i64,
    avg: Decimal,
    close_qty: i64,
    price: Decimal,
    cv: Decimal,
) -> Decimal {
    let q = Decimal::from(close_qty.abs());
    if old_qty > 0 {
        (price - avg) * q * cv
    } else {
        (avg - price) * q * cv
    }
}

/// Unrealized P&L of an open position at `ltp`.
pub fn unrealized_pnl(qty: i64, avg: Decimal, ltp: Decimal, cv: Decimal) -> Decimal {
    (ltp - avg) * Decimal::from(qty) * cv
}

/// Directional P&L percentage at `ltp`.
pub fn pnl_percent(qty: i64, avg: Decimal, ltp: Decimal) -> Decimal {
    if avg <= Decimal::ZERO {
        return Decimal::ZERO;
    }
    let p = (ltp - avg) / avg * Decimal::ONE_HUNDRED;
    let p = if qty >= 0 { p } else { -p };
    p.round_dp(PCT_DP)
}

/// The price an order's margin was computed at.
pub fn margin_price(order: &OrderRow) -> Decimal {
    match order.price_type {
        PriceType::Market | PriceType::Limit => order.price.unwrap_or(Decimal::ZERO),
        PriceType::Sl | PriceType::SlM => order.trigger_price.unwrap_or(Decimal::ZERO),
    }
}

/// Margin the position needs from filling `order` against a position of
/// `position_qty`: the part of the order that opens or adds exposure, priced
/// as the order's margin is priced. Zero for the part that reduces the
/// position and for an order that blocks no margin (a CNC sale).
///
/// An exit placed against a position blocks nothing; if another order has
/// closed that position by the time the exit fills, the exit opens a new one
/// and this is what that position must hold.
pub fn margin_needed_at_fill(
    cfg: &SandboxConfig,
    order: &OrderRow,
    position_qty: i64,
    cv: Decimal,
) -> Decimal {
    if !funds::blocks_margin(&order.symbol, &order.exchange, order.product, order.action) {
        return Decimal::ZERO;
    }
    let signed = order.action.sign() * order.quantity;
    let opening = if position_qty == 0 || position_qty.signum() == signed.signum() {
        signed.abs()
    } else {
        (signed.abs() - position_qty.abs()).max(0)
    };
    if opening == 0 {
        return Decimal::ZERO;
    }
    funds::margin_required(
        cfg,
        &order.symbol,
        &order.exchange,
        order.product,
        order.action,
        funds::notional(opening, margin_price(order), cv),
    )
}

// ---------------------------------------------------------------------------
// Netting
// ---------------------------------------------------------------------------

/// What a fill did to the position and the funds.
#[derive(Debug, Clone, PartialEq)]
pub struct FillEffect {
    pub position: PositionRow,
    pub realized_pnl: Decimal,
    /// Margin returned to available cash by this fill.
    pub released: Decimal,
}

/// Apply a filled order to its position and to the funds, in the caller's
/// transaction (web `_update_position`). `order.margin_blocked` already
/// includes anything the fill itself had to block
/// ([`margin_needed_at_fill`]).
///
/// Margin follows the order into the position, so after every fill
/// `used_margin` equals the margin the books hold:
/// * open / reopen / add: the position takes the order's margin;
/// * reduce: a proportional share of the position margin and all of the
///   order's margin (if any) are released with the realized P&L;
/// * full close: the position margin and the order margin are released;
/// * reversal: the old margin is released; the new position keeps the
///   order margin that covers the excess quantity (at the order's margin
///   price) and the rest is released. The web scales that excess-only margin
///   by the excess share again, so its reversed position holds too little.
#[allow(clippy::too_many_arguments)]
pub fn apply_fill(
    conn: &Connection,
    cfg: &SandboxConfig,
    order: &OrderRow,
    price: Decimal,
    cv: Decimal,
    now: &str,
) -> rusqlite::Result<FillEffect> {
    let user = &order.user_id;
    let signed = order.action.sign() * order.quantity;
    let m_order = order.margin_blocked;
    let existing = get(
        conn,
        user,
        &order.symbol,
        &order.exchange,
        order.product.as_str(),
    )?;

    let mut realized = Decimal::ZERO;
    let mut release_amount = Decimal::ZERO;

    let position = match existing {
        None => {
            let mut p = PositionRow {
                id: 0,
                user_id: user.clone(),
                symbol: order.symbol.clone(),
                exchange: order.exchange.clone(),
                product: order.product,
                quantity: signed,
                average_price: price,
                ltp: Some(price),
                pnl: Decimal::ZERO,
                pnl_percent: Decimal::ZERO,
                accumulated_realized_pnl: Decimal::ZERO,
                today_realized_pnl: Decimal::ZERO,
                margin_blocked: m_order,
                created_at: now.to_string(),
                updated_at: now.to_string(),
            };
            p.id = insert(conn, &p)?;
            p
        }
        Some(mut p) => {
            let old = p.quantity;
            let fin = old + signed;
            if old == 0 {
                // Reopen: realized history kept, everything else restarts.
                p.quantity = signed;
                p.average_price = price;
                p.pnl = Decimal::ZERO;
                p.pnl_percent = Decimal::ZERO;
                p.margin_blocked = m_order;
            } else if fin == 0 {
                realized = realized_pnl(old, p.average_price, signed.abs(), price, cv);
                release_amount = p.margin_blocked + m_order;
                p.accumulated_realized_pnl += realized;
                p.today_realized_pnl += realized;
                p.quantity = 0;
                p.margin_blocked = Decimal::ZERO;
                p.pnl = p.today_realized_pnl;
                p.pnl_percent = Decimal::ZERO;
            } else if (old > 0 && fin > old) || (old < 0 && fin < old) {
                let total_qty = Decimal::from(old.abs() + signed.abs());
                let value = Decimal::from(old.abs()) * p.average_price
                    + Decimal::from(signed.abs()) * price;
                p.average_price = (value / total_qty).round_dp(AVG_DP);
                p.quantity = fin;
                p.margin_blocked += m_order;
            } else {
                let reduced = old.abs().min(signed.abs());
                realized = realized_pnl(old, p.average_price, reduced, price, cv);
                p.accumulated_realized_pnl += realized;
                p.today_realized_pnl += realized;
                if signed.abs() > old.abs() {
                    // Reversal.
                    let excess = signed.abs() - old.abs();
                    let needed = funds::margin_required(
                        cfg,
                        &order.symbol,
                        &order.exchange,
                        order.product,
                        order.action,
                        funds::notional(excess, margin_price(order), cv),
                    );
                    let new_margin = m_order.min(needed).max(Decimal::ZERO);
                    release_amount = p.margin_blocked + (m_order - new_margin);
                    p.quantity = signed.signum() * excess;
                    p.average_price = price;
                    p.margin_blocked = new_margin;
                } else {
                    let release_pos = (p.margin_blocked * Decimal::from(reduced)
                        / Decimal::from(old.abs()))
                    .round_dp_with_strategy(MARGIN_DP, RoundingStrategy::MidpointAwayFromZero);
                    release_amount = release_pos + m_order;
                    p.quantity = fin;
                    p.margin_blocked -= release_pos;
                }
            }
            p.ltp = Some(price);
            if p.quantity != 0 {
                p.pnl = unrealized_pnl(p.quantity, p.average_price, price, cv);
                p.pnl_percent = pnl_percent(p.quantity, p.average_price, price);
            }
            p.updated_at = now.to_string();
            save(conn, &p)?;
            p
        }
    };

    let mut released = Decimal::ZERO;
    if release_amount > Decimal::ZERO || realized != Decimal::ZERO {
        let capital = cfg.starting_capital;
        let outcome = funds::apply(conn, user, capital, now, |f| {
            let amount = if release_amount > f.used_margin {
                tracing::error!(
                    "Sandbox fill tried to release {} with only {} reserved; releasing what is reserved",
                    release_amount,
                    f.used_margin
                );
                f.used_margin
            } else {
                release_amount
            };
            funds::release(f, amount.max(Decimal::ZERO), realized, true)
        })?;
        match outcome {
            Ok(_) => released = release_amount,
            Err(r) => tracing::error!("Sandbox fill could not settle funds: {}", r.0),
        }
    }

    Ok(FillEffect {
        position,
        realized_pnl: realized,
        released,
    })
}

// ---------------------------------------------------------------------------
// Position book
// ---------------------------------------------------------------------------

/// Rows of the current session, after the catch-up reset of stale
/// `today_realized_pnl` and expired-contract settlement (web
/// `get_open_positions` before MTM).
fn session_positions(
    conn: &Connection,
    core: &Core,
    cfg: &SandboxConfig,
) -> rusqlite::Result<Vec<PositionRow>> {
    let now = core.now();
    let boundary = session::last_session_expiry(now, core.opts.session_expiry);
    let boundary_s = ts(boundary);
    let mut out = Vec::new();
    for mut p in list_user(conn, core.user())? {
        let updated = parse_ts(&p.updated_at).unwrap_or(now);
        if p.today_realized_pnl != Decimal::ZERO && updated < boundary {
            // Catch-up reset that leaves updated_at alone, so yesterday's
            // closed rows stay hidden.
            conn.execute(
                "UPDATE sandbox_positions SET today_realized_pnl = '0' WHERE id = ?1",
                params![p.id],
            )?;
            p.today_realized_pnl = Decimal::ZERO;
        }
        let include = if p.updated_at.as_str() >= boundary_s.as_str() {
            p.quantity != 0 || p.today_realized_pnl != Decimal::ZERO
        } else {
            p.product == Product::Nrml && p.quantity != 0
        };
        if include {
            out.push(p);
        }
    }
    let now_s = ts(now);
    let mut settled = Vec::with_capacity(out.len());
    for p in out {
        if p.quantity != 0 {
            let expiry = contract_expiry(core, &p.symbol, &p.exchange);
            if is_contract_expired_now(expiry, &p.exchange, now, cfg) {
                settle_expired(conn, core, cfg, &p, &now_s)?;
                if let Some(fresh) = get_by_id(conn, p.id)? {
                    settled.push(fresh);
                    continue;
                }
            }
        }
        settled.push(p);
    }
    Ok(settled)
}

/// Write MTM for open positions from `quotes` and set funds unrealized P&L
/// to the total over the given open rows. Leaves `updated_at` alone.
fn write_mtm(
    conn: &Connection,
    core: &Core,
    cfg: &SandboxConfig,
    rows: &mut [PositionRow],
    quotes: &HashMap<SymbolKey, Decimal>,
    set_funds: bool,
) -> rusqlite::Result<()> {
    let mut total = Decimal::ZERO;
    for p in rows.iter_mut() {
        if p.quantity == 0 {
            continue;
        }
        if let Some(ltp) = quotes.get(&SymbolKey::new(p.symbol.clone(), p.exchange.clone())) {
            if *ltp > Decimal::ZERO {
                let cv = core.contract_value(&p.symbol, &p.exchange);
                p.ltp = Some(*ltp);
                p.pnl = unrealized_pnl(p.quantity, p.average_price, *ltp, cv);
                p.pnl_percent = pnl_percent(p.quantity, p.average_price, *ltp);
                conn.execute(
                    "UPDATE sandbox_positions SET ltp = ?1, pnl = ?2, pnl_percent = ?3 WHERE id = ?4",
                    params![dec_to_db(*ltp), dec_to_db(p.pnl), dec_to_db(p.pnl_percent), p.id],
                )?;
            }
        }
        total += p.pnl;
    }
    if set_funds {
        let now = core.now_ts();
        let _ = funds::apply(conn, core.user(), cfg.starting_capital, &now, |f| {
            Ok(funds::set_unrealized(f, total))
        })?;
    }
    Ok(())
}

/// Update MTM for every open position on one symbol from a tick (all
/// users); funds unrealized is recomputed for the affected users.
pub(crate) fn mtm_from_tick(
    conn: &Connection,
    core: &Core,
    key: &SymbolKey,
    ltp: Decimal,
) -> rusqlite::Result<usize> {
    let cfg = super::config::SandboxConfig::load(conn)?;
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM sandbox_positions WHERE symbol = ?1 AND exchange = ?2 AND quantity != 0",
        PositionRow::COLUMNS
    ))?;
    let mut rows: Vec<PositionRow> = stmt
        .query_map(params![key.symbol, key.exchange], PositionRow::from_row)?
        .collect::<rusqlite::Result<_>>()?;
    drop(stmt);
    if rows.is_empty() {
        return Ok(0);
    }
    let mut quotes = HashMap::new();
    quotes.insert(key.clone(), ltp);
    write_mtm(conn, core, &cfg, &mut rows, &quotes, false)?;
    let now = core.now_ts();
    let users: std::collections::BTreeSet<String> =
        rows.iter().map(|p| p.user_id.clone()).collect();
    for user in users {
        let all = list_user(conn, &user)?;
        let total: Decimal = all.iter().filter(|p| p.quantity != 0).map(|p| p.pnl).sum();
        let _ = funds::apply(conn, &user, cfg.starting_capital, &now, |f| {
            Ok(funds::set_unrealized(f, total))
        })?;
    }
    Ok(rows.len())
}

/// The session's positions, priced (web `get_open_positions(update_mtm)`).
pub(crate) async fn open_positions(
    core: &Arc<Core>,
    update_mtm: bool,
) -> SbResult<(Vec<PositionRow>, SandboxConfig)> {
    let (rows, cfg) = blocking(core, |c| {
        c.db.with_tx(|tx| {
            let cfg = SandboxConfig::load(tx)?;
            let rows = session_positions(tx, c, &cfg)?;
            Ok::<_, SandboxError>((rows, cfg))
        })
    })
    .await?;
    if !update_mtm {
        return Ok((rows, cfg));
    }
    let keys: Vec<SymbolKey> = {
        let mut k: Vec<SymbolKey> = rows
            .iter()
            .filter(|p| p.quantity != 0)
            .map(|p| SymbolKey::new(p.symbol.clone(), p.exchange.clone()))
            .collect();
        k.sort();
        k.dedup();
        k
    };
    let quotes: HashMap<SymbolKey, Decimal> = if keys.is_empty() {
        HashMap::new()
    } else {
        core.quotes
            .quotes(&keys)
            .await
            .into_iter()
            .map(|(k, q)| (k, q.ltp))
            .collect()
    };
    let rows = blocking(core, move |c| {
        c.db.with_tx(|tx| {
            let cfg = SandboxConfig::load(tx)?;
            let mut rows = rows;
            // Re-read so MTM never overwrites a fill that landed meanwhile.
            let mut fresh = Vec::with_capacity(rows.len());
            for p in rows.drain(..) {
                fresh.push(get_by_id(tx, p.id)?.unwrap_or(p));
            }
            write_mtm(tx, c, &cfg, &mut fresh, &quotes, true)?;
            Ok::<_, SandboxError>(fresh)
        })
    })
    .await?;
    Ok((rows, cfg))
}

/// Build the web position book from session rows.
pub(crate) fn positionbook_reply(core: &Core, rows: &[PositionRow]) -> PositionbookReply {
    let mut data = Vec::with_capacity(rows.len());
    let mut total_unrealized = Decimal::ZERO;
    let mut total_today_realized = Decimal::ZERO;
    let mut total_pnl_today = Decimal::ZERO;
    for p in rows {
        let unrealized = p.pnl;
        let today = p.today_realized_pnl;
        let pos_total = if p.quantity != 0 {
            total_unrealized += unrealized;
            today + unrealized
        } else {
            today
        };
        total_today_realized += today;
        total_pnl_today += pos_total;
        let cv = core.contract_value(&p.symbol, &p.exchange);
        let (pct_v, avg) = if p.quantity != 0 {
            let investment = funds::notional(p.quantity, p.average_price, cv).abs();
            let pc = if investment > Decimal::ZERO {
                pos_total / investment * Decimal::ONE_HUNDRED
            } else {
                Decimal::ZERO
            };
            (pc, money(p.average_price))
        } else {
            (Decimal::ZERO, 0.0)
        };
        data.push(PositionbookRow {
            symbol: p.symbol.clone(),
            exchange: p.exchange.clone(),
            product: p.product.as_str().to_string(),
            quantity: p.quantity,
            average_price: avg,
            ltp: money(p.ltp.unwrap_or(Decimal::ZERO)),
            pnl: money(pos_total),
            pnlpercent: pct(pct_v),
            unrealized_pnl: money(unrealized),
            today_realized_pnl: money(today),
            total_pnl_today: money(pos_total),
            lot_size: float(cv),
        });
    }
    PositionbookReply {
        status: SUCCESS,
        data,
        total_pnl: money(total_pnl_today),
        total_unrealized_pnl: money(total_unrealized),
        total_today_realized_pnl: money(total_today_realized),
        total_pnl_today: money(total_pnl_today),
        mode: ANALYZE,
    }
}

/// How a request to close a position ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// The position is flat.
    Filled,
    /// A close order rests until a usable price arrives (no quote, or a
    /// quote outside its own day range).
    Pending,
    /// The close order was refused when it tried to fill.
    Failed,
}

/// A close request's reply and what it did.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CloseResult {
    pub reply: OrderMessage,
    pub outcome: CloseOutcome,
    /// Whether this request placed a new order (`false` when it returned a
    /// close that was already pending).
    pub placed: bool,
}

/// Resting MARKET orders on `action`'s side of one position: closes that are
/// waiting for a price. Oldest first.
fn pending_market_exits(
    conn: &Connection,
    user: &str,
    symbol: &str,
    exchange: &str,
    product: &str,
    action: Action,
) -> rusqlite::Result<Vec<OrderRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM sandbox_orders WHERE user_id = ?1 AND symbol = ?2 AND exchange = ?3
           AND product = ?4 AND action = ?5 AND price_type = 'MARKET'
           AND order_status IN ('open','trigger pending') ORDER BY id",
        OrderRow::COLUMNS
    ))?;
    let rows = stmt.query_map(
        params![user, symbol, exchange, product, action.as_str()],
        OrderRow::from_row,
    )?;
    rows.collect()
}

/// Close one position with a MARKET order tagged `AUTO_SQUARE_OFF`, holding
/// the position's lock across the read and the order.
pub(crate) async fn close_position(
    core: &Arc<Core>,
    symbol: &str,
    exchange: &str,
    product: &str,
) -> SbResult<OrderMessage> {
    Ok(close_position_outcome(core, symbol, exchange, product)
        .await?
        .reply)
}

/// [`close_position`], reporting whether the position is flat, still
/// waiting on a close order, or the close was refused.
///
/// A MARKET close rests when there is no usable price. Asking again while it
/// rests returns that close instead of placing another: a second exit for the
/// same quantity would, once prices return, close the position and then open
/// the opposite one. Only the quantity no resting close covers is ordered.
pub(crate) async fn close_position_outcome(
    core: &Arc<Core>,
    symbol: &str,
    exchange: &str,
    product: &str,
) -> SbResult<CloseResult> {
    let guard = core
        .locks
        .lock(PositionKey::new(core.user(), exchange, symbol, product))
        .await;
    let (s, e, p) = (
        symbol.to_string(),
        exchange.to_string(),
        product.to_string(),
    );
    let read = blocking(core, move |c| {
        Ok(c.db.with_conn(|conn| -> rusqlite::Result<_> {
            let Some(pos) = get(conn, c.user(), &s, &e, &p)?.filter(|row| row.quantity != 0) else {
                return Ok(None);
            };
            let action = if pos.quantity > 0 {
                Action::Sell
            } else {
                Action::Buy
            };
            let exits = pending_market_exits(conn, c.user(), &s, &e, &p, action)?;
            Ok(Some((pos, action, exits)))
        })?)
    })
    .await?;
    let Some((pos, action, exits)) = read else {
        return Err(SandboxError::not_found(format!(
            "No open position found for {symbol}"
        )));
    };
    let covered: i64 = exits.iter().map(|o| o.quantity).sum();
    let remaining = pos.quantity.abs() - covered;
    if remaining <= 0 {
        let orderid = exits.last().map(|o| o.orderid.clone()).unwrap_or_default();
        tracing::info!(
            "Close of {} {} {} is already pending as order {}",
            symbol,
            exchange,
            product,
            orderid
        );
        return Ok(CloseResult {
            reply: OrderMessage::new(
                orderid,
                format!(
                    "Position close order for {symbol} is already placed and fills when a price is available"
                ),
            ),
            outcome: CloseOutcome::Pending,
            placed: false,
        });
    }
    let req = super::orders::OrderRequest {
        symbol: symbol.to_string(),
        exchange: exchange.to_string(),
        action: action.as_str().to_string(),
        quantity: remaining,
        price: None,
        trigger_price: None,
        price_type: "MARKET".to_string(),
        product: product.to_string(),
        strategy: "AUTO_SQUARE_OFF".to_string(),
    };
    let placed = super::placement::place_locked(core, &guard, req, None, None).await?;
    let (s, e, p, id) = (
        symbol.to_string(),
        exchange.to_string(),
        product.to_string(),
        placed.orderid.clone(),
    );
    let outcome = blocking(core, move |c| {
        Ok(c.db.with_conn(|conn| -> rusqlite::Result<_> {
            let flat = get(conn, c.user(), &s, &e, &p)?
                .map(|row| row.quantity == 0)
                .unwrap_or(true);
            let order = super::orders::get_by_orderid(conn, c.user(), &id)?;
            Ok(if flat {
                CloseOutcome::Filled
            } else if order.map(|o| o.order_status.is_pending()).unwrap_or(false) {
                CloseOutcome::Pending
            } else if covered > 0 {
                // This close filled; an earlier one still waits for a price.
                CloseOutcome::Pending
            } else {
                CloseOutcome::Failed
            })
        })?)
    })
    .await?;
    Ok(CloseResult {
        reply: OrderMessage::new(
            placed.orderid,
            format!("Position close order placed for {symbol}"),
        ),
        outcome,
        placed: true,
    })
}

// ---------------------------------------------------------------------------
// Trade book
// ---------------------------------------------------------------------------

pub fn trades_since(conn: &Connection, user: &str, since: &str) -> rusqlite::Result<Vec<TradeRow>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM sandbox_trades WHERE user_id = ?1 AND trade_timestamp >= ?2
         ORDER BY trade_timestamp DESC, id DESC",
        TradeRow::COLUMNS
    ))?;
    let rows = stmt.query_map(params![user, since], TradeRow::from_row)?;
    rows.collect()
}

/// The trade book. Trade value is the trade's notional, contract value
/// included.
pub(crate) fn tradebook_reply(core: &Core, trades: &[TradeRow]) -> TradebookReply {
    let data = trades
        .iter()
        .map(|t| {
            let price = money(t.price);
            let cv = core.contract_value(&t.symbol, &t.exchange);
            TradebookRow {
                tradeid: t.tradeid.clone(),
                orderid: t.orderid.clone(),
                symbol: t.symbol.clone(),
                exchange: t.exchange.clone(),
                action: t.action.clone(),
                quantity: t.quantity,
                average_price: price,
                price,
                trade_value: money(funds::notional(t.quantity, t.price, cv)),
                product: t.product.clone(),
                strategy: t.strategy.clone().unwrap_or_default(),
                timestamp: display_seconds(&t.trade_timestamp),
            }
        })
        .collect();
    TradebookReply {
        status: SUCCESS,
        data,
        mode: ANALYZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn pnl_formulas() {
        assert_eq!(realized_pnl(10, dec!(100), 4, dec!(110), dec!(1)), dec!(40));
        assert_eq!(
            realized_pnl(-10, dec!(100), 4, dec!(110), dec!(1)),
            dec!(-40)
        );
        assert_eq!(unrealized_pnl(-10, dec!(100), dec!(90), dec!(1)), dec!(100));
        assert_eq!(pnl_percent(-10, dec!(100), dec!(90)), dec!(10));
        assert_eq!(pnl_percent(10, dec!(0), dec!(90)), dec!(0));
    }
}

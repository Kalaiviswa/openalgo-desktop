//! The `orders` toolset: placing, modifying and cancelling orders, closing
//! positions and the analyzer toggle.

use crate::mcp::schema::{Dflt as D, Param, Ty::*};
use crate::mcp::{Risk, Scope, ToolDef};

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "place_order",
        title: "Place Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place a new order (market or limit).

    Args:
        symbol: Stock symbol (e.g., 'RELIANCE')
        quantity: Number of shares
        action: 'BUY' or 'SELL'
        exchange: 'NSE', 'NFO', 'CDS', 'BSE', 'BFO', 'BCD', 'MCX', 'NCDEX'
        price_type: 'MARKET', 'LIMIT', 'SL', 'SL-M'
        product: 'CNC', 'NRML', 'MIS'
        strategy: Strategy name (defaults to 'python mcp')
        price: Limit price (required for LIMIT orders)
        trigger_price: Trigger price (required for SL and SL-M orders)
        disclosed_quantity: Disclosed quantity"#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "quantity",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "action",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Str("NSE"),
            },
            Param {
                name: "price_type",
                ty: Str,
                default: D::Str("MARKET"),
            },
            Param {
                name: "product",
                ty: Str,
                default: D::Str("MIS"),
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
            Param {
                name: "price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "trigger_price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "disclosed_quantity",
                ty: OptInt,
                default: D::Null,
            },
        ],
    },
    ToolDef {
        name: "place_smart_order",
        title: "Place Smart Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place a smart order that considers the current position size (auto-calculates delta
    between requested and current size before sending to the broker).

    Args:
        symbol: Stock symbol
        quantity: Target quantity
        action: 'BUY' or 'SELL'
        position_size: Current position size
        exchange: Exchange name
        price_type: 'MARKET', 'LIMIT', 'SL', 'SL-M'
        product: 'CNC', 'NRML', 'MIS'
        strategy: Strategy name (defaults to 'python mcp')
        price: Limit price (required for LIMIT orders)
        trigger_price: Trigger price (required for SL / SL-M orders)
        disclosed_quantity: Disclosed quantity"#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "quantity",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "action",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "position_size",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Str("NSE"),
            },
            Param {
                name: "price_type",
                ty: Str,
                default: D::Str("MARKET"),
            },
            Param {
                name: "product",
                ty: Str,
                default: D::Str("MIS"),
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
            Param {
                name: "price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "trigger_price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "disclosed_quantity",
                ty: OptInt,
                default: D::Null,
            },
        ],
    },
    ToolDef {
        name: "place_basket_order",
        title: "Place Basket Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place multiple orders in a basket.

    Args:
        orders: List of order dictionaries. Each order should contain:
            - symbol (str): Trading symbol. Required.
            - exchange (str): Exchange code. Required.
            - action (str): BUY or SELL. Required.
            - quantity (int/str): Quantity to trade. Required.
            - pricetype (str): MARKET, LIMIT, SL, SL-M. Optional, defaults to MARKET.
            - product (str): MIS, CNC, NRML. Optional, defaults to MIS.
            - price (str): Required for LIMIT orders.
            - trigger_price (str): Required for SL orders.
        strategy: Strategy name (default: Python)

        Example: [
            {"symbol": "BHEL", "exchange": "NSE", "action": "BUY", "quantity": 1, "pricetype": "MARKET", "product": "MIS"},
            {"symbol": "ZOMATO", "exchange": "NSE", "action": "SELL", "quantity": 1, "pricetype": "MARKET", "product": "MIS"}
        ]

    Returns:
        JSON with results for each order including orderid, status, and symbol"#,
        params: &[
            Param {
                name: "orders",
                ty: ObjectList,
                default: D::Required,
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
        ],
    },
    ToolDef {
        name: "place_split_order",
        title: "Place Split Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place a large order split into smaller chunks.

    Args:
        symbol: Stock symbol (e.g., 'YESBANK')
        quantity: Total quantity to trade
        split_size: Size of each split order
        action: 'BUY' or 'SELL'
        exchange: Exchange name (default: NSE)
        price_type: 'MARKET', 'LIMIT', 'SL', 'SL-M' (default: MARKET)
        product: 'MIS', 'CNC', 'NRML' (default: MIS)
        strategy: Strategy name (default: Python)
        price: Limit price (required for LIMIT orders)
        trigger_price: Trigger price (required for SL orders)
        disclosed_quantity: Disclosed quantity (optional)

    Returns:
        JSON with results array containing each split order's orderid, quantity, and status

    Example:
        # Split 105 shares into orders of 20 each (5 orders of 20 + 1 order of 5)
        place_split_order("YESBANK", 105, 20, "SELL", "NSE")"#,
        params: &[
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "quantity",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "split_size",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "action",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Str("NSE"),
            },
            Param {
                name: "price_type",
                ty: Str,
                default: D::Str("MARKET"),
            },
            Param {
                name: "product",
                ty: Str,
                default: D::Str("MIS"),
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
            Param {
                name: "price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "trigger_price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "disclosed_quantity",
                ty: OptInt,
                default: D::Null,
            },
        ],
    },
    ToolDef {
        name: "place_options_order",
        title: "Place Options Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place an options order with ATM/ITM/OTM offset.

    Args:
        underlying: Underlying symbol (e.g., 'NIFTY', 'BANKNIFTY', 'NIFTY28OCT25FUT')
        exchange: Exchange for underlying ('NSE_INDEX', 'BSE_INDEX', 'NFO')
        offset: Strike offset - 'ATM', 'ITM1'-'ITM50', 'OTM1'-'OTM50'
        option_type: 'CE' for Call or 'PE' for Put
        action: 'BUY' or 'SELL'
        quantity: Absolute quantity — must be a multiple of the contract lot size.
                  Do NOT hardcode lot size — call get_option_symbol() or get_option_chain()
                  first to read the current 'lotsize' from the broker master contract,
                  then pass quantity = lots * lotsize.
        expiry_date: Expiry date in format 'DDMMMYY' (e.g., '28OCT25'). Optional if underlying includes expiry.
        strategy: Strategy name (default: Python)
        price_type: 'MARKET', 'LIMIT', 'SL', 'SL-M' (default: MARKET)
        product: 'MIS', 'NRML' (default: MIS). Note: CNC not supported for options.
        price: Limit price (required for LIMIT orders)
        trigger_price: Trigger price (required for SL and SL-M orders)
        disclosed_quantity: Disclosed quantity (optional)

    Returns:
        JSON with orderid, symbol, underlying_ltp, offset, option_type, mode

    Example:
        # Basic ATM call order
        place_options_order("NIFTY", "NSE_INDEX", "ATM", "CE", "BUY", 75, "28NOV25")

        # Using future as underlying (expiry auto-detected)
        place_options_order("NIFTY28OCT25FUT", "NFO", "ITM2", "CE", "BUY", 75)"#,
        params: &[
            Param {
                name: "underlying",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "offset",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "option_type",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "action",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "quantity",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "expiry_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
            Param {
                name: "price_type",
                ty: Str,
                default: D::Str("MARKET"),
            },
            Param {
                name: "product",
                ty: Str,
                default: D::Str("MIS"),
            },
            Param {
                name: "price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "trigger_price",
                ty: OptNum,
                default: D::Null,
            },
            Param {
                name: "disclosed_quantity",
                ty: OptInt,
                default: D::Null,
            },
        ],
    },
    ToolDef {
        name: "place_options_multi_order",
        title: "Place Multi-Leg Options Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Place a multi-leg options order (spreads, iron condor, straddles, etc.).
    BUY legs are executed first for margin efficiency, then SELL legs.

    Args:
        strategy: Strategy name (defaults to 'python mcp'). Give each multi-leg trade
                  a meaningful name (e.g., 'nifty iron condor') to make tracking easier.
        underlying: Underlying symbol (e.g., 'NIFTY', 'BANKNIFTY', 'NIFTY28OCT25FUT')
        exchange: Exchange for underlying ('NSE_INDEX', 'BSE_INDEX', 'NFO')
        legs: List of leg dictionaries (1-20 legs). Each leg must contain:
            Required:
            - offset: Strike offset ('ATM', 'ITM1'-'ITM50', 'OTM1'-'OTM50')
            - option_type: 'CE' for Call or 'PE' for Put
            - action: 'BUY' or 'SELL'
            - quantity: Absolute quantity — must be a multiple of the contract lot size.
                        Do NOT hardcode lot size. Look up the current 'lotsize' per leg
                        using get_option_symbol() or get_option_chain() first, then pass
                        quantity = lots * lotsize. Lot sizes can change (e.g., NIFTY has
                        changed multiple times) and differ by underlying.
            Optional:
            - expiry_date: Per-leg expiry in DDMMMYY format for diagonal/calendar spreads
            - pricetype: 'MARKET', 'LIMIT', 'SL', 'SL-M' (default: MARKET)
            - product: 'MIS', 'NRML' (default: MIS)
            - price: Limit price for LIMIT orders
            - trigger_price: Trigger price for SL orders
            - disclosed_quantity: Disclosed quantity
        expiry_date: Default expiry date in format 'DDMMMYY' (e.g., '25NOV25') for all legs

    Returns:
        JSON with underlying, underlying_ltp, mode, and results array containing each leg's
        orderid, symbol, offset, option_type, action, and status

    Example - Iron Condor (same expiry):
        [
            {"offset": "OTM10", "option_type": "CE", "action": "BUY", "quantity": 75},
            {"offset": "OTM10", "option_type": "PE", "action": "BUY", "quantity": 75},
            {"offset": "OTM5", "option_type": "CE", "action": "SELL", "quantity": 75},
            {"offset": "OTM5", "option_type": "PE", "action": "SELL", "quantity": 75}
        ]

    Example - Bull Call Spread with NRML:
        [
            {"offset": "ATM", "option_type": "CE", "action": "BUY", "quantity": 75, "product": "NRML"},
            {"offset": "OTM1", "option_type": "CE", "action": "SELL", "quantity": 75, "product": "NRML"}
        ]

    Example - Diagonal Spread (different expiry):
        [
            {"offset": "ITM2", "option_type": "CE", "action": "BUY", "quantity": 75, "expiry_date": "30DEC25"},
            {"offset": "OTM2", "option_type": "CE", "action": "SELL", "quantity": 75, "expiry_date": "25NOV25"}
        ]

    Example - Long Straddle with LIMIT orders:
        [
            {"offset": "ATM", "option_type": "CE", "action": "BUY", "quantity": 30, "pricetype": "LIMIT", "price": 250.0},
            {"offset": "ATM", "option_type": "PE", "action": "BUY", "quantity": 30, "pricetype": "LIMIT", "price": 250.0}
        ]"#,
        params: &[
            Param {
                name: "underlying",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "legs",
                ty: ObjectList,
                default: D::Required,
            },
            Param {
                name: "expiry_date",
                ty: OptStr,
                default: D::Null,
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
        ],
    },
    ToolDef {
        name: "modify_order",
        title: "Modify Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Modify an existing order.

    Args:
        order_id: Order ID to modify
        symbol: Stock symbol
        action: 'BUY' or 'SELL'
        exchange: Exchange name
        product: 'CNC', 'NRML', 'MIS'
        quantity: New quantity
        price: New price (required by the API — use current price if unchanged)
        strategy: Strategy name (defaults to 'python mcp')
        price_type: 'MARKET', 'LIMIT', 'SL', 'SL-M' (defaults to 'LIMIT')
        trigger_price: New trigger price for SL/SL-M orders (default 0)
        disclosed_quantity: New disclosed quantity (default 0)"#,
        params: &[
            Param {
                name: "order_id",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "symbol",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "action",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "exchange",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "product",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "quantity",
                ty: Int,
                default: D::Required,
            },
            Param {
                name: "price",
                ty: Num,
                default: D::Required,
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
            Param {
                name: "price_type",
                ty: Str,
                default: D::Str("LIMIT"),
            },
            Param {
                name: "trigger_price",
                ty: Num,
                default: D::Int(0),
            },
            Param {
                name: "disclosed_quantity",
                ty: Int,
                default: D::Int(0),
            },
        ],
    },
    ToolDef {
        name: "cancel_order",
        title: "Cancel Order",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Cancel a specific order.

    Args:
        order_id: Order ID to cancel
        strategy: Strategy name (defaults to 'python mcp')"#,
        params: &[
            Param {
                name: "order_id",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
        ],
    },
    ToolDef {
        name: "cancel_all_orders",
        title: "Cancel All Orders",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Cancel all open orders for a strategy.

    Args:
        strategy: Strategy name (defaults to 'python mcp')"#,
        params: &[Param {
            name: "strategy",
            ty: Str,
            default: D::Str("python mcp"),
        }],
    },
    ToolDef {
        name: "close_all_positions",
        title: "Close All Positions",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Close all open positions for a strategy.

    Args:
        strategy: Strategy name (defaults to 'python mcp')"#,
        params: &[Param {
            name: "strategy",
            ty: Str,
            default: D::Str("python mcp"),
        }],
    },
    ToolDef {
        name: "analyzer_toggle",
        title: "Toggle Analyzer Mode",
        toolset: "orders",
        scope: Scope::WriteOrders,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: true,
        open_world: true,
        description: r#"Toggle the analyzer mode between analyze (simulated) and live trading.

    Args:
        mode: True for analyze mode (simulated), False for live mode

    Returns:
        JSON with updated analyzer status:
        - data.analyze_mode, data.message, data.mode, data.total_logs
        - status: 'success' or 'error'

    Example:
        analyzer_toggle(True)  # Switch to analyze mode (simulated responses)
        analyzer_toggle(False) # Switch to live trading mode"#,
        params: &[Param {
            name: "mode",
            ty: Bool,
            default: D::Required,
        }],
    },
];

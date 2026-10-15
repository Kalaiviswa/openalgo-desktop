//! The `account` toolset: the order, trade and position books, holdings,
//! funds, margin and the analyzer status.

use crate::mcp::schema::{Dflt as D, Param, Ty::*};
use crate::mcp::{Risk, Scope, ToolDef};

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_open_position",
        title: "Get Open Position",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get current open position for a specific instrument.

    Args:
        symbol: Stock symbol
        exchange: Exchange name
        product: Product type ('CNC', 'NRML', 'MIS')
        strategy: Strategy name (defaults to 'python mcp')"#,
        params: &[
            Param {
                name: "symbol",
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
                name: "strategy",
                ty: Str,
                default: D::Str("python mcp"),
            },
        ],
    },
    ToolDef {
        name: "get_order_status",
        title: "Get Order Status",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::ExternalText,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get status of a specific order.

    Args:
        order_id: Order ID
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
        name: "get_order_book",
        title: "Get Order Book",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::ExternalText,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get all orders from the order book."#,
        params: &[],
    },
    ToolDef {
        name: "get_trade_book",
        title: "Get Trade Book",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::ExternalText,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get all executed trades."#,
        params: &[],
    },
    ToolDef {
        name: "get_position_book",
        title: "Get Position Book",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get all current positions."#,
        params: &[],
    },
    ToolDef {
        name: "get_holdings",
        title: "Get Holdings",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get all holdings (long-term investments)."#,
        params: &[],
    },
    ToolDef {
        name: "get_funds",
        title: "Get Funds",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get account funds and margin information."#,
        params: &[],
    },
    ToolDef {
        name: "calculate_margin",
        title: "Calculate Margin",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Calculate margin requirements for positions.

    Args:
        positions: List of position dictionaries
        Example: [{"symbol": "NIFTY25NOV2525000CE", "exchange": "NFO", "action": "BUY", "product": "NRML", "pricetype": "MARKET", "quantity": "75"}]

        For Futures: [{"symbol": "NIFTY25NOV25FUT", "exchange": "NFO", "action": "BUY", "product": "NRML", "pricetype": "MARKET", "quantity": "25"}]
        For Options: [{"symbol": "NIFTY25NOV2525500CE", "exchange": "NFO", "action": "BUY", "product": "NRML", "pricetype": "MARKET", "quantity": "75"}]

    Returns:
        JSON with total_margin_required, span_margin, and exposure_margin"#,
        params: &[Param {
            name: "positions",
            ty: ObjectList,
            default: D::Required,
        }],
    },
    ToolDef {
        name: "analyzer_status",
        title: "Get Analyzer Status",
        toolset: "account",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: true,
        description: r#"Get the current analyzer status including mode and total logs.

    Returns:
        JSON with analyzer status information:
        - data.analyze_mode: Boolean indicating if analyzer is active
        - data.mode: Current mode ('analyze' or 'live')
        - data.total_logs: Number of logs in analyzer
        - status: 'success' or 'error'"#,
        params: &[],
    },
];

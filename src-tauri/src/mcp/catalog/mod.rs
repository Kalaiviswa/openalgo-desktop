//! The tool catalogue: name, title, toolset, scope, output risk, hints,
//! description and parameters of every tool in the web's MCP server
//! (`openalgo/mcp/mcpserver.py`, scopes from `utils/mcp_tool_registry.py`).
//!
//! Ported verbatim from the web's `tools/list`; the contract test
//! (`tests/it/mcp_contract.rs`) compares the descriptors built from this
//! table with `tests/fixtures/mcp/tools_list.json` and fails on any drift.
//! Edit a description here only together with that fixture.
//!
//! Each toolset has its own file (`orders`, `account`, `market_data`,
//! `utility`, `research`), its tools in the web's order.

use super::ToolDef;
use std::sync::LazyLock;

mod account;
mod market_data;
mod orders;
mod research;
mod utility;

/// Every tool, a toolset after another. `tools/list` sorts by name, so this
/// order is not seen by a client.
pub static TOOLS: LazyLock<Vec<ToolDef>> = LazyLock::new(|| {
    [
        orders::TOOLS,
        account::TOOLS,
        market_data::TOOLS,
        utility::TOOLS,
        research::TOOLS,
    ]
    .concat()
});

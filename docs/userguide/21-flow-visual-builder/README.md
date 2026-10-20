# 21 - Flow Visual Builder

## Not in OpenAlgo Desktop

OpenAlgo web has **Flow** (`/flow`), a drag-and-drop builder for trading
workflows. OpenAlgo Desktop does not have it: it was left out of the desktop
with the other features that need a Python runtime (maintainer decision,
2026-10-03). There is no `/flow` page and no menu entry for it.

To build visual workflows, use
[OpenAlgo web](https://github.com/marketcalls/openalgo)
([docs](https://docs.openalgo.in)).

In the desktop, the nearest ways to automate without writing a server of
your own are:

- the **Strategies** page (the strategy module), which runs entries, exits,
  stops and targets from TradingView and other webhooks;
- [Chartink strategies](../18-chartink-integration/README.md);
- OpenScript studies and strategies in the
  [Charting Terminal](../32-charting-terminal/README.md).

---

**Previous**: [20 - Python Strategies](../20-python-strategies/README.md)

**Next**: [22 - Action Center](../22-action-center/README.md)

# 20 - Python Strategies

## Not in OpenAlgo Desktop

OpenAlgo web has a **Python Strategy Host** (`/python`): you upload a Python
script and OpenAlgo starts, schedules and stops it for you. OpenAlgo Desktop
does not have it. The desktop ships without a Python runtime (maintainer
decision, 2026-10-03), so there is no `/python` page, no menu entry for it,
and the Telegram bot has no `/stoppython` command.

To have OpenAlgo host and schedule your Python scripts, use
[OpenAlgo web](https://github.com/marketcalls/openalgo)
([docs](https://docs.openalgo.in)).

## Running Python against the desktop yourself

Your Python strategies still work with OpenAlgo Desktop as ordinary scripts
that you start yourself. The desktop's API and market data feed are the same
as the web's, so the OpenAlgo Python SDK needs only the desktop's API key:

```python
from openalgo import api

client = api(
    api_key="<APIKEY>",
    host="http://127.0.0.1:5000",
    ws_url="ws://127.0.0.1:8765",
)
print(client.funds())
```

- Install the SDK in your own Python with `pip install openalgo`.
- Copy the API key from the **API Key** page of the desktop.
- Keep OpenAlgo Desktop open and signed in while the script runs. After a
  restart the API answers "Sign in to OpenAlgo to use the API" until you sign
  in.
- Test with analyzer mode on first: the script's orders then go to the
  sandbox.

For strategies that OpenAlgo Desktop runs for you, see the **Strategies**
page (the strategy module, driven by TradingView and other webhooks) and
OpenScript in the [Charting Terminal](../32-charting-terminal/README.md).

---

**Previous**: [19 - GoCharting Integration](../19-gocharting-integration/README.md)

**Next**: [21 - Flow Visual Builder](../21-flow-visual-builder/README.md)

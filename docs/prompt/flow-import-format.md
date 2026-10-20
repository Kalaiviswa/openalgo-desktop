# Flow import format

Flow, OpenAlgo web's visual workflow builder (`/flow`), is not in OpenAlgo
Desktop. Its workflows run on a Python executor, and the desktop needs no
Python, so there is no Flow page and no place to import a Flow JSON file.

To build and run Flow workflows, use OpenAlgo web. Its Flow import format is
documented there, in `docs/prompt/flow-import-format.md` of the OpenAlgo
repository.

In the desktop, the nearest equivalents are:

- the strategy module (`/strategy`) for multi-leg options strategies and
  TradingView signal strategies;
- a Python script using the [OpenAlgo Python SDK](openalgo%20python%20sdk.md)
  against the desktop's API, for custom logic.

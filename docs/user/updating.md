# Updating OpenAlgo Desktop

- [Installing a new version](#installing-a-new-version)
- [Your data is backed up before an update](#your-data-is-backed-up-before-an-update)
- [Going back to an earlier version](#going-back-to-an-earlier-version)

## Installing a new version

Every version is on the
[releases page](https://github.com/marketcalls/openalgo-desktop/releases).
Close OpenAlgo Desktop, download the installer for your system and install it
over the old version, as described in the README under "Download and
install". Your account, settings, broker keys, strategies, sandbox data and
Historify data are kept.

## Your data is backed up before an update

The first time a new version opens your data folder, it backs up your data
before changing anything:

- every database (`openalgo.db`, `logs.db`, `sandbox.db`,
  `historify.duckdb`) and the key file (`vault.json`, when you use one) is
  copied into `backups/<old version>-<date and time>/` inside the data folder
  (where the data folder is: README, "Where your data lives, and backups"),
  readable by your user account only;
- only then does the new version update the databases to its format;
- as soon as the new version has opened your data, the copy is deleted.

The copy is only a safety net for that one start. OpenAlgo keeps no older
copy of your account afterwards, so broker keys you remove, a broker session
that ended, an API key you regenerated or a password you changed or reset
cannot come back from a backup. If a copy is ever left behind (for example
because another program had a file open when OpenAlgo tried to delete it),
OpenAlgo deletes it the next time it starts, and also when you sign out of
your broker, forget a broker, reset your password or reset your account.

That first start can take a little longer when Historify holds a lot of
data, and the backup needs about as much free disk space as your data. If
it cannot be made (a full disk), OpenAlgo changes nothing and asks you to
free some space and start it again.

If the new version cannot open the data after the backup, it puts the
backup back, so everything is exactly as the old version left it, and says
so. Start OpenAlgo again to retry; if it happens again, install the previous
version from the release page: it opens the data as before. In the rare case
the copy cannot be put back, OpenAlgo keeps it, tells you where it is and how
to copy it back by hand. That copy is your data exactly as the previous
version left it a moment before.

OpenAlgo never offers to put back a copy at any other time.

## Going back to an earlier version

`data-version.json` in the data folder records the version that last opened
it. An older version will not open a folder a newer version has used: it
says "Your OpenAlgo data was last opened by version ..." and changes
nothing, because an older version could damage data in a newer format.

OpenAlgo does not keep the data of the earlier version once a newer one
has opened it (see above), so there is no copy to go back to. To use an
earlier version anyway, close OpenAlgo, rename the data folder (for example
to `com.openalgo.desktop-newer`), install the earlier version and start it.
It starts with a new data folder, where you create your account and add
your broker again.

A copy of the whole data folder that you made yourself can also be put
back (README, "Where your data lives, and backups"). It brings back
everything as it was when you made it, including broker keys you have
removed since and the API key of that time, so sign out of your broker and
generate a new API key once it opens if anything has changed since then.

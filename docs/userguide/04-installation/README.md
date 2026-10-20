# 04 - Installation

## Introduction

OpenAlgo Desktop installs like any other app: download one file, open it,
done. There is no Python, no `git clone`, no `.env` file and no server to
configure. This chapter covers each system, the warning unsigned installers
show the first time, where your data is kept, and how to run OpenAlgo Desktop
next to OpenAlgo web.

Coming from OpenAlgo web? Read
[Moving from OpenAlgo web](../../user/moving-from-openalgo-web.md) as well: it
lists what carries over and what you set up again.

## Download

Download the installer for your computer from the
[GitHub Releases page](https://github.com/marketcalls/openalgo-desktop/releases).

| Computer | File to download |
| --- | --- |
| Windows 10 or 11 (64-bit) | ends in `_x64-setup.exe` |
| Mac with Apple Silicon (M1 and later) | ends in `_aarch64.dmg` |
| Mac with an Intel processor | ends in `_x64.dmg` |
| Linux, 64-bit PC (Ubuntu, Debian, Mint) | ends in `_amd64.deb` or `_amd64.AppImage` |
| Linux, 64-bit ARM (ARM64) | ends in `_arm64.deb` or `_aarch64.AppImage` |

Not sure which Mac you have? Open the Apple menu, then **About This Mac**.
"Chip: Apple M..." means Apple Silicon; "Processor: Intel" means Intel.

There is no Raspberry Pi installer. On a Pi, use
[OpenAlgo web](https://github.com/marketcalls/openalgo).

### Check the download (optional)

Each release lists a `SHA256SUMS.txt` file. Compare your file's checksum with
its line there:

| System | Command |
| --- | --- |
| Windows (PowerShell) | `Get-FileHash .\<file> -Algorithm SHA256` |
| macOS | `shasum -a 256 <file>` |
| Linux | `sha256sum <file>` |

## Install

### Windows

1. Run the `_x64-setup.exe` file.
2. It installs for your user only and does not need administrator rights.
3. If WebView2 is missing (some Windows 10 PCs), the setup downloads and
   installs it, which needs an internet connection.

A computer with no internet connection and no WebView2 cannot install from
the setup program alone: install Microsoft's WebView2 "Evergreen Standalone
Installer" first, then run the OpenAlgo setup.

### macOS

1. Open the `.dmg` file.
2. Drag **OpenAlgo Desktop** to **Applications**.

### Linux (x64 or ARM64)

**`.deb` (Ubuntu, Debian, Mint):**

```bash
sudo apt install ./<file>.deb
```

This also installs WebKitGTK 4.1 and GTK 3 if they are missing. Start the app
from your desktop's app menu, or run `openalgo-desktop` in a terminal.

**AppImage:**

```bash
chmod +x OpenAlgo*.AppImage
./OpenAlgo*.AppImage
```

If it says FUSE is missing, install it with `sudo apt install libfuse2` (on
Ubuntu 24.04 the package is `libfuse2t64`), or use the `.deb` instead.

## Opening the app the first time (unsigned installers)

The installers are not yet code-signed, so Windows and macOS warn you the
first time. This is expected. Download only from the GitHub Releases page, and
check the checksum if you want to be sure the file is the one published.

**Windows.** SmartScreen shows "Windows protected your PC". Choose **More
info**, then **Run anyway**.

**macOS.** macOS says the app "cannot be opened because Apple cannot check it
for malicious software", or that it is from an unidentified developer.
Either:

- In Finder, open **Applications**, right-click (or Control-click) **OpenAlgo
  Desktop**, choose **Open**, then **Open** again; or
- Try to open the app once, then go to **System Settings**, **Privacy &
  Security**, scroll to the message about OpenAlgo Desktop, choose **Open
  Anyway**, and confirm. On macOS 15 (Sequoia) and later this is the way that
  works.

You only do this once. If macOS says the app "is damaged and can't be
opened", it is the same check. In Terminal, run:

```bash
xattr -dr com.apple.quarantine "/Applications/OpenAlgo Desktop.app"
```

and open it again.

**Linux** shows no warning.

## Starting and quitting

| System | Start | Quit |
| --- | --- | --- |
| Windows | **Start** menu, then **OpenAlgo Desktop** (or the desktop shortcut) | Close the window, or right-click its taskbar icon and choose **Close window** |
| macOS | **Applications** or **Launchpad**, then **OpenAlgo Desktop** | **OpenAlgo Desktop** menu, then **Quit** (Cmd+Q). Closing the window also quits |
| Linux `.deb` | App menu, then **OpenAlgo Desktop**, or `openalgo-desktop` in a terminal | Close the window |
| Linux AppImage | `./OpenAlgo*.AppImage` from its folder | Close the window |

- **Only one copy runs at a time.** Both copies would need the same ports.
- **Quitting asks first** when something is running (strategies, OpenScript
  runs, scalping stops, open orders): **Quit** or **Keep OpenAlgo Open**.
  Quitting from the Dock, shutting down or a forced quit cannot ask.
- **The dashboard also opens in a browser** on the same computer, at
  `http://127.0.0.1:5000`, while the app runs.

## What you see the first time

1. The app opens on the OpenAlgo home page. Choose **Login**.
2. The sign-in card shows **First time? Set up your account**. Choose it.
3. Create your account. [Chapter 05](../05-first-time-setup/README.md)
   continues from here.

## If port 5000 is already in use

If another program holds port 5000, the app opens on a page that says
"OpenAlgo could not start because port 5000 is already used by another
program", with a **Port** box and a **Try again** button.

- **On a Mac this is usually AirPlay Receiver.** Open **System Settings**,
  **General**, **AirDrop & Handoff**, turn off **AirPlay Receiver**, then
  choose **Try again**.
- **OpenAlgo web, or a second copy of OpenAlgo Desktop**, may be running.
  Close it and choose **Try again**.
- **Or use another port.** Type a free port (for example `5001`) in the
  **Port** box and choose **Try again**. OpenAlgo remembers it. Then use the
  new port in your broker app's redirect URL
  (`http://127.0.0.1:5001/<broker>/callback`), in your scripts' base URL and
  in your AI client's MCP configuration.

If port 8765 (market data) is taken, the app still opens, but live market
data for your SDK and trading platforms does not start. **Server Settings**
shows "Live market data is not running" with the reason. Close the other
program, or choose another **Market data port** there.

More in [Troubleshooting](../../user/troubleshooting.md#the-app-says-a-port-is-already-in-use).

## Running alongside OpenAlgo web

OpenAlgo web and OpenAlgo Desktop use the same default ports (5000 and 8765),
so only one of them can have them.

**Simplest: run one at a time.** Stop OpenAlgo web before you open OpenAlgo
Desktop, and the other way round. Your tools, broker redirect URLs and SDK
scripts then work with both unchanged, because both use the same addresses.

**Side by side:** give OpenAlgo Desktop its own ports.

1. Open OpenAlgo Desktop while OpenAlgo web runs. It opens on the port page.
   Type a free port, for example `5001`, and choose **Try again**.
2. Sign in, open the profile menu, then **Server Settings**. Change **Market
   data port** to a free port, for example `8766`, and choose **Save**.
   Market data moves to the new port within a few seconds.
3. Use the desktop's ports wherever a tool talks to the desktop:

   | Setting | For OpenAlgo Desktop |
   | --- | --- |
   | SDK `host` | `http://127.0.0.1:5001` |
   | SDK `ws_url` | `ws://127.0.0.1:8766` |
   | Broker app redirect URL | `http://127.0.0.1:5001/<broker>/callback` |
   | MCP `--url` | `http://127.0.0.1:5001` |

4. A broker app usually has one redirect URL. If OpenAlgo web uses the same
   broker app on port 5000, create a second app at your broker for the
   desktop, or go back to running one at a time.

| Server Settings field | Default | Takes effect |
| --- | --- | --- |
| **App port** | 5000 | When you restart OpenAlgo Desktop |
| **App address** | `127.0.0.1` | When you restart OpenAlgo Desktop |
| **Market data port** | 8765 | Within a few seconds; connected tools are disconnected and must reconnect |
| **Allow access from other devices** | Off | The app address after a restart; the market data address within a few seconds |

Ports must be between 1024 and 65535, and the app port and the market data
port must differ.

## Where your data lives

Everything OpenAlgo Desktop stores is in one folder:

| System | Folder |
| --- | --- |
| Windows | `%APPDATA%\com.openalgo.desktop` |
| macOS | `~/Library/Application Support/com.openalgo.desktop` |
| Linux (x64 or ARM64) | `~/.local/share/com.openalgo.desktop` |

Inside it:

| File | Holds |
| --- | --- |
| `openalgo.db` | Your account, settings, strategies, encrypted broker credentials, and the locked keys |
| `sandbox.db` | Analyzer mode (the sandbox), apart from live trading |
| `logs.db` | Order and API logs |
| `historify.duckdb` | Historify market data |
| OpenScript and indicator files | Your studies and custom chart indicators |

**To back up**, quit OpenAlgo Desktop and copy the whole folder. Restore it by
quitting and copying the folder back. A backup opens on any computer with the
password you had when you made it. More in the README:
[Where your data lives, and backups](../../../README.md#where-your-data-lives-and-backups).

## Updating

OpenAlgo Desktop checks for a newer version each time it starts, and from the
profile menu, **Check for Updates**. Nothing is downloaded until you choose
**Update**. A `.deb` install is updated by installing the new `.deb` over the
old one. Your data is kept either way. See
[Updating OpenAlgo Desktop](../../user/updating.md).

## Uninstalling

| System | How |
| --- | --- |
| Windows | **Settings**, **Apps** (**Installed apps** on Windows 11, **Apps & features** on Windows 10), **OpenAlgo Desktop**, **Uninstall** |
| macOS | Quit the app, then drag **OpenAlgo Desktop** from **Applications** to the Trash |
| Linux `.deb` | Remove it with your software manager or `apt` |
| Linux AppImage | Delete the `.AppImage` file |

Uninstalling removes the app. To remove your account and data too, delete
the data folder above (back it up first if you may come back).

## Troubleshooting installation

| Problem | Fix |
| --- | --- |
| Windows: "Windows protected your PC" | **More info**, then **Run anyway** |
| macOS: "cannot be opened" or "unidentified developer" | Right-click, **Open**, or **Privacy & Security**, **Open Anyway** |
| macOS: "is damaged and can't be opened" | `xattr -dr com.apple.quarantine "/Applications/OpenAlgo Desktop.app"` |
| Linux AppImage: FUSE missing | `sudo apt install libfuse2` (`libfuse2t64` on Ubuntu 24.04), or use the `.deb` |
| Linux: WebKitGTK 4.1 not found | Install the `.deb` with `apt`, which pulls it in; Ubuntu 20.04 and Debian 11 are too old |
| "Port 5000 is already used" | See [If port 5000 is already in use](#if-port-5000-is-already-in-use) |
| "OpenAlgo Desktop is already running" | Another OpenAlgo window is open. Close it, then start again |
| "Your OpenAlgo data was last opened by version X" | You started an older version after a newer one. See [Going back to an earlier version](../../user/updating.md#going-back-to-an-earlier-version) |

## Next steps

1. [05 - First-time setup](../05-first-time-setup/README.md): create your account.
2. [06 - Broker connection](../06-broker-connection/README.md): add your broker's keys and log in.

---

**Previous**: [03 - System requirements](../03-system-requirements/README.md)

**Next**: [05 - First-time setup](../05-first-time-setup/README.md)

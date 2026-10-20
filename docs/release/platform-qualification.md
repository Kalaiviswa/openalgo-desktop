# Platform qualification

README "Supported systems" is the contract. CI proves part of it on every
change; the rest needs a real machine and a person, and is recorded here.
This page is the checklist for that person (the maintainer, or a tester
the maintainer names), and the record of what was run.

## What CI already proves on every change

Each pull request and each push to `master` (`.github/workflows/ci.yml`,
required by `ci-ok`, and by the release gate on the tagged commit):

| Check | Systems | Where |
| --- | --- | --- |
| Rust tests, Rust coverage floors, frontend tests and coverage thresholds | Ubuntu 22.04 x64 and ARM64, macOS (Apple Silicon), Windows Server 2025 | `rust-test`, `rust-coverage`, `frontend-test` |
| Installers built | Windows x64 NSIS, macOS Apple Silicon and Intel `.dmg`, Linux x64 and ARM64 `.deb` and AppImage | `tauri-build` |
| Install on a fresh machine, first start, account created, signed in, a wrong password refused, keys locked until the sign-in (KEY-01), the app's own window loaded its page | `.deb` on Ubuntu 22.04 x64 and ARM64; setup program on Windows Server 2022 and 2025; `.dmg` on Apple Silicon macOS | `install-smoke` (`launch_smoke.py --first-sign-in --window`) |
| AppImage starts (FUSE absent: extract-and-run) | Ubuntu 22.04 x64 and ARM64 | `install-smoke` |
| `.deb` depends on everything the binary links | Ubuntu 22.04 x64 and ARM64 | `install-smoke` (`check_deb.sh`) |
| Both Mac apps ask for no newer macOS than claimed (10.15; Apple Silicon from 11.0) | Apple Silicon and Intel apps, checked on the Apple Silicon runner | `install-smoke` (`check_macos_min.py`) |
| The data folder and every file in it are closed to Everyone, Users and the other broad groups | Windows Server 2022 and 2025 | `install-smoke` (`check_windows_acl.ps1`) |
| Packaged `mcp` subcommand over piped stdio, alone and against the running app | every installed app above | `install-smoke` (`mcp_stdio_smoke.py`) |
| The installed app driven through its window: account, wrong password, sign-in, signed-in shell, sign-out | `.deb` on Ubuntu 22.04 x64 (WebKitGTK) | `native-e2e` |

What CI cannot show: a computer other than GitHub's build machines, an
Intel Mac or the oldest macOS actually running the app, Windows 10 and 11,
WebView2 provisioning without internet or under policy, other Linux
distributions and desktops, a real broker sign-in in a browser, sleep and
wake, and signed installers. That is this checklist.

## Before each release

On the release's draft installers (the draft release's assets, checked
against its `SHA256SUMS.txt`), in sandbox mode, never placing a live order:

1. **Fresh install, first run.** As a standard (not administrator) user:
   install, start, create the account, sign in, connect a broker you use
   (its sign-in opens in the system browser and comes back to
   `http://127.0.0.1:5000/<broker>/callback`), see quotes stream, place and
   cancel one sandbox order, quit. Start again: signed out, keys locked;
   sign in: the broker session resumes (before 03:00 IST).
2. **Update from the previous release** (once one exists): once in the app
   (Check for Updates, then Update; on a `.deb`, the link to the release
   page) and once by installing over it by hand. Account, settings, broker
   keys, API key, strategies and sandbox data are kept; you sign in again.
   Then start the previous version on the updated data: it must refuse,
   name the newer version and change nothing (README, "Updating OpenAlgo
   Desktop").
3. **Backup and restore.** Close the app, copy the data folder (README,
   "Where your data lives, and backups"), restore it on another computer or
   user account, sign in with the same password: everything opens. A wrong
   password must not.
4. **Forgotten password.** Forgot password on the sign-in page sets a new
   password: account, settings, strategies and history stay; broker keys,
   the API key in your tools and two-factor login are set up again. Reset
   account, on the same page, removes the account and its keys.
5. **Quit, sleep and wake** (ARCH-03, PLATFORM-05). With a sandbox
   strategy or scalping stop running: Quit asks first and names what stops;
   close the lid or sleep for 10 minutes across a scheduled time, wake: the
   feed reconnects, the order books refresh, the scheduled action runs once.
   On Windows over Remote Desktop: disconnecting keeps the app running;
   signing out stops it.
6. **MCP with real clients** (version-bump skill): Claude Desktop and
   Claude Code with a read token: tool list and one quote.
7. **Signed installers** (PLATFORM-06, once the certificates exist): the
   release notes say each installer was found signed; on a clean machine the
   first open shows no Gatekeeper or SmartScreen warning for an unknown
   developer. Never qualify by turning those protections off.

## Once per system before calling it Supported

| System | Also check |
| --- | --- |
| Windows 11 x64 | Per-user install without administrator rights; the data folder's ACL (`icacls "%APPDATA%\com.openalgo.desktop"` lists only you, SYSTEM and Administrators; also on a redirected or roaming profile if you use one) |
| Windows 10 x64 | As 11, plus WebView2 three ways: already present; missing with internet (the setup downloads it); missing offline (install Microsoft's Evergreen Standalone Installer first, as the README says) and under an enterprise policy that manages WebView2 |
| Windows Server 2022 / 2025 with Desktop Experience | CI installs, starts and signs in; by hand: a Remote Desktop session, disconnect versus sign-out, a reboot |
| Windows Server 2019 with Desktop Experience | Stays "Not tested" until one full run of this checklist |
| macOS on Apple Silicon | Opening the unsigned app the first time (README "Opening the app the first time"); dragged to Applications, not run from the `.dmg` |
| macOS on Intel | The Intel `.dmg` on an Intel Mac (CI builds and checks it but no Intel Mac runs it) |
| macOS 10.15 | One first run on 10.15, or raise `minimumSystemVersion` and the README to the oldest macOS that was run |
| Ubuntu 22.04 / 24.04, Debian 12, Linux Mint 21 (x64) | `.deb` with `sudo apt install ./<file>.deb`; the AppImage with FUSE 2 installed and without (README); an X11 and a Wayland session |
| Ubuntu or Debian on ARM64 | As x64, on an ARM laptop, desktop or virtual machine (not a Raspberry Pi: out of scope) |

## Results

One row per run. "Pass" means every step of the section passed; otherwise
name the step and the issue filed.

| Date | Version and commit | System (build) | Machine | Section | Result | By |
| --- | --- | --- | --- | --- | --- | --- |
| | | | | | | |

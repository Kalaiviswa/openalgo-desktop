# Scanner triage 2026-10-22

Base: `763cc12a` (the 1.0.1 candidate). The open CodeQL alerts were seen
at `7b999e49`, the commit the Codex production audit of 2026-10-22
reviewed, and again at master `4221c372` (Rust analysis 2006354515);
`src-tauri/src/security/hashing.rs` and `src-tauri/src/brokers/families/xts/`
are identical at all three commits, so the line numbers below hold for
each. Scope: PROD-SEC-02 and PROD-SEC-03 of that audit
(`03-security-and-credentials.md`), and a check of PROD-SEC-01 at this
base. Reviewer: Claude (agent) for the maintainer, who records the GitHub
dispositions from this file.

## Summary

- **npm (PROD-SEC-02): fixed, not accepted.** The one dev-only advisory
  (four package entries) is gone from the tree. All dependencies: 4 high
  before, 0 after. Production: 0 before and after.
- **CodeQL (PROD-SEC-03): 21 open alerts, none a defect in the shipped
  app.** 19 are fixed test values in `security::hashing`'s inline test
  module: **fixed** by moving that module to its own `tests.rs`, which the
  CodeQL configuration excludes, so they close at the first master scan
  after this branch merges, and later edits to `hashing.rs` raise no new
  round. 2 (the XTS subscription call) are a **false positive** that no
  reasonable code change ends: the flagged data is Angel's client code,
  which CodeQL carries into the XTS feed through a call on
  `Box<dyn BrokerFeed>`; they are for the maintainer to dismiss.
- **Accepted advisories (PROD-SEC-03).** The five RustSec ids in
  `.github/deny.toml` and the six ids in `.trivyignore` were re-checked
  against this base; each now carries the date it was accepted, the date
  it was re-checked and a review date (2027-01-31).
- **PROD-SEC-01: closed at this base**, with one gap closed here (a stored
  entry already at the current version could still put a key into memory)
  and the missing tests added. See the last section.

## Scanners

| Tool | Version | Command | Result |
| --- | --- | --- | --- |
| npm audit, all dependencies | npm 10.9.8, Node 22.23.1 | `npm audit --json` | Before: 4 high (one advisory, see below), exit 1. After: 0, exit 0 |
| npm audit, production | same | `npm audit --omit=dev --audit-level=high` | 0 before and after, exit 0 |
| cargo deny | 0.20.2 | `cargo deny --manifest-path src-tauri/Cargo.toml --config .github/deny.toml --locked check advisories bans licenses sources` | advisories ok, bans ok, licenses ok, sources ok, exit 0, with the dated `deny.toml` (no unused ignore reported); 56 duplicate-version warnings, which are dependency hygiene, not advisories |
| cargo audit | 0.22.2 | `cargo audit --file src-tauri/Cargo.lock` with the ignores read from `deny.toml`, as `security.yml` does | Exit 0 with the five ids ignored, and 6 allowed warnings (below). Without the ignores (`--json`): exactly the five accepted rustls-webpki 0.102.8 advisories, exit 1 |
| gitleaks | 8.30.1 | `gitleaks git -c .github/gitleaks.toml --log-opts=763cc12a..HEAD .`, and `gitleaks dir` over the changed paths | No leaks |
| Trivy | not installed here | | Not run locally. Its last run, at `7b999e49` (Security Scan 39789029523), kept no results under `.trivyignore`; this branch changes no `Cargo.lock` and only removes npm packages, and `security.yml` runs Trivy again on the lockfile change |
| CodeQL | GitHub | `gh api 'repos/marketcalls/openalgo-desktop/code-scanning/alerts?state=open' --paginate` | 21 open, triaged below |
| Dependabot | GitHub | | 0 open (the audit's query); nothing to dismiss |

## npm: the braces chain (PROD-SEC-02)

[GHSA-vfj7-8cjw-p6xm](https://github.com/advisories/GHSA-vfj7-8cjw-p6xm)
(CVE-2026-93687, high): `braces` up to 3.0.3 exhausts the stack on deeply
nested brace patterns. No patched `braces` exists. npm reported it four
times, once per package on the way down:

```
@types/jest-axe 3.5.9 -> @types/jest 30.0.0 -> expect 30.2.0
  -> jest-message-util 30.2.0 -> micromatch 4.0.8 -> braces 3.0.3
```

Every entry was `dev: true`. The chain came in only through type
declarations: `@types/jest-axe` declares `@types/jest`, which declares
`expect`. Nothing the app ships reached it, and no test passes a brace
pattern from outside the repository to it. `jest-axe` itself runs its own
nested `jest-matcher-utils 29.2.2`, which has no `micromatch`.

`jest-message-util` 30.5.1 replaced `micromatch` with `picomatch`, so
`npm update expect` (30.5.2, inside `@types/jest`'s `^30.0.0`) takes
`braces`, `micromatch`, `fill-range`, `is-number` and `to-regex-range` out
of the tree. `package.json` is unchanged; every changed lockfile entry is
dev-only and resolved from registry.npmjs.org. `npx tsc -b` passes;
`npx tsc -p tsconfig.test.json --noEmit` prints byte-identical output on
the old and the new lockfile (that project has errors of its own and is
not a CI gate); the four test files that use `jest-axe` pass (40 tests).

`security.yml` now writes the all-dependency report (`npm-audit-all.json`)
into the npm-audit artifact beside the production report, never failing;
the gate stays `--omit=dev --audit-level=high`. The `security-audit` skill
runs the same report at every periodic review.

## CodeQL (PROD-SEC-03)

### The 21 open alerts

Locations are at `7b999e49` (and master `4221c372`, and the base).
`hashing.rs`'s test module ran from line 324 (`#[cfg(test)]`, `mod tests {`)
to line 649 (its closing brace, the end of the file), so every hashing
location below is inside it. The only `hashing.rs` result outside it in
the master analysis is line 241, `derive_kek`'s zeroed output buffer, which
is alert 256, dismissed earlier as a false positive and not among these 21.

| Alert | Rule | Location | What it flags | Disposition |
| --- | --- | --- | --- | --- |
| 231 | hard-coded-cryptographic-value | `hashing.rs:346:49` | `"Secret@123"` in `a_refused_map_falls_back_to_the_heap_and_signs_in` | Fixed (moved to `hashing/tests.rs`) |
| 232 | hard-coded-cryptographic-value | `hashing.rs:347:30` | `"pw"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 233 | hard-coded-cryptographic-value | `hashing.rs:347:36` | salt `b"0123456789abcdef"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 234 | hard-coded-cryptographic-value | `hashing.rs:348:43` | `"Secret@123"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 235 | hard-coded-cryptographic-value | `hashing.rs:352:42` | `"Secret@123"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 236 | hard-coded-cryptographic-value | `hashing.rs:371:46` | `"anything"` in `the_stand_in_fallback_costs_a_full_check_and_matches_nothing` | Fixed (moved to `hashing/tests.rs`) |
| 237 | hard-coded-cryptographic-value | `hashing.rs:384:45` | `"pw"` in `a_hash_asking_for_huge_costs_is_refused_unrun` | Fixed (moved to `hashing/tests.rs`) |
| 238 | hard-coded-cryptographic-value | `hashing.rs:395:46` | `"pw"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 239 | hard-coded-cryptographic-value | `hashing.rs:445:66` | `"pw"` in `verification_agrees_with_the_crates_verifier` | Fixed (moved to `hashing/tests.rs`) |
| 240 | hard-coded-cryptographic-value | `hashing.rs:505:52` | `"pw"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 241 | hard-coded-cryptographic-value | `hashing.rs:505:71` | `"pW"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 242 | hard-coded-cryptographic-value | `hashing.rs:505:90` | `"pw"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 230 | cleartext-logging | `hashing.rs:510:21` | the `assert_eq!` failure message naming that test password | Fixed (moved to `hashing/tests.rs`) |
| 243 | hard-coded-cryptographic-value | `hashing.rs:552:14` | `"Secret@123"` in `hashes_are_the_crates_own_byte_for_byte` | Fixed (moved to `hashing/tests.rs`) |
| 244 | hard-coded-cryptographic-value | `hashing.rs:553:14` | `"Market#2026"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 245 | hard-coded-cryptographic-value | `hashing.rs:573:17` | `"Secret@123"` in `a_hash_made_before_the_change_still_verifies` | Fixed (moved to `hashing/tests.rs`) |
| 246 | hard-coded-cryptographic-value | `hashing.rs:578:42` | `"Secret@123"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 257 | hard-coded-cryptographic-value | `hashing.rs:626:34` | `"pw"` in `kek_depends_on_password_and_salt` | Fixed (moved to `hashing/tests.rs`) |
| 258 | hard-coded-cryptographic-value | `hashing.rs:626:40` | salt `b"fedcba9876543210"`, same test | Fixed (moved to `hashing/tests.rs`) |
| 226 | cleartext-transmission | `xts/streaming.rs:266:19` | `http.post(url)` in `subscription_call`, data said to come from `auth.user_id()` | False positive |
| 227 | cleartext-transmission | `xts/streaming.rs:268:18` | `http.put(url)`, same function | False positive |


**Hashing (19 alerts): fixed.** Each value is a known-answer input to
Argon2 or a password a test checks against; none is key material of the
app, and the module is `#[cfg(test)]`, never compiled into the installer.
The cleartext-logging alert is a failing assertion's message printing the
test password `pw` to the test runner. This module has raised three rounds
of the same alerts (99 to 110, 247 to 255, and these), each time its lines
moved. The commit `refactor(security): hashing's tests in their own tests.rs,
out of CodeQL's scan` moves it verbatim to
`security/hashing/tests.rs`, declared `#[cfg(test)] mod tests;` as
`websocket/manager/tests.rs` is; every test is unchanged, the Linux-only
heap-fallback one included.

Why this closes them: `.github/codeql/codeql-config.yml` lists
`src-tauri/src/**/tests.rs` under `paths-ignore`, and the master analysis
shows the filter holds for Rust: of its 78 results none is in a `tests.rs`
file or under `src-tauri/tests/`, although those files hold many fixed test
passwords (alert 121, in `server/tests.rs`, was raised before that line
was added to the configuration). An alert that the next analysis of the
default branch no longer reports is closed as fixed. No query is turned
off: the exclusion is by file, and the production code left in
`hashing.rs` is still scanned.

**XTS (2 alerts): false positive, no reasonable code fix.** The analysis'
code flow (SARIF of Rust analysis 2006354515, the same at `7b999e49`) does
not start in XTS at all:

```
brokers/angel/streaming.rs:136  auth.user_id()               (source: Angel's client code)
brokers/angel/mod.rs:505        AngelFeed::from_auth(...)    (the whole feed tainted)
services/broker_runtime.rs:219  broker.create_feed(...)
websocket/manager.rs:277        Supervisor { feed }          (Box<dyn BrokerFeed>)
websocket/manager.rs:924, 886.. self.feed.<method>(...)      (resolved to every implementation)
brokers/families/xts/streaming.rs:943/955  XtsFeed::subscribe_frames / unsubscribe_frames, self
brokers/families/xts/streaming.rs:600      format!("{}{}/instruments/subscription", self.connector.base_url, ..)
brokers/families/xts/streaming/worker.rs:118  SubscriptionTarget { url }
brokers/families/xts/streaming.rs:266/268  http.post(url) / http.put(url)
```

CodeQL resolves the manager's calls on `Box<dyn BrokerFeed>` to every
feed's method, so the Angel feed object stands in for the XTS feed's
`self`, and its taint rules make every field read of a tainted value
tainted (`TaintTrackingImpl.qll`: "read steps give rise to taint steps"),
the XTS base URL included. At run time the two are different objects:
nothing of Angel's reaches an XTS request. The dismissed alert 157
(pocketful) comes from the same Angel source by the same route.

What the XTS call really sends, and what holds it: the market-data token
in the `Authorization` header (headers are not a sink of this query) to
`<base URL><subscription path>/instruments/subscription`, which carries no
user id. Before either request is built, `token_transport_allowed(url)`
(`streaming.rs:191`, called at `:259`) refuses anything but `https://`, or
`http://` to a loopback host (the test doubles), and the call returns a
refusal without sending. Every member's base URL is `https://`; only tests
pass another (`XtsBroker::with_base_url`). The unit test
`token_only_travels_over_tls_or_loopback` (`xts/tests.rs`) checks every
member's real subscription URL is allowed and that remote `http`, private
address `http`, `ws` and malformed URLs are refused. The audit's own probe
of the same predicate (`03-security-and-credentials.md`, "CodeQL XTS
226/227") agrees. Redirect and proxy behaviour is not part of this
disposition.

Why a code change does not end them (CodeQL 2.27.3 sources,
`rust/ql/src/queries/security/CWE-311/CleartextTransmission.ql` and
`CleartextTransmissionExtensions.qll`):

- The query has no scheme check. Its reqwest sinks are the URL argument of
  `Client::get`, `post`, `put`, `patch`, `delete`, `head` and `request`
  (model kind `request-url`; no header or body is one), and its only
  barriers are models of kind `transmission` or `request-url`. An https-only URL type would not
  end it either: `url::Url::parse` is modelled to carry taint
  (`url.model.yml`), so a URL parsed from the same string stays tainted.
- Under that path no field of the XTS feed is clean, so building the URL
  differently inside the feed changes nothing. Ending it would take one
  of: sending through `reqwest::Request::new` and `Client::execute`, which
  are not modelled as sinks (that hides the call from the query rather
  than fixing anything, and reopens it when they are modelled); starting
  the subscription worker outside the feed's trait methods (a lifecycle
  change to a live market-data path, for no runtime gain); or a model pack
  declaring `token_transport_allowed` a barrier guard of kind
  `transmission` (the right statement of the guard, but code-scanning
  configuration that cannot be checked without CodeQL). None was made.

AliceBlue's `prepare_session` also sends a bearer token from feed code
(`http.post(format!("{}{}", ep.base, path))` to its fixed `https` base); it
raises no alert and was left as it is.

### What the maintainer records on GitHub

The 19 hashing alerts need nothing: they close as fixed at the first master
scan after this branch merges. Dismiss only these two, as **false
positive** (the comment is under GitHub's 280 characters). Done on
2026-10-23 with the maintainer's approval; the same day Dependabot alert 2
(glib, GHSA-wrw7-89jp-8q8g) was re-filed from "tolerable risk" to "not
used" with the proof in the table below, as CLAUDE.md requires:

- **226, 227**: "Path starts at Angel's auth.user_id() and reaches the XTS
  feed only via CodeQL resolving Box<dyn BrokerFeed> calls to every impl.
  XTS sends no user id; token_transport_allowed refuses non-TLS URLs
  (streaming.rs:259). docs/audit/2026-10-22/scanner-triage.md"

```bash
r=repos/marketcalls/openalgo-desktop/code-scanning/alerts
x="Path starts at Angel's auth.user_id() and reaches the XTS feed only via CodeQL resolving Box<dyn BrokerFeed> calls to every impl. XTS sends no user id; token_transport_allowed refuses non-TLS URLs (streaming.rs:259). docs/audit/2026-10-22/scanner-triage.md"
for n in 226 227; do
  gh api -X PATCH "$r/$n" -f state=dismissed -f dismissed_reason="false positive" -f dismissed_comment="$x"
done
```

## Accepted advisories (PROD-SEC-03)

| Ids | Package | Accepted | Re-checked | Review by | Why the vulnerable code never runs, and what was checked |
| --- | --- | --- | --- | --- | --- |
| RUSTSEC-2026-0049, -0098, -0099, -0104; GHSA-82j2-j2ch-gfr8, GHSA-pwjx-qhcg-rvj4, GHSA-965h-392x-2mh5, GHSA-xgp8-3hg3-c2mh | rustls-webpki 0.102.8 | 2026-10-08 | 2026-10-22 | 2027-01-31 | Only `rumqttc 0.25.1` depends on it (`Cargo.lock`; `rustls` and `rustls-platform-verifier` use 0.103.16). rumqttc 0.25.1 names it only as an error type (`tls.rs:42`, `WebPki(#[from] webpki::Error)`), and its `TlsConfiguration::Rustls` branch clones the config it is given (`tls.rs:131`). The IIFL Capital feed, the only rumqttc user, always passes our rustls 0.23 config with the webpki roots (`iiflcapital/mqtt_relay.rs` `tls_config`, `options`), verified by 0.103.16. rumqttc 0.25.1 is still the newest release on crates.io and its main branch still pins `rustls-webpki = "0.102.8"` |
| RUSTSEC-2026-0382; CVE-2026-111943 | rustls-webpki 0.102.8 | 2026-10-22 | 2026-10-22 | 2027-01-31 | As above. The same CVE also names RUSTSEC-2026-0381 for webpki-roots, fixed by webpki-roots 1.0.10 (in `Cargo.lock`) and not accepted |
| GHSA-wrw7-89jp-8q8g | glib 0.18.5 (unsound `VariantStrIter`) | 2026-10-07 | 2026-10-22 | 2027-01-31 | Linux only, through Tauri's GTK 0.18 stack (tauri 2.12.2, gtk 0.18.2). No app code names glib (the only matches in `src-tauri/src` are "glibc" in comments). The unsound iterator is reached only through `Variant::array_iter_str` (glib `variant.rs:843`), and no crate in the lockfile's Linux stack calls it: on 2026-10-23 a search of the registry sources of every glib 0.18.5 dependent in `Cargo.lock` (atk, cairo-rs, gdk, gdk-pixbuf, gdkx11, gio, gtk, javascriptcore-rs, pango, soup3, webkit2gtk, glib-macros) and of muda, tao, wry, rfd, tray-icon and libappindicator found `VariantStrIter` and `array_iter_str` only in glib itself. GTK's C code cannot call a Rust iterator |

`deny.toml` and `.trivyignore` now carry these dates in each reason; the
ids `security.yml` reads from `deny.toml` are unchanged (the five above).

**Informational notices, not accepted and not gating** (the policy reports
transitive unmaintained or unsound crates without failing; from the
cargo-audit run above). None is a vulnerability advisory:

| Notice | Crate | Comes from |
| --- | --- | --- |
| RUSTSEC-2024-0370, unmaintained | proc-macro-error 1.0.4 | `glib-macros`, `gtk3-macros`: build-time macros of Tauri's Linux GTK stack |
| RUSTSEC-2025-0134, unmaintained | rustls-pemfile 2.2.0 | rumqttc 0.25.1 (IIFL Capital feed), the same crate as the accepted rustls-webpki |
| RUSTSEC-2026-0371, -0370, -0372, unmaintained | zbus_names 3.0.0, zvariant 4.2.0, zvariant_derive 4.2.0 | `zbus` <- `secret-service` <- `keyring`: the Linux D-Bus chain of the one-time keychain migration (KEY-01) |
| RUSTSEC-2024-0429, unsound | glib 0.18.5 | Tauri's Linux GTK stack; the same issue as GHSA-wrw7-89jp-8q8g, accepted in `.trivyignore` above |

They go with the moves `CLAUDE.md` plans: rumqttc to rustls-webpki 0.103,
Tauri to gtk-rs 0.20, and `keyring` removed once the migration releases
have shipped.

## PROD-SEC-01 at this base

The audit's acceptance list, checked against `763cc12a` (which carries
#192, "keep the API key in memory, never in browser storage") and this
branch. "Fails without" means the test was run with that guard removed and
failed, then passed with it restored.

| Acceptance | Where | Test | Fails without |
| --- | --- | --- | --- |
| A key stored by 1.0.0 is removed before the store is used | `authStore` `version: 1`, `migrate` (zustand writes the entry back during hydration) | `authStore.test.ts` "erases the key a 1.0.0 install stored, as soon as the store loads", "loads a stored 1.0.0 entry without a user as signed out" | `partialize` |
| Memory never takes a key from storage, whatever the entry's version | `authStore` `merge` (new): only `user` and `isAuthenticated` are read back | "never reads a key from storage, whatever version the entry says" (version 1 and no version) | `merge`: **this was open at the base**; zustand's default merge copies every stored field when the version matches |
| Memory-only state, never written | `authStore` `partialize` | "keeps the API key in memory and out of storage" | `partialize` |
| A second window, or one left open (stale tab), never passes the key through storage | `partialize`, `merge` | "never passes the key between windows through storage" (new) | `partialize` |
| Refetch only from a verified session; reload before the check | `AuthSync` (`/auth/session-status`, the hold-back until it answers) | `AuthSync.test.tsx` "after a reload, holds the page until the server has checked the session" (new), "holds a page back until the API key is in memory" | the hold-back |
| Explicit logout | `authStore.logout` | "forgets the key and the key check at logout" | `apiKey: null` in `logout` |
| Remote logout: the session ended in another window, by a password change or a reset | `force_logout` (`useSocket`) calls `logout`; the server also closes the signed-out socket (S-09) | `SocketProvider.test.tsx` "drops the API key when the session ends in another window or device" (new) | `apiKey: null` in `logout` |
| Remote logout found at a reload | `AuthSync` signs the window out when the check says so | "after a reload, signs the window out when the session ended elsewhere" (new) | the `logout()` call in `AuthSync` |
| Key rotation | the API Key page puts the new key in the store; the server refuses the old one at once | `ApiKey.test.tsx` (a regenerated key reaches the store) | (from #192) |

The five new tests passed 10 runs out of 10.

Residuals, neither a retention of the key on disk:

- A window that stays open while the key is regenerated in another window
  keeps the old key in memory until it reloads or its session ends; the
  server refuses that key from the moment it is replaced, so its requests
  fail rather than trade. The web behaves the same.
- A browser tab still running the 1.0.0 page after the upgrade writes the
  key again if the trader signs in through that tab; the next 1.0.1 page
  load erases it. The app window itself reloads with the update.

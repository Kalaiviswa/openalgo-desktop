import sys
p=sys.argv[1]
s=open(p).read()
def rep(old,new):
    global s
    assert old in s, old[:80]
    s=s.replace(old,new,1)
rep('''  can do (window control, opening the browser, OS keychain prompts). This is''','''  can do (window control, opening the browser, the account reset and the
  fresh start confirmed in a native dialog). This is''')
rep('''- **No `.env`, no plaintext secrets on disk.** The data-encryption key and the
  API-key pepper live in the OS keychain (macOS Keychain, Windows Credential
  Manager, Linux Secret Service). Where no keychain exists (headless Linux,
  some Raspberry Pi setups), fall back to a key derived from the user's password
  and say so in the UI. A hard-coded or XOR-obfuscated key is not encryption.''','''- **No `.env`, no plaintext secrets on disk, no OS keychain** (maintainer
  decision 2026-10-13, KEY-01, `docs/port/no-keychain.md`). The
  data-encryption key and the API-key pepper are wrapped with
  Argon2id(login password) in the one-row `key_vault` table of `openalgo.db`,
  beside the password hash, on every platform. A copied database is useless
  without the password. A hard-coded or XOR-obfuscated key is not encryption.
- **Unlocked while signed in, wiped at logout.** The keys are in memory from
  a sign-in that issued a session (account setup counts) until the logout or
  the app quitting, then dropped and zeroed, with every cache cleared. Until
  the trader signs in after a start, and after a logout, `/api/v1` (503, the
  web's envelope), `/mcp`, the feed and the webhooks answer "Sign in to
  OpenAlgo...", broker sessions do not resume, the bots do not run, and
  nothing decrypts. The 03:00 boundary ends sessions as before but does not
  lock: the rule is logout or quit.
- **A password change rewrites the hash and the wrapped keys in one SQLite
  transaction.** A forgotten password is replaced by starting fresh, from the
  desktop window only: new keys; the account, settings and history stay;
  broker keys, the API key and TOTP are set up again. Installs from earlier
  builds move their keys (keychain, `vault.json`, `secrets.dat`) into the
  vault once, at the first sign-in, and the keychain is not touched again.''')
rep('''- API keys are verified through an HMAC index plus Argon2, with a short cache;''','''- API keys are verified through an HMAC index plus Argon2, with the web's
  caches (verified keys 10 hours, invalid keys 5 minutes);''')
rep('''- Only account setup creates key material. A keychain that is locked or
  denies access at startup looks like no keychain at all
  (`KeyStoreError::Unavailable`); a sign-in that then wrote a vault with new
  keys locked the account out for good. Keychain writes put the data key
  last and read both keys back; a password change writes the new vault
  beside the old one before the hash changes.''','''- **The keys live in openalgo.db, locked with the password (KEY-01).** No
  code calls the OS keychain except the one-time move of an earlier build's
  keys at sign-in (`SecurityManager::open_for_sign_in`): only for the
  account's own name, only while no vault row exists, and the entries are
  deleted only after the copy is committed with `synchronous = FULL` and
  verified. A new install makes zero keychain calls (a test counts them).
  Keys are installed only with the session a sign-in issues
  (`issue_if_current`); a sign-in waiting for its authenticator code keeps
  them in its pending browser session. A new encrypted column is added to
  `key_vault::drop_encrypted_values` and `key_vault::witness` in the same
  change.
- **Every cache follows the web's rules** (`crate::cache::TtlCache`, the
  Rust `LockedTTLCache`): bounded, with a TTL; readers fill through the
  generation they read before their query; writers clear after their
  commit; logout, a password change or reset, a key rotation and a broker
  switch clear them. A new cache gets a race test and a row in the cache
  table of `docs/port/no-keychain.md`.''')
rep('''- A key file opened by a password is trusted only once the password also
  matches the stored hash (`SecurityManager::unlock_for_sign_in`); around a
  password change an old or uncommitted password opens a file but must
  unlock nothing.''','''- Keys a password opened are trusted only once the password also matches
  the stored hash, read again under the lock a password change holds
  (`SecurityManager::open_for_sign_in`); a password that matches neither
  installs nothing.''')
open(p,'w').write(s)

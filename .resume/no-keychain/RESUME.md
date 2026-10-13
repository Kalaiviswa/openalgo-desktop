# RESUME: KEY-01 no OS keychain (paused 2026-10-13)

Design note: docs/port/no-keychain.md. Draft PR #60 (pr/no-keychain), keep draft.
This .resume folder is on wip/no-keychain only; never push it to pr/.

## Done (on pr/no-keychain, CI rust-test green on 4 OSes for the first push)
- crate::cache::TtlCache; API-key caches (verified 10 h/1024, invalid 5 min/512),
  order mode 60 s/128; master load_master_if; FactsCache; race tests.
- Locked refusals: /api/v1 503 envelope, /mcp 503 locked (+stdio), webhooks 503, feed.
- Migration 081 + db/sqlite/key_vault.rs (wrap/unwrap/witness/drop_encrypted_values).
- KeyStore::harness_keys (MemoryKeyStore opens unlocked), CountingKeyStore.
- Error recovery codes (envelope.rs, error.rs), adapter on_logout caches, bots autostart idempotent.
- Frontend: Login recovery states, StartFreshCard, ResetPassword page, ApiKey note (vitest 3139 pass;
  fail-before seen: 4 Login tests, 1 StartFreshCard test).
- Docs: README, troubleshooting, migration page, security-audit skill.
- Local lane: 19/20 targeted tests passed; the rotation race test failed from a shared hook slot,
  fixed in 9514bb7 (hooks per state) but NOT rerun yet.

## Next, in order (only after #48 AND #58 are merged to master)
1. Rebase onto master. Apply drafts here: security_mod.rs.new -> src/security/mod.rs;
   make_auth_service.py (input: master auth_service.rs, uses auth_block.rs);
   make_routes_auth.py; make_session_web.py (in place on session/web.rs);
   make_state.py (in place); make_shell.py <src-tauri> (build.rs, lib.rs, commands start_fresh);
   remove the /auth/reset-password route (routes/mod.rs, public list in server/tests.rs,
   its tests); data_migrations::run without legacy flags (rewrite the upgrade test to sign in);
   system_info key-mode line; session-status key_mode/locked.
2. Add src/server/tests/key_vault.rs from key_vault_tests.rs; replace credentials_lifecycle
   SEC-02 file-staging, SEC-03, SEC-04, SEC-09 tests (rule 6); signs_in helper installs keys.
3. aes zeroize: aes = { version = "0.8", features = ["zeroize"] }, update Cargo.lock once.
4. Lane: fmt, targeted tests, fail-before for each acceptance test (disable guard, see it fail,
   restore). CLAUDE.md via make_claude_md.py. Fix status row KEY-01 in
   docs/codex-evaluation/B-security-credentials-limits-config.md. Dismiss CodeQL test-password
   alerts only after checking each is in #[cfg(test)].
5. Update PR #60 body with fail-before evidence; ready + merge when ci-ok is green.

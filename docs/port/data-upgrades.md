# Data-folder upgrades: backup, restore, downgrade refusal (REL-02)

Desktop only: the web upgrades its database in place with its own
migration scripts. The code is `src-tauri/src/db/upgrade.rs`, called from
the app start through `AppState::open_upgrading`.

## What happens at start

`data-version.json` in the data folder records the version that last opened
it, written only after an open succeeded.

| Recorded | This copy | What happens |
| --- | --- | --- |
| nothing, no stores | any | a new folder: open, record |
| nothing, stores present | any | from before versions were recorded: back up, open, record |
| an older version | newer | back up every store, open (migrations run), record, delete the backup |
| the same version | same | open (any leftover backup is deleted) |
| a newer version | older | refused: nothing is opened, migrated or written |

The stores are `openalgo.db`, `logs.db`, `sandbox.db` (copied with `VACUUM
INTO`), `historify.duckdb` with its `.wal`, and the key files `vault.json`,
`vault.next.json`, `secrets.dat` (copied byte for byte while nothing has
them open: the app start takes `data-folder.lock` first, so a second running
copy is refused before it reads anything). A backup is built in
`backups/.partial-<name>/` and renamed into place only once complete, so a
crash mid-backup leaves nothing that counts as a backup, and the live stores
were only read.

## Decisions from the security review (2026-10-15)

The background review of the first REL-02 commit raised four candidates.
Each was traced against the code; the decisions:

1. **Retention past a deletion: held, fixed.** Backups kept the last three
   copies of `openalgo.db` and the key files. Encrypted broker keys and
   tokens, the API key, the TOTP secret and the password hash survived there
   after the trader forgot a broker, logged out, regenerated the key, changed
   the password or reset the account.
   - Two fixes were possible: purge or rewrite every backup on each of those
     actions, or keep backups only until the upgrade is confirmed good.
   - **Chosen: a backup lives only for the start that made it.** It is
     deleted as soon as the upgraded open succeeds; this happens on every
     successful open, so leftovers of earlier starts go too. It is also
     deleted right after an automatic restore, when the folder equals it.
     Account reset empties `backups/` as well.
   - Why: a purge on every deleting action must find every such action now
     and in the future (KEY-01's "start fresh" and forgotten-password
     resets, forgetting a broker, logout's broker revoke, password and TOTP
     changes). Missing one leaks. Rewriting copies of encrypted databases
     also needs the keys. A backup that never outlives a successful start
     needs neither, and nothing the trader deletes while the app runs can
     be in a backup, because no backup exists then.
   - The one exception is a backup that could not be put back after a
     failed migration. It is kept and named on the start-up page, because it
     is then the only good copy of the data. The app has not run since it
     was made, so it holds nothing the trader has deleted since. A later
     start that fails again and puts its own backup back removes only that
     one, so this copy is not lost; the next successful open deletes it.
   - Second line (follow-up of 2026-10-15): a successful start that cannot
     delete a backup (on Windows another program, such as a virus scanner
     reading the new files, can hold one open) logs it and goes on, so a
     leftover could sit in `backups/` while the app runs. Every action of
     the review's list therefore also empties `backups/`
     (`upgrade::purge_backups`): forgetting a broker
     (`BrokerConfigService::forget`), every broker sign-out, logout's
     included (`BrokerAuthService::revoke_held`), a password change or
     forgotten-password reset (`AuthService::set_password`) and account
     reset (`AuthService::reset_account`). This is not the primary
     guarantee (a new deleting action that forgets the call still has no
     backup to leak into after a normal start); it only closes the
     leftover case.
   - Cost: OpenAlgo offers no way back to an earlier version's data. The
     user guide says to keep a copy of the whole folder made by hand.
   - Tests: `no_backup_outlives_a_successful_open`,
     `an_upgrade_backs_up_every_store_before_the_open_then_deletes_the_backup`
     (unit), `an_upgrade_backs_up_every_store_and_keeps_no_copy_once_open`
     and `no_backup_survives_a_deleting_action` (integration: a leftover
     planted while the app runs is gone after each of the four actions).
2. **Restore resurrecting revoked state: partly held, fixed.** The
   automatic restore already ran only for a failed open, immediately, from
   the backup made at that same start. That backup is the state the old
   version left, so it cannot bring back anything revoked since. The user
   guide, however, offered a manual restore of an older backup. That would
   bring back a revoked broker session, an old API key, or an old password
   hash and vault (a leaked password would unlock again).
   - The manual path is gone. Backups no longer outlive a start (1), and
     the guide now says to keep a whole-folder copy by hand.
   - `db::upgrade::restore` is private: the only caller is `open_data`,
     right after the failed open, with the backup that same start made. No
     command, route or later start can put an older copy back.
   - The SEC-05b revoke marker (`broker-revoke.pending`) is not a store, so
     a restore never removes it, and nothing in `AppState::open` reads or
     clears it (the session poll and sign-in do, after the open), so a
     failed open cannot have consumed it either.
   - Tests: `a_failed_migration_puts_every_store_back_from_that_starts_backup`.
     An older planted backup is never the one restored, and the revoke
     marker survives (unit and integration).
3. **Permissions: partly held, fixed.** Folders were created and then set
   to 0700, and files were set to 0600 after copying. `VACUUM INTO` created
   its file with SQLite's default (0644 under umask 022) until then; the
   enclosing 0700 folder kept other users out, but the file itself was
   readable. Now every folder is created 0700 and every file 0600 from its
   first byte: the `VACUUM INTO` target is created owner-only and empty
   first, the DuckDB and key files are streamed into owner-only files, and
   so are a restore's staging files and the folder lock. On Windows the
   data folder's ACL is inherited: every folder and file is created with
   the default security of its parent, and nothing sets an entry of its
   own.
   - Tests: `backups_are_owner_only_from_the_first_byte` (Unix),
     `backups_carry_only_the_data_folders_acl_on_windows` (Windows: every
     access entry on `backups/`, the backup and its files is inherited).
4. **Bounded retention and link safety: held, fixed.** Pruning kept three.
   But a `backups` that was a link was followed: making a backup chmod-ed,
   and cleanup deleted `.partial-` folders in, the folder it pointed to.
   Now `backups` must be a plain folder (a link or a file stops the upgrade
   with a trader message), and entries in it are listed without following
   links, so a link named like a backup is skipped. Removal only touches
   plain folders inside `backups/`. At most three leftovers are kept.
   - Test: `backup_cleanup_never_follows_a_link_out_of_the_folder` (Unix).

## Not covered by a backup

Anything an upgrade changes outside the data folder (the OS keychain). Such
a change must happen only after the open has succeeded. KEY-01's keychain
migration must delete keychain entries only then.

//! OpenAlgo API key: verification for `/api/v1`, regeneration and order mode.
//!
//! Verification: a keyed digest of the presented key (HMAC-SHA256 under a
//! random per-process key, the same construction as the stored lookup
//! index) indexes two bounded caches with the web's sizes and lifetimes
//! (`database/auth_db.py`): verified keys for 10 hours, storing only the
//! account name, and invalid keys for 5 minutes. On a miss the HMAC index
//! finds the one candidate row and Argon2 verifies it. The order mode
//! (auto or semi-auto) is cached for 60 seconds and invalidated when it
//! changes.
//!
//! Every reader fills through the generation it read before its query, and
//! every writer clears after its commit, so a check in flight across a
//! logout, a key regeneration, a password change or reset or a broker switch
//! cannot put its answer back (`crate::cache`). While the keys are locked
//! (before the trader signs in after a start, and after a logout) nothing
//! is checked and nothing cached answers: the caller is told to sign in.

use crate::cache::TtlCache;
use crate::db::sqlite::api_keys;
use crate::error::{AppError, Result};
use crate::security::hashing::lookup_hmac;
use crate::security::Secret;
use crate::state::AppState;
use rand::RngCore;
use std::time::{Duration, Instant};

/// Verified keys: at most this many, each for [`VERIFIED_TTL`] (web
/// `verified_api_key_cache`).
pub const VERIFIED_CAP: usize = 1024;
pub const VERIFIED_TTL: Duration = Duration::from_secs(36_000);
/// Invalid keys: at most this many, each for [`INVALID_TTL`] (web
/// `invalid_api_key_cache`).
pub const INVALID_CAP: usize = 512;
pub const INVALID_TTL: Duration = Duration::from_secs(300);
/// The order mode, for [`ORDER_MODE_TTL`] (web `order_mode_cache`).
pub const ORDER_MODE_CAP: usize = 128;
pub const ORDER_MODE_TTL: Duration = Duration::from_secs(60);

/// What a presented key turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    /// The stored key; carries the account it belongs to.
    Valid(String),
    Invalid,
    /// The keys are not in memory: nobody has signed in since OpenAlgo
    /// started, or the trader logged out. Nothing was checked.
    Locked,
}

/// The generations a key check read before its query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Generations {
    verified: u64,
    invalid: u64,
}

/// The API-key caches. Entries are keyed by an HMAC of the presented key
/// under a random key that lives only in this process, so the maps never
/// hold the API key or an unsalted hash of it, and a verified entry holds
/// only the account name.
pub struct ApiKeyCache {
    cache_key: [u8; 32],
    verified: TtlCache<String, String>,
    invalid: TtlCache<String, ()>,
    order_mode: TtlCache<(), String>,
    /// Argon2 checks run so far. They run only on an HMAC index hit, so an
    /// invented key never costs one.
    hash_checks: std::sync::atomic::AtomicU64,
    /// The generation again, for those that must act when it moves: the
    /// market data feed closes its authenticated clients (`subscribe`).
    changed: tokio::sync::watch::Sender<u64>,
}

impl Default for ApiKeyCache {
    fn default() -> Self {
        let mut cache_key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut cache_key);
        Self {
            cache_key,
            verified: TtlCache::new(VERIFIED_CAP, VERIFIED_TTL),
            invalid: TtlCache::new(INVALID_CAP, INVALID_TTL),
            order_mode: TtlCache::new(ORDER_MODE_CAP, ORDER_MODE_TTL),
            hash_checks: std::sync::atomic::AtomicU64::new(0),
            changed: tokio::sync::watch::channel(0).0,
        }
    }
}

impl ApiKeyCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Argon2 checks of API keys this instance has run.
    pub fn hash_checks(&self) -> u64 {
        self.hash_checks.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn note_hash_check(&self) {
        self.hash_checks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn digest(&self, key: &str) -> String {
        lookup_hmac(&self.cache_key, key)
    }

    /// The generations to fill through, read before checking.
    pub fn generations(&self) -> Generations {
        Generations {
            verified: self.verified.generation(),
            invalid: self.invalid.generation(),
        }
    }

    /// A cached answer for `key`: `Some(Valid)` or `Some(Invalid)`.
    pub fn get(&self, key: &str, now: Instant) -> Option<KeyCheck> {
        let d = self.digest(key);
        if let Some(user) = self.verified.get(&d, now) {
            return Some(KeyCheck::Valid(user));
        }
        self.invalid.get(&d, now).map(|_| KeyCheck::Invalid)
    }

    /// Store an answer computed while `gens` were current; dropped when the
    /// caches were cleared in between. A `Locked` answer is never stored.
    pub fn fill(&self, key: &str, answer: &KeyCheck, gens: Generations, now: Instant) {
        let d = self.digest(key);
        match answer {
            KeyCheck::Valid(user) => {
                self.verified.fill(d, user.clone(), gens.verified, now);
            }
            KeyCheck::Invalid => {
                self.invalid.fill(d, (), gens.invalid, now);
            }
            KeyCheck::Locked => {}
        }
    }

    /// The cached order mode and the generation to fill through.
    fn order_mode(&self, now: Instant) -> (Option<String>, u64) {
        self.order_mode.get_with_generation(&(), now)
    }

    /// Drop every entry of every cache here and stop every check in flight
    /// from storing its answer. Run after the commit of a key regeneration,
    /// a password change or reset, an account reset, a broker switch or a
    /// session start or end, and on logout. Subscribers are told
    /// (`subscribe`).
    pub fn clear(&self) {
        self.verified.clear();
        self.invalid.clear();
        self.order_mode.clear();
        self.changed.send_replace(self.verified.generation());
    }

    /// A receiver that sees every `clear`: whoever authenticated a caller
    /// with the key before must make it authenticate again.
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.changed.subscribe()
    }

    /// Entries in the key caches (verified and invalid).
    pub fn len(&self) -> usize {
        self.verified.len() + self.invalid.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0 && self.order_mode.is_empty()
    }

    #[cfg(test)]
    fn sizes(&self) -> (usize, usize, usize) {
        (
            self.verified.maxsize(),
            self.invalid.maxsize(),
            self.order_mode.maxsize(),
        )
    }
}

/// Test hook: runs between a check's query and its fill, standing in for a
/// writer that commits while the check is in flight.
#[cfg(test)]
pub(crate) mod test_hooks {
    use parking_lot::Mutex;

    type Hook = Box<dyn FnOnce() + Send>;
    /// One hook per app state: tests run in parallel, each on its own.
    pub(crate) static BEFORE_FILL: Mutex<Vec<(usize, Hook)>> = Mutex::new(Vec::new());

    /// Run `f` once, in the next check of `state`'s caches, after its query.
    pub(crate) fn before_next_fill(
        state: &crate::state::AppState,
        f: impl FnOnce() + Send + 'static,
    ) {
        BEFORE_FILL
            .lock()
            .push((state as *const _ as usize, Box::new(f)));
    }

    pub(super) fn run(state: &crate::state::AppState) {
        let hook = {
            let mut g = BEFORE_FILL.lock();
            let me = state as *const _ as usize;
            g.iter().position(|(s, _)| *s == me).map(|i| g.remove(i))
        };
        if let Some((_, f)) = hook {
            f();
        }
    }
}

pub struct ApiKeyService;

impl ApiKeyService {
    /// Whether `api_key` is the stored OpenAlgo key. False while locked.
    pub fn is_valid(state: &AppState, api_key: &str) -> bool {
        matches!(Self::check(state, api_key), KeyCheck::Valid(_))
    }

    /// Check `api_key`. While the keys are locked the answer is `Locked`,
    /// before any cache is read: nothing cached before a logout answers
    /// after it.
    pub fn check(state: &AppState, api_key: &str) -> KeyCheck {
        if api_key.is_empty() {
            return KeyCheck::Invalid;
        }
        if !state.security.is_unlocked() {
            return KeyCheck::Locked;
        }
        let now = Instant::now();
        // Read before the cache and before the query: a writer clearing in
        // between makes the fill below a no-op.
        let gens = state.api_keys.generations();
        if let Some(hit) = state.api_keys.get(api_key, now) {
            return hit;
        }
        let answer = match Self::verify_uncached(state, api_key) {
            Ok(Some(user)) => KeyCheck::Valid(user),
            Ok(None) => KeyCheck::Invalid,
            Err(AppError::Locked) => return KeyCheck::Locked,
            Err(e) => {
                tracing::error!("API key check failed: {}", e);
                return KeyCheck::Invalid;
            }
        };
        #[cfg(test)]
        test_hooks::run(state);
        state.api_keys.fill(api_key, &answer, gens, now);
        answer
    }

    /// The account the key belongs to, or None when it is not the stored
    /// key.
    fn verify_uncached(state: &AppState, api_key: &str) -> Result<Option<String>> {
        let lookup = state.security.api_key_lookup(api_key)?;
        let row = {
            let conn = state.sqlite.conn()?;
            api_keys::find_by_lookup(&conn, &lookup)?
        };
        let Some(row) = row else {
            return Ok(None);
        };
        state.api_keys.note_hash_check();
        if !state.security.verify_password(api_key, &row.key_hash)? {
            return Ok(None);
        }
        if let Ok(conn) = state.sqlite.conn() {
            let _ = api_keys::touch(&conn, row.id);
        }
        Ok(Some(row.name))
    }

    /// Generate and store a new key for `username`; returns it once. The
    /// caches are cleared after the new key is stored.
    pub fn regenerate(state: &AppState, username: &str) -> Result<Secret> {
        let key = api_keys::generate_api_key();
        {
            let conn = state.sqlite.conn()?;
            api_keys::upsert_for_user(&conn, &state.security, username, &key)?;
        }
        state.api_keys.clear();
        Ok(Secret::new(key))
    }

    /// The key for the signed-in session (API key page, session status).
    pub fn current(state: &AppState) -> Result<Option<Secret>> {
        let conn = state.sqlite.conn()?;
        api_keys::get_plaintext(&conn, &state.security)
    }

    /// `auto` or `semi_auto`, cached for a minute.
    pub fn order_mode(state: &AppState) -> Result<String> {
        let now = Instant::now();
        let (hit, gen) = state.api_keys.order_mode(now);
        if let Some(m) = hit {
            return Ok(m);
        }
        let mode = {
            let conn = state.sqlite.conn()?;
            api_keys::get_order_mode(&conn)?.unwrap_or_else(|| "auto".into())
        };
        #[cfg(test)]
        test_hooks::run(state);
        state.api_keys.order_mode.fill((), mode.clone(), gen, now);
        Ok(mode)
    }

    /// Store the order mode; the cache is invalidated after the write.
    pub fn set_order_mode(state: &AppState, mode: &str) -> Result<bool> {
        let changed = {
            let conn = state.sqlite.conn()?;
            api_keys::set_order_mode(&conn, mode)?
        };
        state.api_keys.order_mode.invalidate(&());
        Ok(changed)
    }
}

/// Used by the older services that re-check the key.
pub fn require_valid(state: &AppState, api_key: &str) -> Result<()> {
    if ApiKeyService::is_valid(state, api_key) {
        Ok(())
    } else {
        Err(crate::error::AppError::Auth(
            "Invalid openalgo apikey".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Duration = Duration::from_secs(3600);

    /// S-12: an answer computed before a regeneration is not cached after it.
    #[test]
    fn a_check_racing_a_regeneration_cannot_cache_the_old_key() {
        let c = ApiKeyCache::new();
        let now = Instant::now();
        let gens = c.generations();
        c.clear(); // the key was regenerated while the check ran
        c.fill("old-key", &KeyCheck::Valid("u".into()), gens, now);
        assert_eq!(c.get("old-key", now), None);
        c.fill(
            "new-key",
            &KeyCheck::Valid("u".into()),
            c.generations(),
            now,
        );
        assert_eq!(c.get("new-key", now), Some(KeyCheck::Valid("u".into())));
    }

    /// KEY-01 (web parity): verified keys for 10 hours, invalid keys for 5
    /// minutes, the order mode for 60 seconds, each with the web's maxsize.
    #[test]
    fn caches_have_the_webs_sizes_and_lifetimes() {
        let c = ApiKeyCache::new();
        assert_eq!(c.sizes(), (1024, 512, 128));
        let t0 = Instant::now();
        c.fill("good", &KeyCheck::Valid("u".into()), c.generations(), t0);
        c.fill("bad", &KeyCheck::Invalid, c.generations(), t0);
        let gen = c.order_mode(t0).1;
        c.order_mode.fill((), "auto".into(), gen, t0);
        let at = |d: Duration| t0 + d;
        assert_eq!(c.get("good", at(9 * H)), Some(KeyCheck::Valid("u".into())));
        assert_eq!(c.get("good", at(10 * H + Duration::from_secs(1))), None);
        assert_eq!(
            c.get("bad", at(Duration::from_secs(299))),
            Some(KeyCheck::Invalid)
        );
        assert_eq!(c.get("bad", at(Duration::from_secs(301))), None);
        assert_eq!(
            c.order_mode(at(Duration::from_secs(59))).0.as_deref(),
            Some("auto")
        );
        assert_eq!(c.order_mode(at(Duration::from_secs(61))).0, None);
    }

    #[test]
    fn caches_stay_bounded() {
        let c = ApiKeyCache::new();
        let t0 = Instant::now();
        for i in 0..(VERIFIED_CAP * 2) {
            c.fill(
                &format!("v{}", i),
                &KeyCheck::Valid("u".into()),
                c.generations(),
                t0,
            );
            c.fill(&format!("i{}", i), &KeyCheck::Invalid, c.generations(), t0);
        }
        assert_eq!(c.verified.len(), VERIFIED_CAP);
        assert_eq!(c.invalid.len(), INVALID_CAP);
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn a_locked_answer_is_never_cached() {
        let c = ApiKeyCache::new();
        let now = Instant::now();
        c.fill("k", &KeyCheck::Locked, c.generations(), now);
        assert_eq!(c.get("k", now), None);
        assert!(c.is_empty());
    }

    #[test]
    fn cache_key_is_keyed_per_process_not_a_plain_hash() {
        use sha2::{Digest, Sha256};
        let a = ApiKeyCache::new();
        let b = ApiKeyCache::new();
        let key = "presented-api-key";
        let plain = hex::encode(Sha256::digest(key.as_bytes()));
        let da = a.digest(key);
        assert_eq!(da, a.digest(key), "same cache, same key: stable");
        assert_ne!(da, b.digest(key), "independent caches use independent keys");
        assert_ne!(da, plain, "not an unsalted SHA-256 of the key");
        assert!(!da.contains(key));
        a.fill(
            key,
            &KeyCheck::Valid("u".into()),
            a.generations(),
            Instant::now(),
        );
        assert_eq!(a.verified.get(&da, Instant::now()).as_deref(), Some("u"));
        assert_eq!(a.verified.get(&plain, Instant::now()), None);
        assert_eq!(
            a.get(key, Instant::now()),
            Some(KeyCheck::Valid("u".into()))
        );
        assert_eq!(b.get(key, Instant::now()), None);
    }

    // ------------------------------------------------- races, end to end

    fn account() -> (crate::state::testing::TestCtx, String) {
        let t = crate::state::testing::build(
            crate::brokers::BrokerRegistry::with(vec![]),
            chrono::Utc::now(),
        );
        crate::services::auth_service::AuthService::setup(
            &t.ctx,
            "trader",
            "t@example.com",
            "Pw@12345",
        )
        .unwrap();
        let key = ApiKeyService::current(&t.ctx)
            .unwrap()
            .unwrap()
            .expose()
            .to_string();
        (t, key)
    }

    /// KEY-01: a key check whose query ran before a logout cleared the
    /// caches answers its own caller but leaves nothing cached.
    #[tokio::test]
    async fn a_key_check_in_flight_across_a_logout_is_not_cached() {
        let (t, key) = account();
        let ctx = t.ctx.clone();
        let c2 = ctx.clone();
        test_hooks::before_next_fill(&ctx, move || c2.api_keys.clear());
        assert_eq!(
            ApiKeyService::check(&ctx, &key),
            KeyCheck::Valid("trader".into())
        );
        assert_eq!(
            ctx.api_keys.get(&key, Instant::now()),
            None,
            "repopulated after the clear"
        );
    }

    /// KEY-01: a check of the old key in flight while the key is
    /// regenerated cannot cache it as valid: the next check refuses it.
    #[tokio::test]
    async fn a_key_check_in_flight_across_a_rotation_is_not_cached() {
        let (t, old) = account();
        let ctx = t.ctx.clone();
        let c2 = ctx.clone();
        test_hooks::before_next_fill(&ctx, move || {
            ApiKeyService::regenerate(&c2, "trader").unwrap();
        });
        assert_eq!(
            ApiKeyService::check(&ctx, &old),
            KeyCheck::Valid("trader".into())
        );
        assert_eq!(ApiKeyService::check(&ctx, &old), KeyCheck::Invalid);
        let new = ApiKeyService::current(&ctx).unwrap().unwrap();
        assert!(ApiKeyService::is_valid(&ctx, new.expose()));
    }

    /// KEY-01: an order-mode read in flight while the mode changes cannot
    /// put the old mode back for its 60 seconds.
    #[tokio::test]
    async fn an_order_mode_read_in_flight_across_a_change_is_not_cached() {
        let (t, _) = account();
        let ctx = t.ctx.clone();
        assert_eq!(ApiKeyService::order_mode(&ctx).unwrap(), "auto");
        ctx.api_keys.clear();
        let c2 = ctx.clone();
        test_hooks::before_next_fill(&ctx, move || {
            ApiKeyService::set_order_mode(&c2, "semi_auto").unwrap();
        });
        assert_eq!(
            ApiKeyService::order_mode(&ctx).unwrap(),
            "auto",
            "read before the change"
        );
        assert_eq!(ApiKeyService::order_mode(&ctx).unwrap(), "semi_auto");
        // A change invalidates a cached mode at once.
        ApiKeyService::set_order_mode(&ctx, "auto").unwrap();
        assert_eq!(ApiKeyService::order_mode(&ctx).unwrap(), "auto");
    }

    /// KEY-01: while the keys are locked nothing is checked and no cached
    /// answer is given, even one stored before the lock.
    #[tokio::test]
    async fn while_locked_no_key_is_checked_or_answered_from_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = crate::state::AppState::open(
            dir.path(),
            crate::state::OpenOptions {
                keystore: std::sync::Arc::new(crate::security::keystore::UnavailableKeyStore),
                clock: crate::clock::ManualClock::new(chrono::Utc::now()),
                brokers: std::sync::Arc::new(crate::brokers::BrokerRegistry::with(vec![])),
            },
        )
        .unwrap();
        assert!(!ctx.security.is_unlocked());
        ctx.api_keys.fill(
            "k",
            &KeyCheck::Valid("trader".into()),
            ctx.api_keys.generations(),
            Instant::now(),
        );
        assert_eq!(ApiKeyService::check(&ctx, "k"), KeyCheck::Locked);
        assert_eq!(ctx.api_keys.hash_checks(), 0);
    }
}

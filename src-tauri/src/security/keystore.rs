//! Where the data key and the pepper live.
//!
//! Production uses the OS keychain through the `keyring` crate (macOS
//! Keychain, Windows Credential Manager, Linux Secret Service). Tests use
//! `MemoryKeyStore`. `KeyStoreError::Unavailable` means there is no usable
//! keychain on this machine (headless Linux, some Raspberry Pi setups); the
//! security manager then falls back to a password-derived key.

use parking_lot::Mutex;
use std::collections::HashMap;

pub const SERVICE: &str = "com.openalgo.desktop";
pub const DATA_KEY: &str = "data-key";
pub const PEPPER: &str = "api-key-pepper";

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error("no usable keychain: {0}")]
    Unavailable(String),
    #[error("keychain failure: {0}")]
    Failure(String),
}

pub trait KeyStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, KeyStoreError>;
    fn set(&self, name: &str, value: &str) -> Result<(), KeyStoreError>;
    fn delete(&self, name: &str) -> Result<(), KeyStoreError>;
    fn kind(&self) -> &'static str;
}

/// OS keychain.
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self {
            service: SERVICE.to_string(),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, KeyStoreError> {
        keyring::Entry::new(&self.service, name).map_err(classify)
    }
}

impl Default for KeyringStore {
    fn default() -> Self {
        Self::new()
    }
}

fn classify(e: keyring::Error) -> KeyStoreError {
    match e {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            KeyStoreError::Unavailable(e.to_string())
        }
        other => KeyStoreError::Failure(other.to_string()),
    }
}

impl KeyStore for KeyringStore {
    fn get(&self, name: &str) -> Result<Option<String>, KeyStoreError> {
        match self.entry(name)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(classify(e)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), KeyStoreError> {
        self.entry(name)?.set_password(value).map_err(classify)
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(classify(e)),
        }
    }

    fn kind(&self) -> &'static str {
        "keychain"
    }
}

/// In-memory store for tests and for the in-process HTTP test harness.
#[derive(Default)]
pub struct MemoryKeyStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemoryKeyStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, name: &str) -> Result<Option<String>, KeyStoreError> {
        Ok(self.values.lock().get(name).cloned())
    }

    fn set(&self, name: &str, value: &str) -> Result<(), KeyStoreError> {
        self.values
            .lock()
            .insert(name.to_string(), value.to_string());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.values.lock().remove(name);
        Ok(())
    }

    fn kind(&self) -> &'static str {
        "memory"
    }
}

/// A store that behaves like a machine with no keychain.
#[derive(Default)]
pub struct UnavailableKeyStore;

impl KeyStore for UnavailableKeyStore {
    fn get(&self, _: &str) -> Result<Option<String>, KeyStoreError> {
        Err(KeyStoreError::Unavailable("no secret service".into()))
    }
    fn set(&self, _: &str, _: &str) -> Result<(), KeyStoreError> {
        Err(KeyStoreError::Unavailable("no secret service".into()))
    }
    fn delete(&self, _: &str) -> Result<(), KeyStoreError> {
        Err(KeyStoreError::Unavailable("no secret service".into()))
    }
    fn kind(&self) -> &'static str {
        "unavailable"
    }
}

/// A keychain whose faults a test switches on, over an in-memory store:
/// locked or access denied (every call answers `Unavailable`, as
/// `classify` maps those platform errors), and writes that start failing
/// after a number of successful ones (a keychain refusing a write midway).
#[cfg(test)]
#[derive(Default)]
pub struct FaultyKeyStore {
    pub inner: MemoryKeyStore,
    pub unavailable: std::sync::atomic::AtomicBool,
    /// Writes still allowed before every further write fails; `None`
    /// allows all.
    pub writes_left: Mutex<Option<usize>>,
}

#[cfg(test)]
impl FaultyKeyStore {
    pub fn set_unavailable(&self, on: bool) {
        self.unavailable
            .store(on, std::sync::atomic::Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), KeyStoreError> {
        if self.unavailable.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(KeyStoreError::Unavailable("the keychain is locked".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
impl KeyStore for FaultyKeyStore {
    fn get(&self, name: &str) -> Result<Option<String>, KeyStoreError> {
        self.check()?;
        self.inner.get(name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), KeyStoreError> {
        self.check()?;
        let mut left = self.writes_left.lock();
        match left.as_mut() {
            Some(0) => return Err(KeyStoreError::Failure("write refused".into())),
            Some(n) => *n -= 1,
            None => {}
        }
        self.inner.set(name, value)
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.check()?;
        self.inner.delete(name)
    }

    fn kind(&self) -> &'static str {
        "test keychain"
    }
}

/// A keychain that counts every call, over an in-memory store: the tests
/// that prove a new install never touches the OS keychain and a migrated
/// one never reads it again (KEY-01).
#[cfg(test)]
#[derive(Default)]
pub struct CountingKeyStore {
    pub inner: FaultyKeyStore,
    pub gets: std::sync::atomic::AtomicUsize,
    pub sets: std::sync::atomic::AtomicUsize,
    pub deletes: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl CountingKeyStore {
    /// Every call of any kind so far.
    pub fn calls(&self) -> usize {
        use std::sync::atomic::Ordering::SeqCst;
        self.gets.load(SeqCst) + self.sets.load(SeqCst) + self.deletes.load(SeqCst)
    }

    pub fn reads(&self) -> usize {
        self.gets.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Put a value in without counting it (an earlier build's entries).
    pub fn seed(&self, name: &str, value: &str) {
        self.inner.inner.set(name, value).unwrap_or_default();
    }

    /// What is stored, without counting.
    pub fn peek(&self, name: &str) -> Option<String> {
        self.inner.inner.get(name).ok().flatten()
    }
}

#[cfg(test)]
impl KeyStore for CountingKeyStore {
    fn get(&self, name: &str) -> Result<Option<String>, KeyStoreError> {
        self.gets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.get(name)
    }

    fn set(&self, name: &str, value: &str) -> Result<(), KeyStoreError> {
        self.sets.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.set(name, value)
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        self.deletes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.delete(name)
    }

    fn kind(&self) -> &'static str {
        "counted test keychain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip() {
        let s = MemoryKeyStore::new();
        assert_eq!(s.get(DATA_KEY).unwrap(), None);
        s.set(DATA_KEY, "abc").unwrap();
        assert_eq!(s.get(DATA_KEY).unwrap().as_deref(), Some("abc"));
        s.delete(DATA_KEY).unwrap();
        assert_eq!(s.get(DATA_KEY).unwrap(), None);
    }

    #[test]
    fn unavailable_store_reports_unavailable() {
        assert!(matches!(
            UnavailableKeyStore.get(DATA_KEY),
            Err(KeyStoreError::Unavailable(_))
        ));
    }
}

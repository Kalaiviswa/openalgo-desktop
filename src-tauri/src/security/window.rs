//! The app window's proof (KEY-01 security review).
//!
//! The local server cannot tell the OpenAlgo Desktop window from a browser
//! tab or a program on the same computer: all of them are loopback, with the
//! same host and origin. The window can do one thing they cannot: call the
//! shell's commands. Each start makes a random token; the `window_proof`
//! command gives it to the main window on the app's own page only, and the
//! sign-in page sends it in the `X-OpenAlgo-Window` header. A sign-in from
//! this computer that carries it is the app window's: only that one reads an
//! earlier build's keychain, moves keys, and is told why keys could not be
//! opened (`server::middleware::sign_in_from`).
//!
//! The token is never stored, never logged, and new at every start.

use crate::security::Secret;
use rand::RngCore;
use subtle::ConstantTimeEq;

/// The request header the app window's sign-in carries the proof in.
pub const HEADER: &str = "x-openalgo-window";

pub struct WindowProof(Secret);

impl WindowProof {
    /// A new random proof (32 bytes, hex).
    pub fn new() -> Self {
        let mut b = zeroize::Zeroizing::new([0u8; 32]);
        rand::rngs::OsRng.fill_bytes(&mut b[..]);
        WindowProof(Secret::new(hex::encode(&b[..])))
    }

    /// The token, for the `window_proof` command only.
    pub fn token(&self) -> &Secret {
        &self.0
    }

    /// Whether `presented` is this proof (constant time).
    pub fn matches(&self, presented: &str) -> bool {
        let mine = self.0.expose().as_bytes();
        mine.len() == presented.len() && bool::from(mine.ct_eq(presented.as_bytes()))
    }
}

impl Default for WindowProof {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WindowProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WindowProof([REDACTED])")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_matches_itself_only_and_never_prints() {
        let a = WindowProof::new();
        let b = WindowProof::new();
        assert!(a.matches(a.token().expose()));
        assert!(!a.matches(b.token().expose()));
        assert!(!a.matches(""));
        assert!(!a.matches(&a.token().expose()[..63]));
        assert_eq!(a.token().expose().len(), 64);
        assert_eq!(format!("{:?}", a), "WindowProof([REDACTED])");
    }
}

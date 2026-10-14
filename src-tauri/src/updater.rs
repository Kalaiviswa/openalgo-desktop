//! The desktop shell's side of in-app updates: the Tauri update plugin
//! behind [`UpdateBackend`]. The decisions (which version is newer, what a
//! restart interrupts, what the trader confirmed) are in
//! `services::update_service`.
//!
//! The plugin reads the update address and the public key from
//! `tauri.conf.json` (`plugins.updater`). It fetches `latest.json`, picks
//! this computer's entry, downloads it and verifies its minisign signature
//! against that key, including the version written into the signature
//! (`requireSignedVersion`), so an answer from the update address cannot
//! pair a new version number with an older signed release. Pages get none
//! of the plugin's commands: no capability names `updater:*`.

use crate::services::update_service::{
    is_newer, Offer, Package, ProgressFn, UpdateBackend, UpdateFailure, CHECK_TIMEOUT,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use std::path::Path;
use std::time::Duration;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};

/// How long the download of an update may take in all (an AppImage is
/// over 100 MB; a slow line needs minutes).
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How this copy was installed. `bundle` is the bundle type the Tauri
/// bundler wrote into this binary (`tauri::utils::platform::bundle_type`,
/// which answers `App` for any macOS binary); `appimage` whether the
/// AppImage runtime started it; `exe` this executable's path.
pub fn package_of(
    bundle: Option<BundleType>,
    development: bool,
    appimage: bool,
    exe: Option<&Path>,
) -> Package {
    if development {
        return Package::Development;
    }
    match bundle {
        Some(BundleType::App) | Some(BundleType::Dmg) => {
            if exe.is_some_and(inside_app_bundle) {
                Package::MacApp
            } else {
                Package::Unknown
            }
        }
        Some(BundleType::Nsis) => Package::WindowsNsis,
        // The AppImage file itself is replaced: without the runtime's
        // address of it there is nothing to replace.
        Some(BundleType::AppImage) if appimage => Package::LinuxAppImage,
        Some(BundleType::Deb) => Package::LinuxDeb,
        Some(BundleType::Rpm) => Package::LinuxRpm,
        _ => Package::Unknown,
    }
}

/// `<name>.app/Contents/MacOS/<binary>`.
fn inside_app_bundle(exe: &Path) -> bool {
    let macos = exe.parent();
    let contents = macos.and_then(Path::parent);
    let bundle = contents.and_then(Path::parent);
    macos
        .and_then(Path::file_name)
        .is_some_and(|n| n == "MacOS")
        && contents
            .and_then(Path::file_name)
            .is_some_and(|n| n == "Contents")
        && bundle
            .and_then(Path::extension)
            .is_some_and(|e| e.eq_ignore_ascii_case("app"))
}

/// The plugin's error, as the trader's kind of failure (the text stays for
/// the log).
pub fn failure(e: tauri_plugin_updater::Error) -> UpdateFailure {
    use tauri_plugin_updater::Error as E;
    let detail = e.to_string();
    match e {
        E::Reqwest(_) | E::Network(_) => UpdateFailure::Network(detail),
        E::ReleaseNotFound | E::Serialization(_) | E::Semver(_) | E::UrlParse(_) => {
            UpdateFailure::Manifest(detail)
        }
        E::TargetNotFound(_) | E::TargetsNotFound(_) | E::UnsupportedArch | E::UnsupportedOs => {
            UpdateFailure::NoPlatform(detail)
        }
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => UpdateFailure::Signature(detail),
        _ => UpdateFailure::Install(detail),
    }
}

/// The update plugin as the update service's backend. Holds the update
/// last offered and, once downloaded and verified, its bytes (one slot,
/// dropped on Later, a failed install or exit).
pub struct PluginBackend<R: Runtime> {
    app: AppHandle<R>,
    package: Package,
    /// Overrides the configured update address (tests).
    endpoint: Option<url::Url>,
    offered: Mutex<Option<Update>>,
    downloaded: Mutex<Option<(Update, Vec<u8>)>>,
}

impl<R: Runtime> PluginBackend<R> {
    pub fn new(app: AppHandle<R>, package: Package) -> Self {
        Self {
            app,
            package,
            endpoint: None,
            offered: Mutex::new(None),
            downloaded: Mutex::new(None),
        }
    }

    /// The same backend asking `endpoint` instead of the configured address.
    pub fn with_endpoint(app: AppHandle<R>, package: Package, endpoint: url::Url) -> Self {
        Self {
            endpoint: Some(endpoint),
            ..Self::new(app, package)
        }
    }

    /// Whether a downloaded, verified update is waiting.
    pub fn has_download(&self) -> bool {
        self.downloaded.lock().is_some()
    }

    /// Install the downloaded update. On macOS and Linux this replaces the
    /// app (or the AppImage) on disk and the caller restarts; on Windows it
    /// starts the installer (passive, per-user) and this process exits.
    pub fn install(&self) -> Result<(), UpdateFailure> {
        let Some((update, bytes)) = self.downloaded.lock().take() else {
            return Err(UpdateFailure::Install(
                "no downloaded update is waiting".into(),
            ));
        };
        update.install(&bytes).map_err(|e| match failure(e) {
            UpdateFailure::Install(d) => UpdateFailure::Install(d),
            other => UpdateFailure::Install(other.detail().to_string()),
        })
    }
}

#[async_trait]
impl<R: Runtime> UpdateBackend for PluginBackend<R> {
    fn package(&self) -> Package {
        self.package
    }

    async fn check(&self) -> Result<Option<Offer>, UpdateFailure> {
        let mut builder = self
            .app
            .updater_builder()
            .version_comparator(|current, release| is_newer(&current, &release.version))
            .timeout(CHECK_TIMEOUT);
        if let Some(e) = &self.endpoint {
            builder = builder.endpoints(vec![e.clone()]).map_err(failure)?;
        }
        let found = builder
            .build()
            .map_err(failure)?
            .check()
            .await
            .map_err(failure)?;
        let offer = found.as_ref().map(|u| {
            let published = u
                .raw_json
                .get("pub_date")
                .and_then(|d| d.as_str())
                .map(str::to_string);
            Offer::new(&u.version, u.body.clone(), published)
        });
        *self.offered.lock() = found;
        Ok(offer)
    }

    async fn download(&self, version: &str, mut progress: ProgressFn) -> Result<(), UpdateFailure> {
        let offered = self.offered.lock().clone();
        let Some(mut update) = offered.filter(|u| u.version == version) else {
            return Err(UpdateFailure::Manifest(format!(
                "version {} is no longer the offered update",
                version
            )));
        };
        update.timeout = Some(DOWNLOAD_TIMEOUT);
        let mut received: u64 = 0;
        let bytes = update
            .download(
                |chunk, total| {
                    received = received.saturating_add(chunk as u64);
                    progress(received, total);
                },
                || {},
            )
            .await
            .map_err(failure)?;
        *self.downloaded.lock() = Some((update, bytes));
        Ok(())
    }

    fn discard(&self) {
        self.downloaded.lock().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use serde_json::{json, Value};

    /// The key the maintainer published for updates (public; the private
    /// half is a GitHub secret). The app must carry exactly this one.
    const MAINTAINER_PUBKEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDEzMzc5QTJBRTVDMEFCNzEKUldSeHE4RGxLcG8zRXllUlJUVll3VmR6ckMxZzY5QXFqaGxpc3VLMTEzSEcyaDRGeEluODdXTFEK";
    const ENDPOINT: &str =
        "https://github.com/marketcalls/openalgo-desktop/releases/latest/download/latest.json";

    pub(super) fn shipped_config() -> Value {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json");
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    }

    /// The app carries the maintainer's public key and the GitHub address,
    /// enforces the signed version and installs passively on Windows. The
    /// updater artifacts are signed only by the release workflow (CI and
    /// local builds have no private key), so the committed config does not
    /// ask for them.
    #[test]
    fn the_shipped_config_pins_the_maintainers_key_and_address() {
        let conf = shipped_config();
        let up = &conf["plugins"]["updater"];
        assert_eq!(up["pubkey"], MAINTAINER_PUBKEY);
        assert_eq!(up["endpoints"], json!([ENDPOINT]));
        assert_eq!(up["requireSignedVersion"], true);
        assert_eq!(up["windows"]["installMode"], "passive");
        assert!(up.get("dangerousInsecureTransportProtocol").is_none());
        assert!(up.get("allowDowngrades").is_none());
        assert!(conf["bundle"].get("createUpdaterArtifacts").is_none());
        assert_eq!(
            conf["bundle"]["windows"]["nsis"]["installMode"],
            "currentUser"
        );
        // The plugin accepts it (an https address, a well-formed key).
        let parsed: tauri_plugin_updater::Config = serde_json::from_value(up.clone()).unwrap();
        assert!(parsed.require_signed_version);
        let file = String::from_utf8(B64.decode(MAINTAINER_PUBKEY).unwrap()).unwrap();
        let mut lines = file.lines();
        assert_eq!(
            lines.next(),
            Some("untrusted comment: minisign public key: 13379A2AE5C0AB71")
        );
        let raw = B64.decode(lines.next().unwrap()).unwrap();
        assert_eq!(raw.len(), 42);
        assert_eq!(&raw[..2], b"Ed");
    }

    /// Pages reach updates only through the local server and the one
    /// restart command; the plugin's own commands are granted to none.
    #[test]
    fn no_page_is_granted_the_update_plugin() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("capabilities");
        for f in std::fs::read_dir(dir).unwrap() {
            let text = std::fs::read_to_string(f.unwrap().path()).unwrap();
            assert!(!text.contains("updater:"), "{}", text);
        }
        for p in crate::commands::REMOTE_APP_PERMISSIONS {
            assert!(!p.starts_with("updater:"), "{}", p);
        }
        assert!(crate::commands::REMOTE_APP_PERMISSIONS.contains(&"allow-restart-to-update"));
    }

    #[test]
    fn only_installer_built_copies_update_themselves() {
        let app = Path::new("/Applications/OpenAlgo Desktop.app/Contents/MacOS/openalgo-desktop");
        let loose = Path::new("/Users/me/src/target/release/openalgo-desktop");
        assert_eq!(
            package_of(Some(BundleType::App), false, false, Some(app)),
            Package::MacApp
        );
        // bundle_type() says App for every macOS binary, a loose one too.
        assert_eq!(
            package_of(Some(BundleType::App), false, false, Some(loose)),
            Package::Unknown
        );
        assert_eq!(
            package_of(Some(BundleType::Nsis), false, false, None),
            Package::WindowsNsis
        );
        assert_eq!(
            package_of(Some(BundleType::AppImage), false, true, None),
            Package::LinuxAppImage
        );
        assert_eq!(
            package_of(Some(BundleType::AppImage), false, false, None),
            Package::Unknown
        );
        assert_eq!(
            package_of(Some(BundleType::Deb), false, false, None),
            Package::LinuxDeb
        );
        assert_eq!(
            package_of(Some(BundleType::Rpm), false, false, None),
            Package::LinuxRpm
        );
        assert_eq!(package_of(None, false, false, None), Package::Unknown);
        assert_eq!(
            package_of(Some(BundleType::Nsis), true, false, None),
            Package::Development
        );
    }
}

/// The real update plugin on Tauri's mock runtime, against a fake update
/// address. Not on Windows: an app built on the mock runtime links window
/// code that needs the common-controls v6 manifest, which test executables
/// do not carry (the test binary fails to load with
/// STATUS_ENTRYPOINT_NOT_FOUND). The signature check is the same code on
/// every platform.
#[cfg(all(test, not(windows)))]
mod plugin_tests {
    use super::tests::shipped_config;
    use super::*;
    use axum::{extract::State, http::StatusCode, response::IntoResponse, routing::get, Router};
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

    /// A minisign key pair made for one test, never the real key. Signs the
    /// way the Tauri CLI does: prehashed (BLAKE2b-512), with the version in
    /// the trusted comment.
    struct TestKey {
        signing: SigningKey,
        keynum: [u8; 8],
    }

    impl TestKey {
        fn generate() -> Self {
            Self {
                signing: SigningKey::from_bytes(&rand::random::<[u8; 32]>()),
                keynum: rand::random::<[u8; 8]>(),
            }
        }

        /// base64 of the public key file, as `tauri.conf.json` holds it.
        fn pubkey(&self) -> String {
            let mut raw = b"Ed".to_vec();
            raw.extend_from_slice(&self.keynum);
            raw.extend_from_slice(self.signing.verifying_key().as_bytes());
            let file = format!(
                "untrusted comment: minisign public key: {:016X}\n{}\n",
                u64::from_le_bytes(self.keynum),
                B64.encode(raw)
            );
            B64.encode(file)
        }

        /// base64 of the `.sig` file for `data`, signed for `version`.
        fn sign(&self, data: &[u8], version: &str) -> String {
            self.sign_with_comment(
                data,
                &format!(
                    "timestamp:1760500000\tfile:OpenAlgo.Desktop.app.tar.gz\tversion:{}",
                    version
                ),
            )
        }

        /// The same with any trusted comment (one naming no version, as
        /// signatures made before the Tauri CLI recorded it).
        fn sign_with_comment(&self, data: &[u8], trusted: &str) -> String {
            use blake2::{Blake2b512, Digest};
            let sig = self.signing.sign(&Blake2b512::digest(data));
            let mut raw = b"ED".to_vec();
            raw.extend_from_slice(&self.keynum);
            raw.extend_from_slice(&sig.to_bytes());
            let mut whole = sig.to_bytes().to_vec();
            whole.extend_from_slice(trusted.as_bytes());
            let global = self.signing.sign(&whole);
            let file = format!(
                "untrusted comment: signature from tauri secret key\n{}\ntrusted comment: {}\n{}\n",
                B64.encode(raw),
                trusted,
                B64.encode(global.to_bytes())
            );
            B64.encode(file)
        }
    }

    /// A fake update address on an ephemeral loopback port: serves
    /// `/latest.json` and `/artifact`, or 404 for an empty manifest.
    struct FakeServer {
        url: String,
        task: tokio::task::JoinHandle<()>,
    }

    #[derive(Clone)]
    struct Served {
        manifest: Option<Value>,
        artifact: Vec<u8>,
    }

    impl Drop for FakeServer {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn serve(make: impl FnOnce(&str) -> Served) -> FakeServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = Arc::new(make(&url));
        async fn manifest(State(s): State<Arc<Served>>) -> axum::response::Response {
            match &s.manifest {
                Some(m) => axum::Json(m.clone()).into_response(),
                None => StatusCode::NOT_FOUND.into_response(),
            }
        }
        async fn artifact(State(s): State<Arc<Served>>) -> Vec<u8> {
            s.artifact.clone()
        }
        let app = Router::new()
            .route("/latest.json", get(manifest))
            .route("/artifact", get(artifact))
            .with_state(served);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        FakeServer { url, task }
    }

    fn target() -> String {
        tauri_plugin_updater::target().expect("a supported platform")
    }

    fn manifest(url: &str, version: &str, signature: &str) -> Value {
        json!({
            "version": version,
            "notes": "Faster option chain.\n\n- Fixes the expiry picker.",
            "pub_date": "2026-10-15T04:30:00Z",
            "platforms": {
                target(): {"url": format!("{}/artifact", url), "signature": signature}
            }
        })
    }

    /// A mock app (version 0.1.0) with the shipped update settings
    /// (`tauri.conf.json`, the signed-version check among them) and the
    /// shipped version policy, trusting `pubkey` in place of the
    /// maintainer's key.
    fn app_trusting(pubkey: &str) -> tauri::App<MockRuntime> {
        let mut updater = shipped_config()["plugins"]["updater"].clone();
        updater["pubkey"] = json!(pubkey);
        let mut context = mock_context(noop_assets());
        context
            .config_mut()
            .plugins
            .0
            .insert("updater".into(), updater);
        mock_builder()
            .plugin(
                tauri_plugin_updater::Builder::new()
                    .default_version_comparator(|current, release| {
                        is_newer(&current, &release.version)
                    })
                    .build(),
            )
            .build(context)
            .unwrap()
    }

    fn backend(app: &tauri::App<MockRuntime>, server: &FakeServer) -> PluginBackend<MockRuntime> {
        PluginBackend::with_endpoint(
            app.handle().clone(),
            Package::MacApp,
            url::Url::parse(&format!("{}/latest.json", server.url)).unwrap(),
        )
    }

    fn no_progress() -> ProgressFn {
        Box::new(|_, _| {})
    }

    #[tokio::test]
    async fn a_signed_update_is_offered_downloaded_and_verified() {
        let key = TestKey::generate();
        let artifact = b"the new OpenAlgo Desktop".to_vec();
        let sig = key.sign(&artifact, "0.2.0");
        let server = serve(|url| Served {
            manifest: Some(manifest(url, "0.2.0", &sig)),
            artifact: artifact.clone(),
        })
        .await;
        let app = app_trusting(&key.pubkey());
        let b = backend(&app, &server);
        let offer = b.check().await.unwrap().expect("0.2.0 is newer than 0.1.0");
        assert_eq!(offer.version, "0.2.0");
        assert_eq!(
            offer.notes.as_deref(),
            Some("Faster option chain.\n\n- Fixes the expiry picker.")
        );
        assert_eq!(offer.published_at.as_deref(), Some("2026-10-15T04:30:00Z"));
        let seen = Arc::new(Mutex::new(0u64));
        let s2 = seen.clone();
        b.download("0.2.0", Box::new(move |n, _| *s2.lock() = n))
            .await
            .unwrap();
        assert_eq!(*seen.lock(), artifact.len() as u64);
        assert!(b.has_download());
        assert_eq!(b.downloaded.lock().as_ref().unwrap().1, artifact);
        b.discard();
        assert!(!b.has_download());
        // A version that is not the one offered is never downloaded.
        assert!(matches!(
            b.download("0.3.0", no_progress()).await,
            Err(UpdateFailure::Manifest(_))
        ));
    }

    #[tokio::test]
    async fn a_tampered_download_fails_its_signature_check() {
        let key = TestKey::generate();
        let sig = key.sign(b"the real update", "0.2.0");
        let server = serve(|url| Served {
            manifest: Some(manifest(url, "0.2.0", &sig)),
            artifact: b"something else".to_vec(),
        })
        .await;
        let app = app_trusting(&key.pubkey());
        let b = backend(&app, &server);
        assert!(b.check().await.unwrap().is_some());
        let got = b.download("0.2.0", no_progress()).await;
        assert!(matches!(got, Err(UpdateFailure::Signature(_))), "{:?}", got);
        assert!(!b.has_download(), "nothing unverified is kept");
    }

    #[tokio::test]
    async fn a_signature_from_another_key_is_refused() {
        let ours = TestKey::generate();
        let theirs = TestKey::generate();
        let artifact = b"an update signed by someone else".to_vec();
        let sig = theirs.sign(&artifact, "0.2.0");
        let server = serve(|url| Served {
            manifest: Some(manifest(url, "0.2.0", &sig)),
            artifact: artifact.clone(),
        })
        .await;
        let app = app_trusting(&ours.pubkey());
        let b = backend(&app, &server);
        assert!(b.check().await.unwrap().is_some());
        let got = b.download("0.2.0", no_progress()).await;
        assert!(matches!(got, Err(UpdateFailure::Signature(_))), "{:?}", got);
    }

    /// An answer that pairs a new version number with an older signed
    /// release is refused: the signature names the version it was made for.
    #[tokio::test]
    async fn a_signature_made_for_another_version_is_refused() {
        let key = TestKey::generate();
        let old_release = b"genuine but older".to_vec();
        let sig = key.sign(&old_release, "0.1.5");
        let server = serve(|url| Served {
            manifest: Some(manifest(url, "0.2.0", &sig)),
            artifact: old_release.clone(),
        })
        .await;
        let app = app_trusting(&key.pubkey());
        let b = backend(&app, &server);
        assert!(b.check().await.unwrap().is_some());
        let got = b.download("0.2.0", no_progress()).await;
        assert!(matches!(got, Err(UpdateFailure::Signature(_))), "{:?}", got);
    }

    /// `requireSignedVersion` (the shipped setting): a signature naming no
    /// version is refused, so an older release signed before versions were
    /// recorded cannot be passed off under a new version number.
    #[tokio::test]
    async fn a_signature_that_names_no_version_is_refused() {
        let key = TestKey::generate();
        let artifact = b"signed without a version".to_vec();
        let sig = key.sign_with_comment(&artifact, "timestamp:1760500000\tfile:old.app.tar.gz");
        let server = serve(|url| Served {
            manifest: Some(manifest(url, "0.2.0", &sig)),
            artifact: artifact.clone(),
        })
        .await;
        let app = app_trusting(&key.pubkey());
        let b = backend(&app, &server);
        assert!(b.check().await.unwrap().is_some());
        let got = b.download("0.2.0", no_progress()).await;
        assert!(matches!(got, Err(UpdateFailure::Signature(_))), "{:?}", got);
    }

    /// Prerelease tags: the same version, an older one and a pre-release
    /// are not offered to a release build.
    #[tokio::test]
    async fn the_same_an_older_or_a_prerelease_version_is_not_offered() {
        let key = TestKey::generate();
        for version in ["0.1.0", "0.0.9", "0.2.0-beta.1", "v0.1.0"] {
            let sig = key.sign(b"x", version.trim_start_matches('v'));
            let server = serve(|url| Served {
                manifest: Some(manifest(url, version, &sig)),
                artifact: b"x".to_vec(),
            })
            .await;
            let app = app_trusting(&key.pubkey());
            let got = backend(&app, &server).check().await.unwrap();
            assert_eq!(got, None, "{}", version);
        }
    }

    #[tokio::test]
    async fn a_release_without_this_computer_is_reported() {
        let key = TestKey::generate();
        let server = serve(|url| Served {
            manifest: Some(json!({
                "version": "0.2.0",
                "platforms": {"plan9-mips": {"url": format!("{}/artifact", url), "signature": "x"}}
            })),
            artifact: Vec::new(),
        })
        .await;
        let app = app_trusting(&key.pubkey());
        let got = backend(&app, &server).check().await;
        assert!(
            matches!(got, Err(UpdateFailure::NoPlatform(_))),
            "{:?}",
            got
        );
    }

    #[tokio::test]
    async fn a_missing_or_unreachable_update_address_is_reported() {
        let key = TestKey::generate();
        let app = app_trusting(&key.pubkey());
        // The latest release has no latest.json.
        let server = serve(|_| Served {
            manifest: None,
            artifact: Vec::new(),
        })
        .await;
        let got = backend(&app, &server).check().await;
        assert!(matches!(got, Err(UpdateFailure::Manifest(_))), "{:?}", got);
        // Not JSON the updater understands.
        let server = serve(|_| Served {
            manifest: Some(json!({"hello": "world"})),
            artifact: Vec::new(),
        })
        .await;
        let got = backend(&app, &server).check().await;
        assert!(matches!(got, Err(UpdateFailure::Manifest(_))), "{:?}", got);
        // Nothing listening.
        let closed = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let b = PluginBackend::with_endpoint(
            app.handle().clone(),
            Package::MacApp,
            url::Url::parse(&format!("http://{}/latest.json", closed)).unwrap(),
        );
        let got = b.check().await;
        assert!(matches!(got, Err(UpdateFailure::Network(_))), "{:?}", got);
    }
}

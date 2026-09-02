use std::{
    sync::mpsc::{self, Receiver},
    time::Duration,
};

use semver::Version;

use super::{
    keys::KeyRing,
    manifest::{self, ClientContext, PackageFormat, RecoveryAsset, ReleaseAsset, ReleaseManifest},
};

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateState {
    Disabled(String),
    Idle,
    Checking,
    Available {
        manifest: ReleaseManifest,
        asset: Box<ReleaseAsset>,
    },
    Downloading,
    Verifying,
    Installing,
    RestartRequired,
    Recovery(String),
    Failed(String),
    UpToDate,
    HandOff(String),
}

pub trait Transport: Send + Sync + 'static {
    fn get(&self, url: &str, maximum_bytes: u64) -> Result<Vec<u8>, String>;
}

pub trait Installer: Send + Sync + 'static {
    fn install(
        &self,
        format: PackageFormat,
        payload: &[u8],
        recovery: Option<&VerifiedRecovery>,
    ) -> Result<UpdateState, String>;
}

#[derive(Clone, Debug)]
pub struct VerifiedRecovery {
    pub asset: RecoveryAsset,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStrategy {
    RestorePreviousAppImage,
    ReinstallNsis,
    RestoreMacApp,
    SystemPackageManager,
}

pub fn recovery_strategy(format: PackageFormat) -> RecoveryStrategy {
    match format {
        PackageFormat::Appimage => RecoveryStrategy::RestorePreviousAppImage,
        PackageFormat::Nsis => RecoveryStrategy::ReinstallNsis,
        PackageFormat::App => RecoveryStrategy::RestoreMacApp,
        PackageFormat::Deb => RecoveryStrategy::SystemPackageManager,
    }
}

pub struct CheckOnlyInstaller;

impl Installer for CheckOnlyInstaller {
    fn install(
        &self,
        _format: PackageFormat,
        _payload: &[u8],
        _recovery: Option<&VerifiedRecovery>,
    ) -> Result<UpdateState, String> {
        Err("installation requires explicit user confirmation".to_string())
    }
}

pub struct PlatformInstaller;

impl Installer for PlatformInstaller {
    fn install(
        &self,
        format: PackageFormat,
        payload: &[u8],
        recovery: Option<&VerifiedRecovery>,
    ) -> Result<UpdateState, String> {
        match recovery_strategy(format) {
            RecoveryStrategy::SystemPackageManager => Ok(UpdateState::HandOff(
                "Mise à jour déléguée au gestionnaire de paquets système.".to_string(),
            )),
            RecoveryStrategy::ReinstallNsis => {
                let recovery = recovery.ok_or_else(|| {
                    "verified NSIS recovery payload missing; manual reinstall required".to_string()
                })?;
                let path = std::env::temp_dir().join("DevToolBox-update.exe");
                let recovery_path = std::env::temp_dir().join(format!(
                    "DevToolBox-recovery-{}.exe",
                    recovery.asset.version
                ));
                std::fs::write(&recovery_path, &recovery.payload)
                    .map_err(|error| error.to_string())?;
                std::fs::write(&path, payload).map_err(|error| error.to_string())?;
                std::process::Command::new(&path)
                    .arg("/S")
                    .spawn()
                    .map_err(|error| format!("cannot launch NSIS updater: {error}"))?;
                Ok(UpdateState::RestartRequired)
            }
            RecoveryStrategy::RestorePreviousAppImage => install_appimage(payload),
            RecoveryStrategy::RestoreMacApp => {
                let recovery = recovery.ok_or_else(|| {
                    "verified macOS recovery payload missing; manual reinstall required".to_string()
                })?;
                let recovery_path = std::env::temp_dir().join(format!(
                    "DevToolBox-recovery-{}.dmg",
                    recovery.asset.version
                ));
                std::fs::write(&recovery_path, &recovery.payload)
                    .map_err(|error| error.to_string())?;
                Ok(UpdateState::HandOff(
                    "Archives macOS de mise à jour et de récupération vérifiées; installation manuelle sécurisée requise.".to_string(),
                ))
            }
        }
    }
}

#[cfg(unix)]
fn install_appimage(payload: &[u8]) -> Result<UpdateState, String> {
    use std::os::unix::fs::PermissionsExt as _;
    let current = std::env::var_os("APPIMAGE")
        .map(std::path::PathBuf::from)
        .ok_or_else(|| "APPIMAGE path unavailable; manual reinstall required".to_string())?;
    let backup = current.with_extension("AppImage.previous");
    let staged = current.with_extension("AppImage.new");
    std::fs::copy(&current, &backup).map_err(|error| error.to_string())?;
    std::fs::write(&staged, payload).map_err(|error| error.to_string())?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    std::fs::rename(&staged, &current).map_err(|error| {
        let _ = std::fs::copy(&backup, &current);
        format!("AppImage replacement failed; previous image restored: {error}")
    })?;
    Ok(UpdateState::RestartRequired)
}

#[cfg(not(unix))]
fn install_appimage(_payload: &[u8]) -> Result<UpdateState, String> {
    Err("AppImage installation is only available on Linux".to_string())
}

pub fn current_package_format() -> Option<PackageFormat> {
    use cargo_packager_resource_resolver::PackageFormat as PackagerFormat;
    match cargo_packager_resource_resolver::current_format().ok()? {
        PackagerFormat::App | PackagerFormat::Dmg => Some(PackageFormat::App),
        PackagerFormat::Nsis => Some(PackageFormat::Nsis),
        PackagerFormat::AppImage => Some(PackageFormat::Appimage),
        PackagerFormat::Deb => Some(PackageFormat::Deb),
        _ => None,
    }
}

pub fn current_target() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    (os, arch)
}

pub struct HttpTransport;

impl Transport for HttpTransport {
    fn get(&self, url: &str, maximum_bytes: u64) -> Result<Vec<u8>, String> {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .https_only(true)
            .max_redirects(3)
            .build();
        let agent = ureq::Agent::new_with_config(config);
        let mut attempt = 0;
        let mut response = loop {
            match agent.get(url).call() {
                Ok(response) => break response,
                Err(error) => {
                    let status = match &error {
                        ureq::Error::StatusCode(code) => Some(*code),
                        _ => None,
                    };
                    let Some(delay) = status.and_then(|code| backoff_seconds(code, attempt)) else {
                        return Err(error.to_string());
                    };
                    if attempt >= 2 {
                        return Err(error.to_string());
                    }
                    std::thread::sleep(Duration::from_secs(delay));
                    attempt += 1;
                }
            }
        };
        response
            .body_mut()
            .with_config()
            .limit(maximum_bytes)
            .read_to_vec()
            .map_err(|error| error.to_string())
    }
}

pub struct UpdateService<T, I> {
    transport: T,
    installer: I,
    ring: KeyRing,
}

impl<T: Transport, I: Installer> UpdateService<T, I> {
    pub fn new(transport: T, installer: I, ring: KeyRing) -> Self {
        Self {
            transport,
            installer,
            ring,
        }
    }

    pub fn check(
        &self,
        endpoint: &str,
        context: &ClientContext<'_>,
    ) -> Result<(ReleaseManifest, ReleaseAsset), String> {
        if self.ring.is_empty() {
            return Err("updater non configuré: aucune clé de production embarquée".to_string());
        }
        let bytes = self.transport.get(endpoint, 1024 * 1024)?;
        let manifest: ReleaseManifest =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let asset = manifest::select_asset(&manifest, context)?.clone();
        Ok((manifest, asset))
    }

    #[cfg(test)]
    pub fn download_and_install(
        &self,
        manifest: &ReleaseManifest,
        asset: &ReleaseAsset,
        now_epoch_days: u64,
    ) -> Result<UpdateState, String> {
        self.download_and_install_with_progress(manifest, asset, now_epoch_days, |_| {})
    }

    fn download_and_install_with_progress(
        &self,
        manifest: &ReleaseManifest,
        asset: &ReleaseAsset,
        now_epoch_days: u64,
        mut progress: impl FnMut(UpdateState),
    ) -> Result<UpdateState, String> {
        if matches!(asset.format, PackageFormat::Deb) {
            return Ok(UpdateState::HandOff(
                "Mise à jour déléguée au gestionnaire de paquets système.".to_string(),
            ));
        }
        progress(UpdateState::Downloading);
        let recovery = if matches!(asset.format, PackageFormat::Nsis | PackageFormat::App) {
            let recovery_asset = asset.recovery.as_ref().ok_or_else(|| {
                "payload de récupération absent; réinstallation manuelle requise".to_string()
            })?;
            let payload = self
                .transport
                .get(&recovery_asset.url, recovery_asset.size)
                .map_err(|error| format!("recovery download failed: {error}"))?;
            manifest::verify_recovery_payload(recovery_asset, &payload, &self.ring, now_epoch_days)
                .map_err(|error| format!("recovery verification failed: {error}"))?;
            Some(VerifiedRecovery {
                asset: recovery_asset.clone(),
                payload,
            })
        } else {
            None
        };
        let payload = self.transport.get(&asset.url, asset.size)?;
        progress(UpdateState::Verifying);
        manifest::verify_payload(
            asset,
            &payload,
            &self.ring,
            manifest.version.minor,
            now_epoch_days,
        )?;
        progress(UpdateState::Installing);
        self.installer
            .install(asset.format, &payload, recovery.as_ref())
    }
}

pub fn spawn_check<T: Transport, I: Installer>(
    service: UpdateService<T, I>,
    endpoint: String,
    current: Version,
    os: String,
    arch: String,
    format: PackageFormat,
    now_epoch_days: u64,
) -> Receiver<UpdateState> {
    let (sender, receiver) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let _ = sender.send(UpdateState::Checking);
        let context = ClientContext {
            current: &current,
            os: &os,
            arch: &arch,
            format,
            now_epoch_days,
        };
        match service.check(&endpoint, &context) {
            Ok((manifest, asset)) => {
                let _ = sender.send(UpdateState::Available {
                    manifest,
                    asset: Box::new(asset),
                });
            }
            Err(error) if error.contains("not newer") => {
                let _ = sender.send(UpdateState::UpToDate);
            }
            Err(error) => {
                let state = if error.contains("recovery") {
                    UpdateState::Recovery(error)
                } else {
                    UpdateState::Failed(error)
                };
                let _ = sender.send(state);
            }
        }
    });
    receiver
}

pub fn spawn_install<T: Transport, I: Installer>(
    service: UpdateService<T, I>,
    manifest: ReleaseManifest,
    asset: ReleaseAsset,
    now_epoch_days: u64,
) -> Receiver<UpdateState> {
    let (sender, receiver) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        match service.download_and_install_with_progress(
            &manifest,
            &asset,
            now_epoch_days,
            |state| {
                let _ = sender.send(state);
            },
        ) {
            Ok(state) => {
                let _ = sender.send(state);
            }
            Err(error) => {
                let state = if error.contains("recovery") {
                    UpdateState::Recovery(error)
                } else {
                    UpdateState::Failed(error)
                };
                let _ = sender.send(state);
            }
        }
    });
    receiver
}

pub fn should_auto_check(
    last_epoch_secs: Option<u64>,
    now_epoch_secs: u64,
    jitter_secs: u64,
) -> bool {
    let jitter = jitter_secs.min(3600);
    last_epoch_secs.is_none_or(|last| now_epoch_secs.saturating_sub(last) >= 86_400 + jitter)
}

pub fn backoff_seconds(status: u16, attempt: u32) -> Option<u64> {
    matches!(status, 403 | 429).then(|| (30_u64.saturating_mul(1 << attempt.min(6))).min(3600))
}

#[cfg(test)]
pub fn cargo_packager_config(public_key: String) -> cargo_packager_updater::Config {
    cargo_packager_updater::Config {
        endpoints: vec![super::MANIFEST_ENDPOINT.parse().expect("constant URL")],
        pubkey: public_key,
        ..Default::default()
    }
}

pub fn update_check_path() -> std::path::PathBuf {
    crate::platform::state_log_path().with_file_name("update-check.txt")
}

pub fn read_last_check() -> Option<u64> {
    std::fs::read_to_string(update_check_path())
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn record_check(now_epoch_secs: u64) {
    let path = update_check_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let temporary = path.with_extension("tmp");
    if std::fs::write(&temporary, now_epoch_secs.to_string()).is_ok() {
        let _ = std::fs::rename(temporary, path);
    }
}

pub fn machine_jitter_secs(machine_id: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    machine_id.hash(&mut hasher);
    hasher.finish() % 3601
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::{Signer as _, SigningKey};
    use sha2::{Digest as _, Sha256};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use crate::update::manifest::PayloadSignature;

    struct Fixtures(Mutex<HashMap<String, Vec<u8>>>);
    impl Transport for Fixtures {
        fn get(&self, url: &str, maximum_bytes: u64) -> Result<Vec<u8>, String> {
            let value = self.0.lock().unwrap().get(url).cloned().ok_or("offline")?;
            if value.len() as u64 > maximum_bytes {
                Err("fixture exceeds limit".into())
            } else {
                Ok(value)
            }
        }
    }
    struct Recorder;
    impl Installer for Recorder {
        fn install(
            &self,
            _format: PackageFormat,
            _payload: &[u8],
            _recovery: Option<&VerifiedRecovery>,
        ) -> Result<UpdateState, String> {
            Ok(UpdateState::RestartRequired)
        }
    }

    struct RecoveryRecorder(Arc<Mutex<Vec<bool>>>);
    impl Installer for RecoveryRecorder {
        fn install(
            &self,
            _format: PackageFormat,
            _payload: &[u8],
            recovery: Option<&VerifiedRecovery>,
        ) -> Result<UpdateState, String> {
            self.0.lock().unwrap().push(recovery.is_some());
            Ok(UpdateState::RestartRequired)
        }
    }

    type SignedFixture = (
        UpdateService<Fixtures, RecoveryRecorder>,
        ReleaseManifest,
        ReleaseAsset,
        Arc<Mutex<Vec<bool>>>,
    );

    fn signed_fixture(tamper_recovery: bool) -> SignedFixture {
        let update = b"update-v0.11.0".to_vec();
        let recovery = b"recovery-v0.10.0".to_vec();
        let signing = SigningKey::from_bytes(&[7; 32]);
        let encode_signature = |bytes: &[u8], minor| PayloadSignature {
            key_id: "fixture".into(),
            signature: base64::engine::general_purpose::STANDARD
                .encode(signing.sign(bytes).to_bytes()),
            activated_minor: minor,
            valid_until_epoch_days: 30_000,
        };
        let recovery_asset = RecoveryAsset {
            version: Version::parse("0.10.0").unwrap(),
            os: "windows".into(),
            arch: "x86_64".into(),
            format: PackageFormat::Nsis,
            url: "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.10.0/DevToolBox-recovery.exe".into(),
            size: recovery.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&recovery)),
            signatures: vec![encode_signature(&recovery, 10)],
        };
        let asset = ReleaseAsset {
            os: "windows".into(),
            arch: "x86_64".into(),
            format: PackageFormat::Nsis,
            url: "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.11.0/DevToolBox-update.exe".into(),
            size: update.len() as u64,
            sha256: format!("{:x}", Sha256::digest(&update)),
            signatures: vec![encode_signature(&update, 11)],
            recovery: Some(recovery_asset.clone()),
        };
        let manifest = ReleaseManifest {
            schema_version: 1,
            version: Version::parse("0.11.0").unwrap(),
            notes: "fixture".into(),
            assets: vec![asset.clone()],
        };
        let mut responses = HashMap::new();
        responses.insert(asset.url.clone(), update);
        responses.insert(
            recovery_asset.url.clone(),
            if tamper_recovery {
                b"tampered-recovery".to_vec()
            } else {
                recovery
            },
        );
        let public_key =
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().as_bytes());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let service = UpdateService::new(
            Fixtures(Mutex::new(responses)),
            RecoveryRecorder(Arc::clone(&calls)),
            KeyRing::from_json(&format!(r#"{{"fixture":"{public_key}"}}"#)).unwrap(),
        );
        (service, manifest, asset, calls)
    }

    #[test]
    fn cadence_jitter_and_backoff_are_bounded() {
        assert!(should_auto_check(None, 1, 99_999));
        assert!(!should_auto_check(Some(100), 86_500, 3600));
        assert!(should_auto_check(Some(100), 90_100, 3600));
        assert_eq!(backoff_seconds(429, 10), Some(1920));
        assert_eq!(backoff_seconds(500, 1), None);
        assert!(machine_jitter_secs("fixture") <= 3600);
    }

    #[test]
    fn empty_production_ring_disables_network_before_transport() {
        let service = UpdateService::new(
            Fixtures(Mutex::new(HashMap::new())),
            Recorder,
            KeyRing::default(),
        );
        let version = Version::parse("0.10.0").unwrap();
        let context = ClientContext {
            current: &version,
            os: "windows",
            arch: "x86_64",
            format: PackageFormat::Nsis,
            now_epoch_days: 20_000,
        };
        assert!(service
            .check("https://example.invalid", &context)
            .unwrap_err()
            .contains("non configuré"));
    }

    #[test]
    fn packager_manifest_contract_uses_the_pinned_endpoint() {
        let config = cargo_packager_config("fixture".into());
        assert_eq!(
            config.endpoints[0].as_str(),
            super::super::MANIFEST_ENDPOINT
        );
    }

    #[test]
    fn recovery_is_downloaded_and_verified_before_installer_runs() {
        let (service, manifest, asset, calls) = signed_fixture(false);
        assert_eq!(
            service.download_and_install(&manifest, &asset, 20_000),
            Ok(UpdateState::RestartRequired)
        );
        assert_eq!(*calls.lock().unwrap(), vec![true]);
    }

    #[test]
    fn invalid_recovery_blocks_installation_without_touching_current_version() {
        let (service, manifest, asset, calls) = signed_fixture(true);
        let error = service
            .download_and_install(&manifest, &asset, 20_000)
            .unwrap_err();
        assert!(error.contains("recovery"));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn interrupted_installation_has_a_deterministic_recovery_strategy() {
        assert_eq!(
            recovery_strategy(PackageFormat::Appimage),
            RecoveryStrategy::RestorePreviousAppImage
        );
        assert_eq!(
            recovery_strategy(PackageFormat::Nsis),
            RecoveryStrategy::ReinstallNsis
        );
        assert_eq!(
            recovery_strategy(PackageFormat::App),
            RecoveryStrategy::RestoreMacApp
        );
        assert_eq!(
            recovery_strategy(PackageFormat::Deb),
            RecoveryStrategy::SystemPackageManager
        );
    }
}

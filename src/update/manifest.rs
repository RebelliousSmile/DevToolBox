#[cfg(test)]
use base64::Engine as _;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::keys::KeyRing;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_PAYLOAD_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PackageFormat {
    App,
    Nsis,
    Appimage,
    Deb,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PayloadSignature {
    pub key_id: String,
    pub signature: String,
    pub activated_minor: u64,
    pub valid_until_epoch_days: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub os: String,
    pub arch: String,
    pub format: PackageFormat,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub signatures: Vec<PayloadSignature>,
    pub recovery: Option<RecoveryAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RecoveryAsset {
    pub version: Version,
    pub os: String,
    pub arch: String,
    pub format: PackageFormat,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub signatures: Vec<PayloadSignature>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: Version,
    pub notes: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Clone, Debug)]
pub struct ClientContext<'a> {
    pub current: &'a Version,
    pub os: &'a str,
    pub arch: &'a str,
    pub format: PackageFormat,
    pub now_epoch_days: u64,
}

pub fn select_asset<'a>(
    manifest: &'a ReleaseManifest,
    context: &ClientContext<'_>,
) -> Result<&'a ReleaseAsset, String> {
    if manifest.schema_version != SCHEMA_VERSION {
        return Err("unsupported update manifest schema".to_string());
    }
    let first = Version::parse(super::FIRST_UPDATER_VERSION).expect("constant semver");
    if context.current < &first {
        return Err("manual installation required before integrated updates".to_string());
    }
    if &manifest.version <= context.current {
        return Err("release is not newer than the installed version".to_string());
    }
    let asset = manifest
        .assets
        .iter()
        .find(|asset| {
            asset.os == context.os && asset.arch == context.arch && asset.format == context.format
        })
        .ok_or_else(|| "no update asset for this platform".to_string())?;
    validate_asset(asset, &manifest.version, context)?;
    Ok(asset)
}

fn validate_asset(
    asset: &ReleaseAsset,
    release: &Version,
    context: &ClientContext<'_>,
) -> Result<(), String> {
    if asset.size == 0 || asset.size > MAX_PAYLOAD_BYTES {
        return Err("update payload size is outside the accepted range".to_string());
    }
    validate_release_url(&asset.url)?;
    if matches!(asset.format, PackageFormat::App | PackageFormat::Nsis) {
        let recovery = asset
            .recovery
            .as_ref()
            .ok_or_else(|| "signed recovery payload is required".to_string())?;
        validate_recovery(recovery, context)?;
    }
    let eligible = asset.signatures.iter().any(|signature| {
        context.now_epoch_days <= signature.valid_until_epoch_days
            && release.minor <= signature.activated_minor.saturating_add(2)
    });
    if !eligible {
        return Err("no signature inside the rotation window".to_string());
    }
    Ok(())
}

fn validate_recovery(recovery: &RecoveryAsset, context: &ClientContext<'_>) -> Result<(), String> {
    if &recovery.version != context.current
        || recovery.os != context.os
        || recovery.arch != context.arch
        || recovery.format != context.format
    {
        return Err("recovery payload does not match the installed application".to_string());
    }
    if recovery.size == 0 || recovery.size > MAX_PAYLOAD_BYTES {
        return Err("recovery payload size is outside the accepted range".to_string());
    }
    validate_release_url(&recovery.url)?;
    let expected_tag = format!("/releases/download/v{}/", recovery.version);
    if !recovery.url.contains(&expected_tag) {
        return Err("recovery payload URL must reference the installed version tag".to_string());
    }
    if !recovery.signatures.iter().any(|signature| {
        context.now_epoch_days <= signature.valid_until_epoch_days
            && recovery.version.minor <= signature.activated_minor.saturating_add(2)
    }) {
        return Err("recovery payload has no signature inside the rotation window".to_string());
    }
    Ok(())
}

fn validate_release_url(value: &str) -> Result<(), String> {
    let url = cargo_packager_updater::url::Url::parse(value)
        .map_err(|error| format!("invalid update URL: {error}"))?;
    let allowed = url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url
            .path()
            .starts_with("/RebelliousSmile/DevToolBox/releases/download/");
    if allowed {
        Ok(())
    } else {
        Err("update URL must be an immutable DevToolBox GitHub release asset".to_string())
    }
}

pub fn verify_payload(
    asset: &ReleaseAsset,
    payload: &[u8],
    ring: &KeyRing,
    release_minor: u64,
    now_epoch_days: u64,
) -> Result<String, String> {
    if payload.len() as u64 != asset.size {
        return Err("downloaded payload size mismatch".to_string());
    }
    let digest = Sha256::digest(payload);
    if format!("{digest:x}") != asset.sha256.to_ascii_lowercase() {
        return Err("downloaded payload hash mismatch".to_string());
    }
    for signature in &asset.signatures {
        if now_epoch_days > signature.valid_until_epoch_days
            || release_minor > signature.activated_minor.saturating_add(2)
        {
            continue;
        }
        if ring
            .verify(&signature.key_id, payload, &signature.signature)
            .is_ok()
        {
            return Ok(signature.key_id.clone());
        }
    }
    Err("no known valid signature accepted the payload".to_string())
}

pub fn verify_recovery_payload(
    asset: &RecoveryAsset,
    payload: &[u8],
    ring: &KeyRing,
    now_epoch_days: u64,
) -> Result<String, String> {
    if payload.len() as u64 != asset.size {
        return Err("downloaded recovery payload size mismatch".to_string());
    }
    let digest = Sha256::digest(payload);
    if format!("{digest:x}") != asset.sha256.to_ascii_lowercase() {
        return Err("downloaded recovery payload hash mismatch".to_string());
    }
    for signature in &asset.signatures {
        if now_epoch_days > signature.valid_until_epoch_days
            || asset.version.minor > signature.activated_minor.saturating_add(2)
        {
            continue;
        }
        if ring
            .verify(&signature.key_id, payload, &signature.signature)
            .is_ok()
        {
            return Ok(signature.key_id.clone());
        }
    }
    Err("no known valid signature accepted the recovery payload".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn fixture() -> (ReleaseManifest, KeyRing, Vec<u8>) {
        let payload = b"fixture update".to_vec();
        let recovery_payload = b"fixture recovery".to_vec();
        let signing = SigningKey::from_bytes(&[9; 32]);
        let key =
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().as_bytes());
        let signature =
            base64::engine::general_purpose::STANDARD.encode(signing.sign(&payload).to_bytes());
        let digest = Sha256::digest(&payload);
        let recovery_signature = base64::engine::general_purpose::STANDARD
            .encode(signing.sign(&recovery_payload).to_bytes());
        let recovery_digest = Sha256::digest(&recovery_payload);
        (
            ReleaseManifest {
                schema_version: 1,
                version: Version::parse("0.11.0").unwrap(),
                notes: "fixture".into(),
                assets: vec![ReleaseAsset {
                    os: "windows".into(),
                    arch: "x86_64".into(),
                    format: PackageFormat::Nsis,
                    url: "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.11.0/DevToolBox.exe".into(),
                    size: payload.len() as u64,
                    sha256: format!("{digest:x}"),
                    signatures: vec![PayloadSignature {
                        key_id: "fixture".into(),
                        signature,
                        activated_minor: 11,
                        valid_until_epoch_days: 30_000,
                    }],
                    recovery: Some(RecoveryAsset {
                        version: Version::parse("0.10.0").unwrap(),
                        os: "windows".into(),
                        arch: "x86_64".into(),
                        format: PackageFormat::Nsis,
                        url: "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.10.0/DevToolBox.exe".into(),
                        size: recovery_payload.len() as u64,
                        sha256: format!("{recovery_digest:x}"),
                        signatures: vec![PayloadSignature {
                            key_id: "fixture".into(),
                            signature: recovery_signature,
                            activated_minor: 10,
                            valid_until_epoch_days: 30_000,
                        }],
                    }),
                }],
            },
            KeyRing::from_json(&format!(r#"{{"fixture":"{key}"}}"#)).unwrap(),
            payload,
        )
    }

    fn context<'a>(current: &'a Version) -> ClientContext<'a> {
        ClientContext {
            current,
            os: "windows",
            arch: "x86_64",
            format: PackageFormat::Nsis,
            now_epoch_days: 20_000,
        }
    }

    #[test]
    fn newer_matching_signed_payload_is_accepted() {
        let (manifest, ring, payload) = fixture();
        let current = Version::parse("0.10.0").unwrap();
        let asset = select_asset(&manifest, &context(&current)).unwrap();
        assert_eq!(
            verify_payload(asset, &payload, &ring, manifest.version.minor, 20_000).unwrap(),
            "fixture"
        );
    }

    #[test]
    fn downgrade_wrong_platform_external_url_and_oversize_are_rejected() {
        let (mut manifest, _, _) = fixture();
        let current = Version::parse("0.11.0").unwrap();
        assert!(select_asset(&manifest, &context(&current)).is_err());
        let current = Version::parse("0.10.0").unwrap();
        manifest.assets[0].os = "macos".into();
        assert!(select_asset(&manifest, &context(&current)).is_err());
        manifest.assets[0].os = "windows".into();
        manifest.assets[0].url = "https://example.com/payload".into();
        assert!(select_asset(&manifest, &context(&current)).is_err());
        manifest.assets[0].url =
            "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.11.0/a".into();
        manifest.assets[0].size = MAX_PAYLOAD_BYTES + 1;
        assert!(select_asset(&manifest, &context(&current)).is_err());
    }

    #[test]
    fn missing_recovery_or_modified_payload_blocks_auto_install() {
        let (mut manifest, ring, mut payload) = fixture();
        let current = Version::parse("0.10.0").unwrap();
        manifest.assets[0].recovery = None;
        assert!(select_asset(&manifest, &context(&current)).is_err());
        let (valid_manifest, _, _) = fixture();
        manifest.assets[0].recovery = valid_manifest.assets[0].recovery.clone();
        let asset = select_asset(&manifest, &context(&current)).unwrap();
        payload[0] ^= 1;
        assert!(verify_payload(asset, &payload, &ring, 11, 20_000).is_err());
    }

    #[test]
    fn recovery_must_match_current_platform_tag_and_signature() {
        let (mut manifest, ring, _) = fixture();
        let current = Version::parse("0.10.0").unwrap();
        manifest.assets[0].recovery.as_mut().unwrap().arch = "aarch64".into();
        assert!(select_asset(&manifest, &context(&current)).is_err());

        let (mut manifest, _, _) = fixture();
        let recovery = manifest.assets[0].recovery.as_mut().unwrap();
        recovery.url =
            "https://github.com/RebelliousSmile/DevToolBox/releases/download/v0.9.1/old.exe".into();
        assert!(select_asset(&manifest, &context(&current)).is_err());

        let (manifest, _, _) = fixture();
        let recovery = manifest.assets[0].recovery.as_ref().unwrap();
        assert!(verify_recovery_payload(recovery, b"tampered recovery", &ring, 20_000).is_err());
    }
}

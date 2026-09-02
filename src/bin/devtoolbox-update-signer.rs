use std::{env, fs, path::PathBuf};

use base64::Engine as _;
use ed25519_dalek::{Signer as _, SigningKey};
use serde::Serialize;
use sha2::{Digest as _, Sha256};

#[derive(Serialize)]
struct Signature {
    key_id: String,
    signature: String,
    activated_minor: u64,
    valid_until_epoch_days: u64,
}

#[derive(Serialize)]
struct SignatureFile {
    sha256: String,
    size: u64,
    signatures: Vec<Signature>,
}

fn main() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or("usage: signer INPUT OUTPUT MINOR VALID_UNTIL")?,
    );
    let output = PathBuf::from(args.next().ok_or("missing output path")?);
    let minor: u64 = args
        .next()
        .ok_or("missing activation minor")?
        .to_string_lossy()
        .parse()
        .map_err(|_| "invalid activation minor")?;
    let valid_until: u64 = args
        .next()
        .ok_or("missing validity date")?
        .to_string_lossy()
        .parse()
        .map_err(|_| "invalid validity date")?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }

    let path = env::var_os("DEVTOOLBOX_UPDATE_PRIVATE_KEYS")
        .map(PathBuf::from)
        .ok_or("DEVTOOLBOX_UPDATE_PRIVATE_KEYS must point to an offline key JSON")?;
    let keys: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if keys.is_empty() {
        return Err("private key JSON is empty".into());
    }
    let public_path = env::var_os("DEVTOOLBOX_UPDATE_PUBLIC_KEYS")
        .map(PathBuf::from)
        .ok_or("DEVTOOLBOX_UPDATE_PUBLIC_KEYS must point to the embedded public key JSON")?;
    let public_keys: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(public_path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    let payload = fs::read(&input).map_err(|error| error.to_string())?;
    let mut signatures = Vec::new();
    for (key_id, value) in keys {
        let encoded = value
            .as_str()
            .ok_or_else(|| format!("private key {key_id} is not a string"))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|error| format!("invalid private key {key_id}: {error}"))?;
        let seed: [u8; 32] = decoded
            .try_into()
            .map_err(|_| format!("private key {key_id} must be a 32-byte seed"))?;
        let signing = SigningKey::from_bytes(&seed);
        let expected_public = public_keys
            .get(&key_id)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                format!("private key {key_id} has no public key in the build keyring")
            })?;
        let actual_public =
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().as_bytes());
        if actual_public != expected_public {
            return Err(format!("private/public updater key mismatch for {key_id}"));
        }
        signatures.push(Signature {
            key_id,
            signature: base64::engine::general_purpose::STANDARD
                .encode(signing.sign(&payload).to_bytes()),
            activated_minor: minor,
            valid_until_epoch_days: valid_until,
        });
    }
    signatures.sort_by(|left, right| left.key_id.cmp(&right.key_id));
    let sidecar = SignatureFile {
        sha256: format!("{:x}", Sha256::digest(&payload)),
        size: payload.len() as u64,
        signatures,
    };
    fs::write(
        output,
        serde_json::to_vec_pretty(&sidecar).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

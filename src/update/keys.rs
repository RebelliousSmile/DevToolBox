use std::collections::HashMap;

use base64::Engine as _;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

include!(concat!(env!("OUT_DIR"), "/update_keys.rs"));

#[derive(Default)]
pub struct KeyRing {
    keys: HashMap<String, VerifyingKey>,
}

impl KeyRing {
    pub fn embedded() -> Result<Self, String> {
        Self::from_json(UPDATE_KEYRING_JSON)
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let encoded: HashMap<String, String> =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        let mut keys = HashMap::new();
        for (id, value) in encoded {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(|error| format!("invalid key {id}: {error}"))?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| format!("invalid key length for {id}"))?;
            let key = VerifyingKey::from_bytes(&bytes)
                .map_err(|error| format!("invalid key {id}: {error}"))?;
            keys.insert(id, key);
        }
        Ok(Self { keys })
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    #[allow(dead_code)]
    pub fn fingerprints(&self) -> Vec<(String, String)> {
        let mut values: Vec<_> = self
            .keys
            .iter()
            .map(|(id, key)| {
                let digest = Sha256::digest(key.as_bytes());
                (id.clone(), format!("{digest:x}"))
            })
            .collect();
        values.sort();
        values
    }

    pub fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> Result<(), String> {
        let key = self
            .keys
            .get(key_id)
            .ok_or_else(|| format!("unknown updater key: {key_id}"))?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .map_err(|error| format!("invalid signature encoding: {error}"))?;
        let signature = Signature::from_slice(&bytes)
            .map_err(|error| format!("invalid signature length: {error}"))?;
        key.verify(payload, &signature)
            .map_err(|_| "update signature rejected".to_string())
    }
}

pub fn configured() -> bool {
    UPDATE_KEYS_CONFIGURED
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn generated_constants_and_fingerprint_match_the_input() {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(signing.verifying_key().as_bytes());
        let ring = KeyRing::from_json(&format!(r#"{{"fixture":"{encoded}"}}"#)).unwrap();
        let payload = b"signed fixture";
        let signature =
            base64::engine::general_purpose::STANDARD.encode(signing.sign(payload).to_bytes());
        ring.verify("fixture", payload, &signature).unwrap();
        assert_eq!(ring.fingerprints().len(), 1);
        assert!(!configured());
    }

    #[test]
    fn unknown_or_modified_signatures_are_rejected() {
        let ring = KeyRing::from_json("{}").unwrap();
        assert!(ring.verify("missing", b"payload", "bad").is_err());
    }
}

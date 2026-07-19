use std::{collections::HashMap, path::Path};

#[cfg(target_os = "macos")]
use apple_native_keyring_store::protected;
#[cfg(target_os = "macos")]
use keyring_core::{self};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use getrandom::fill as random_fill;
#[cfg(not(target_os = "macos"))]
use keyring::Entry;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

const KEYCHAIN_SERVICE: &str = "com.codexselect.desktop.master-key";
#[cfg(target_os = "macos")]
const PROTECTED_KEYCHAIN_SERVICE: &str = "com.codexselect.desktop.master-key.v2";
const KEYCHAIN_ACCOUNT: &str = "default-installation";
const NONCE_SIZE: usize = 12;

#[derive(Debug, Error)]
pub enum VaultError {
    #[cfg(target_os = "macos")]
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring_core::Error),
    #[cfg(not(target_os = "macos"))]
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
    #[error("randomness error: {0}")]
    Random(#[from] getrandom::Error),
    #[error("invalid keychain master key")]
    InvalidMasterKey,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid encrypted secret metadata")]
    InvalidMetadata,
    #[error("legacy keychain migration failed: {0}")]
    LegacyKeychain(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedSecret {
    pub credential_version: u32,
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

pub struct SecretVault {
    master_key: Zeroizing<[u8; 32]>,
    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    keychain_entry: keyring_core::Entry,
    #[cfg(not(target_os = "macos"))]
    #[allow(dead_code)]
    keychain_entry: keyring::Entry,
    #[allow(dead_code)]
    data_dir: std::path::PathBuf,
}

impl SecretVault {
    pub fn load_or_create(data_dir: &Path) -> Result<Self, VaultError> {
        let master_key = load_master_key()?;
        Ok(Self {
            master_key: Zeroizing::new(master_key),
            keychain_entry: keychain_entry_for_current_platform()?,
            data_dir: data_dir.to_path_buf(),
        })
    }

    pub fn encrypt(
        &self,
        credential_id: &str,
        version: u32,
        plaintext: &[u8],
    ) -> Result<EncryptedSecret, VaultError> {
        let cipher = Aes256Gcm::new_from_slice(&self.master_key[..])
            .map_err(|_| VaultError::InvalidMasterKey)?;
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        random_fill(&mut nonce_bytes)?;
        let aad = associated_data(credential_id, version);
        let nonce =
            Nonce::try_from(nonce_bytes.as_slice()).map_err(|_| VaultError::InvalidMetadata)?;
        let ciphertext = cipher
            .encrypt(
                &nonce,
                aes_gcm::aead::Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::Encrypt)?;
        Ok(EncryptedSecret {
            credential_version: version,
            nonce: nonce_bytes.to_vec(),
            ciphertext,
        })
    }

    pub fn decrypt(
        &self,
        credential_id: &str,
        encrypted: &EncryptedSecret,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        if encrypted.nonce.len() != NONCE_SIZE {
            return Err(VaultError::InvalidMetadata);
        }
        let cipher = Aes256Gcm::new_from_slice(&self.master_key[..])
            .map_err(|_| VaultError::InvalidMasterKey)?;
        let aad = associated_data(credential_id, encrypted.credential_version);
        let nonce =
            Nonce::try_from(encrypted.nonce.as_slice()).map_err(|_| VaultError::InvalidMetadata)?;
        let plaintext = cipher
            .decrypt(
                &nonce,
                aes_gcm::aead::Payload {
                    msg: &encrypted.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| VaultError::Decrypt)?;
        Ok(Zeroizing::new(plaintext))
    }
}

fn load_master_key() -> Result<[u8; 32], VaultError> {
    #[cfg(target_os = "macos")]
    {
        let protected_entry = protected_entry(PROTECTED_KEYCHAIN_SERVICE)?;
        match protected_entry.get_password() {
            Ok(encoded) => decode_master_key(&encoded),
            Err(keyring_core::Error::NoEntry) => {
                // Migrate the existing legacy item once. After this, all launches use the
                // protected after-first-unlock item and do not re-open the legacy prompt path.
                let legacy_entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)?;
                let encoded = match legacy_entry.get_password() {
                    Ok(encoded) => encoded,
                    Err(keyring::Error::NoEntry) => return generate_and_store(&protected_entry),
                    Err(error) => return Err(VaultError::LegacyKeychain(error.to_string())),
                };
                let key = decode_master_key(&encoded)?;
                protected_entry.set_password(&encoded)?;
                Ok(key)
            }
            Err(error) => Err(VaultError::Keychain(error)),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)?;
        match entry.get_password() {
            Ok(encoded) => decode_master_key(&encoded),
            Err(keyring::Error::NoEntry) => generate_and_store(&entry),
            Err(error) => Err(VaultError::Keychain(error)),
        }
    }
}

fn decode_master_key(encoded: &str) -> Result<[u8; 32], VaultError> {
    let bytes = hex::decode(encoded).map_err(|_| VaultError::InvalidMasterKey)?;
    if bytes.len() != 32 {
        return Err(VaultError::InvalidMasterKey);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(target_os = "macos")]
fn generate_and_store(entry: &keyring_core::Entry) -> Result<[u8; 32], VaultError> {
    let mut key = [0u8; 32];
    random_fill(&mut key)?;
    entry.set_password(&hex::encode(key))?;
    Ok(key)
}

#[cfg(not(target_os = "macos"))]
fn generate_and_store(entry: &keyring::Entry) -> Result<[u8; 32], VaultError> {
    let mut key = [0u8; 32];
    random_fill(&mut key)?;
    entry.set_password(&hex::encode(key))?;
    Ok(key)
}

#[cfg(target_os = "macos")]
fn protected_entry(service: &str) -> Result<keyring_core::Entry, VaultError> {
    let store = protected::Store::new()?;
    keyring_core::set_default_store(store);
    let modifiers = HashMap::from([("access-policy", "after-first-unlock")]);
    Ok(keyring_core::Entry::new_with_modifiers(
        service,
        KEYCHAIN_ACCOUNT,
        &modifiers,
    )?)
}

#[cfg(target_os = "macos")]
fn keychain_entry_for_current_platform() -> Result<keyring_core::Entry, VaultError> {
    protected_entry(PROTECTED_KEYCHAIN_SERVICE)
}

#[cfg(not(target_os = "macos"))]
fn keychain_entry_for_current_platform() -> Result<keyring::Entry, VaultError> {
    Ok(keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT)?)
}

impl Drop for SecretVault {
    fn drop(&mut self) {
        self.master_key.zeroize();
    }
}

fn associated_data(credential_id: &str, version: u32) -> Vec<u8> {
    format!("codex-select\0{credential_id}\0{version}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_secret_binds_credential_and_version() {
        // The keychain-backed constructor is intentionally not used in unit tests;
        // this verifies the serialized envelope shape and metadata contract.
        let secret = EncryptedSecret {
            credential_version: 3,
            nonce: vec![0; 12],
            ciphertext: vec![1, 2, 3],
        };
        let json = serde_json::to_string(&secret).expect("secret serializes");
        let decoded: EncryptedSecret = serde_json::from_str(&json).expect("secret deserializes");
        assert_eq!(secret, decoded);
        assert_ne!(associated_data("a", 1), associated_data("b", 1));
        assert_ne!(associated_data("a", 1), associated_data("a", 2));
    }
}

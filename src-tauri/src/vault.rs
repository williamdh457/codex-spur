use std::{fs, path::Path};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use getrandom::fill as random_fill;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::{Zeroize, Zeroizing};

/// Local-only master key (mode 0600). Never stored in macOS Keychain.
///
/// Keychain was previously used, but unsigned/dev binaries get a new code identity
/// on every rebuild, so macOS re-prompts for the login password on every launch.
/// Local file avoids that completely.
const MASTER_KEY_FILE: &str = "master_key.hex";
const NONCE_SIZE: usize = 12;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("randomness error: {0}")]
    Random(#[from] getrandom::Error),
    #[error("invalid master key")]
    InvalidMasterKey,
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed")]
    Decrypt,
    #[error("invalid encrypted secret metadata")]
    InvalidMetadata,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
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
    #[allow(dead_code)]
    data_dir: std::path::PathBuf,
}

impl SecretVault {
    /// Load or create the installation master key from a local 0600 file only.
    ///
    /// This path never touches macOS Keychain, so opening the app never asks
    /// for the login keychain password.
    pub fn load_or_create(data_dir: &Path) -> Result<Self, VaultError> {
        fs::create_dir_all(data_dir)?;
        let key_path = data_dir.join(MASTER_KEY_FILE);

        let master_key = if let Some(key) = read_key_file(&key_path)? {
            key
        } else {
            let mut key = [0u8; 32];
            random_fill(&mut key)?;
            write_key_file(&key_path, &hex::encode(key))?;
            key
        };

        Ok(Self {
            master_key: Zeroizing::new(master_key),
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

impl Drop for SecretVault {
    fn drop(&mut self) {
        self.master_key.zeroize();
    }
}

fn decode_master_key(encoded: &str) -> Result<[u8; 32], VaultError> {
    let bytes = hex::decode(encoded.trim()).map_err(|_| VaultError::InvalidMasterKey)?;
    if bytes.len() != 32 {
        return Err(VaultError::InvalidMasterKey);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn read_key_file(path: &Path) -> Result<Option<[u8; 32]>, VaultError> {
    if !path.exists() {
        return Ok(None);
    }
    let encoded = fs::read_to_string(path)?;
    Ok(Some(decode_master_key(&encoded)?))
}

fn write_key_file(path: &Path, encoded: &str) -> Result<(), VaultError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{}\n", encoded.trim()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn associated_data(credential_id: &str, version: u32) -> Vec<u8> {
    format!("codex-select\0{credential_id}\0{version}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "codex-spur-vault-{}-{}-{}",
            label,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    }

    #[test]
    fn encrypted_secret_binds_credential_and_version() {
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

    #[test]
    fn load_from_existing_local_key_roundtrips() {
        let dir = temp_dir("existing");
        fs::create_dir_all(&dir).expect("temp dir");
        let encoded = "aa".repeat(32);
        write_key_file(&dir.join(MASTER_KEY_FILE), &encoded).expect("write");

        let vault = SecretVault::load_or_create(&dir).expect("load");
        let encrypted = vault
            .encrypt("cred-1", 1, b"secret-payload")
            .expect("encrypt");
        let plain = vault.decrypt("cred-1", &encrypted).expect("decrypt");
        assert_eq!(plain.as_slice(), b"secret-payload");

        let key = read_key_file(&dir.join(MASTER_KEY_FILE))
            .expect("read")
            .expect("some");
        assert_eq!(hex::encode(key), encoded);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creates_local_key_when_none_exists() {
        let dir = temp_dir("new");
        let vault = SecretVault::load_or_create(&dir).expect("create");
        assert!(dir.join(MASTER_KEY_FILE).exists());
        let encrypted = vault.encrypt("c", 1, b"x").expect("encrypt");
        let plain = vault.decrypt("c", &encrypted).expect("decrypt");
        assert_eq!(plain.as_slice(), b"x");

        // Second load must reuse the same key without any external store.
        let vault2 = SecretVault::load_or_create(&dir).expect("reload");
        let plain2 = vault2.decrypt("c", &encrypted).expect("decrypt again");
        assert_eq!(plain2.as_slice(), b"x");
        let _ = fs::remove_dir_all(&dir);
    }
}

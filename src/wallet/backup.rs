//! WASM backup: encrypt/decrypt wallet state to/from bytes.

use amplify::s;
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Aead};
use generic_array::GenericArray;
use scrypt::{
    Scrypt,
    password_hash::{PasswordHasher, Salt, SaltString, rand_core::OsRng},
};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::error::InternalError;

pub(crate) const BACKUP_VERSION: u8 = 1;
/// 24-byte nonce for XChaCha20Poly1305 (single-shot mode).
const NONCE_LEN: usize = 24;

/// Public (unencrypted) metadata stored alongside the encrypted payload.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct BackupPubData {
    pub(crate) salt: String,
    pub(crate) nonce: String, // hex-encoded 24-byte nonce
    pub(crate) version: u8,
}

/// The serializable wallet state that gets encrypted.
#[derive(Serialize, Deserialize)]
pub(crate) struct WalletBackupPayload {
    pub(crate) db: crate::database::memory_db::InMemoryDb,
    pub(crate) bdk_changeset: Option<bdk_wallet::ChangeSet>,
    /// Strict-encoded MemStash, base64
    pub(crate) stock_stash_b64: String,
    /// Strict-encoded MemState, base64
    pub(crate) stock_state_b64: String,
    /// Strict-encoded MemIndex, base64
    pub(crate) stock_index_b64: String,
}

/// Derive a 32-byte key from password + salt using Scrypt.
pub(crate) fn derive_key(
    password: &str,
    salt_b64: &str,
) -> Result<GenericArray<u8, typenum::U32>, Error> {
    let salt = Salt::from_b64(salt_b64).map_err(InternalError::from)?;
    let password_hash = Scrypt
        .hash_password_customized(
            password.as_bytes(),
            None,
            None,
            scrypt::Params::default(),
            salt,
        )
        .map_err(InternalError::from)?;
    let hash_output = password_hash
        .hash
        .ok_or(InternalError::NoPasswordHashError)?;
    Ok(*GenericArray::from_slice(hash_output.as_bytes()))
}

/// Encrypt plaintext bytes using XChaCha20Poly1305.
pub(crate) fn encrypt_payload(
    plaintext: &[u8],
    password: &str,
) -> Result<(Vec<u8>, BackupPubData), Error> {
    let salt = SaltString::generate(&mut OsRng);
    let key = derive_key(password, salt.as_str())?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| Error::Internal {
        details: format!("RNG unavailable: {e}"),
    })?;
    let nonce = GenericArray::from_slice(&nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&key);
    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| InternalError::AeadError(e.to_string()))?;

    let pub_data = BackupPubData {
        salt: salt.to_string(),
        nonce: hex::encode(nonce_bytes),
        version: BACKUP_VERSION,
    };
    Ok((ciphertext, pub_data))
}

/// Decrypt ciphertext bytes using XChaCha20Poly1305.
pub(crate) fn decrypt_payload(
    ciphertext: &[u8],
    password: &str,
    pub_data: &BackupPubData,
) -> Result<Vec<u8>, Error> {
    if pub_data.version != BACKUP_VERSION {
        return Err(Error::UnsupportedBackupVersion {
            version: pub_data.version.to_string(),
        });
    }
    let key = derive_key(password, &pub_data.salt)?;
    let nonce_bytes =
        hex::decode(&pub_data.nonce).map_err(|e| InternalError::HexError(e.to_string()))?;
    let nonce = GenericArray::from_slice(&nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&key);
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| Error::WrongPassword)
}

/// Encode: `[4-byte LE pub_data_len][pub_data JSON][encrypted payload]`
pub(crate) fn encode_backup(ciphertext: &[u8], pub_data: &BackupPubData) -> Result<Vec<u8>, Error> {
    let pub_json = serde_json::to_vec(pub_data).map_err(InternalError::from)?;
    let pub_len = (pub_json.len() as u32).to_le_bytes();
    let mut out = Vec::with_capacity(4 + pub_json.len() + ciphertext.len());
    out.extend_from_slice(&pub_len);
    out.extend_from_slice(&pub_json);
    out.extend_from_slice(ciphertext);
    Ok(out)
}

/// Decode: split backup bytes into (BackupPubData, ciphertext).
pub(crate) fn decode_backup(data: &[u8]) -> Result<(BackupPubData, &[u8]), Error> {
    if data.len() < 4 {
        return Err(Error::InvalidBackup);
    }
    let pub_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if data.len() < 4 + pub_len {
        return Err(Error::InvalidBackup);
    }
    let pub_data: BackupPubData =
        serde_json::from_slice(&data[4..4 + pub_len]).map_err(|_| Error::InvalidBackup)?;
    let ciphertext = &data[4 + pub_len..];
    Ok((pub_data, ciphertext))
}

use rgbstd::persistence::{MemIndex, MemStash, MemState};
use strict_encoding::{StrictDeserialize, StrictSerialize};

impl super::Wallet {
    /// Create an encrypted backup of the wallet state.
    /// Returns the backup as bytes (password-protected).
    pub fn backup(&self, password: &str) -> Result<Vec<u8>, Error> {
        let payload_json = self.serialize_backup_payload()?;

        let (ciphertext, pub_data) = encrypt_payload(&payload_json, password)?;
        let backup_bytes = encode_backup(&ciphertext, &pub_data)?;

        self.update_backup_info(true)?;

        Ok(backup_bytes)
    }

    /// Restore wallet state from an encrypted backup.
    /// The wallet must be created first via `Wallet::new()` with the same mnemonic/xpubs.
    pub fn restore_backup(&mut self, backup_bytes: &[u8], password: &str) -> Result<(), Error> {
        // 1. Decode envelope
        let (pub_data, ciphertext) = decode_backup(backup_bytes)?;

        // 2. Decrypt
        let payload_json = decrypt_payload(ciphertext, password, &pub_data)?;

        // 3. Deserialize payload
        let payload: WalletBackupPayload =
            serde_json::from_slice(&payload_json).map_err(|_| Error::InvalidBackup)?;

        // 4. Restore InMemoryDb + BDK via existing snapshot mechanism
        let snapshot = super::idb_store::WalletSnapshot {
            db: payload.db,
            bdk_changeset: payload.bdk_changeset,
            signed_psbts: Default::default(),
            received_consignments: Default::default(),
            stock_stash_b64: None,
            stock_state_b64: None,
            stock_index_b64: None,
            reuse_address_index: Default::default(),
        };
        self.restore_from_snapshot(snapshot)?;

        // 5. Restore RGB stock from strict-encoded components
        let stash_bytes = general_purpose::STANDARD
            .decode(&payload.stock_stash_b64)
            .map_err(InternalError::from)?;
        let state_bytes = general_purpose::STANDARD
            .decode(&payload.stock_state_b64)
            .map_err(InternalError::from)?;
        let index_bytes = general_purpose::STANDARD
            .decode(&payload.stock_index_b64)
            .map_err(InternalError::from)?;

        const MAX: usize = u32::MAX as usize;
        let stash = MemStash::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(stash_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;
        let state = MemState::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(state_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;
        let index = MemIndex::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(index_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;

        let stock = rgbstd::persistence::Stock::with(stash, state, index);
        *self.rgb_stock.borrow_mut() = Some(stock);

        // Persist restored state to IndexedDB so it survives page reloads
        self.trigger_auto_backup();

        Ok(())
    }

    /// Returns true if the wallet has been modified since the last backup.
    pub fn backup_info(&self) -> Result<bool, Error> {
        let info = self.database.get_backup_info()?;
        match info {
            None => Ok(true), // never backed up
            Some(info) => {
                if info.last_backup_timestamp.is_empty() {
                    Ok(true)
                } else {
                    Ok(info.last_operation_timestamp > info.last_backup_timestamp)
                }
            }
        }
    }
}

use super::vss::{VssBackupClient, VssBackupConfig, VssBackupInfo};

impl super::Wallet {
    /// Configure VSS backup for this wallet.
    pub fn configure_vss_backup(&mut self, config: &VssBackupConfig) {
        self.vss_client = Some(VssBackupClient::new(config));
    }

    /// Disable VSS backup.
    pub fn disable_vss_backup(&mut self) {
        self.vss_client = None;
    }

    /// Upload an encrypted backup to the configured VSS server.
    /// The wallet must have a VSS client configured via configure_vss_backup().
    pub async fn vss_backup(&self) -> Result<i64, Error> {
        let client = self.vss_client.as_ref().ok_or(Error::Internal {
            details: s!("VSS backup not configured"),
        })?;

        let payload_json = self.serialize_backup_payload()?;
        let fingerprint = self.wallet_data.master_fingerprint.clone();

        self.update_backup_info(true)?;
        match client.upload_backup(&payload_json, &fingerprint).await {
            Ok(version) => Ok(version),
            Err(e) => {
                let _ = self.update_backup_info(false);
                Err(e)
            }
        }
    }

    /// Download and restore wallet state from VSS server.
    pub async fn vss_restore_backup(&mut self) -> Result<(), Error> {
        let client = self.vss_client.as_ref().ok_or(Error::Internal {
            details: s!("VSS backup not configured"),
        })?;

        let payload_json = client.download_backup().await?;
        let payload: WalletBackupPayload =
            serde_json::from_slice(&payload_json).map_err(|_| Error::InvalidBackup)?;

        let snapshot = super::idb_store::WalletSnapshot {
            db: payload.db,
            bdk_changeset: payload.bdk_changeset,
            signed_psbts: Default::default(),
            received_consignments: Default::default(),
            stock_stash_b64: None,
            stock_state_b64: None,
            stock_index_b64: None,
            reuse_address_index: Default::default(),
        };
        self.restore_from_snapshot(snapshot)?;

        const MAX: usize = u32::MAX as usize;
        let stash_bytes = general_purpose::STANDARD
            .decode(&payload.stock_stash_b64)
            .map_err(InternalError::from)?;
        let state_bytes = general_purpose::STANDARD
            .decode(&payload.stock_state_b64)
            .map_err(InternalError::from)?;
        let index_bytes = general_purpose::STANDARD
            .decode(&payload.stock_index_b64)
            .map_err(InternalError::from)?;

        let stash = MemStash::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(stash_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;
        let state = MemState::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(state_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;
        let index = MemIndex::from_strict_serialized::<MAX>(
            amplify::confinement::Confined::<Vec<u8>, 0, MAX>::try_from(index_bytes)
                .map_err(|e| InternalError::StockError(e.to_string()))?,
        )
        .map_err(|e| InternalError::StockError(e.to_string()))?;

        let stock = rgbstd::persistence::Stock::with(stash, state, index);
        *self.rgb_stock.borrow_mut() = Some(stock);

        // Persist restored state to IndexedDB so it survives page reloads
        self.trigger_auto_backup();

        Ok(())
    }

    /// Query VSS backup status.
    pub async fn vss_backup_info(&self) -> Result<VssBackupInfo, Error> {
        let client = self.vss_client.as_ref().ok_or(Error::Internal {
            details: s!("VSS backup not configured"),
        })?;
        let server_version = client.get_backup_version().await?;
        let backup_required = self.backup_info()?;

        Ok(VssBackupInfo {
            backup_exists: server_version.is_some(),
            server_version,
            backup_required,
        })
    }

    /// Serialize the wallet state to JSON bytes (shared by local backup and VSS).
    fn serialize_backup_payload(&self) -> Result<Vec<u8>, Error> {
        let stock_ref = self.rgb_stock.borrow();
        let stock = stock_ref.as_ref().ok_or_else(|| Error::Internal {
            details: s!("RGB stock is currently in use (runtime active)"),
        })?;

        let stash_bytes = stock
            .as_stash_provider()
            .to_strict_serialized::<{ u32::MAX as usize }>()
            .map_err(|e| InternalError::StockError(e.to_string()))?;
        let state_bytes = stock
            .as_state_provider()
            .to_strict_serialized::<{ u32::MAX as usize }>()
            .map_err(|e| InternalError::StockError(e.to_string()))?;
        let index_bytes = stock
            .as_index_provider()
            .to_strict_serialized::<{ u32::MAX as usize }>()
            .map_err(|e| InternalError::StockError(e.to_string()))?;

        drop(stock_ref);

        let payload = WalletBackupPayload {
            db: self.database.as_ref().clone(),
            bdk_changeset: self.bdk_database.get_data().clone(),
            stock_stash_b64: general_purpose::STANDARD.encode(stash_bytes.as_unconfined()),
            stock_state_b64: general_purpose::STANDARD.encode(state_bytes.as_unconfined()),
            stock_index_b64: general_purpose::STANDARD.encode(index_bytes.as_unconfined()),
        };

        serde_json::to_vec(&payload).map_err(|e| InternalError::from(e).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypt_decrypt_round_trip() {
        let plaintext = b"hello, RGB wallet backup!";
        let password = "strong_password_42";

        let (ciphertext, pub_data) = encrypt_payload(plaintext, password).unwrap();
        assert_ne!(ciphertext, plaintext);
        assert_eq!(pub_data.version, BACKUP_VERSION);

        let decrypted = decrypt_payload(&ciphertext, password, &pub_data).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn wrong_password_fails() {
        let plaintext = b"secret data";
        let (ciphertext, pub_data) = encrypt_payload(plaintext, "correct").unwrap();

        let result = decrypt_payload(&ciphertext, "wrong", &pub_data);
        assert!(matches!(result, Err(Error::WrongPassword)));
    }

    #[test]
    fn unsupported_version_fails() {
        let plaintext = b"data";
        let (ciphertext, mut pub_data) = encrypt_payload(plaintext, "pw").unwrap();
        pub_data.version = 99;

        let result = decrypt_payload(&ciphertext, "pw", &pub_data);
        assert!(matches!(
            result,
            Err(Error::UnsupportedBackupVersion { .. })
        ));
    }

    #[test]
    fn encode_decode_round_trip() {
        let ciphertext = b"encrypted_blob_here";
        let pub_data = BackupPubData {
            salt: "dGVzdHNhbHQ".to_string(),
            nonce: hex::encode([0xABu8; NONCE_LEN]),
            version: BACKUP_VERSION,
        };

        let encoded = encode_backup(ciphertext, &pub_data).unwrap();
        let (decoded_pub, decoded_ct) = decode_backup(&encoded).unwrap();

        assert_eq!(decoded_ct, ciphertext);
        assert_eq!(decoded_pub.salt, pub_data.salt);
        assert_eq!(decoded_pub.nonce, pub_data.nonce);
        assert_eq!(decoded_pub.version, BACKUP_VERSION);
    }

    #[test]
    fn decode_too_short_fails() {
        assert!(matches!(decode_backup(&[0, 1]), Err(Error::InvalidBackup)));
        assert!(matches!(decode_backup(&[]), Err(Error::InvalidBackup)));
    }

    #[test]
    fn decode_truncated_pub_data_fails() {
        // pub_len says 100 but only 5 bytes follow
        let data = [100u8, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(decode_backup(&data), Err(Error::InvalidBackup)));
    }

    #[test]
    fn derive_key_deterministic() {
        let salt = SaltString::generate(&mut OsRng);
        let k1 = derive_key("password", salt.as_str()).unwrap();
        let k2 = derive_key("password", salt.as_str()).unwrap();
        assert_eq!(k1, k2);

        let k3 = derive_key("different", salt.as_str()).unwrap();
        assert_ne!(k1, k3);
    }
}

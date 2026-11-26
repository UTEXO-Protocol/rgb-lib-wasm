//! WASM backup: encrypt/decrypt wallet state to/from bytes.

use amplify::s;
use base64::{Engine as _, engine::general_purpose};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
use scrypt::{Params, phc::Salt, scrypt};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::error::InternalError;

pub(crate) const BACKUP_VERSION: u8 = 1;
/// 24-byte nonce for XChaCha20Poly1305 (single-shot mode).
const NONCE_LEN: usize = 24;

pub(crate) fn parse_backup_timestamp(timestamp: &str) -> i128 {
    timestamp.parse().expect("backup timestamp is an integer")
}

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
    /// Encoded RGB store, base64
    pub(crate) rgb_store: String,
    /// Pinned derivation index per keychain for address reuse.
    #[serde(default)]
    pub(crate) reuse_address_index: std::collections::HashMap<bdk_wallet::KeychainKind, u32>,
    /// Artifacts keyed by TXID. A broadcast send keeps only its consignments.
    #[serde(default)]
    pub(crate) transfer_artifacts:
        std::collections::HashMap<String, super::online::TransferArtifacts>,
    pub(crate) received_consignments: std::collections::HashMap<String, Vec<u8>>,
    pub(crate) received_consignment_meta:
        std::collections::HashMap<String, super::ReceivedConsignmentMeta>,
    pub(crate) received_signed_txs: std::collections::HashMap<String, Vec<u8>>,
    /// Received mints, keyed by receive dir of the transfer.
    pub(crate) received_mints: std::collections::HashMap<String, super::ReceivedMint>,
    /// Mint locks a received consignment waits for, keyed by receive dir of the transfer.
    pub(crate) pending_mint_locks: std::collections::HashMap<String, super::PendingMintLocks>,
    /// Media bytes keyed by hex digest.
    pub(crate) media: std::collections::HashMap<String, Vec<u8>>,
    /// Settings and public keys of the wallet.
    pub(crate) manifest: super::WalletManifest,
}

/// Derive a 32-byte key from password + salt using Scrypt.
pub(crate) fn derive_key(password: &str, salt_b64: &str) -> Result<Key, Error> {
    let salt = Salt::from_b64(salt_b64).map_err(InternalError::from)?;
    let mut key = [0u8; 32];
    scrypt(
        password.as_bytes(),
        salt.as_ref(),
        &Params::RECOMMENDED,
        &mut key,
    )
    .map_err(|e| Error::Internal {
        details: e.to_string(),
    })?;
    Ok(key.into())
}

/// Generate a random base64 salt.
fn generate_salt() -> Result<String, Error> {
    let mut bytes = [0u8; Salt::RECOMMENDED_LENGTH];
    getrandom::getrandom(&mut bytes).map_err(|e| Error::Internal {
        details: format!("RNG unavailable: {e}"),
    })?;
    Ok(Salt::new(&bytes).map_err(InternalError::from)?.to_string())
}

/// Encrypt plaintext bytes using XChaCha20Poly1305.
pub(crate) fn encrypt_payload(
    plaintext: &[u8],
    password: &str,
) -> Result<(Vec<u8>, BackupPubData), Error> {
    let salt = generate_salt()?;
    let key = derive_key(password, &salt)?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    getrandom::getrandom(&mut nonce_bytes).map_err(|e| Error::Internal {
        details: format!("RNG unavailable: {e}"),
    })?;
    let nonce = XNonce::from(nonce_bytes);

    let cipher = XChaCha20Poly1305::new(&key);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| InternalError::AeadError(e.to_string()))?;

    let pub_data = BackupPubData {
        salt,
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
    let nonce = XNonce::try_from(nonce_bytes.as_slice()).map_err(|_| Error::InvalidBackup)?;

    let cipher = XChaCha20Poly1305::new(&key);
    cipher
        .decrypt(&nonce, ciphertext)
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
    let (len, rest) = data.split_first_chunk::<4>().ok_or(Error::InvalidBackup)?;
    let pub_len = u32::from_le_bytes(*len) as usize;
    let (pub_json, ciphertext) = rest.split_at_checked(pub_len).ok_or(Error::InvalidBackup)?;
    let pub_data: BackupPubData =
        serde_json::from_slice(pub_json).map_err(|_| Error::InvalidBackup)?;
    Ok((pub_data, ciphertext))
}

use strict_encoding::{StrictDeserialize, StrictSerialize};

impl super::Wallet {
    /// Create an encrypted backup of the wallet state.
    /// Returns the backup as bytes (password-protected).
    pub fn backup(&self, password: &str) -> Result<Vec<u8>, Error> {
        let payload_json = self.serialize_backup_payload()?;

        let (ciphertext, pub_data) = encrypt_payload(&payload_json, password)?;
        let backup_bytes = encode_backup(&ciphertext, &pub_data)?;

        self.update_backup_info(true)?;
        // A failed save does not stop the export. The next call returns the save error.
        let _ = self.queue_save();

        Ok(backup_bytes)
    }

    /// Restore wallet state from an encrypted backup.
    /// The wallet must be created first with the same keys, witness version and vanilla keychain; other keys give `WalletSettingMismatch`. The wallet keeps its own mutable settings.
    /// The restored state persists only if this wallet may save: it was built with `Wallet::restore()` or `Wallet::load()`, or its data directory holds no snapshot.
    /// A restore that fails changes nothing. When only the save fails, the restored state stays in memory and the call returns a persistence error.
    /// A backup of another network gives [`Error::BackupNetworkMismatch`].
    pub fn restore_backup(&mut self, backup_bytes: &[u8], password: &str) -> Result<(), Error> {
        let (pub_data, ciphertext) = decode_backup(backup_bytes)?;
        let payload_json = decrypt_payload(ciphertext, password, &pub_data)?;
        self.restore_payload(&payload_json)?;

        // Persist restored state to IndexedDB so it survives page reloads
        self.queue_save()?;
        super::idb_store::request_persistent_storage();

        Ok(())
    }

    /// Restore the wallet and the RGB stock from a backup payload. Both restore paths use it.
    fn restore_payload(&mut self, payload_json: &[u8]) -> Result<(), Error> {
        let payload: WalletBackupPayload =
            serde_json::from_slice(payload_json).map_err(|_| Error::InvalidBackup)?;
        if payload.manifest.bitcoin_network != self.wallet_data.bitcoin_network {
            return Err(Error::BackupNetworkMismatch {
                backup_network: payload.manifest.bitcoin_network,
                wallet_network: self.wallet_data.bitcoin_network,
            });
        }
        // Without a BDK changeset the restore skips the descriptor check.
        if payload.bdk_changeset.is_none() && self.bdk_database.get_data().is_some() {
            return Err(Error::InvalidBackup);
        }

        let snapshot = super::idb_store::WalletSnapshot {
            writer: 0,
            sequence: 0,
            db: payload.db,
            bdk_changeset: payload.bdk_changeset,
            transfer_artifacts: payload.transfer_artifacts,
            received_consignments: payload.received_consignments,
            received_consignment_meta: payload.received_consignment_meta,
            received_signed_txs: payload.received_signed_txs,
            received_mints: payload.received_mints,
            pending_mint_locks: payload.pending_mint_locks,
            media: payload.media,
            rgb_store: payload.rgb_store,
            reuse_address_index: payload.reuse_address_index,
            manifest: payload.manifest,
            backup_versions: self.backup_versions.borrow().clone(),
        };
        self.restore_from_snapshot(snapshot)
    }

    /// Returns true if the wallet has been modified since the last backup.
    pub fn backup_info(&self) -> Result<bool, Error> {
        Ok(self.database.get_backup_info()?.is_some_and(|info| {
            parse_backup_timestamp(&info.last_operation_timestamp)
                > parse_backup_timestamp(&info.last_backup_timestamp)
        }))
    }
}

use std::rc::Rc;

use bdk_wallet::bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};

use super::backup_store::{
    BACKUP_NAMESPACE, BackupTarget, KvStore, backup_key, check_new_version, decode_backup_blob,
    encode_backup_blob, store_label,
};
use super::vss::{VssBackupConfig, VssBackupInfo, VssStore};

impl super::Wallet {
    fn set_backup_target(&mut self, store: Box<dyn KvStore>, id: String, key: SecretKey) {
        self.backup_store = Some(Rc::new(BackupTarget {
            store,
            id,
            key,
            lock: tokio::sync::Mutex::new(()),
        }));
    }

    /// Configure VSS backup for this wallet.
    pub fn configure_vss_backup(&mut self, config: &VssBackupConfig) {
        let pubkey = PublicKey::from_secret_key(&Secp256k1::signing_only(), &config.signing_key);
        let id = store_label(
            "vss",
            &[&config.server_url, &config.store_id, &pubkey.to_string()],
        );
        self.set_backup_target(Box::new(VssStore::new(config)), id, config.signing_key);
    }

    /// Configure an app backup store. `store_id` names the store on this device. The store must
    /// honour expected versions: the library relies on it to never overwrite a newer backup.
    pub fn configure_backup_store(
        &mut self,
        store: Box<dyn KvStore>,
        store_id: &str,
        encryption_key: SecretKey,
    ) {
        let id = store_label("app", &[store_id]);
        self.set_backup_target(store, id, encryption_key);
    }

    /// Disable the backup store.
    pub fn disable_vss_backup(&mut self) {
        self.backup_store = None;
    }

    fn store_key(&self) -> String {
        backup_key(
            &self.keys.master_fingerprint,
            self.wallet_data.bitcoin_network,
        )
    }

    fn backup_target(&self) -> Result<Rc<BackupTarget>, Error> {
        self.backup_store.clone().ok_or(Error::Internal {
            details: s!("VSS backup not configured"),
        })
    }

    /// Upload an encrypted backup to the configured backup store. Return the store version.
    /// A write over a version that this wallet did not see fails with a conflict.
    pub async fn vss_backup(&self) -> Result<i64, Error> {
        let target = self.backup_target()?;
        let _guard = target.lock.lock().await;

        let payload_json = self.serialize_backup_payload()?;
        let blob = encode_backup_blob(&payload_json, &self.keys.master_fingerprint, &target.key)?;

        self.update_backup_info(true)?;
        let expected = self.backup_versions.borrow().get(&target.id).copied();
        let key = self.store_key();
        let store = &target.store;
        let mut res = store
            .write(BACKUP_NAMESPACE, &key, blob.clone(), expected)
            .await
            .and_then(|v| check_new_version(v, expected));
        if matches!(res, Err(Error::VssVersionConflict { .. })) && expected.is_some() {
            // Create the key again only when the store lost it.
            match store.read(BACKUP_NAMESPACE, &key).await {
                Ok(None) => {
                    res = store
                        .write(BACKUP_NAMESPACE, &key, blob, None)
                        .await
                        .and_then(|v| check_new_version(v, None));
                }
                Ok(Some(_)) => {}
                Err(e) => res = Err(e),
            }
        }
        match &res {
            Ok(version) => {
                self.backup_versions
                    .borrow_mut()
                    .insert(target.id.clone(), *version);
                *self.last_vss_backup_error.borrow_mut() = None;
            }
            Err(e) => {
                let _ = self.update_backup_info(false);
                *self.last_vss_backup_error.borrow_mut() = Some(e.to_string());
            }
        }
        self.flush().await?;
        res
    }

    /// Upload a backup if the wallet has no backup record or has changes not yet backed up. Call after
    /// `configure_vss_backup` so enabling backups never leaves an empty store.
    /// Returns the new server version, or `None` when no upload was needed.
    pub async fn ensure_initial_backup(&self) -> Result<Option<i64>, Error> {
        self.backup_target()?;
        if self.database.get_backup_info()?.is_some() && !self.backup_info()? {
            return Ok(None);
        }
        self.vss_backup().await.map(Some)
    }

    /// Download and restore wallet state from the configured backup store.
    /// A restore that fails changes nothing. When only the save fails, the restored state stays in memory and the call returns a persistence error.
    /// A backup of another network gives [`Error::BackupNetworkMismatch`].
    pub async fn vss_restore_backup(&mut self) -> Result<(), Error> {
        let target = self.backup_target()?;
        let _guard = target.lock.lock().await;

        let key = self.store_key();
        let stored = target
            .store
            .read(BACKUP_NAMESPACE, &key)
            .await?
            .ok_or(Error::VssBackupNotFound)?;
        let payload_json =
            decode_backup_blob(&stored.value, &self.keys.master_fingerprint, &target.key)?;
        self.restore_payload(&payload_json)?;
        self.backup_versions
            .borrow_mut()
            .insert(target.id.clone(), stored.version);

        // Persist restored state to IndexedDB so it survives page reloads
        self.flush().await?;
        super::idb_store::request_persistent_storage();

        Ok(())
    }

    /// Query the backup status of the configured backup store.
    pub async fn vss_backup_info(&self) -> Result<VssBackupInfo, Error> {
        let target = self.backup_target()?;
        let _guard = target.lock.lock().await;
        let key = self.store_key();
        let server_version = target
            .store
            .read(BACKUP_NAMESPACE, &key)
            .await?
            .map(|r| r.version);
        let backup_required = self.backup_info()?;

        Ok(VssBackupInfo {
            backup_exists: server_version.is_some(),
            server_version,
            backup_required,
            last_backup_error: self.last_vss_backup_error.borrow().clone(),
        })
    }

    /// Serialize the wallet state to JSON bytes (shared by local backup and VSS).
    fn serialize_backup_payload(&self) -> Result<Vec<u8>, Error> {
        let stock_ref = self.rgb_stock.borrow();
        let stock = stock_ref.as_ref().ok_or_else(|| Error::Internal {
            details: s!("RGB stock is currently in use (runtime active)"),
        })?;

        let rgb_store =
            super::idb_store::serialize_stock(stock).ok_or_else(|| Error::Internal {
                details: s!("failed to serialize RGB stock"),
            })?;

        drop(stock_ref);

        let payload = WalletBackupPayload {
            db: self.database.as_ref().clone(),
            bdk_changeset: self.bdk_database.get_data().clone(),
            rgb_store,
            reuse_address_index: self.reuse_address_index.clone(),
            transfer_artifacts: self.transfer_artifacts.clone(),
            received_consignments: self.received_consignments.clone(),
            received_consignment_meta: self.received_consignment_meta.clone(),
            received_signed_txs: self.received_signed_txs.clone(),
            received_mints: self.received_mints.clone(),
            pending_mint_locks: self.pending_mint_locks.clone(),
            media: self.media.clone(),
            manifest: super::WalletManifest::new(&self.wallet_data, &self.keys),
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
    fn decrypt_wrong_nonce_length_fails() {
        let (ciphertext, mut pub_data) = encrypt_payload(b"data", "pw").unwrap();
        pub_data.nonce = s!("00");

        let result = decrypt_payload(&ciphertext, "pw", &pub_data);
        assert!(matches!(result, Err(Error::InvalidBackup)));
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
        let salt = generate_salt().unwrap();
        let k1 = derive_key("password", &salt).unwrap();
        let k2 = derive_key("password", &salt).unwrap();
        assert_eq!(k1, k2);

        let k3 = derive_key("different", &salt).unwrap();
        assert_ne!(k1, k3);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn backup_info_is_false_without_row() {
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;

        let wallet = make_test_wallet(false);
        assert!(wallet.database.get_backup_info().unwrap().is_none());
        assert!(!wallet.backup_info().unwrap());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn backup_info_compares_integer_timestamps() {
        use crate::database::memory_db::{ActiveValue, DbBackupInfoActMod};
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;

        let wallet = make_test_wallet(false);
        let info = wallet.update_backup_info(true).unwrap().unwrap();
        assert_eq!(info.last_operation_timestamp, "0");
        assert!(!wallet.backup_info().unwrap());

        wallet.update_backup_info(false).unwrap();
        assert!(wallet.backup_info().unwrap());

        // as text "10" < "9"
        wallet
            .database
            .update_backup_info(&mut DbBackupInfoActMod {
                last_backup_timestamp: ActiveValue::Set(s!("9")),
                last_operation_timestamp: ActiveValue::Set(s!("10")),
                ..Default::default()
            })
            .unwrap();
        assert!(wallet.backup_info().unwrap());
    }

    #[cfg(not(target_arch = "wasm32"))]
    // A sender proxy that has no ACK to serve.
    fn ack_proxy() -> (mockito::ServerGuard, mockito::Mock) {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(s!("ack.get")))
            .with_header("content-type", "application/json")
            .with_body(r#"{"jsonrpc":"2.0","id":null,"result":{"acks":{},"signatures":{}}}"#)
            .create();
        (server, mock)
    }

    #[cfg(not(target_arch = "wasm32"))]
    // Rows of one outgoing transfer after the ACK. `asset_id` is `None` for a sats-only send.
    fn seed_acknowledged_transfer(
        wallet: &mut crate::wallet::Wallet,
        txid: &str,
        asset_id: Option<&str>,
        sender_proxy: &str,
    ) -> i32 {
        use crate::database::enums::{AssetSchema, TransferStatus};
        use crate::database::memory_db::{
            ActiveValue, DbAssetActMod, DbAssetTransferActMod, DbBatchTransferActMod,
            DbTransferActMod,
        };

        let batch_idx = wallet
            .database
            .set_batch_transfer(DbBatchTransferActMod {
                txid: ActiveValue::Set(Some(txid.to_string())),
                status: ActiveValue::Set(TransferStatus::WaitingCounterparty),
                expiration: ActiveValue::Set(Some(i64::MAX)),
                min_confirmations: ActiveValue::Set(1),
                operation_secret_key: ActiveValue::Set(Some(vec![7; 32])),
                ..Default::default()
            })
            .unwrap();
        if let Some(id) = asset_id {
            wallet
                .database
                .set_asset(DbAssetActMod {
                    id: ActiveValue::Set(id.to_string()),
                    schema: ActiveValue::Set(AssetSchema::Nia),
                    added_at: ActiveValue::Set(1000),
                    name: ActiveValue::Set(s!("Test Asset")),
                    precision: ActiveValue::Set(8),
                    initial_supply: ActiveValue::Set(s!("1000")),
                    timestamp: ActiveValue::Set(1000),
                    ..Default::default()
                })
                .unwrap();
        }
        let asset_transfer_idx = wallet
            .database
            .set_asset_transfer(DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_idx),
                asset_id: ActiveValue::Set(asset_id.map(String::from)),
                ..Default::default()
            })
            .unwrap();
        let transfer_idx = wallet
            .database
            .set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                incoming: ActiveValue::Set(false),
                recipient_id: ActiveValue::Set(Some(s!("recipient"))),
                ack: ActiveValue::Set(Some(true)),
                proxy_transfer_id: ActiveValue::Set(Some(s!("transfer"))),
                ..Default::default()
            })
            .unwrap();
        wallet
            .save_sender_transport_endpoint(transfer_idx, sender_proxy)
            .unwrap();
        batch_idx
    }

    #[cfg(not(target_arch = "wasm32"))]
    // Rows and artifacts of one settled outgoing transfer without a recipient.
    fn seed_ended_without_recipient(
        wallet: &mut crate::wallet::Wallet,
        txid: &str,
        asset_id: &str,
        transition: crate::database::enums::TypeOfTransition,
        bytes: Vec<u8>,
    ) -> i32 {
        use crate::database::enums::{AssetSchema, TransferStatus};
        use crate::database::memory_db::{
            ActiveValue, DbAssetActMod, DbAssetTransferActMod, DbBatchTransferActMod,
            DbTransferActMod,
        };
        use crate::wallet::online::TransferArtifacts;

        let batch_idx = wallet
            .database
            .set_batch_transfer(DbBatchTransferActMod {
                txid: ActiveValue::Set(Some(txid.to_string())),
                status: ActiveValue::Set(TransferStatus::Settled),
                expiration: ActiveValue::Set(Some(i64::MAX)),
                min_confirmations: ActiveValue::Set(1),
                ..Default::default()
            })
            .unwrap();
        wallet
            .database
            .set_asset(DbAssetActMod {
                id: ActiveValue::Set(asset_id.to_string()),
                schema: ActiveValue::Set(AssetSchema::Ifa),
                added_at: ActiveValue::Set(1000),
                name: ActiveValue::Set(s!("Test Asset")),
                precision: ActiveValue::Set(8),
                initial_supply: ActiveValue::Set(s!("1000")),
                timestamp: ActiveValue::Set(1000),
                ..Default::default()
            })
            .unwrap();
        let asset_transfer_idx = wallet
            .database
            .set_asset_transfer(DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_idx),
                asset_id: ActiveValue::Set(Some(asset_id.to_string())),
                transition_type: ActiveValue::Set(Some(transition)),
                ..Default::default()
            })
            .unwrap();
        let idx = wallet
            .database
            .set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                incoming: ActiveValue::Set(false),
                ..Default::default()
            })
            .unwrap();
        wallet.transfer_artifacts.insert(
            txid.to_string(),
            TransferArtifacts {
                signed_tx: Some(vec![1]),
                consignment_bytes: crate::HashMap::from([(asset_id.to_string(), bytes)]),
                ..Default::default()
            },
        );
        idx
    }

    #[cfg(not(target_arch = "wasm32"))]
    // Rows of one incoming transfer, with `stored` as its received consignment.
    fn seed_receive(
        wallet: &mut crate::wallet::Wallet,
        rid: &str,
        status: crate::database::enums::TransferStatus,
        txid: Option<&str>,
        stored: Option<Vec<u8>>,
        hash_of: &[u8],
    ) -> i32 {
        use crate::database::enums::RecipientTypeFull;
        use crate::database::memory_db::{
            ActiveValue, DbAssetTransferActMod, DbBatchTransferActMod, DbTransferActMod,
        };
        use crate::encryption::sha256_digest;

        let batch_idx = wallet
            .database
            .set_batch_transfer(DbBatchTransferActMod {
                txid: ActiveValue::Set(txid.map(String::from)),
                status: ActiveValue::Set(status),
                expiration: ActiveValue::Set(Some(i64::MAX)),
                min_confirmations: ActiveValue::Set(1),
                ..Default::default()
            })
            .unwrap();
        let asset_transfer_idx = wallet
            .database
            .set_asset_transfer(DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_idx),
                asset_id: ActiveValue::Set(Some(s!("rgb:rcv"))),
                ..Default::default()
            })
            .unwrap();
        if let Some(bytes) = stored {
            wallet.received_consignments.insert(rid.to_string(), bytes);
        }
        wallet
            .database
            .set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                incoming: ActiveValue::Set(true),
                recipient_id: ActiveValue::Set(Some(rid.to_string())),
                recipient_type: ActiveValue::Set(Some(RecipientTypeFull::Witness { vout: None })),
                consignment_hash: ActiveValue::Set(Some(hex::encode(sha256_digest(hash_of)))),
                ..Default::default()
            })
            .unwrap()
    }

    // A transfer returns the consignment of its kind and status.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn get_consignment_by_kind() {
        use crate::database::enums::{TransferStatus::*, TypeOfTransition};
        use crate::database::memory_db::{ActiveValue, DbAssetTransferActMod};
        use crate::encryption::sha256_digest;
        use crate::keys::generate_keys;
        use crate::utils::MemStock;
        use crate::wallet::ReceivedMint;
        use crate::wallet::offline::PendingMintLocks;
        use crate::wallet::offline::address_reuse_tests::{
            make_test_wallet_with_keys, pending_mint_locks,
        };

        let keys = generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        );
        let mut wallet = make_test_wallet_with_keys(keys.clone(), false);
        let burn = seed_ended_without_recipient(
            &mut wallet,
            &"4".repeat(64),
            "rgb:burn",
            TypeOfTransition::Burn,
            vec![1, 2, 3],
        );
        let link = seed_ended_without_recipient(
            &mut wallet,
            &"6".repeat(64),
            "rgb:link",
            TypeOfTransition::Link,
            vec![7],
        );
        assert_eq!(wallet.get_consignment(burn), Ok(Some(vec![1, 2, 3])));
        assert_eq!(wallet.get_consignment(link), Ok(Some(vec![7])));
        assert_eq!(
            wallet.get_consignment(999),
            Err(Error::TransferNotFound { idx: 999 })
        );

        let h = |b: &[u8]| hex::encode(sha256_digest(b));
        let none = seed_receive(&mut wallet, "none", WaitingCounterparty, None, None, &[]);
        assert_eq!(wallet.get_consignment(none), Ok(None));
        let unvalidated = seed_receive(
            &mut wallet,
            "unvalidated",
            WaitingCounterparty,
            None,
            Some(vec![9]),
            &[9],
        );
        assert_eq!(wallet.get_consignment(unvalidated), Ok(None));
        // a transfer that listTransfers hides fails as an unknown one
        let hidden = wallet
            .database
            .get_db_data(false)
            .unwrap()
            .transfers
            .into_iter()
            .find(|t| t.idx == unvalidated)
            .unwrap()
            .asset_transfer_idx;
        wallet
            .database
            .update_asset_transfer(&mut DbAssetTransferActMod {
                idx: ActiveValue::Set(hidden),
                user_driven: ActiveValue::Set(false),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            wallet.get_consignment(unvalidated),
            Err(Error::TransferNotFound { idx: unvalidated })
        );
        let mint = seed_receive(
            &mut wallet,
            "mint",
            WaitingCounterparty,
            None,
            Some(vec![8]),
            &[8],
        );
        let lock = |digest| PendingMintLocks {
            consignment_digest: digest,
            ..pending_mint_locks()
        };
        wallet.pending_mint_locks.insert(s!("mint"), lock(h(&[8])));
        // a lock wait can be for the history only, before the mint check
        assert_eq!(wallet.get_consignment(mint), Ok(None));
        wallet.received_mints.insert(
            s!("mint"),
            ReceivedMint {
                opid: s!("opid"),
                lock_id: s!("lock"),
                amount: 1,
                before_block: 2,
                evm_chain_id: 3,
                bridge_contract: s!("bridge"),
            },
        );
        wallet.pending_mint_locks.insert(s!("mint"), lock(h(&[7])));
        assert_eq!(wallet.get_consignment(mint), Ok(None));
        wallet.pending_mint_locks.remove("mint");
        assert_eq!(wallet.get_consignment(mint), Ok(None));
        wallet.pending_mint_locks.insert(s!("mint"), lock(h(&[8])));
        assert_eq!(wallet.get_consignment(mint), Ok(Some(vec![8])));

        let settled = seed_receive(
            &mut wallet,
            "settled",
            Settled,
            Some(&"5".repeat(64)),
            Some(vec![5]),
            &[5],
        );
        assert_eq!(wallet.get_consignment(settled), Ok(Some(vec![5])));
        let mismatch = seed_receive(
            &mut wallet,
            "mismatch",
            WaitingConfirmations,
            Some(&"8".repeat(64)),
            Some(vec![2]),
            &[3],
        );
        assert_eq!(wallet.get_consignment(mismatch), Ok(None));

        *wallet.rgb_stock.borrow_mut() = Some(MemStock::default());
        let mut restored = make_test_wallet_with_keys(keys.clone(), false);
        restored
            .restore_payload(&wallet.serialize_backup_payload().unwrap())
            .unwrap();
        let mut from_snapshot = make_test_wallet_with_keys(keys, false);
        from_snapshot
            .restore_from_snapshot(wallet.snapshot().unwrap())
            .unwrap();
        for w in [&restored, &from_snapshot] {
            assert_eq!(w.get_consignment(burn), Ok(Some(vec![1, 2, 3])));
            assert_eq!(w.get_consignment(mint), Ok(Some(vec![8])));
            assert_eq!(w.get_consignment(settled), Ok(Some(vec![5])));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_after_restore_backup_reads_signed_tx_impl(asset_id: Option<&str>) {
        use crate::keys::generate_keys;
        use crate::utils::MemStock;
        use crate::wallet::Online;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet_with_keys;
        use crate::wallet::online::{OnlineData, TransferArtifacts};

        let txid = "1".repeat(64);
        let indexer_url = "http://127.0.0.1:1";
        let password = "pw";
        let keys = generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        );

        let mut sender = make_test_wallet_with_keys(keys.clone(), false);
        *sender.rgb_stock.borrow_mut() = Some(MemStock::default());
        let (proxy, _mock) = ack_proxy();
        let batch_idx = seed_acknowledged_transfer(&mut sender, &txid, asset_id, &proxy.url());
        // The stored TX is a sentinel: refresh rejects it only after it reads it back.
        sender.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_tx: Some(vec![0xff]),
                ..Default::default()
            },
        );

        // The payload, encrypted and encoded as `backup()` does.
        let payload = sender.serialize_backup_payload().unwrap();
        let (ciphertext, pub_data) = encrypt_payload(&payload, password).unwrap();
        let backup_bytes = encode_backup(&ciphertext, &pub_data).unwrap();
        drop(sender);

        let mut rebuilt = make_test_wallet_with_keys(keys, false);
        rebuilt.restore_backup(&backup_bytes, password).unwrap();

        rebuilt.online_data = Some(OnlineData::for_test(indexer_url));
        let online = Online {
            id: 1,
            indexer_url: indexer_url.to_string(),
        };
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(rebuilt.refresh(online, None, vec![], true))
            .unwrap();

        let refreshed = &result[&batch_idx];
        assert!(
            matches!(&refreshed.failure, Some(Error::Internal { details }) if details.starts_with("invalid signed TX")),
            "rebuilt wallet lost the signed TX: {:?}",
            refreshed.failure
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_after_restore_backup_reads_signed_tx() {
        refresh_after_restore_backup_reads_signed_tx_impl(None);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_after_restore_backup_reads_signed_tx_for_rgb_asset() {
        refresh_after_restore_backup_reads_signed_tx_impl(Some("rgb:test-asset"));
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_fails_transfer_without_signed_tx_impl(asset_id: Option<&str>) {
        use crate::database::enums::TransferStatus;
        use crate::wallet::Online;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;
        use crate::wallet::online::OnlineData;

        let txid = "1".repeat(64);
        let indexer_url = "http://127.0.0.1:1";
        // A wallet restored from a backup without artifacts, ACK already received.
        let mut wallet = make_test_wallet(false);
        let (proxy, _mock) = ack_proxy();
        let batch_idx = seed_acknowledged_transfer(&mut wallet, &txid, asset_id, &proxy.url());
        let seeded_expiration = wallet
            .database
            .get_db_data(false)
            .unwrap()
            .batch_transfers
            .iter()
            .find(|t| t.idx == batch_idx)
            .unwrap()
            .expiration;

        wallet.online_data = Some(OnlineData::for_test(indexer_url));
        let online = Online {
            id: 1,
            indexer_url: indexer_url.to_string(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = rt
            .block_on(wallet.refresh(online.clone(), None, vec![], true))
            .unwrap();

        let refreshed = &result[&batch_idx];
        assert!(
            matches!(&refreshed.failure, Some(Error::MissingTransferArtifacts { txid: t }) if *t == txid),
            "unexpected failure: {:?}",
            refreshed.failure
        );
        assert_eq!(refreshed.updated_status, Some(TransferStatus::Failed));
        let failed_expiration = wallet
            .database
            .get_db_data(false)
            .unwrap()
            .batch_transfers
            .iter()
            .find(|t| t.idx == batch_idx)
            .unwrap()
            .expiration;
        assert_ne!(
            failed_expiration, seeded_expiration,
            "expiration was not refreshed when the transfer failed"
        );
        let result = rt
            .block_on(wallet.refresh(online, None, vec![], true))
            .unwrap();
        assert!(!result.contains_key(&batch_idx));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_fails_transfer_without_signed_tx() {
        refresh_fails_transfer_without_signed_tx_impl(None);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_fails_transfer_without_signed_tx_for_rgb_asset() {
        refresh_fails_transfer_without_signed_tx_impl(Some("rgb:test-asset"));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn refresh_skips_and_fail_transfers_fails_expired_initiated_sends() {
        use crate::database::enums::TransferStatus;
        use crate::database::memory_db::{ActiveValue, DbBatchTransferActMod};
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;
        use crate::wallet::online::{
            OnlineData, RefreshFilter, RefreshTransferStatus, TransferArtifacts,
        };
        use crate::wallet::{Online, Outpoint};

        let indexer_url = "http://127.0.0.1:1";
        let mut wallet = make_test_wallet(false);
        let (proxy, _mock) = ack_proxy();
        let mut seeded = vec![];
        for (txid, expiration) in [("1".repeat(64), 1), ("2".repeat(64), i64::MAX)] {
            let asset_id = format!("rgb:{}", &txid[..1]);
            let batch_idx =
                seed_acknowledged_transfer(&mut wallet, &txid, Some(&asset_id), &proxy.url());
            let batch_transfer = wallet
                .database
                .get_db_data(false)
                .unwrap()
                .batch_transfers
                .into_iter()
                .find(|t| t.idx == batch_idx)
                .unwrap();
            let mut updated: DbBatchTransferActMod = batch_transfer.into();
            updated.status = ActiveValue::Set(TransferStatus::Initiated);
            updated.expiration = ActiveValue::Set(Some(expiration));
            wallet.database.update_batch_transfer(&mut updated).unwrap();
            let input = Outpoint {
                txid: txid.clone(),
                vout: 0,
            };
            wallet.transfer_artifacts.insert(
                txid,
                TransferArtifacts {
                    inputs: vec![input.clone()],
                    ..Default::default()
                },
            );
            seeded.push((batch_idx, input));
        }
        let [(expired_idx, _), (pending_idx, pending_input)] = <[_; 2]>::try_from(seeded).unwrap();

        wallet.online_data = Some(OnlineData::for_test(indexer_url));
        let online = Online {
            id: 1,
            indexer_url: indexer_url.to_string(),
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let filter = vec![RefreshFilter {
            status: RefreshTransferStatus::WaitingCounterparty,
            incoming: false,
        }];
        let result = rt
            .block_on(wallet.refresh(online.clone(), None, filter, true))
            .unwrap();
        assert!(result.is_empty());

        assert!(
            rt.block_on(wallet.fail_transfers(online, None, false, true))
                .unwrap()
        );
        let statuses: Vec<(i32, TransferStatus)> = wallet
            .database
            .get_db_data(false)
            .unwrap()
            .batch_transfers
            .iter()
            .map(|t| (t.idx, t.status))
            .collect();
        assert_eq!(
            statuses,
            vec![
                (expired_idx, TransferStatus::Failed),
                (pending_idx, TransferStatus::Initiated)
            ]
        );
        assert_eq!(
            wallet.get_reserved_outpoints().unwrap(),
            vec![pending_input]
        );
    }

    // The backup, which VSS shares, and the IndexedDB snapshot carry the transfer artifacts and
    // the data of the proxy v1 flow.
    #[test]
    fn vss_restore_payload_keeps_transfer_artifacts() {
        // vss_restore_backup uses restore_payload, so this covers the VSS path.
        use crate::BTreeSet;
        use crate::keys::generate_keys;
        use crate::utils::MemStock;
        use crate::wallet::offline::ReceivedConsignmentMeta;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet_with_keys;
        use crate::wallet::online::TransferArtifacts;

        let txid = "2".repeat(64);
        let keys = generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        );

        let mut sender = make_test_wallet_with_keys(keys.clone(), false);
        *sender.rgb_stock.borrow_mut() = Some(MemStock::default());
        sender.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_tx: Some(vec![1]),
                ..Default::default()
            },
        );
        let meta = ReceivedConsignmentMeta {
            proxy_url: Some(s!("http://127.0.0.1:3000")),
            transfer_id: Some(s!("transfer")),
            sender_transport_endpoint: s!("rpc://127.0.0.1:3000"),
            sender_pubkey: "11".repeat(32),
            consignment_hash: "22".repeat(32),
            batch: true,
            unsafe_txids: BTreeSet::from(["33".repeat(32)]),
        };
        sender
            .received_consignments
            .insert(s!("recipient"), vec![1, 2, 3]);
        sender
            .received_consignment_meta
            .insert(s!("recipient"), meta.clone());
        sender
            .received_signed_txs
            .insert(s!("recipient"), vec![4, 5, 6]);
        let pml = crate::wallet::offline::address_reuse_tests::pending_mint_locks();
        sender
            .pending_mint_locks
            .insert(s!("recipient"), pml.clone());
        sender.media.insert(s!("digest"), vec![7, 8]);
        let payload_json = sender.serialize_backup_payload().unwrap();

        let mut restored = make_test_wallet_with_keys(keys.clone(), false);
        restored.restore_payload(&payload_json).unwrap();
        let mut from_snapshot = make_test_wallet_with_keys(keys, false);
        from_snapshot
            .restore_from_snapshot(sender.snapshot().unwrap())
            .unwrap();

        assert_eq!(
            restored
                .transfer_artifacts
                .get(&txid)
                .and_then(|a| a.signed_tx.clone()),
            Some(vec![1])
        );
        for restored in [&restored, &from_snapshot] {
            assert_eq!(
                restored.received_consignments.get("recipient"),
                Some(&vec![1, 2, 3])
            );
            assert_eq!(
                restored.received_consignment_meta.get("recipient"),
                Some(&meta)
            );
            assert_eq!(
                restored.received_signed_txs.get("recipient"),
                Some(&vec![4, 5, 6])
            );
            assert_eq!(restored.pending_mint_locks.get("recipient"), Some(&pml));
            assert_eq!(restored.media.get("digest"), Some(&vec![7, 8]));
        }
    }

    // A wallet with one transfer, its artifacts and received data.
    #[cfg(not(target_arch = "wasm32"))]
    fn wallet_with_transfer(keys: crate::keys::Keys) -> crate::wallet::Wallet {
        use crate::utils::MemStock;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet_with_keys;
        use crate::wallet::online::TransferArtifacts;

        let mut wallet = make_test_wallet_with_keys(keys, false);
        *wallet.rgb_stock.borrow_mut() = Some(MemStock::default());
        seed_acknowledged_transfer(
            &mut wallet,
            &"3".repeat(64),
            Some("rgb:test-asset"),
            "rpc://127.0.0.1:1",
        );
        wallet.transfer_artifacts.insert(
            "3".repeat(64),
            TransferArtifacts {
                signed_tx: Some(vec![1]),
                ..Default::default()
            },
        );
        wallet
            .received_consignments
            .insert(s!("recipient"), vec![1, 2, 3]);
        wallet
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn backup_payload_of(keys: crate::wallet::SinglesigKeys) -> WalletBackupPayload {
        use crate::utils::MemStock;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet_with_singlesig_keys;

        let source = make_test_wallet_with_singlesig_keys(keys, false);
        *source.rgb_stock.borrow_mut() = Some(MemStock::default());
        serde_json::from_slice(&source.serialize_backup_payload().unwrap()).unwrap()
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn assert_failed_restore_keeps_wallet(
        wallet: &mut crate::wallet::Wallet,
        payload: &WalletBackupPayload,
    ) -> Error {
        let before = wallet.serialize_backup_payload().unwrap();
        let (ciphertext, pub_data) =
            encrypt_payload(&serde_json::to_vec(payload).unwrap(), "pw").unwrap();
        let backup = encode_backup(&ciphertext, &pub_data).unwrap();

        let err = wallet
            .restore_backup(&backup, "pw")
            .expect_err("restore of a broken backup succeeded");

        assert_eq!(
            wallet.serialize_backup_payload().unwrap(),
            before,
            "the failed restore changed the wallet"
        );
        err
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn failed_restore_with_bad_rgb_store_keeps_wallet() {
        let keys = crate::keys::generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        );
        let mut wallet = wallet_with_transfer(keys.clone());
        let mut payload = backup_payload_of(crate::wallet::SinglesigKeys::from_keys(&keys, None));
        payload.rgb_store = s!("not base64!");

        assert_failed_restore_keeps_wallet(&mut wallet, &payload);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn failed_restore_of_other_wallet_keeps_wallet() {
        use crate::keys::generate_keys;

        let mut wallet = wallet_with_transfer(generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        ));
        let mut payload = backup_payload_of(crate::wallet::SinglesigKeys::from_keys(
            &generate_keys(
                crate::BitcoinNetwork::Regtest,
                crate::WitnessVersion::Taproot,
            ),
            None,
        ));
        assert!(payload.bdk_changeset.is_some());

        assert_failed_restore_keeps_wallet(&mut wallet, &payload);

        payload.bdk_changeset = None;
        assert_failed_restore_keeps_wallet(&mut wallet, &payload);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn restore_backup_refuses_changed_settings() {
        let keys = crate::keys::generate_keys(
            crate::BitcoinNetwork::Regtest,
            crate::WitnessVersion::Taproot,
        );
        let mut wallet = wallet_with_transfer(keys.clone());
        let payload = backup_payload_of(crate::wallet::SinglesigKeys::from_keys(&keys, Some(1)));

        let err = assert_failed_restore_keeps_wallet(&mut wallet, &payload);

        assert!(
            matches!(
                &err,
                Error::WalletSettingMismatch { setting, expected, provided }
                    if setting == "vanilla_keychain" && expected == "1" && provided == "0"
            ),
            "{err:?}"
        );

        let mut payload = backup_payload_of(crate::wallet::SinglesigKeys::from_keys(&keys, None));
        payload.manifest.bitcoin_network = crate::BitcoinNetwork::Signet;

        let err = assert_failed_restore_keeps_wallet(&mut wallet, &payload);

        assert!(
            matches!(
                err,
                Error::BackupNetworkMismatch {
                    backup_network: crate::BitcoinNetwork::Signet,
                    wallet_network: crate::BitcoinNetwork::Regtest
                }
            ),
            "{err:?}"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    use crate::wallet::backup_store::{StoreFuture, StoredValue};

    /// An in-memory store that follows the `KvStore` contract. Clones share the data.
    #[cfg(not(target_arch = "wasm32"))]
    #[derive(Clone, Default)]
    struct MemKvStore(
        Rc<std::cell::RefCell<std::collections::BTreeMap<(String, String), StoredValue>>>,
    );

    #[cfg(not(target_arch = "wasm32"))]
    impl KvStore for MemKvStore {
        fn read<'a>(
            &'a self,
            namespace: &'a str,
            key: &'a str,
        ) -> StoreFuture<'a, Option<StoredValue>> {
            let entry = self
                .0
                .borrow()
                .get(&(namespace.to_string(), key.to_string()))
                .cloned();
            Box::pin(async move { Ok(entry) })
        }

        fn write<'a>(
            &'a self,
            namespace: &'a str,
            key: &'a str,
            value: Vec<u8>,
            expected_version: Option<i64>,
        ) -> StoreFuture<'a, i64> {
            Box::pin(async move {
                let mut data = self.0.borrow_mut();
                let k = (namespace.to_string(), key.to_string());
                let cur = data.get(&k).map(|v| v.version);
                if cur != expected_version {
                    return Err(Error::VssVersionConflict {
                        details: s!("version"),
                    });
                }
                let version = cur.unwrap_or(0) + 1;
                data.insert(k, StoredValue { value, version });
                Ok(version)
            })
        }

        fn remove<'a>(
            &'a self,
            namespace: &'a str,
            key: &'a str,
            _version: Option<i64>,
        ) -> StoreFuture<'a, ()> {
            self.0
                .borrow_mut()
                .remove(&(namespace.to_string(), key.to_string()));
            Box::pin(async { Ok(()) })
        }

        fn list<'a>(&'a self, _namespace: &'a str) -> StoreFuture<'a, Vec<String>> {
            unreachable!()
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn wallet_on(
        keys: &crate::keys::Keys,
        network: crate::BitcoinNetwork,
        store: &MemKvStore,
    ) -> crate::wallet::Wallet {
        use crate::utils::MemStock;
        use crate::wallet::offline::address_reuse_tests::make_test_wallet_on_network;

        let mut wallet = make_test_wallet_on_network(
            crate::wallet::SinglesigKeys::from_keys(keys, None),
            false,
            network,
        );
        *wallet.rgb_stock.borrow_mut() = Some(MemStock::default());
        wallet.configure_backup_store(
            Box::new(store.clone()),
            "mem",
            SecretKey::from_slice(&[1; 32]).unwrap(),
        );
        wallet
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn backup_store_keeps_one_backup_per_network() {
        use crate::BitcoinNetwork::{Regtest, Signet};
        use crate::api::evm::tests::block_on;

        let keys = crate::keys::generate_keys(Regtest, crate::WitnessVersion::Taproot);
        let store = MemKvStore::default();
        let mut r = wallet_on(&keys, Regtest, &store);
        r.media.insert(s!("regtest"), vec![1]);
        block_on(r.vss_backup()).unwrap();

        let mut s = wallet_on(&keys, Signet, &store);
        assert!(!block_on(s.vss_backup_info()).unwrap().backup_exists);
        assert!(matches!(
            block_on(s.vss_restore_backup()),
            Err(Error::VssBackupNotFound)
        ));
        s.media.insert(s!("signet"), vec![2]);
        block_on(s.vss_backup()).unwrap();

        let fp = &keys.master_fingerprint;
        assert_eq!(
            store.0.borrow().keys().cloned().collect::<Vec<_>>(),
            [
                (s!("backup"), format!("wallet/{fp}/Regtest")),
                (s!("backup"), format!("wallet/{fp}/Signet")),
            ]
        );
        for (network, own, other) in [
            (Regtest, "regtest", "signet"),
            (Signet, "signet", "regtest"),
        ] {
            let mut w = wallet_on(&keys, network, &store);
            block_on(w.vss_restore_backup()).unwrap();
            assert!(w.media.contains_key(own), "{network}");
            assert!(!w.media.contains_key(other), "{network}");
        }
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn backup_store_refuses_other_network() {
        use crate::BitcoinNetwork::{Regtest, Signet};
        use crate::api::evm::tests::block_on;

        let keys = crate::keys::generate_keys(Regtest, crate::WitnessVersion::Taproot);
        let store = MemKvStore::default();
        block_on(wallet_on(&keys, Regtest, &store).vss_backup()).unwrap();
        let mut s = wallet_on(&keys, Signet, &store);
        block_on(s.vss_backup()).unwrap();
        {
            let fp = &keys.master_fingerprint;
            let mut data = store.0.borrow_mut();
            let regtest = data[&(s!("backup"), backup_key(fp, Regtest))].value.clone();
            let signet = data
                .get_mut(&(s!("backup"), backup_key(fp, Signet)))
                .unwrap();
            signet.value = regtest;
            signet.version += 1;
        }
        let before = s.serialize_backup_payload().unwrap();
        let versions = s.backup_versions.borrow().clone();

        let err = block_on(s.vss_restore_backup()).unwrap_err();

        assert!(
            matches!(
                err,
                Error::BackupNetworkMismatch {
                    backup_network: Regtest,
                    wallet_network: Signet
                }
            ),
            "{err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Backup network Regtest does not match the wallet network Signet"
        );
        assert_eq!(s.serialize_backup_payload().unwrap(), before);
        assert_eq!(*s.backup_versions.borrow(), versions);
    }
}

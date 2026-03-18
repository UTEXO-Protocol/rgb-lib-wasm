//! VSS (Versioned Storage Service) cloud backup module.
//!
//! Implements a WASM-compatible VSS client using reqwest + prost,
//! since upstream vss-client-ng depends on bitreq/tokio::net which
//! cannot compile to wasm32-unknown-unknown.

#![allow(missing_docs)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::{Read, Write};

use bdk_wallet::bitcoin::hashes::{Hash as _, sha256::Hash as Sha256};
use bdk_wallet::bitcoin::secp256k1::{Message, Secp256k1, SecretKey, SignOnly};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, aead::stream};
use hkdf::Hkdf;
use prost::Message as ProstMessage; // trait (for .decode() etc.)
use serde::{Deserialize, Serialize};
use sha2::Sha256 as HkdfSha256;

use crate::Error;
use crate::error::InternalError;

// --- Protobuf types (matching vss-server proto) ---

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetObjectRequest {
    #[prost(string, tag = "1")]
    pub store_id: String,
    #[prost(string, tag = "2")]
    pub key: String,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct GetObjectResponse {
    #[prost(message, optional, tag = "2")]
    pub value: Option<KeyValue>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PutObjectRequest {
    #[prost(string, tag = "1")]
    pub store_id: String,
    #[prost(int64, optional, tag = "2")]
    pub global_version: Option<i64>,
    #[prost(message, repeated, tag = "3")]
    pub transaction_items: Vec<KeyValue>,
    #[prost(message, repeated, tag = "4")]
    pub delete_items: Vec<KeyValue>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct PutObjectResponse {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct KeyValue {
    #[prost(string, tag = "1")]
    pub key: String,
    #[prost(int64, tag = "2")]
    pub version: i64,
    #[prost(bytes = "vec", tag = "3")]
    pub value: Vec<u8>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ErrorResponse {
    #[prost(enumeration = "ErrorCode", tag = "1")]
    pub error_code: i32,
    #[prost(string, tag = "2")]
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, prost::Enumeration)]
#[repr(i32)]
pub enum ErrorCode {
    Unknown = 0,
    ConflictException = 1,
    InvalidRequestException = 2,
    InternalServerException = 3,
    NoSuchKeyException = 4,
    AuthException = 5,
}

// --- VSS Error handling ---

#[derive(Debug)]
pub(crate) enum VssError {
    NoSuchKey(String),
    InvalidRequest(String),
    Conflict(String),
    Auth(String),
    InternalServer(String),
    Internal(String),
}

impl VssError {
    pub(crate) fn from_http(status: u16, body: &[u8]) -> Self {
        match ErrorResponse::decode(body) {
            Ok(err_resp) => match ErrorCode::from_i32(err_resp.error_code) {
                Some(ErrorCode::NoSuchKeyException) => Self::NoSuchKey(err_resp.message),
                Some(ErrorCode::ConflictException) => Self::Conflict(err_resp.message),
                Some(ErrorCode::AuthException) => Self::Auth(err_resp.message),
                Some(ErrorCode::InvalidRequestException) => Self::InvalidRequest(err_resp.message),
                Some(ErrorCode::InternalServerException) => Self::InternalServer(err_resp.message),
                _ => Self::Internal(format!(
                    "Unknown error code {}: {}",
                    err_resp.error_code, err_resp.message
                )),
            },
            Err(e) => Self::Internal(format!("HTTP {status}, decode error: {e}")),
        }
    }
}

impl From<VssError> for Error {
    fn from(e: VssError) -> Self {
        match e {
            VssError::NoSuchKey(_) => Error::VssBackupNotFound,
            VssError::Conflict(msg) => Error::VssVersionConflict { details: msg },
            VssError::Auth(msg) => Error::VssAuth { details: msg },
            VssError::InvalidRequest(msg) => Error::VssError {
                details: format!("Invalid request: {msg}"),
            },
            VssError::InternalServer(msg) => Error::VssError {
                details: format!("Server error: {msg}"),
            },
            VssError::Internal(msg) => Error::VssError { details: msg },
        }
    }
}

// --- SigsAuth (matches vss-client-ng sigs_auth.rs) ---

const SIGNING_CONSTANT: &[u8] = b"VSS Signature Authorizer Signing Salt Constant..................";

fn build_auth_token(secret_key: &SecretKey, secp_ctx: &Secp256k1<SignOnly>) -> String {
    let pubkey = secret_key.public_key(secp_ctx);
    let now = (js_sys::Date::now() / 1000.0) as u64;

    // buffer: 64 (constant) + 33 (pubkey) + 20 (max timestamp digits)
    let mut buffer = [0u8; 64 + 33 + 20];
    let mut stream = &mut buffer[..];
    stream.write_all(SIGNING_CONSTANT).unwrap();
    stream.write_all(&pubkey.serialize()).unwrap();
    write!(stream, "{now}").unwrap();
    let bytes_remaining = stream.len();
    let bytes_to_sign = &buffer[..buffer.len() - bytes_remaining];

    let hash = Sha256::hash(bytes_to_sign);
    let sig = secp_ctx.sign_ecdsa(&Message::from_digest(hash.to_byte_array()), secret_key);

    let mut out = String::with_capacity((33 + 64 + 20) * 2);
    write!(&mut out, "{pubkey:x}").unwrap();
    for c in sig.serialize_compact() {
        write!(&mut out, "{c:02x}").unwrap();
    }
    write!(&mut out, "{now}").unwrap();
    out
}

// --- WASM VSS HTTP Client ---

const APPLICATION_OCTET_STREAM: &str = "application/octet-stream";

pub(crate) struct WasmVssClient {
    base_url: String,
    signing_key: SecretKey,
    secp_ctx: Secp256k1<SignOnly>,
    http: reqwest::Client,
}

impl WasmVssClient {
    pub(crate) fn new(base_url: String, signing_key: SecretKey) -> Self {
        Self {
            base_url,
            signing_key,
            secp_ctx: Secp256k1::signing_only(),
            http: reqwest::Client::new(),
        }
    }

    async fn post_request<T: ProstMessage + Default>(
        &self,
        request: &impl ProstMessage,
        endpoint: &str,
    ) -> Result<T, VssError> {
        let url = format!("{}/{endpoint}", self.base_url);
        let body = request.encode_to_vec();
        let auth_token = build_auth_token(&self.signing_key, &self.secp_ctx);

        let resp = self
            .http
            .post(&url)
            .header("content-type", APPLICATION_OCTET_STREAM)
            .header("Authorization", auth_token)
            .body(body)
            .send()
            .await
            .map_err(|e| VssError::Internal(format!("HTTP error: {e}")))?;

        let status = resp.status().as_u16();
        let resp_bytes = resp
            .bytes()
            .await
            .map_err(|e| VssError::Internal(format!("Read body error: {e}")))?;

        if status == 200 {
            T::decode(resp_bytes.as_ref())
                .map_err(|e| VssError::Internal(format!("Decode response: {e}")))
        } else {
            Err(VssError::from_http(status, &resp_bytes))
        }
    }

    pub(crate) async fn get_object(
        &self,
        request: &GetObjectRequest,
    ) -> Result<GetObjectResponse, VssError> {
        self.post_request(request, "getObject").await
    }

    pub(crate) async fn put_object(
        &self,
        request: &PutObjectRequest,
    ) -> Result<PutObjectResponse, VssError> {
        self.post_request(request, "putObjects").await
    }
}

// --- Encryption (matches upstream vss.rs) ---

/// 239 bytes plaintext + 16-byte AEAD tag = 255 bytes per encrypted chunk
const BACKUP_BUFFER_LEN_ENCRYPT: usize = 239;
const BACKUP_BUFFER_LEN_DECRYPT: usize = BACKUP_BUFFER_LEN_ENCRYPT + 16;
const BACKUP_KEY_LENGTH: usize = 32;
/// 19-byte nonce for streaming XChaCha20Poly1305 (EncryptorBE32)
const BACKUP_NONCE_LENGTH: usize = 19;
const BACKUP_SALT_LENGTH: usize = 32;
const VSS_BACKUP_VERSION: u8 = 1;

const HKDF_INFO: &[u8] = b"rgb-lib-vss-backup-encryption-v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VssEncryptionMetadata {
    pub salt: String,
    pub nonce: String,
    pub version: u8,
}

impl VssEncryptionMetadata {
    fn new() -> Result<Self, Error> {
        let mut salt = [0u8; BACKUP_SALT_LENGTH];
        let mut nonce = [0u8; BACKUP_NONCE_LENGTH];
        getrandom::getrandom(&mut salt).map_err(|e| Error::Internal {
            details: format!("RNG unavailable: {e}"),
        })?;
        getrandom::getrandom(&mut nonce).map_err(|e| Error::Internal {
            details: format!("RNG unavailable: {e}"),
        })?;
        Ok(Self {
            salt: hex::encode(salt),
            nonce: hex::encode(nonce),
            version: VSS_BACKUP_VERSION,
        })
    }

    fn nonce_bytes(&self) -> Result<[u8; BACKUP_NONCE_LENGTH], Error> {
        let bytes = hex::decode(&self.nonce).map_err(|e| Error::Internal {
            details: format!("Invalid nonce hex: {e}"),
        })?;
        bytes[..BACKUP_NONCE_LENGTH]
            .try_into()
            .map_err(|_| Error::Internal {
                details: "Invalid nonce length".to_string(),
            })
    }
}

fn derive_encryption_key(
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
) -> Result<Key, Error> {
    let salt_bytes = hex::decode(&metadata.salt).map_err(|e| Error::Internal {
        details: format!("Invalid salt hex: {e}"),
    })?;
    let hk = Hkdf::<HkdfSha256>::new(Some(&salt_bytes), &signing_key.secret_bytes());
    let mut key_bytes = [0u8; BACKUP_KEY_LENGTH];
    hk.expand(HKDF_INFO, &mut key_bytes)
        .map_err(|e| Error::Internal {
            details: format!("HKDF expansion failed: {e}"),
        })?;
    Ok(Key::clone_from_slice(&key_bytes))
}

pub(crate) fn encrypt_data(
    data: &[u8],
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
) -> Result<Vec<u8>, Error> {
    let key = derive_encryption_key(signing_key, metadata)?;
    let aead = XChaCha20Poly1305::new(&key);
    let nonce = metadata.nonce_bytes()?;
    let nonce = chacha20poly1305::aead::generic_array::GenericArray::from_slice(&nonce);

    let mut stream_encryptor = stream::EncryptorBE32::from_aead(aead, nonce);
    let mut encrypted = Vec::new();
    let mut buffer = [0u8; BACKUP_BUFFER_LEN_ENCRYPT];
    let mut reader = std::io::Cursor::new(data);

    loop {
        let read_count = reader.read(&mut buffer).map_err(|e| Error::Internal {
            details: format!("Read error: {e}"),
        })?;
        if read_count == BACKUP_BUFFER_LEN_ENCRYPT {
            let ciphertext = stream_encryptor
                .encrypt_next(buffer.as_slice())
                .map_err(|e| Error::Internal {
                    details: format!("Encryption error: {e}"),
                })?;
            encrypted.extend(ciphertext);
        } else {
            let ciphertext = stream_encryptor
                .encrypt_last(&buffer[..read_count])
                .map_err(|e| Error::Internal {
                    details: format!("Encryption error: {e}"),
                })?;
            encrypted.extend(ciphertext);
            break;
        }
    }
    Ok(encrypted)
}

pub(crate) fn decrypt_data(
    encrypted: &[u8],
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
) -> Result<Vec<u8>, Error> {
    let key = derive_encryption_key(signing_key, metadata)?;
    let aead = XChaCha20Poly1305::new(&key);
    let nonce = metadata.nonce_bytes()?;
    let nonce = chacha20poly1305::aead::generic_array::GenericArray::from_slice(&nonce);

    let mut stream_decryptor = stream::DecryptorBE32::from_aead(aead, nonce);
    let mut decrypted = Vec::new();
    let mut buffer = [0u8; BACKUP_BUFFER_LEN_DECRYPT];
    let mut reader = std::io::Cursor::new(encrypted);

    loop {
        let read_count = reader.read(&mut buffer).map_err(|e| Error::Internal {
            details: format!("Read error: {e}"),
        })?;
        if read_count == BACKUP_BUFFER_LEN_DECRYPT {
            let cleartext = stream_decryptor
                .decrypt_next(buffer.as_slice())
                .map_err(|_| Error::VssError {
                    details: "Decryption failed: wrong signing key or corrupted data".to_string(),
                })?;
            decrypted.extend(cleartext);
        } else if read_count == 0 {
            break;
        } else {
            let cleartext = stream_decryptor
                .decrypt_last(&buffer[..read_count])
                .map_err(|_| Error::VssError {
                    details: "Decryption failed: wrong signing key or corrupted data".to_string(),
                })?;
            decrypted.extend(cleartext);
            break;
        }
    }
    Ok(decrypted)
}

// --- VSS Backup Keys ---

const BACKUP_KEY_DATA: &str = "backup/data";
const BACKUP_KEY_METADATA: &str = "backup/metadata";
const BACKUP_KEY_MANIFEST: &str = "backup/manifest";
const BACKUP_KEY_FINGERPRINT: &str = "backup/fingerprint";

/// Backup manifest stored on the VSS server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    pub chunk_count: usize,
    pub total_size: usize,
    pub encrypted: bool,
    pub version: u8,
}

/// Configuration for VSS backup.
#[derive(Clone)]
pub struct VssBackupConfig {
    pub server_url: String,
    pub store_id: String,
    pub signing_key: SecretKey,
}

impl VssBackupConfig {
    pub fn new(server_url: String, store_id: String, signing_key: SecretKey) -> Self {
        Self {
            server_url,
            store_id,
            signing_key,
        }
    }
}

/// Info returned by vss_backup_info().
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VssBackupInfo {
    pub backup_exists: bool,
    pub server_version: Option<i64>,
    pub backup_required: bool,
}

/// WASM-compatible VSS backup client.
pub struct VssBackupClient {
    client: WasmVssClient,
    store_id: String,
    signing_key: SecretKey,
}

impl VssBackupClient {
    pub fn new(config: &VssBackupConfig) -> Self {
        Self {
            client: WasmVssClient::new(config.server_url.clone(), config.signing_key),
            store_id: config.store_id.clone(),
            signing_key: config.signing_key,
        }
    }

    async fn get_current_version(&self, key: &str) -> Result<Option<i64>, Error> {
        let request = GetObjectRequest {
            store_id: self.store_id.clone(),
            key: key.to_string(),
        };
        match self.client.get_object(&request).await {
            Ok(resp) => Ok(resp.value.map(|kv| kv.version)),
            Err(VssError::NoSuchKey(_)) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Upload encrypted backup data to VSS server. Returns the new version.
    pub async fn upload_backup(&self, plaintext: &[u8], fingerprint: &str) -> Result<i64, Error> {
        let metadata = VssEncryptionMetadata::new()?;
        let encrypted = encrypt_data(plaintext, &self.signing_key, &metadata)?;

        let data_version = self
            .get_current_version(BACKUP_KEY_DATA)
            .await?
            .unwrap_or(0);
        let manifest_version = self
            .get_current_version(BACKUP_KEY_MANIFEST)
            .await?
            .unwrap_or(0);
        let fingerprint_version = self
            .get_current_version(BACKUP_KEY_FINGERPRINT)
            .await?
            .unwrap_or(0);
        let metadata_version = self
            .get_current_version(BACKUP_KEY_METADATA)
            .await?
            .unwrap_or(0);

        let manifest = BackupManifest {
            chunk_count: 1,
            total_size: encrypted.len(),
            encrypted: true,
            version: VSS_BACKUP_VERSION,
        };
        let manifest_json = serde_json::to_vec(&manifest).map_err(InternalError::from)?;
        let metadata_json = serde_json::to_vec(&metadata).map_err(InternalError::from)?;

        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![
                KeyValue {
                    key: BACKUP_KEY_DATA.to_string(),
                    version: data_version,
                    value: encrypted,
                },
                KeyValue {
                    key: BACKUP_KEY_MANIFEST.to_string(),
                    version: manifest_version,
                    value: manifest_json,
                },
                KeyValue {
                    key: BACKUP_KEY_FINGERPRINT.to_string(),
                    version: fingerprint_version,
                    value: fingerprint.as_bytes().to_vec(),
                },
                KeyValue {
                    key: BACKUP_KEY_METADATA.to_string(),
                    version: metadata_version,
                    value: metadata_json,
                },
            ],
            delete_items: vec![],
        };

        self.client
            .put_object(&request)
            .await
            .map_err(Error::from)?;

        Ok(data_version + 1)
    }

    /// Download and decrypt backup from VSS server.
    pub async fn download_backup(&self) -> Result<Vec<u8>, Error> {
        let manifest_resp = self
            .client
            .get_object(&GetObjectRequest {
                store_id: self.store_id.clone(),
                key: BACKUP_KEY_MANIFEST.to_string(),
            })
            .await
            .map_err(Error::from)?;

        let manifest_bytes = manifest_resp
            .value
            .map(|kv| kv.value)
            .ok_or(Error::VssBackupNotFound)?;
        let manifest: BackupManifest =
            serde_json::from_slice(&manifest_bytes).map_err(|e| Error::Internal {
                details: format!("Failed to parse manifest: {e}"),
            })?;

        let data_resp = self
            .client
            .get_object(&GetObjectRequest {
                store_id: self.store_id.clone(),
                key: BACKUP_KEY_DATA.to_string(),
            })
            .await
            .map_err(Error::from)?;
        let raw_data = data_resp
            .value
            .map(|kv| kv.value)
            .ok_or(Error::VssBackupNotFound)?;

        if manifest.encrypted {
            let meta_resp = self
                .client
                .get_object(&GetObjectRequest {
                    store_id: self.store_id.clone(),
                    key: BACKUP_KEY_METADATA.to_string(),
                })
                .await
                .map_err(Error::from)?;
            let meta_bytes = meta_resp
                .value
                .map(|kv| kv.value)
                .ok_or(Error::VssBackupNotFound)?;
            let enc_metadata: VssEncryptionMetadata =
                serde_json::from_slice(&meta_bytes).map_err(|e| Error::Internal {
                    details: format!("Failed to parse encryption metadata: {e}"),
                })?;
            decrypt_data(&raw_data, &self.signing_key, &enc_metadata)
        } else {
            Ok(raw_data)
        }
    }

    /// Get the server-side backup version, or None if no backup exists.
    pub async fn get_backup_version(&self) -> Result<Option<i64>, Error> {
        self.get_current_version(BACKUP_KEY_MANIFEST).await
    }

    /// Delete backup data from VSS server.
    pub async fn delete_backup(&self) -> Result<(), Error> {
        let keys = [
            BACKUP_KEY_DATA,
            BACKUP_KEY_MANIFEST,
            BACKUP_KEY_METADATA,
            BACKUP_KEY_FINGERPRINT,
        ];
        let mut delete_items = Vec::new();
        for key in keys {
            if let Some(version) = self.get_current_version(key).await? {
                delete_items.push(KeyValue {
                    key: key.to_string(),
                    version,
                    value: vec![],
                });
            }
        }
        if delete_items.is_empty() {
            return Ok(());
        }
        let request = PutObjectRequest {
            store_id: self.store_id.clone(),
            global_version: None,
            transaction_items: vec![],
            delete_items,
        };
        self.client
            .put_object(&request)
            .await
            .map_err(Error::from)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bdk_wallet::bitcoin::secp256k1::rand::rngs::OsRng;

    fn test_signing_key() -> SecretKey {
        let secp = Secp256k1::new();
        let (sk, _) = secp.generate_keypair(&mut OsRng);
        sk
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let data = b"Hello, VSS backup!".to_vec();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        assert_ne!(encrypted, data);
        let decrypted = decrypt_data(&encrypted, &key, &metadata).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_large_data() {
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_exact_buffer_boundary() {
        let data: Vec<u8> = (0..BACKUP_BUFFER_LEN_ENCRYPT)
            .map(|i| (i % 256) as u8)
            .collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_multi_buffer() {
        let data: Vec<u8> = (0..(BACKUP_BUFFER_LEN_ENCRYPT * 3 + 50))
            .map(|i| (i % 256) as u8)
            .collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_empty() {
        let data: Vec<u8> = vec![];
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn wrong_key_fails() {
        let data = b"Secret data".to_vec();
        let key1 = test_signing_key();
        let key2 = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key1, &metadata).unwrap();
        let result = decrypt_data(&encrypted, &key2, &metadata);
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_data_fails() {
        let data = b"Test data".to_vec();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let mut encrypted = encrypt_data(&data, &key, &metadata).unwrap();
        if !encrypted.is_empty() {
            let mid = encrypted.len() / 2;
            encrypted[mid] ^= 0xFF;
        }
        assert!(decrypt_data(&encrypted, &key, &metadata).is_err());
    }

    #[test]
    fn different_salts_produce_different_keys() {
        let key = test_signing_key();
        let m1 = VssEncryptionMetadata::new().unwrap();
        let m2 = VssEncryptionMetadata::new().unwrap();
        let k1 = derive_encryption_key(&key, &m1).unwrap();
        let k2 = derive_encryption_key(&key, &m2).unwrap();
        assert_ne!(k1, k2);
    }

    #[test]
    fn metadata_serialization_roundtrip() {
        let metadata = VssEncryptionMetadata::new().unwrap();
        let json = serde_json::to_string(&metadata).unwrap();
        let parsed: VssEncryptionMetadata = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.version, VSS_BACKUP_VERSION);
        assert_eq!(parsed.nonce.len(), BACKUP_NONCE_LENGTH * 2);
        assert_eq!(parsed.salt.len(), BACKUP_SALT_LENGTH * 2);
    }
}

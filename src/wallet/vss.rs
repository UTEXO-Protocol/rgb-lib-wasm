//! VSS (Versioned Storage Service) cloud backup module.
//!
//! Implements a WASM-compatible VSS client using fetch + prost,
//! since upstream vss-client-ng depends on bitreq/tokio::net which
//! cannot compile to wasm32-unknown-unknown.

#![allow(missing_docs)]

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write;

use bdk_wallet::bitcoin::hashes::{Hash as _, sha256::Hash as Sha256};
use bdk_wallet::bitcoin::secp256k1::{Message, Secp256k1, SecretKey, SignOnly};
use chacha20poly1305::{Key, KeyInit, XChaCha20Poly1305, XNonce, aead::Aead};
use hkdf::Hkdf;
use prost::Message as ProstMessage; // trait (for .decode() etc.)
use serde::{Deserialize, Serialize};
use sha2::Sha256 as HkdfSha256;

use super::backup_store::{
    KvStore, MAX_BACKUP_LEN, MAX_KEY_LEN, MAX_LIST_KEYS, MAX_LIST_PAGES, StoreFuture, StoredValue,
};
use crate::Error;
use crate::encryption::chunk_nonce;

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
pub struct DeleteObjectRequest {
    #[prost(string, tag = "1")]
    pub store_id: String,
    #[prost(message, optional, tag = "2")]
    pub key_value: Option<KeyValue>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct DeleteObjectResponse {}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListKeyVersionsRequest {
    #[prost(string, tag = "1")]
    pub store_id: String,
    #[prost(string, optional, tag = "2")]
    pub key_prefix: Option<String>,
    #[prost(int32, optional, tag = "3")]
    pub page_size: Option<i32>,
    #[prost(string, optional, tag = "4")]
    pub page_token: Option<String>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct ListKeyVersionsResponse {
    #[prost(message, repeated, tag = "1")]
    pub key_versions: Vec<KeyValue>,
    #[prost(string, optional, tag = "2")]
    pub next_page_token: Option<String>,
    #[prost(int64, optional, tag = "3")]
    pub global_version: Option<i64>,
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
pub enum VssError {
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

/// Maximum size, in bytes, of a VSS response body.
const MAX_VSS_BODY: u64 = MAX_BACKUP_LEN as u64 + 4096;

pub struct WasmVssClient {
    base_url: String,
    signing_key: SecretKey,
    secp_ctx: Secp256k1<SignOnly>,
}

impl WasmVssClient {
    pub fn new(base_url: String, signing_key: SecretKey) -> Self {
        Self {
            base_url,
            signing_key,
            secp_ctx: Secp256k1::signing_only(),
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

        let http_err = |e: gloo_net::Error| VssError::Internal(format!("HTTP error: {e}"));
        // VSS uses the indexer timeout.
        let signal =
            web_sys::AbortSignal::timeout_with_u32(u32::from(crate::utils::INDEXER_TIMEOUT) * 1000);
        let resp = gloo_net::http::Request::post(&url)
            .header("content-type", APPLICATION_OCTET_STREAM)
            .header("Authorization", &auth_token)
            .abort_signal(Some(&signal))
            .body(js_sys::Uint8Array::from(&body[..]))
            .map_err(http_err)?
            .send()
            .await
            .map_err(http_err)?;

        let status = resp.status();
        let resp_bytes = crate::utils::read_body_limited(resp.body(), MAX_VSS_BODY)
            .await
            .map_err(|e| VssError::Internal(format!("Read body error: {e}")))?;
        if resp_bytes.len() as u64 > MAX_VSS_BODY {
            return Err(VssError::Internal("response too large".to_string()));
        }

        if status == 200 {
            T::decode(resp_bytes.as_ref())
                .map_err(|e| VssError::Internal(format!("Decode response: {e}")))
        } else {
            Err(VssError::from_http(status, &resp_bytes))
        }
    }

    pub async fn get_object(
        &self,
        request: &GetObjectRequest,
    ) -> Result<GetObjectResponse, VssError> {
        self.post_request(request, "getObject").await
    }

    pub async fn put_object(
        &self,
        request: &PutObjectRequest,
    ) -> Result<PutObjectResponse, VssError> {
        self.post_request(request, "putObjects").await
    }

    /// Delete a single key at its current `version`. VSS honors deletes only
    /// against the object's current version, so callers must read it first
    /// (a blind delete is rejected with a version conflict).
    pub async fn remove_object(
        &self,
        store_id: &str,
        key: &str,
        version: i64,
    ) -> Result<PutObjectResponse, VssError> {
        let request = PutObjectRequest {
            store_id: store_id.to_string(),
            global_version: None,
            transaction_items: vec![],
            delete_items: vec![KeyValue {
                key: key.to_string(),
                version,
                value: vec![],
            }],
        };
        self.put_object(&request).await
    }

    pub async fn delete_object(
        &self,
        request: &DeleteObjectRequest,
    ) -> Result<DeleteObjectResponse, VssError> {
        self.post_request(request, "deleteObject").await
    }

    pub async fn list_key_versions(
        &self,
        request: &ListKeyVersionsRequest,
    ) -> Result<ListKeyVersionsResponse, VssError> {
        self.post_request(request, "listKeyVersions").await
    }
}

// --- Encryption (matches upstream vss.rs) ---

/// 239 bytes plaintext + 16-byte AEAD tag = 255 bytes per encrypted chunk
const BACKUP_BUFFER_LEN_ENCRYPT: usize = 239;
pub(crate) const BACKUP_BUFFER_LEN_DECRYPT: usize = BACKUP_BUFFER_LEN_ENCRYPT + 16;
const BACKUP_KEY_LENGTH: usize = 32;
/// 19-byte nonce prefix for chunked XChaCha20Poly1305.
///
/// Public because external consumers of [`encrypt_data`]/[`decrypt_data`] (e.g. the
/// rgb-lightning-node wasm-sdk VSS KV store) build [`VssEncryptionMetadata`] by hand
/// and must agree on this length to parse their wire envelopes.
pub const BACKUP_NONCE_LENGTH: usize = 19;
/// HKDF salt length used by [`encrypt_data`]/[`decrypt_data`]. Public for the same
/// reason as [`BACKUP_NONCE_LENGTH`].
pub const BACKUP_SALT_LENGTH: usize = 32;

const HKDF_INFO: &[u8] = b"rgb-lib-vss-backup-encryption-v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VssEncryptionMetadata {
    pub salt: String,
    pub nonce: String,
    pub version: u8,
}

impl VssEncryptionMetadata {
    fn nonce_bytes(&self) -> Result<[u8; BACKUP_NONCE_LENGTH], Error> {
        let bytes = hex::decode(&self.nonce).map_err(|e| Error::Internal {
            details: format!("Invalid nonce hex: {e}"),
        })?;
        bytes.try_into().map_err(|_| Error::InvalidBackup)
    }
}

fn derive_encryption_key(
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
    info: Option<&[u8]>,
) -> Result<Key, Error> {
    let salt_bytes = hex::decode(&metadata.salt).map_err(|e| Error::Internal {
        details: format!("Invalid salt hex: {e}"),
    })?;
    let hk = Hkdf::<HkdfSha256>::new(Some(&salt_bytes), &signing_key.secret_bytes());
    let mut key_bytes = [0u8; BACKUP_KEY_LENGTH];
    hk.expand(info.unwrap_or(HKDF_INFO), &mut key_bytes)
        .map_err(|e| Error::Internal {
            details: format!("HKDF expansion failed: {e}"),
        })?;
    Ok(key_bytes.into())
}

/// Encrypt `data` for VSS storage. `info` domain-separates the derived key via
/// HKDF; pass `None` to use the default wallet-backup tag (`HKDF_INFO`), or a
/// distinct tag (e.g. for an LDK KV stream) so its keys never collide with the
/// wallet-backup stream even under the same signing key.
pub fn encrypt_data(
    data: &[u8],
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
    info: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let key = derive_encryption_key(signing_key, metadata, info)?;
    let aead = XChaCha20Poly1305::new(&key);
    let nonce = metadata.nonce_bytes()?;

    let (chunks, last) = data.as_chunks::<BACKUP_BUFFER_LEN_ENCRYPT>();
    let mut encrypted = Vec::new();
    for (position, (chunk, is_last)) in chunks
        .iter()
        .map(|c| (c.as_slice(), false))
        .chain([(last, true)])
        .enumerate()
    {
        let chunk_nonce = XNonce::from(chunk_nonce(&nonce, position as u32, is_last));
        encrypted.extend(
            aead.encrypt(&chunk_nonce, chunk)
                .map_err(|e| Error::Internal {
                    details: format!("Encryption error: {e}"),
                })?,
        );
    }
    Ok(encrypted)
}

/// Decrypt VSS-stored `encrypted` data. `info` must match the tag passed to
/// [`encrypt_data`] (`None` = default `HKDF_INFO`) for the round-trip to succeed.
pub fn decrypt_data(
    encrypted: &[u8],
    signing_key: &SecretKey,
    metadata: &VssEncryptionMetadata,
    info: Option<&[u8]>,
) -> Result<Vec<u8>, Error> {
    let key = derive_encryption_key(signing_key, metadata, info)?;
    let aead = XChaCha20Poly1305::new(&key);
    let nonce = metadata.nonce_bytes()?;

    let (chunks, last) = encrypted.as_chunks::<BACKUP_BUFFER_LEN_DECRYPT>();
    let last = (!last.is_empty()).then_some((last, true));
    let mut decrypted = Vec::new();
    for (position, (chunk, is_last)) in chunks
        .iter()
        .map(|c| (c.as_slice(), false))
        .chain(last)
        .enumerate()
    {
        let chunk_nonce = XNonce::from(chunk_nonce(&nonce, position as u32, is_last));
        decrypted.extend(
            aead.decrypt(&chunk_nonce, chunk)
                .map_err(|_| Error::VssError {
                    details: "Decryption failed: wrong signing key or corrupted data".to_string(),
                })?,
        );
    }
    Ok(decrypted)
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
    /// Error of the most recent failed backup upload, cleared on success
    pub last_backup_error: Option<String>,
}

/// The VSS adapter of [`KvStore`]. It stores `namespace/key` in one VSS store.
pub struct VssStore {
    client: WasmVssClient,
    store_id: String,
    page_size: Option<i32>,
}

impl VssStore {
    pub fn new(config: &VssBackupConfig) -> Self {
        Self {
            client: WasmVssClient::new(config.server_url.clone(), config.signing_key),
            store_id: config.store_id.clone(),
            page_size: None,
        }
    }

    /// Set the page size of `list` calls.
    pub fn with_list_page_size(mut self, size: i32) -> Self {
        self.page_size = Some(size);
        self
    }
}

fn vss_store_err(details: &str) -> Error {
    Error::VssError {
        details: details.to_string(),
    }
}

fn vss_key(ns: &str, key: &str) -> Result<String, Error> {
    if ns.is_empty() || ns.contains('/') || key.is_empty() || ns.len() + key.len() > MAX_KEY_LEN {
        return Err(vss_store_err("invalid namespace or key"));
    }
    Ok(format!("{ns}/{key}"))
}

impl KvStore for VssStore {
    fn read<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Option<StoredValue>> {
        Box::pin(async move {
            let key = vss_key(namespace, key)?;
            let request = GetObjectRequest {
                store_id: self.store_id.clone(),
                key: key.clone(),
            };
            let kv = match self.client.get_object(&request).await {
                Ok(GetObjectResponse { value: Some(kv) }) => kv,
                Ok(GetObjectResponse { value: None }) | Err(VssError::NoSuchKey(_)) => {
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            };
            if kv.key != key || kv.version < 1 || kv.value.len() > MAX_BACKUP_LEN {
                return Err(vss_store_err("VSS returned an invalid object"));
            }
            Ok(Some(StoredValue {
                value: kv.value,
                version: kv.version,
            }))
        })
    }

    fn write<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        value: Vec<u8>,
        expected_version: Option<i64>,
    ) -> StoreFuture<'a, i64> {
        Box::pin(async move {
            let key = vss_key(namespace, key)?;
            if expected_version.is_some_and(|v| v < 1) {
                return Err(vss_store_err("invalid expected version"));
            }
            let request = PutObjectRequest {
                store_id: self.store_id.clone(),
                global_version: None,
                transaction_items: vec![KeyValue {
                    key,
                    version: expected_version.unwrap_or(0),
                    value,
                }],
                delete_items: vec![],
            };
            self.client.put_object(&request).await?;
            match expected_version {
                None => Ok(1),
                Some(v) => v
                    .checked_add(1)
                    .ok_or_else(|| vss_store_err("backup store version overflow")),
            }
        })
    }

    fn remove<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        version: Option<i64>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let key = vss_key(namespace, key)?;
            match version {
                Some(v) => {
                    self.client.remove_object(&self.store_id, &key, v).await?;
                }
                None => {
                    let request = DeleteObjectRequest {
                        store_id: self.store_id.clone(),
                        key_value: Some(KeyValue {
                            key,
                            version: -1,
                            value: vec![],
                        }),
                    };
                    self.client.delete_object(&request).await?;
                }
            }
            Ok(())
        })
    }

    fn list<'a>(&'a self, namespace: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let prefix = vss_key(namespace, "x")?;
            let prefix = &prefix[..prefix.len() - 1];
            let mut keys = vec![];
            let mut count = 0usize;
            let mut page_token = None;
            for _ in 0..MAX_LIST_PAGES {
                let request = ListKeyVersionsRequest {
                    store_id: self.store_id.clone(),
                    key_prefix: Some(prefix.to_string()),
                    page_size: self.page_size,
                    page_token,
                };
                let resp = self.client.list_key_versions(&request).await?;
                count += resp.key_versions.len();
                if count > MAX_LIST_KEYS {
                    return Err(vss_store_err("backup store list has too many keys"));
                }
                keys.extend(
                    resp.key_versions
                        .into_iter()
                        .filter_map(|kv| kv.key.strip_prefix(prefix).map(str::to_string)),
                );
                match resp.next_page_token {
                    Some(token) if !token.is_empty() => page_token = Some(token),
                    _ => return Ok(keys),
                }
            }
            Err(vss_store_err("backup store list has too many pages"))
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bdk_wallet::bitcoin::secp256k1::rand::rngs::OsRng;

    const VSS_BACKUP_VERSION: u8 = 1;

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
    }
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
        let encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        assert_ne!(encrypted, data);
        let decrypted = decrypt_data(&encrypted, &key, &metadata, None).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_large_data() {
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata, None).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_exact_buffer_boundary() {
        let data: Vec<u8> = (0..BACKUP_BUFFER_LEN_ENCRYPT)
            .map(|i| (i % 256) as u8)
            .collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata, None).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn encrypt_decrypt_multi_buffer() {
        let data: Vec<u8> = (0..(BACKUP_BUFFER_LEN_ENCRYPT * 3 + 50))
            .map(|i| (i % 256) as u8)
            .collect();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let mut encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata, None).unwrap();
        assert_eq!(decrypted, data);

        // Each chunk has its own nonce, so swapped chunks fail.
        let (first, rest) = encrypted.split_at_mut(BACKUP_BUFFER_LEN_DECRYPT);
        first.swap_with_slice(&mut rest[..BACKUP_BUFFER_LEN_DECRYPT]);
        assert!(decrypt_data(&encrypted, &key, &metadata, None).is_err());
    }

    #[test]
    fn encrypt_decrypt_empty() {
        let data: Vec<u8> = vec![];
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        let decrypted = decrypt_data(&encrypted, &key, &metadata, None).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn wrong_key_fails() {
        let data = b"Secret data".to_vec();
        let key1 = test_signing_key();
        let key2 = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let encrypted = encrypt_data(&data, &key1, &metadata, None).unwrap();
        let result = decrypt_data(&encrypted, &key2, &metadata, None);
        assert!(result.is_err());
    }

    #[test]
    fn corrupted_data_fails() {
        let data = b"Test data".to_vec();
        let key = test_signing_key();
        let metadata = VssEncryptionMetadata::new().unwrap();
        let mut encrypted = encrypt_data(&data, &key, &metadata, None).unwrap();
        if !encrypted.is_empty() {
            let mid = encrypted.len() / 2;
            encrypted[mid] ^= 0xFF;
        }
        assert!(decrypt_data(&encrypted, &key, &metadata, None).is_err());
    }

    #[test]
    fn wrong_nonce_length_fails() {
        let key = test_signing_key();
        for nonce in ["00".to_string(), "00".repeat(BACKUP_NONCE_LENGTH + 1)] {
            let metadata = VssEncryptionMetadata {
                nonce,
                ..VssEncryptionMetadata::new().unwrap()
            };
            assert!(matches!(
                decrypt_data(&[], &key, &metadata, None),
                Err(Error::InvalidBackup)
            ));
            assert!(matches!(
                encrypt_data(&[], &key, &metadata, None),
                Err(Error::InvalidBackup)
            ));
        }
    }

    #[test]
    fn different_salts_produce_different_keys() {
        let key = test_signing_key();
        let m1 = VssEncryptionMetadata::new().unwrap();
        let m2 = VssEncryptionMetadata::new().unwrap();
        let k1 = derive_encryption_key(&key, &m1, None).unwrap();
        let k2 = derive_encryption_key(&key, &m2, None).unwrap();
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

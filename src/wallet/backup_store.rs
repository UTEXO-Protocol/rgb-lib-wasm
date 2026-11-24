//! Backup store: the key-value interface that the wallet backup goes through.

use std::future::Future;
use std::pin::Pin;

use bdk_wallet::bitcoin::secp256k1::SecretKey;
use js_sys::{Array, Function, JsString, Promise, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

use super::vss::{
    BACKUP_BUFFER_LEN_DECRYPT, BACKUP_NONCE_LENGTH, BACKUP_SALT_LENGTH, VssEncryptionMetadata,
    decrypt_data, encrypt_data,
};
use crate::Error;

/// Namespace of the wallet backup.
pub const BACKUP_NAMESPACE: &str = "backup";
/// Key of the wallet backup.
pub const BACKUP_KEY: &str = "wallet";
/// Maximum size, in bytes, of a backup blob.
pub const MAX_BACKUP_LEN: usize = 256 * 1024 * 1024;
/// Maximum number of keys that `list` accepts.
pub const MAX_LIST_KEYS: usize = 10_000;
/// Maximum number of pages that `list` reads.
pub const MAX_LIST_PAGES: usize = 1_000;
/// Maximum length, in bytes, of a key.
pub const MAX_KEY_LEN: usize = 1024;

const BLOB_VERSION: u8 = 1;
const BLOB_HEADER_LEN: usize = 1 + BACKUP_SALT_LENGTH + BACKUP_NONCE_LENGTH;

/// A value and its version, as a store returns it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredValue {
    /// The stored bytes.
    pub value: Vec<u8>,
    /// The version of the stored bytes.
    pub version: i64,
}

/// The future that a [`KvStore`] method returns.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, Error>> + 'a>>;

/// A versioned key-value store for the wallet backup.
///
/// - A version is a safe integer of 1 or more.
/// - `read` returns `None` when the key is absent.
/// - `write` without an expected version creates the key. It fails with
///   [`Error::VssVersionConflict`] when the key exists.
/// - `write` with an expected version writes only when the stored version is equal to it.
///   Otherwise it fails with [`Error::VssVersionConflict`], also when the key is absent.
/// - `write` returns the new version: greater than the expected one, or 1 or more for a create.
/// - `remove` with a version removes only that version. It fails with
///   [`Error::VssVersionConflict`] on another version or an absent key.
/// - `remove` without a version removes the key. An absent key is not an error.
/// - `list` returns the keys of the namespace.
pub trait KvStore {
    /// Read the value and version of a key.
    fn read<'a>(&'a self, namespace: &'a str, key: &'a str)
    -> StoreFuture<'a, Option<StoredValue>>;
    /// Write a value when the stored version is the expected one. Return the new version.
    fn write<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        value: Vec<u8>,
        expected_version: Option<i64>,
    ) -> StoreFuture<'a, i64>;
    /// Remove a key, only at `version` when it is set.
    fn remove<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        version: Option<i64>,
    ) -> StoreFuture<'a, ()>;
    /// List the keys of a namespace.
    fn list<'a>(&'a self, namespace: &'a str) -> StoreFuture<'a, Vec<String>>;
}

/// Return a local label for a store. JSON quoting keeps different parts from colliding.
pub(crate) fn store_label(kind: &str, parts: &[&str]) -> String {
    serde_json::to_string(&(kind, parts)).expect("strings serialize")
}

/// The configured backup store of a wallet.
pub(crate) struct BackupTarget {
    pub(crate) store: Box<dyn KvStore>,
    pub(crate) id: String,
    pub(crate) key: SecretKey,
    pub(crate) lock: tokio::sync::Mutex<()>,
}

/// Accept a new version only when it is greater than the expected one.
pub(crate) fn check_new_version(new: i64, expected: Option<i64>) -> Result<i64, Error> {
    if new > expected.unwrap_or(0) {
        Ok(new)
    } else {
        Err(Error::VssError {
            details: "backup store returned no new version: versioned writes are required"
                .to_string(),
        })
    }
}

fn fill_random(buf: &mut [u8]) -> Result<(), Error> {
    getrandom::getrandom(buf).map_err(|e| Error::Internal {
        details: format!("RNG unavailable: {e}"),
    })
}

fn encryption_metadata(salt: &[u8], nonce: &[u8]) -> VssEncryptionMetadata {
    VssEncryptionMetadata {
        salt: hex::encode(salt),
        nonce: hex::encode(nonce),
        version: BLOB_VERSION,
    }
}

/// Encrypt a backup payload into one blob. The fingerprint goes inside the ciphertext.
pub fn encode_backup_blob(
    payload: &[u8],
    fingerprint: &str,
    key: &SecretKey,
) -> Result<Vec<u8>, Error> {
    let fp_len = u8::try_from(fingerprint.len()).map_err(|_| Error::Internal {
        details: "fingerprint is too long".to_string(),
    })?;
    let mut salt = [0u8; BACKUP_SALT_LENGTH];
    let mut nonce = [0u8; BACKUP_NONCE_LENGTH];
    fill_random(&mut salt)?;
    fill_random(&mut nonce)?;
    let mut plain = Vec::with_capacity(1 + fingerprint.len() + payload.len());
    plain.push(fp_len);
    plain.extend_from_slice(fingerprint.as_bytes());
    plain.extend_from_slice(payload);
    let mut blob = vec![BLOB_VERSION];
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce);
    blob.extend(encrypt_data(
        &plain,
        key,
        &encryption_metadata(&salt, &nonce),
        None,
    )?);
    if blob.len() > MAX_BACKUP_LEN {
        return Err(Error::VssError {
            details: format!("backup is larger than {MAX_BACKUP_LEN} bytes"),
        });
    }
    Ok(blob)
}

/// Check and decrypt a backup blob. Return the payload.
pub fn decode_backup_blob(
    blob: &[u8],
    fingerprint: &str,
    key: &SecretKey,
) -> Result<Vec<u8>, Error> {
    if blob.len() > MAX_BACKUP_LEN || blob.len() < BLOB_HEADER_LEN {
        return Err(Error::InvalidBackup);
    }
    if blob[0] != BLOB_VERSION {
        return Err(Error::UnsupportedBackupVersion {
            version: blob[0].to_string(),
        });
    }
    let (salt, rest) = blob[1..].split_at(BACKUP_SALT_LENGTH);
    let (nonce, ciphertext) = rest.split_at(BACKUP_NONCE_LENGTH);
    // A valid ciphertext ends with a short chunk.
    if ciphertext.len() % BACKUP_BUFFER_LEN_DECRYPT == 0 {
        return Err(Error::InvalidBackup);
    }
    let plain = decrypt_data(ciphertext, key, &encryption_metadata(salt, nonce), None)?;
    let (&fp_len, rest) = plain.split_first().ok_or(Error::InvalidBackup)?;
    let (fp, payload) = rest
        .split_at_checked(fp_len as usize)
        .ok_or(Error::InvalidBackup)?;
    if fp != fingerprint.as_bytes() {
        return Err(Error::FingerprintMismatch);
    }
    Ok(payload.to_vec())
}

/// A [`KvStore`] that calls a JS object with the methods `read`, `write`, `remove` and `list`.
///
/// A method may return a promise. A conflict is an error with `code` equal to `"CONFLICT"`.
pub struct JsKvStore {
    obj: JsValue,
    read: Function,
    write: Function,
    remove: Function,
    list: Function,
}

fn js_store_error(e: JsValue) -> Error {
    let field = |name: &str| {
        e.is_object()
            .then(|| Reflect::get(&e, &JsValue::from_str(name)).ok())
            .flatten()
    };
    let details = field("message")
        .and_then(|m| m.dyn_into::<JsString>().ok())
        .map(|m| String::from(m.slice(0, 200)))
        .unwrap_or_else(|| "backup store error".to_string());
    if field("code").is_some_and(|c| c == "CONFLICT") {
        Error::VssVersionConflict { details }
    } else {
        Error::VssError { details }
    }
}

fn js_version(v: &JsValue) -> Option<i64> {
    v.as_f64()
        .filter(|n| js_sys::Number::is_safe_integer(v) && *n >= 1.0)
        .map(|n| n as i64)
}

fn js_opt_version(v: Option<i64>) -> JsValue {
    v.map_or(JsValue::UNDEFINED, |v| JsValue::from_f64(v as f64))
}

impl JsKvStore {
    /// Wrap a JS object. Refuse a value that is not an object or that misses a method.
    pub fn new(store: JsValue) -> Result<Self, Error> {
        if !store.is_object() {
            return Err(Error::VssError {
                details: "backup store is not an object".to_string(),
            });
        }
        let method = |name: &str| {
            Reflect::get(&store, &JsValue::from_str(name))
                .ok()
                .and_then(|f| f.dyn_into::<Function>().ok())
                .ok_or_else(|| Error::VssError {
                    details: format!("backup store has no {name} method"),
                })
        };
        let (read, write, remove, list) = (
            method("read")?,
            method("write")?,
            method("remove")?,
            method("list")?,
        );
        Ok(Self {
            obj: store,
            read,
            write,
            remove,
            list,
        })
    }

    async fn call(&self, f: &Function, args: &[JsValue]) -> Result<JsValue, Error> {
        let args: Array = args.iter().collect();
        let ret = f.apply(&self.obj, &args).map_err(js_store_error)?;
        JsFuture::from(Promise::resolve(&ret))
            .await
            .map_err(js_store_error)
    }
}

impl KvStore for JsKvStore {
    fn read<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
    ) -> StoreFuture<'a, Option<StoredValue>> {
        Box::pin(async move {
            let ret = self
                .call(&self.read, &[namespace.into(), key.into()])
                .await?;
            if ret.is_null() || ret.is_undefined() {
                return Ok(None);
            }
            let invalid = || Error::VssError {
                details: "backup store returned an invalid read result".to_string(),
            };
            if !ret.is_object() {
                return Err(invalid());
            }
            let value = Reflect::get(&ret, &"value".into())
                .ok()
                .and_then(|v| v.dyn_into::<Uint8Array>().ok())
                .filter(|v| v.length() as usize <= MAX_BACKUP_LEN)
                .ok_or_else(invalid)?;
            let version = Reflect::get(&ret, &"version".into())
                .ok()
                .and_then(|v| js_version(&v))
                .ok_or_else(invalid)?;
            Ok(Some(StoredValue {
                value: value.to_vec(),
                version,
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
            let args = [
                namespace.into(),
                key.into(),
                Uint8Array::from(&value[..]).into(),
                js_opt_version(expected_version),
            ];
            let ret = self.call(&self.write, &args).await?;
            js_version(&ret).ok_or_else(|| Error::VssError {
                details:
                    "backup store does not support versioned writes: write returned no version"
                        .to_string(),
            })
        })
    }

    fn remove<'a>(
        &'a self,
        namespace: &'a str,
        key: &'a str,
        version: Option<i64>,
    ) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let args = [namespace.into(), key.into(), js_opt_version(version)];
            self.call(&self.remove, &args).await.map(|_| ())
        })
    }

    fn list<'a>(&'a self, namespace: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let ret = self.call(&self.list, &[namespace.into()]).await?;
            let invalid = || Error::VssError {
                details: "backup store returned an invalid list result".to_string(),
            };
            if !Array::is_array(&ret) {
                return Err(invalid());
            }
            let keys: Array = ret.unchecked_into();
            if keys.length() as usize > MAX_LIST_KEYS {
                return Err(invalid());
            }
            keys.iter()
                .map(|k| {
                    k.dyn_into::<JsString>()
                        .ok()
                        .filter(|s| s.length() as usize <= MAX_KEY_LEN)
                        .map(String::from)
                        .ok_or_else(invalid)
                })
                .collect()
        })
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use bdk_wallet::bitcoin::secp256k1::{Secp256k1, rand::rngs::OsRng};

    fn key() -> SecretKey {
        Secp256k1::new().generate_keypair(&mut OsRng).0
    }

    const FP: &str = "a1b2c3d4";

    #[test]
    fn blob_round_trip() {
        let k = key();
        let payload: Vec<u8> = (0..(3 * 239 + 5)).map(|i| i as u8).collect();
        let blob = encode_backup_blob(&payload, FP, &k).unwrap();
        assert_eq!(decode_backup_blob(&blob, FP, &k).unwrap(), payload);
    }

    #[test]
    fn blob_refuses_short_header() {
        for len in [0, BLOB_HEADER_LEN - 1] {
            let res = decode_backup_blob(&vec![1; len], FP, &key());
            assert!(matches!(res, Err(Error::InvalidBackup)), "{res:?}");
        }
    }

    #[test]
    fn blob_refuses_unknown_format() {
        let k = key();
        let mut blob = encode_backup_blob(b"{}", FP, &k).unwrap();
        blob[0] = 2;
        let res = decode_backup_blob(&blob, FP, &k);
        assert!(
            matches!(res, Err(Error::UnsupportedBackupVersion { .. })),
            "{res:?}"
        );
    }

    #[test]
    fn blob_refuses_chunk_boundary_truncation() {
        let k = key();
        let blob = encode_backup_blob(&[7; 600], FP, &k).unwrap();
        let res = decode_backup_blob(&blob[..BLOB_HEADER_LEN + BACKUP_BUFFER_LEN_DECRYPT], FP, &k);
        assert!(matches!(res, Err(Error::InvalidBackup)), "{res:?}");
        let res = decode_backup_blob(&blob[..blob.len() - 1], FP, &k);
        assert!(matches!(res, Err(Error::VssError { .. })), "{res:?}");
    }

    #[test]
    fn blob_refuses_other_fingerprint() {
        let k = key();
        let blob = encode_backup_blob(b"{}", FP, &k).unwrap();
        let res = decode_backup_blob(&blob, "ffffffff", &k);
        assert!(matches!(res, Err(Error::FingerprintMismatch)), "{res:?}");
    }

    #[test]
    fn blob_refuses_bad_fingerprint_length() {
        let k = key();
        let mut salt = [0u8; BACKUP_SALT_LENGTH];
        let mut nonce = [0u8; BACKUP_NONCE_LENGTH];
        fill_random(&mut salt).unwrap();
        fill_random(&mut nonce).unwrap();
        let mut plain = vec![200];
        plain.extend([0; 10]);
        let mut blob = vec![BLOB_VERSION];
        blob.extend(salt);
        blob.extend(nonce);
        blob.extend(encrypt_data(&plain, &k, &encryption_metadata(&salt, &nonce), None).unwrap());
        let res = decode_backup_blob(&blob, FP, &k);
        assert!(matches!(res, Err(Error::InvalidBackup)), "{res:?}");
    }

    #[test]
    fn check_new_version_cases() {
        assert!(check_new_version(1, None).is_ok());
        assert!(check_new_version(0, None).is_err());
        assert!(check_new_version(3, Some(3)).is_err());
        assert!(check_new_version(2, Some(3)).is_err());
        assert!(check_new_version(4, Some(3)).is_ok());
    }
}

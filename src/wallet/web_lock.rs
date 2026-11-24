//! Exclusive Web Lock of an open wallet.

use std::cell::RefCell;
use std::collections::HashMap;

use js_sys::{Function, Object, Promise, Reflect};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::console;

use crate::Error;

thread_local! {
    /// Release promises of dropped wallets, keyed by lock name.
    static RELEASING: RefCell<HashMap<String, Promise>> = RefCell::new(HashMap::new());
}

/// Releases the lock when dropped.
pub(crate) struct LockRelease(Function);

impl Drop for LockRelease {
    fn drop(&mut self) {
        let _ = self.0.call0(&JsValue::UNDEFINED);
    }
}

/// Records the release of the lock when dropped, so a reopen waits for it.
pub(crate) struct WebLock {
    name: String,
    released: Promise,
}

impl Drop for WebLock {
    fn drop(&mut self) {
        RELEASING.with(|r| {
            r.borrow_mut()
                .insert(self.name.clone(), self.released.clone())
        });
    }
}

fn warn(e: &JsValue) {
    console::warn_1(&format!("Web Locks unavailable: {e:?}").into());
}

/// Take the exclusive Web Lock of the wallet with this IndexedDB key, without waiting.
/// Return `None` when the Web Locks API is not available.
pub(crate) async fn acquire(idb_key: &str) -> Result<Option<(WebLock, LockRelease)>, Error> {
    if cfg!(not(target_arch = "wasm32")) {
        return Ok(None);
    }
    let Some((locks, request)) = Reflect::get(&js_sys::global(), &"navigator".into())
        .ok()
        .filter(|n| n.is_object())
        .and_then(|n| Reflect::get(&n, &"locks".into()).ok())
        .filter(|l| l.is_object())
        .and_then(|l| {
            let request = Reflect::get(&l, &"request".into()).ok()?;
            Some((l, request.dyn_into::<Function>().ok()?))
        })
    else {
        return Ok(None);
    };

    let name = format!(
        "rgb-lib-wasm-wallet-{}",
        hex::encode(crate::encryption::sha256_digest(idb_key.as_bytes()))
    );
    if let Some(p) = RELEASING.with(|r| r.borrow_mut().remove(&name)) {
        let _ = JsFuture::from(p).await;
    }

    let mut release = None;
    let held = Promise::new(&mut |res, _| release = Some(res));
    let mut grant = None;
    let granted = Promise::new(&mut |res, _| grant = Some(res));
    let (release, grant) = (release.unwrap(), grant.unwrap());
    let callback = Closure::once_into_js(move |lock: JsValue| -> JsValue {
        let ok = !lock.is_null();
        let _ = grant.call1(&JsValue::UNDEFINED, &ok.into());
        if ok { held.into() } else { JsValue::UNDEFINED }
    });
    let options = Object::new();
    let _ = Reflect::set(&options, &"ifAvailable".into(), &true.into());

    let released = match request
        .call3(&locks, &name.clone().into(), &options, &callback)
        .and_then(|r| r.dyn_into::<Promise>())
    {
        Ok(released) => released,
        Err(e) => {
            warn(&e);
            return Ok(None);
        }
    };
    let guards = (
        WebLock {
            name,
            released: released.clone(),
        },
        LockRelease(release),
    );

    // The race also handles a rejected request.
    match JsFuture::from(Promise::race(&js_sys::Array::of2(&granted, &released))).await {
        Ok(v) if v.is_truthy() => Ok(Some(guards)),
        Ok(_) => Err(Error::WalletInUse),
        Err(e) => {
            warn(&e);
            Ok(None)
        }
    }
}

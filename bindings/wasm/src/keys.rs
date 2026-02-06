use wasm_bindgen::prelude::*;
use rgb_lib::{generate_keys as rust_generate_keys, restore_keys as rust_restore_keys};
use crate::error::RgbLibError;
use crate::utils::Network;

#[wasm_bindgen]
#[derive(Debug, Clone)]
pub struct Keys {
    account_xpub_vanilla: String,
    account_xpub_colored: String,
    mnemonic: String,
    master_fingerprint: String,
}

#[wasm_bindgen]
impl Keys {
    #[wasm_bindgen(constructor)]
    pub fn new(
        account_xpub_vanilla: String,
        account_xpub_colored: String,
        mnemonic: String,
        master_fingerprint: String,
    ) -> Keys {
        Keys {
            account_xpub_vanilla,
            account_xpub_colored,
            mnemonic,
            master_fingerprint,
        }
    }

    #[wasm_bindgen(getter)]
    pub fn account_xpub_vanilla(&self) -> String {
        self.account_xpub_vanilla.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn account_xpub_colored(&self) -> String {
        self.account_xpub_colored.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn mnemonic(&self) -> String {
        self.mnemonic.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn master_fingerprint(&self) -> String {
        self.master_fingerprint.clone()
    }
}

#[wasm_bindgen]
pub fn generate_keys(network: Network) -> Result<Keys, RgbLibError> {
    let rust_keys = rust_generate_keys(network.into());
    Ok(Keys {
        account_xpub_vanilla: rust_keys.account_xpub_vanilla,
        account_xpub_colored: rust_keys.account_xpub_colored,
        mnemonic: rust_keys.mnemonic,
        master_fingerprint: rust_keys.master_fingerprint,
    })
}

#[wasm_bindgen]
pub fn restore_keys(network: Network, mnemonic: String) -> Result<Keys, RgbLibError> {
    let rust_keys = rust_restore_keys(network.into(), mnemonic)
        .map_err(|e| RgbLibError::from(e))?;
    
    Ok(Keys {
        account_xpub_vanilla: rust_keys.account_xpub_vanilla,
        account_xpub_colored: rust_keys.account_xpub_colored,
        mnemonic: rust_keys.mnemonic,
        master_fingerprint: rust_keys.master_fingerprint,
    })
}

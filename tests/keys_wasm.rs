use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

use rgb_lib_wasm::{BitcoinNetwork, generate_keys, restore_keys};

#[wasm_bindgen_test]
fn test_generate_keys_all_networks() {
    for network in [
        BitcoinNetwork::Mainnet,
        BitcoinNetwork::Testnet,
        BitcoinNetwork::Regtest,
        BitcoinNetwork::Signet,
    ] {
        let keys = generate_keys(network);
        assert!(!keys.mnemonic.is_empty());
        assert!(!keys.xpub.is_empty());
        assert!(!keys.master_fingerprint.is_empty());
        assert!(!keys.account_xpub_vanilla.is_empty());
        assert!(!keys.account_xpub_colored.is_empty());
        assert_eq!(keys.mnemonic.split_whitespace().count(), 12);
    }
}

#[wasm_bindgen_test]
fn test_restore_keys_roundtrip() {
    let original = generate_keys(BitcoinNetwork::Regtest);
    let restored = restore_keys(BitcoinNetwork::Regtest, original.mnemonic.clone()).unwrap();
    assert_eq!(original.mnemonic, restored.mnemonic);
    assert_eq!(original.xpub, restored.xpub);
    assert_eq!(original.master_fingerprint, restored.master_fingerprint);
    assert_eq!(original.account_xpub_vanilla, restored.account_xpub_vanilla);
    assert_eq!(original.account_xpub_colored, restored.account_xpub_colored);
}

#[wasm_bindgen_test]
fn test_restore_keys_cross_network() {
    let mainnet_keys = generate_keys(BitcoinNetwork::Mainnet);
    let testnet_keys =
        restore_keys(BitcoinNetwork::Testnet, mainnet_keys.mnemonic.clone()).unwrap();
    assert_eq!(mainnet_keys.mnemonic, testnet_keys.mnemonic);
    // xpubs differ because derivation path includes coin type
    assert_ne!(mainnet_keys.xpub, testnet_keys.xpub);
}

#[wasm_bindgen_test]
fn test_restore_keys_invalid_mnemonic() {
    let result = restore_keys(
        BitcoinNetwork::Regtest,
        "invalid mnemonic words".to_string(),
    );
    assert!(result.is_err());
}

#[wasm_bindgen_test]
fn test_generate_keys_unique() {
    let keys1 = generate_keys(BitcoinNetwork::Regtest);
    let keys2 = generate_keys(BitcoinNetwork::Regtest);
    assert_ne!(keys1.mnemonic, keys2.mnemonic);
    assert_ne!(keys1.xpub, keys2.xpub);
}

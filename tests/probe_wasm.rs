use rgb_lib_wasm::wallet::{DatabaseType, Wallet, WalletData};
use rgb_lib_wasm::{AssetSchema, BitcoinNetwork, generate_keys};
use wasm_bindgen_test::*;
wasm_bindgen_test_configure!(run_in_browser);
mod utils;
use utils::*;

fn now() -> f64 {
    js_sys::Date::now() / 1000.0
}

#[wasm_bindgen_test]
async fn probe() {
    let mut out = String::new();
    for i in 0..6 {
        let t = now();
        mine_blocks(1).await;
        let m = now();
        wait_for_esplora_sync().await;
        out += &format!(
            "block {i}: mine {:.2}s sync-wait {:.2}s\n",
            m - t,
            now() - m
        );
    }
    let keys = generate_keys(BitcoinNetwork::Regtest);
    let mut wallet = Wallet::new(WalletData {
        data_dir: "/tmp/rgb_probe".to_string(),
        bitcoin_network: BitcoinNetwork::Regtest,
        database_type: DatabaseType::Sqlite,
        max_allocations_per_utxo: 5,
        account_xpub_vanilla: keys.account_xpub_vanilla.clone(),
        account_xpub_colored: keys.account_xpub_colored.clone(),
        mnemonic: Some(keys.mnemonic.clone()),
        master_fingerprint: keys.master_fingerprint.clone(),
        vanilla_keychain: None,
        supported_schemas: vec![AssetSchema::Nia],
        reuse_addresses: false,
    })
    .unwrap();
    let t = now();
    let online = wallet.go_online(go_online_options(false)).await.unwrap();
    out += &format!("go_online {:.2}s\n", now() - t);
    let t = now();
    fund_and_sync(&mut wallet, &online, "1.0").await;
    out += &format!("fund_and_sync {:.2}s\n", now() - t);
    for i in 0..3 {
        let t = now();
        create_utxos(&mut wallet, &online, false, Some(1), Some(1000)).await;
        out += &format!("create_utxos {i} {:.2}s\n", now() - t);
    }
    let t = now();
    wallet.sync(online.clone()).await.unwrap();
    out += &format!("sync {:.2}s\n", now() - t);
    let t = now();
    refresh(&mut wallet, &online).await;
    out += &format!("refresh {:.2}s\n", now() - t);
    panic!("PROBE\n{out}");
}

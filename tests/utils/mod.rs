use serde::{Deserialize, Serialize};

pub const REGTEST_HELPER_URL: &str = "http://127.0.0.1:8080";
pub const ESPLORA_URL: &str = "http://127.0.0.1:8094/regtest/api";
pub const PROXY_URL: &str = "http://127.0.0.1:3000/json-rpc";
pub const VSS_SERVER_URL: &str = "http://127.0.0.1:8082/vss";

#[derive(Serialize)]
struct MineRequest {
    blocks: u32,
}

#[derive(Deserialize)]
struct MineResponse {
    height: u64,
}

#[derive(Serialize)]
struct FundRequest {
    address: String,
    amount: String,
}

#[derive(Deserialize)]
struct FundResponse {
    txid: String,
    #[allow(dead_code)]
    height: u64,
}

#[derive(Deserialize)]
struct HeightResponse {
    height: u64,
}

pub async fn mine_blocks(n: u32) -> u64 {
    let client = reqwest::Client::new();
    let resp: MineResponse = client
        .post(format!("{}/mine", REGTEST_HELPER_URL))
        .json(&MineRequest { blocks: n })
        .send()
        .await
        .expect("mine request failed")
        .json()
        .await
        .expect("mine response parse failed");
    resp.height
}

pub async fn fund_address(addr: &str, amount: &str) -> String {
    let client = reqwest::Client::new();
    let resp: FundResponse = client
        .post(format!("{}/fund", REGTEST_HELPER_URL))
        .json(&FundRequest {
            address: addr.to_string(),
            amount: amount.to_string(),
        })
        .send()
        .await
        .expect("fund request failed")
        .json()
        .await
        .expect("fund response parse failed");
    resp.txid
}

pub async fn get_block_height() -> u64 {
    let client = reqwest::Client::new();
    let resp: HeightResponse = client
        .get(format!("{}/height", REGTEST_HELPER_URL))
        .send()
        .await
        .expect("height request failed")
        .json()
        .await
        .expect("height response parse failed");
    resp.height
}

pub async fn wait_for_esplora_sync() {
    let target = get_block_height().await;
    let client = reqwest::Client::new();
    // 120 iterations × 500ms = 60s timeout
    for _ in 0..120 {
        if let Ok(resp) = client
            .get(format!("{}/blocks/tip/height", ESPLORA_URL))
            .send()
            .await
        {
            if let Ok(text) = resp.text().await {
                if let Ok(h) = text.trim().parse::<u64>() {
                    if h >= target {
                        return;
                    }
                }
            }
        }
        sleep_ms(500).await;
    }
    panic!("Esplora did not sync to height {} within 60s", target);
}

pub async fn sleep_ms(ms: u32) {
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        web_sys::window()
            .expect("no window")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32)
            .expect("setTimeout failed");
    });
    wasm_bindgen_futures::JsFuture::from(promise).await.unwrap();
}

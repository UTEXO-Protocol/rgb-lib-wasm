use base64::{Engine, engine::general_purpose};
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

/// Fetch consignment bytes from the proxy server via JSON-RPC.
///
/// Returns the decoded consignment bytes and the txid from the proxy response.
#[allow(dead_code)]
pub async fn get_consignment_from_proxy(recipient_id: &str) -> (Vec<u8>, String) {
    #[derive(Serialize)]
    struct JsonRpcRequest<T: Serialize> {
        method: String,
        jsonrpc: String,
        id: Option<u64>,
        params: Option<T>,
    }

    #[derive(Serialize)]
    struct RecipientIDParam {
        recipient_id: String,
    }

    #[derive(Deserialize)]
    struct GetConsignmentResponse {
        consignment: String,
        txid: String,
    }

    #[derive(Deserialize)]
    struct JsonRpcResponse<T> {
        result: Option<T>,
    }

    let client = reqwest::Client::new();
    let body = JsonRpcRequest {
        method: "consignment.get".to_string(),
        jsonrpc: "2.0".to_string(),
        id: None,
        params: Some(RecipientIDParam {
            recipient_id: recipient_id.to_string(),
        }),
    };
    let resp: JsonRpcResponse<GetConsignmentResponse> = client
        .post(PROXY_URL)
        .json(&body)
        .send()
        .await
        .expect("proxy get_consignment request failed")
        .json()
        .await
        .expect("proxy get_consignment response parse failed");
    let result = resp.result.expect("no consignment found on proxy");
    let bytes = general_purpose::STANDARD
        .decode(&result.consignment)
        .expect("consignment base64 decode failed");
    (bytes, result.txid)
}

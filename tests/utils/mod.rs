use rgb_lib_wasm::wallet::{
    AssetFilter, IndexerConfig, Online, OnlineOptions, ReceiveData, Recipient, Transfer, Wallet,
    WitnessData,
};
use rgb_lib_wasm::{Assignment, Encryption, Error, TransferStatus};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;
use wasm_bindgen::JsValue;

pub const REGTEST_HELPER_URL: &str = "http://127.0.0.1:8080";
pub const ESPLORA_URL: &str = "http://127.0.0.1:8094/regtest/api";
pub const PROXY_URL: &str = "http://127.0.0.1:3000/json-rpc";
#[allow(dead_code)]
pub const PROXY_B_URL: &str = "http://127.0.0.1:3010/json-rpc";
#[allow(dead_code)]
pub const PROXY_AUTH_URL: &str = "http://127.0.0.1:3020/json-rpc";
#[allow(dead_code)]
pub const PROXY_AUTH_SEED: &str =
    "c01959890b0f215ff37637c58762a4eab0a2ec8be09c6a8436034c8f34b1c07e";
pub const VSS_SERVER_URL: &str = "http://127.0.0.1:8082/vss";
#[allow(dead_code)]
pub const EVM_RPC_URL: &str = "http://127.0.0.1:8545";

#[allow(dead_code)]
const DURATION_RCV_TRANSFER: u64 = 86400;
#[allow(dead_code)]
const DURATION_SEND_TRANSFER: u64 = 3600;

pub fn go_online_options(skip_consistency_check: bool) -> OnlineOptions {
    OnlineOptions {
        indexer_config: IndexerConfig::Esplora {
            url: ESPLORA_URL.to_string(),
        },
        skip_consistency_check,
        proxy_auth_keys: HashMap::new(),
        evm_chains: HashMap::new(),
    }
}

#[allow(dead_code)]
pub fn default_rcv_expiration() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64 + DURATION_RCV_TRANSFER
}

#[allow(dead_code)]
pub fn default_send_expiration() -> u64 {
    (js_sys::Date::now() / 1000.0) as u64 + DURATION_SEND_TRANSFER
}

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

thread_local! {
    // public key, expiration and encryption of the invoices created by the receive helpers
    static RECEIVES: RefCell<HashMap<String, (String, u64, Encryption)>> =
        RefCell::new(HashMap::new());
}

fn register_receive(data: &ReceiveData, encryption: Encryption) {
    RECEIVES.with(|r| {
        r.borrow_mut().insert(
            data.recipient_id.clone(),
            (
                data.public_key.clone(),
                data.expiration_timestamp,
                encryption,
            ),
        )
    });
}

#[allow(dead_code)]
pub async fn witness_rcv_enc(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: Option<String>,
    assignment: Assignment,
    expiration: u64,
    endpoints: Vec<String>,
    encryption: Option<Encryption>,
) -> Result<ReceiveData, Error> {
    let data = wallet
        .witness_receive(
            Some(online.clone()),
            asset_id,
            assignment,
            expiration,
            endpoints,
            1,
            encryption,
        )
        .await?;
    register_receive(&data, encryption.unwrap_or(Encryption::Required));
    Ok(data)
}

#[allow(dead_code)]
pub async fn witness_rcv(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: Option<String>,
    assignment: Assignment,
    expiration: u64,
    endpoints: Vec<String>,
) -> Result<ReceiveData, Error> {
    witness_rcv_enc(
        wallet, online, asset_id, assignment, expiration, endpoints, None,
    )
    .await
}

#[allow(dead_code)]
pub async fn blind_rcv_enc(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: Option<String>,
    assignment: Assignment,
    expiration: u64,
    endpoints: Vec<String>,
    encryption: Option<Encryption>,
) -> Result<ReceiveData, Error> {
    let data = wallet
        .blind_receive(
            Some(online.clone()),
            asset_id,
            assignment,
            expiration,
            endpoints,
            1,
            encryption,
        )
        .await?;
    register_receive(&data, encryption.unwrap_or(Encryption::Required));
    Ok(data)
}

#[allow(dead_code)]
pub async fn blind_rcv(
    wallet: &mut Wallet,
    online: &Online,
    asset_id: Option<String>,
    assignment: Assignment,
    expiration: u64,
    endpoints: Vec<String>,
) -> Result<ReceiveData, Error> {
    blind_rcv_enc(
        wallet, online, asset_id, assignment, expiration, endpoints, None,
    )
    .await
}

/// Install the interposer on `fetch`, which records the proxy calls.
#[allow(dead_code)]
pub fn spy_rpc() {
    js_sys::Function::new_no_args(
        "if (!globalThis.__rgbFetch) {
            globalThis.__rgbFetch = globalThis.fetch;
            globalThis.__rgbHooks = [];
            globalThis.__rgbCalls = [];
            globalThis.fetch = async function (input, init) {
                const self = this;
                const orig = () => globalThis.__rgbFetch.call(self, input, init);
                const url = input instanceof Request ? input.url : String(input);
                const req = input instanceof Request ? input.clone() : null;
                let method = null;
                let params = null;
                try {
                    const body = init && init.body;
                    const parse = (fd) => {
                        method = String(fd.get('method') || '');
                        params = JSON.parse(String(fd.get('params') || 'null'));
                    };
                    if (body instanceof FormData) {
                        parse(body);
                    } else if (typeof body === 'string') {
                        const j = JSON.parse(body);
                        method = j.method;
                        params = j.params;
                    } else if (req && req.method === 'POST') {
                        const ct = req.headers.get('content-type') || '';
                        if (ct.includes('multipart')) {
                            parse(await req.formData());
                        } else {
                            const j = await req.json();
                            method = j.method;
                            params = j.params;
                        }
                    }
                } catch (e) {
                    method = null;
                }
                if (method === null) {
                    const reason = globalThis.__rgbBroadcastRejection;
                    if (!reason || !url.endsWith('/tx')) return orig();
                    const resp = new Response(reason, { status: 400 });
                    Object.defineProperty(resp, 'url', { value: url });
                    return resp;
                }
                globalThis.__rgbCalls.push({ url, method, params });
                const hook = globalThis.__rgbHooks.find(
                    (h) => url.includes(h.url) && h.method === method,
                );
                if (!hook) return orig();
                if (hook.before) {
                    hook.fn(params, null);
                    return orig();
                }
                const res = await orig();
                const json = await res.clone().json();
                const out = hook.fn(params, json);
                const resp = new Response(JSON.stringify(out), {
                    status: res.status,
                    headers: { 'content-type': 'application/json' },
                });
                // reqwest parses the response URL.
                Object.defineProperty(resp, 'url', { value: res.url });
                return resp;
            };
        }",
    )
    .call0(&JsValue::NULL)
    .unwrap();
}

// `before` hooks throw a network error before the call reaches the proxy, the others transform the
// parsed response of the proxy: `js` is the body of `(req, res) => res`, where `req` is the parsed
// params and `res` the parsed response, and it may throw a `TypeError` to simulate a network
// failure after the proxy handled the call.
fn rpc_hook_impl(url_part: &str, method: &str, js: &str, before: bool) {
    spy_rpc();
    let hook = js_sys::Object::new();
    js_sys::Reflect::set(&hook, &"url".into(), &url_part.into()).unwrap();
    js_sys::Reflect::set(&hook, &"method".into(), &method.into()).unwrap();
    js_sys::Reflect::set(&hook, &"before".into(), &before.into()).unwrap();
    js_sys::Reflect::set(
        &hook,
        &"fn".into(),
        &js_sys::Function::new_with_args("req, res", js),
    )
    .unwrap();
    let hooks = js_sys::Reflect::get(&js_sys::global(), &"__rgbHooks".into()).unwrap();
    let push: js_sys::Function = js_sys::Reflect::get(&hooks, &"push".into()).unwrap().into();
    push.call1(&hooks, &hook).unwrap();
}

/// Transform the responses of the `method` calls to the proxy at `url_part`.
#[allow(dead_code)]
pub fn rpc_hook(url_part: &str, method: &str, js: &str) {
    rpc_hook_impl(url_part, method, js, false);
}

/// Make the `method` calls to the proxy at `url_part` fail with a network error before they reach
/// the proxy, until [`clear_rpc_hooks`].
#[allow(dead_code)]
pub fn rpc_fail(url_part: &str, method: &str) {
    rpc_hook_impl(
        url_part,
        method,
        "throw new TypeError('injected failure')",
        true,
    );
}

/// Make the indexer reject TX broadcasts with `reason`, until [`clear_rpc_hooks`].
#[allow(dead_code)]
pub fn reject_broadcasts(reason: &str) {
    spy_rpc();
    js_sys::Reflect::set(
        &js_sys::global(),
        &"__rgbBroadcastRejection".into(),
        &reason.into(),
    )
    .unwrap();
}

/// Remove the hooks and forget the calls seen so far.
#[allow(dead_code)]
pub fn clear_rpc_hooks() {
    js_sys::Reflect::set(
        &js_sys::global(),
        &"__rgbBroadcastRejection".into(),
        &JsValue::UNDEFINED,
    )
    .unwrap();
    if let Ok(hooks) = js_sys::Reflect::get(&js_sys::global(), &"__rgbHooks".into()) {
        if !hooks.is_undefined() {
            js_sys::Reflect::set(&hooks, &"length".into(), &0.into()).unwrap();
            let calls = js_sys::Reflect::get(&js_sys::global(), &"__rgbCalls".into()).unwrap();
            js_sys::Reflect::set(&calls, &"length".into(), &0.into()).unwrap();
        }
    }
}

fn rpc_calls(method: &str) -> Vec<JsValue> {
    let calls = js_sys::Reflect::get(&js_sys::global(), &"__rgbCalls".into()).unwrap();
    if calls.is_undefined() {
        return vec![];
    }
    js_sys::Array::from(&calls)
        .iter()
        .filter(|c| {
            js_sys::Reflect::get(c, &"method".into())
                .unwrap()
                .as_string()
                .as_deref()
                == Some(method)
        })
        .collect()
}

/// The params of the last `method` call seen.
#[allow(dead_code)]
pub fn rpc_capture(method: &str) -> JsValue {
    let call = rpc_calls(method).pop().expect("no such call seen");
    js_sys::Reflect::get(&call, &"params".into()).unwrap()
}

/// The number of `method` calls seen since the last [`clear_rpc_hooks`].
#[allow(dead_code)]
pub fn rpc_count(method: &str) -> u32 {
    rpc_calls(method).len() as u32
}

/// Call a JSON-RPC method of a proxy directly.
#[allow(dead_code)]
pub async fn rpc_post(url: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
    reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": null,
            "method": method,
            "params": params,
        }))
        .send()
        .await
        .expect("proxy request failed")
        .json()
        .await
        .expect("proxy response parse failed")
}

#[allow(dead_code)]
pub async fn fund_and_sync(wallet: &mut Wallet, online: &Online, amount: &str) {
    let addr = wallet.get_address().unwrap();
    fund_address(&addr, amount).await;
    wait_for_esplora_sync().await;
    wallet.sync(online.clone()).await.unwrap();
}

#[allow(dead_code)]
pub fn transport_endpoint(url: &str) -> String {
    format!("rpc://{}", url.trim_start_matches("http://"))
}

#[allow(dead_code)]
pub fn recipient(id: &str, amount: u64, sat: Option<u64>, url: &str) -> Recipient {
    let (public_key, expiration, encryption) = RECEIVES
        .with(|r| r.borrow().get(id).cloned())
        .expect("the receive was not created with a receive helper");
    Recipient {
        recipient_id: id.to_string(),
        witness_data: sat.map(|amount_sat| WitnessData {
            amount_sat,
            blinding: None,
        }),
        assignment: Assignment::Fungible(amount),
        transport_endpoints: vec![transport_endpoint(url)],
        public_key: Some(public_key),
        encryption,
        expiration_timestamp: Some(expiration),
    }
}

#[allow(dead_code)]
pub fn transfer_of(wallet: &Wallet, recipient_id: &str) -> Transfer {
    wallet
        .list_transfers(AssetFilter::Any, None)
        .unwrap()
        .into_iter()
        .find(|t| t.recipient_id.as_deref() == Some(recipient_id))
        .unwrap()
}

#[allow(dead_code)]
pub async fn refresh(wallet: &mut Wallet, online: &Online) {
    wallet
        .refresh(online.clone(), None, vec![], false)
        .await
        .unwrap();
}

#[allow(dead_code)]
pub async fn settle(wallet: &mut Wallet, online: &Online, recipient_ids: &[String]) {
    for _ in 0..5 {
        mine_blocks(1).await;
        wait_for_esplora_sync().await;
        wallet.sync(online.clone()).await.unwrap();
        refresh(wallet, online).await;
        if recipient_ids
            .iter()
            .all(|r| transfer_of(wallet, r).status == TransferStatus::Settled)
        {
            return;
        }
        sleep_ms(1000).await;
    }
    let statuses: Vec<_> = recipient_ids
        .iter()
        .map(|r| transfer_of(wallet, r).status)
        .collect();
    panic!("transfers did not settle: {statuses:?}");
}

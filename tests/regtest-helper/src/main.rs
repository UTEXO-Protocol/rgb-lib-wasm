use std::sync::Arc;

use axum::{Json, Router, extract::State, http::StatusCode, routing::{get, post}};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

struct AppState {
    rpc_url: String,
    rpc_user: String,
    rpc_pass: String,
    client: reqwest::Client,
    miner_address: Mutex<Option<String>>,
}

#[derive(Serialize)]
struct RpcRequest {
    jsonrpc: &'static str,
    id: &'static str,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse {
    result: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

impl AppState {
    async fn rpc_call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let req = RpcRequest {
            jsonrpc: "1.0",
            id: "helper",
            method: method.to_string(),
            params,
        };
        let resp = self
            .client
            .post(&self.rpc_url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_pass))
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let body: RpcResponse = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {e}"))?;

        if let Some(err) = body.error {
            return Err(format!("RPC error: {err}"));
        }
        body.result.ok_or_else(|| "null result".to_string())
    }

    async fn rpc_call_wallet(
        &self,
        wallet: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let url = format!("{}/wallet/{}", self.rpc_url, wallet);
        let req = RpcRequest {
            jsonrpc: "1.0",
            id: "helper",
            method: method.to_string(),
            params,
        };
        let resp = self
            .client
            .post(&url)
            .basic_auth(&self.rpc_user, Some(&self.rpc_pass))
            .json(&req)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let body: RpcResponse = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {e}"))?;

        if let Some(err) = body.error {
            return Err(format!("RPC error: {err}"));
        }
        body.result.ok_or_else(|| "null result".to_string())
    }

    async fn ensure_miner_wallet(&self) -> Result<String, String> {
        let mut addr_lock = self.miner_address.lock().await;
        if let Some(addr) = addr_lock.as_ref() {
            return Ok(addr.clone());
        }

        // Try to load existing miner wallet, create if not found
        let load_result = self
            .rpc_call("loadwallet", serde_json::json!(["miner"]))
            .await;
        if load_result.is_err() {
            // Wallet doesn't exist or already loaded — try create
            let create_result = self
                .rpc_call(
                    "createwallet",
                    serde_json::json!(["miner"]),
                )
                .await;
            // Ignore "already exists/loaded" errors
            if let Err(e) = &create_result {
                if !e.contains("already exists") && !e.contains("already loaded") {
                    return Err(format!("Failed to create miner wallet: {e}"));
                }
            }
        }

        let addr_val = self
            .rpc_call_wallet("miner", "getnewaddress", serde_json::json!([]))
            .await?;
        let addr = addr_val
            .as_str()
            .ok_or("getnewaddress didn't return string")?
            .to_string();
        *addr_lock = Some(addr.clone());
        Ok(addr)
    }
}

// --- Handlers ---

#[derive(Serialize)]
struct StatusResponse {
    ok: bool,
}

async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let ok = state
        .rpc_call("getblockchaininfo", serde_json::json!([]))
        .await
        .is_ok();
    Json(StatusResponse { ok })
}

#[derive(Serialize)]
struct HeightResponse {
    height: u64,
}

async fn height_handler(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HeightResponse>, (StatusCode, String)> {
    let result = state
        .rpc_call("getblockcount", serde_json::json!([]))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let height = result.as_u64().unwrap_or(0);
    Ok(Json(HeightResponse { height }))
}

#[derive(Deserialize)]
struct MineRequest {
    blocks: u32,
}

#[derive(Serialize)]
struct MineResponse {
    height: u64,
}

async fn mine_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MineRequest>,
) -> Result<Json<MineResponse>, (StatusCode, String)> {
    let miner_addr = state
        .ensure_miner_wallet()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    state
        .rpc_call_wallet(
            "miner",
            "generatetoaddress",
            serde_json::json!([req.blocks, miner_addr]),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let height_val = state
        .rpc_call("getblockcount", serde_json::json!([]))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let height = height_val.as_u64().unwrap_or(0);

    Ok(Json(MineResponse { height }))
}

#[derive(Deserialize)]
struct FundRequest {
    address: String,
    #[serde(default = "default_amount")]
    amount: String,
}

fn default_amount() -> String {
    "1.0".to_string()
}

#[derive(Serialize)]
struct FundResponse {
    txid: String,
    height: u64,
}

async fn fund_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FundRequest>,
) -> Result<Json<FundResponse>, (StatusCode, String)> {
    let miner_addr = state
        .ensure_miner_wallet()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Parse amount as f64 for the RPC call
    let amount: f64 = req
        .amount
        .parse()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid amount: {e}")))?;

    // Send BTC from miner wallet to the target address
    let txid_val = state
        .rpc_call_wallet(
            "miner",
            "sendtoaddress",
            serde_json::json!([req.address, amount]),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let txid = txid_val.as_str().unwrap_or("").to_string();

    // Mine 1 block to confirm the transaction
    state
        .rpc_call_wallet(
            "miner",
            "generatetoaddress",
            serde_json::json!([1, miner_addr]),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let height_val = state
        .rpc_call("getblockcount", serde_json::json!([]))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let height = height_val.as_u64().unwrap_or(0);

    Ok(Json(FundResponse { txid, height }))
}

#[tokio::main]
async fn main() {
    let rpc_url =
        std::env::var("BITCOIND_RPC_URL").unwrap_or_else(|_| "http://bitcoind:18443".to_string());
    let rpc_user =
        std::env::var("BITCOIND_RPC_USER").unwrap_or_else(|_| "user".to_string());
    let rpc_pass =
        std::env::var("BITCOIND_RPC_PASS").unwrap_or_else(|_| "default_password".to_string());

    let state = Arc::new(AppState {
        rpc_url,
        rpc_user,
        rpc_pass,
        client: reqwest::Client::new(),
        miner_address: Mutex::new(None),
    });

    let app = Router::new()
        .route("/status", get(status_handler))
        .route("/height", get(height_handler))
        .route("/mine", post(mine_handler))
        .route("/fund", post(fund_handler))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080")
        .await
        .expect("failed to bind to 0.0.0.0:8080");

    println!("regtest-helper listening on 0.0.0.0:8080");
    axum::serve(listener, app).await.expect("server error");
}

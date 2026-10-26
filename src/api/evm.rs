use super::*;

use reqwest::StatusCode;
use std::cell::RefCell;

/// keccak256("FundsIn(address,uint256,uint64)")
const FUNDS_IN_TOPIC: &str = "0xf1a18caea297591892fc07ea412a5e617d8e51e1155912d8871793e1d4e70f87";

/// Selector of `length()` on a reject list contract (`IRgbRejectList`)
pub(crate) const REJECT_LIST_LENGTH_SELECTOR: &str = "0x1f7b6d32";
/// Selector of `entries(uint256,uint256)` on a reject list contract (`IRgbRejectList`)
pub(crate) const REJECT_LIST_ENTRIES_SELECTOR: &str = "0x17b937e6";
/// Maximum number of reject list entries requested with a single `entries` call
const REJECT_LIST_PAGE_SIZE: u64 = 1000;
/// Maximum number of blocks whose logs are requested with a single `eth_getLogs` call
const LOGS_BLOCK_RANGE: u64 = 1000;
/// Number of times a JSON-RPC request is sent again when it fails for a temporary reason
const RPC_RETRIES: u32 = 2;
/// Pause before a JSON-RPC request is sent again, multiplied by the number of the retry
#[cfg(not(test))]
const RPC_RETRY_PAUSE: Duration = Duration::from_secs(1);
#[cfg(test)]
const RPC_RETRY_PAUSE: Duration = Duration::from_millis(10);
/// Maximum number of blocks scanned for a mint event after the block the mint commits to
const MAX_MINT_SCAN_BLOCKS: u64 = 500_000;

#[cfg(test)]
thread_local! {
    pub(crate) static MOCK_MAX_MINT_SCAN_BLOCKS: RefCell<Option<u64>> = const { RefCell::new(None) };
}

/// Maximum number of blocks scanned for a mint event (see [`MAX_MINT_SCAN_BLOCKS`]).
pub(crate) fn max_mint_scan_blocks() -> u64 {
    #[cfg(test)]
    if let Some(max_scan_blocks) = MOCK_MAX_MINT_SCAN_BLOCKS.with_borrow(|m| *m) {
        return max_scan_blocks;
    }
    MAX_MINT_SCAN_BLOCKS
}

const NO_PARAMS: [u8; 0] = [];

/// JSON-RPC client for an EVM node.
pub(crate) struct EvmClient {
    client: reqwest::Client,
    rpc_url: String,
}

/// A single log entry returned by `eth_getLogs`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvmLog {
    /// Indexed topic hashes (topic[0] = event signature hash).
    pub topics: Vec<String>,
    /// ABI-encoded non-indexed parameters.
    pub data: String,
    /// Block number (hex).
    pub block_number: Option<String>,
}

/// Decoded FundsIn event from the bridge contract.
#[derive(Debug)]
pub(crate) struct FundsInEvent {
    /// Locked amount.
    pub amount: u64,
    /// RGB operation ID (32 bytes).
    pub operation_id: [u8; 32],
    /// Block the event was emitted in, if reported by the node.
    pub block_number: Option<u64>,
}

impl FundsInEvent {
    /// Whether this event commits to `opid` with the exact `amount` and follows `after_block`.
    /// An event with no block number never confirms.
    pub(crate) fn confirms(&self, opid: &OpId, amount: u64, after_block: u64) -> bool {
        OpId::from(self.operation_id) == *opid
            && self.amount == amount
            && self.block_number.is_some_and(|block| block > after_block)
    }
}

fn strip_hex_prefix<'a>(value: &'a str, what: &str) -> Result<&'a str, Error> {
    value.strip_prefix("0x").ok_or_else(|| Error::Network {
        details: format!("invalid EVM {what} {value}: missing 0x prefix"),
    })
}

fn parse_quantity(value: &str, what: &str) -> Result<u64, Error> {
    u64::from_str_radix(strip_hex_prefix(value, what)?, 16).map_err(|e| Error::Network {
        details: format!("invalid EVM {what} {value}: {e}"),
    })
}

fn parse_block_number(hex_block: &str) -> Result<u64, Error> {
    parse_quantity(hex_block, "block number")
}

/// Decode the ABI word (32 bytes) at word `index` of the `0x`-prefixed hex `data`.
fn abi_word(data: &str, index: usize) -> Result<[u8; 32], Error> {
    // slice bytes: a str slice can split a multi-byte char and panic
    let hex = strip_hex_prefix(data, "ABI data")?.as_bytes();
    let start = index * 64;
    let end = start + 64;
    if hex.len() < end {
        return Err(Error::Network {
            details: format!("ABI data too short: expected at least {end} hex chars"),
        });
    }
    let mut buf = [0u8; 32];
    hex::decode_to_slice(&hex[start..end], &mut buf).map_err(|e| Error::Network {
        details: format!("ABI hex decode error: {e}"),
    })?;
    Ok(buf)
}

/// Decode the ABI word at word `index` as a `u64`. Return `None` if it does not fit.
fn abi_word_u64(data: &str, index: usize) -> Result<Option<u64>, Error> {
    let word = abi_word(data, index)?;
    if word[..24].iter().any(|b| *b != 0) {
        return Ok(None);
    }
    Ok(Some(u64::from_be_bytes(word[24..32].try_into().unwrap())))
}

/// An entry of a reject list kept by an EVM contract (`IRgbRejectList.Entry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RejectListEntry {
    pub opout: Opout,
    /// Whether the opout is rejected (`true`) or allowed (`false`)
    pub reject: bool,
}

/// The entries of a reject list kept by an EVM contract, in append order.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RejectListLog {
    /// All the entries, as of the latest block
    pub entries: Vec<RejectListEntry>,
    /// How many of the leading entries are in finalized blocks
    pub finalized: usize,
}

impl RejectListLog {
    /// The opouts currently rejected and allowed, as `(reject, allow)`.
    ///
    /// The latest finalized entry for an opout wins. A reject counts as soon as it is seen. An
    /// allow counts only when it is finalized and no later entry rejects the opout.
    pub(crate) fn decisions(&self) -> (HashSet<Opout>, HashSet<Opout>) {
        let (finalized, pending) = self.entries.split_at(self.finalized);
        let finalized = finalized
            .iter()
            .map(|e| (e.opout, e.reject))
            .collect::<HashMap<_, _>>();
        let mut reject = pending
            .iter()
            .filter(|e| e.reject)
            .map(|e| e.opout)
            .collect::<HashSet<_>>();
        let mut allow = HashSet::new();
        for (opout, rejected) in finalized {
            if rejected {
                reject.insert(opout);
            } else if !reject.contains(&opout) {
                allow.insert(opout);
            }
        }
        (reject, allow)
    }
}

fn abi_error(details: &str) -> Error {
    Error::Network {
        details: format!("invalid ABI data: {details}"),
    }
}

fn abi_word_u16(data: &str, index: usize) -> Result<u16, Error> {
    abi_word_u64(data, index)?
        .and_then(|v| u16::try_from(v).ok())
        .ok_or_else(|| abi_error("bad uint16"))
}

/// Decode the result of `entries(uint256,uint256)`, an ABI-encoded
/// `(bytes32, uint16, uint16, bool)[]`: the array offset, its length, then four words per entry.
fn decode_reject_list_entries(data: &str) -> Result<Vec<RejectListEntry>, Error> {
    let offset = abi_word_u64(data, 0)?
        .filter(|o| o % 32 == 0)
        .ok_or_else(|| abi_error("bad array offset"))?;
    let len_index = (offset / 32) as usize;
    let len = abi_word_u64(data, len_index)?.ok_or_else(|| abi_error("bad array length"))?;
    let mut entries = vec![];
    for i in 0..len as usize {
        let base = len_index + 1 + 4 * i;
        let opout = Opout {
            op: OpId::from(abi_word(data, base)?),
            ty: AssignmentType::with(abi_word_u16(data, base + 1)?),
            no: abi_word_u16(data, base + 2)?,
        };
        let reject = match abi_word_u64(data, base + 3)? {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(abi_error("bad bool")),
        };
        entries.push(RejectListEntry { opout, reject });
    }
    Ok(entries)
}

impl EvmLog {
    /// Parse this log as a FundsIn event.
    ///
    /// Return `None` if the topic does not match or if the amount does not fit in a `u64`.
    pub fn as_funds_in(&self) -> Result<Option<FundsInEvent>, Error> {
        let Some(topic0) = self.topics.first() else {
            return Ok(None);
        };
        if !topic0.eq_ignore_ascii_case(FUNDS_IN_TOPIC) {
            return Ok(None);
        }
        let block_number = self
            .block_number
            .as_deref()
            .map(parse_block_number)
            .transpose()?;
        // event FundsIn(address indexed sender, uint256 indexed operationId, uint64 amount)
        let [_, _, operation_id_topic] = self.topics.as_slice() else {
            return Err(Error::Network {
                details: format!(
                    "unexpected FundsIn topic count: {} (expected 3)",
                    self.topics.len()
                ),
            });
        };
        let data_hex = strip_hex_prefix(&self.data, "ABI data")?;
        if data_hex.len() != 64 {
            return Err(Error::Network {
                details: format!(
                    "unexpected FundsIn ABI payload size: {} hex chars (expected 64)",
                    data_hex.len()
                ),
            });
        }
        let operation_id = abi_word(operation_id_topic, 0)?;
        let Some(amount) = abi_word_u64(&self.data, 0)? else {
            return Ok(None);
        };
        Ok(Some(FundsInEvent {
            amount,
            operation_id,
            block_number,
        }))
    }
}

#[derive(Debug, Serialize)]
struct RpcRequest<P: Serialize> {
    jsonrpc: &'static str,
    method: &'static str,
    params: P,
    id: u64,
}

#[derive(Debug, Deserialize)]
struct RpcResponse<R> {
    result: Option<R>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

/// Transaction call object for `eth_call`.
#[derive(Debug, Serialize)]
struct CallObject {
    to: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct BlockHeader {
    number: String,
    #[serde(default)]
    timestamp: Option<String>,
}

/// Filter object for `eth_getLogs`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LogFilter {
    address: String,
    from_block: String,
    to_block: String,
    topics: Vec<Option<String>>,
}

impl EvmClient {
    pub(crate) fn new(rpc_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            rpc_url: rpc_url.to_string(),
        }
    }

    fn req_err(e: reqwest::Error) -> Error {
        Error::Network {
            details: format!("EVM RPC: {e}"),
        }
    }

    /// Post a JSON-RPC request. Send it again, up to [`RPC_RETRIES`] times after a pause, when it
    /// cannot be sent or the node is temporarily unavailable (HTTP 502, 503 or 504). A timed out
    /// request is not sent again.
    async fn post(&self, body: &impl Serialize) -> Result<reqwest::Response, Error> {
        let mut retries = 0;
        loop {
            let res = self.client.post(&self.rpc_url).json(body).send().await;
            let temporary = match &res {
                Ok(resp) => matches!(
                    resp.status(),
                    StatusCode::BAD_GATEWAY
                        | StatusCode::SERVICE_UNAVAILABLE
                        | StatusCode::GATEWAY_TIMEOUT
                ),
                Err(e) => !e.is_timeout() && !e.is_builder(),
            };
            if !temporary || retries == RPC_RETRIES {
                return res.map_err(Self::req_err);
            }
            retries += 1;
            let pause = RPC_RETRY_PAUSE * retries;
            #[cfg(target_arch = "wasm32")]
            {
                // setTimeout of the global scope, so that it also works in a worker
                let promise = js_sys::Promise::new(&mut |resolve, _| {
                    let set_timeout: js_sys::Function =
                        js_sys::Reflect::get(&js_sys::global(), &"setTimeout".into())
                            .expect("setTimeout")
                            .into();
                    set_timeout
                        .call2(
                            &wasm_bindgen::JsValue::NULL,
                            &resolve,
                            &(pause.as_millis() as f64).into(),
                        )
                        .expect("setTimeout call");
                });
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            }
            #[cfg(not(target_arch = "wasm32"))]
            std::thread::sleep(pause);
        }
    }

    async fn call<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<RpcResponse<R>, Error> {
        let body = RpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: 1,
        };
        self.post(&body).await?.json().await.map_err(Self::req_err)
    }

    /// Call a JSON-RPC method. Fail if it reports an error or no result.
    async fn rpc<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, Error> {
        let resp: RpcResponse<R> = self.call(method, params).await?;
        if let Some(err) = resp.error {
            return Err(Error::Network {
                details: format!("{method} error {}: {}", err.code, err.message),
            });
        }
        resp.result.ok_or_else(|| Error::Network {
            details: format!("{method} returned null result"),
        })
    }

    /// EIP-155 chain ID of the chain the node is on (`eth_chainId`).
    pub(crate) async fn chain_id(&self) -> Result<u64, Error> {
        parse_quantity(
            &self.rpc::<_, String>("eth_chainId", NO_PARAMS).await?,
            "chain ID",
        )
    }

    async fn block_number(&self) -> Result<u64, Error> {
        parse_block_number(&self.rpc::<_, String>("eth_blockNumber", NO_PARAMS).await?)
    }

    /// Number of the latest finalized block.
    pub(crate) async fn finalized_block_number(&self) -> Result<u64, Error> {
        let header: BlockHeader = self
            .rpc("eth_getBlockByNumber", ("finalized", false))
            .await?;
        parse_block_number(&header.number)
    }

    /// The `FundsIn` logs for `opid` emitted by `contract` from `from_block` to `to_block`.
    ///
    /// A node error is returned apart, so that the caller can retry with a smaller range. An
    /// HTTP 413 is such an error too, whatever its body. An error with HTTP 429, 401 or 403 is a
    /// failure instead, as a smaller range is refused too.
    async fn get_logs(
        &self,
        contract: &EvmAddress,
        opid: &OpId,
        from_block: u64,
        to_block: u64,
    ) -> Result<Result<Vec<EvmLog>, RpcError>, Error> {
        let body = RpcRequest {
            jsonrpc: "2.0",
            method: "eth_getLogs",
            params: [LogFilter {
                address: contract.to_string(),
                from_block: format!("0x{from_block:x}"),
                to_block: format!("0x{to_block:x}"),
                topics: vec![
                    Some(FUNDS_IN_TOPIC.to_string()),
                    None,
                    Some(format!("0x{}", hex::encode(opid.to_byte_array()))),
                ],
            }],
            id: 1,
        };
        let resp = self.post(&body).await?;
        let status = resp.status();
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            let text = resp.text().await.unwrap_or_default();
            let details: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
            return Ok(Err(RpcError {
                code: status.as_u16().into(),
                message: format!(
                    "(HTTP {status}) {}",
                    details.chars().take(200).collect::<String>()
                ),
            }));
        }
        let resp: RpcResponse<Vec<EvmLog>> = resp.json().await.map_err(Self::req_err)?;
        if let Some(err) = resp.error {
            if matches!(
                status,
                StatusCode::TOO_MANY_REQUESTS | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
            ) {
                return Err(Error::Network {
                    details: format!(
                        "eth_getLogs error {} (HTTP {status}): {}",
                        err.code, err.message
                    ),
                });
            }
            return Ok(Err(err));
        }
        resp.result.map(Ok).ok_or_else(|| Error::Network {
            details: s!("eth_getLogs returned null result"),
        })
    }

    /// Check that the node serves the latest finalized block and the logs of the oldest blocks.
    /// Fail with [`Error::InvalidEvmRpcUrl`] if it does not.
    pub(crate) async fn check_mint_support(&self) -> Result<(), Error> {
        self.finalized_block_number()
            .await
            .map_err(|e| Error::InvalidEvmRpcUrl {
                details: format!("cannot get the latest finalized block: {e}"),
            })?;
        let logs_error = |details: String| Error::InvalidEvmRpcUrl {
            details: format!(
                "cannot get the logs of the oldest blocks, an endpoint serving the whole log \
                 history is needed: {details}"
            ),
        };
        match self
            .get_logs(&EvmAddress::default(), &OpId::from([0u8; 32]), 0, 0)
            .await
        {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(err)) => Err(logs_error(format!(
                "eth_getLogs error {}: {}",
                err.code, err.message
            ))),
            Err(e) => Err(logs_error(e.to_string())),
        }
    }

    /// Whether `contract` emitted a `FundsIn` event for `opid` with the exact `amount`, after
    /// `after_block` and up to `last_block`.
    pub(crate) async fn find_funds_in(
        &self,
        contract: &EvmAddress,
        opid: &OpId,
        amount: u64,
        after_block: u64,
        last_block: u64,
    ) -> Result<bool, Error> {
        Ok(self
            .find_funds_in_block(contract, opid, amount, after_block, after_block, last_block)
            .await?
            .is_some())
    }

    /// The block of the first `FundsIn` event for `opid` with the exact `amount` after
    /// `scanned_block` and up to `last_block` that follows `after_block`, if any.
    /// A log range that the node refuses is halved and requested again.
    async fn find_funds_in_block(
        &self,
        contract: &EvmAddress,
        opid: &OpId,
        amount: u64,
        after_block: u64,
        scanned_block: u64,
        last_block: u64,
    ) -> Result<Option<u64>, Error> {
        let mut block_range = LOGS_BLOCK_RANGE;
        let mut from_block = scanned_block.saturating_add(1);
        while from_block <= last_block {
            let to_block = min(from_block.saturating_add(block_range - 1), last_block);
            let logs = match self.get_logs(contract, opid, from_block, to_block).await? {
                Ok(logs) => logs,
                Err(_) if to_block > from_block => {
                    block_range = (to_block - from_block).div_ceil(2);
                    continue;
                }
                Err(err) => {
                    return Err(Error::Network {
                        details: format!("eth_getLogs error {}: {}", err.code, err.message),
                    });
                }
            };
            for log in logs {
                if let Some(event) = log.as_funds_in()?
                    && event.confirms(opid, amount, after_block)
                {
                    return Ok(event.block_number);
                }
            }
            let Some(next_block) = to_block.checked_add(1) else {
                break;
            };
            from_block = next_block;
        }
        Ok(None)
    }

    /// Timestamp of the block including a `FundsIn` event for `opid` with the exact `amount`
    /// after `after_block`, finalized or not, if any.
    pub(crate) async fn funds_in_inclusion(
        &self,
        contract: &EvmAddress,
        opid: &OpId,
        amount: u64,
        after_block: u64,
    ) -> Result<Option<u64>, Error> {
        let latest_block = self.block_number().await?;
        let Some(block) = self
            .find_funds_in_block(
                contract,
                opid,
                amount,
                after_block,
                after_block,
                latest_block,
            )
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(self.block_timestamp(block).await?))
    }

    /// Timestamp of the block with the given number.
    pub(crate) async fn block_timestamp(&self, block: u64) -> Result<u64, Error> {
        let header: BlockHeader = self
            .rpc("eth_getBlockByNumber", (format!("0x{block:x}"), false))
            .await?;
        let timestamp = header.timestamp.ok_or_else(|| Error::Network {
            details: format!("the node reports no timestamp for block {block}"),
        })?;
        parse_quantity(&timestamp, "block timestamp")
    }

    async fn eth_call(
        &self,
        contract: &EvmAddress,
        data: String,
        block: u64,
    ) -> Result<String, Error> {
        let call = CallObject {
            to: contract.to_string(),
            data,
        };
        self.rpc("eth_call", (call, format!("0x{block:x}"))).await
    }

    /// Length of the reject list at `contract` as of `block`. Return `None` if there is no
    /// contract there.
    async fn reject_list_length(
        &self,
        contract: &EvmAddress,
        block: u64,
    ) -> Result<Option<u64>, Error> {
        let len_data = self
            .eth_call(contract, REJECT_LIST_LENGTH_SELECTOR.to_string(), block)
            .await?;
        if len_data == "0x" {
            return Ok(None);
        }
        abi_word_u64(&len_data, 0)?
            .map(Some)
            .ok_or_else(|| abi_error("reject list length doesn't fit in a u64"))
    }

    /// All the entries of the `IRgbRejectList` contract at `contract`, in append order.
    ///
    /// The list is append-only. So the entries as of the latest finalized block are the leading
    /// entries as of the latest block. Only the length is read at the finalized block.
    pub(crate) async fn reject_list_entries(
        &self,
        contract: &EvmAddress,
    ) -> Result<RejectListLog, Error> {
        let block = self.block_number().await?;
        let finalized_block = min(self.finalized_block_number().await?, block);
        // a contract deployed in a non-finalized block has no finalized entries
        let finalized_len = self
            .reject_list_length(contract, finalized_block)
            .await?
            .unwrap_or(0);
        let len = self
            .reject_list_length(contract, block)
            .await?
            .ok_or_else(|| Error::Network {
                details: format!("no contract at the reject list address {contract}"),
            })?;
        if finalized_len > len {
            return Err(Error::Network {
                details: s!("reject list shorter at the latest block than at the finalized one"),
            });
        }
        if len > MAX_REJECT_LIST_ENTRIES {
            return Err(Error::Network {
                details: format!(
                    "reject list too long: {len} entries, at most {MAX_REJECT_LIST_ENTRIES} supported"
                ),
            });
        }
        let mut entries = vec![];
        let mut start = 0;
        while start < len {
            let end = min(start.saturating_add(REJECT_LIST_PAGE_SIZE), len);
            let data = format!("{REJECT_LIST_ENTRIES_SELECTOR}{start:064x}{end:064x}");
            let page = decode_reject_list_entries(&self.eth_call(contract, data, block).await?)?;
            if page.len() as u64 != end - start {
                return Err(abi_error("unexpected reject list page size"));
            }
            entries.extend(page);
            start = end;
        }
        Ok(RejectListLog {
            entries,
            finalized: finalized_len as usize,
        })
    }
}

/// A resolver of mint events that scans only `max_scan_blocks` blocks after the block each mint
/// commits to. The minter chooses that block, so without a limit a scan can cover the whole chain.
pub(crate) struct BoundedAnchorResolver<'a> {
    client: &'a EvmClient,
    max_scan_blocks: u64,
    /// Mints with no event in the blocks within the limit, all of them finalized
    exhausted: RefCell<HashSet<OpId>>,
}

impl<'a> BoundedAnchorResolver<'a> {
    pub(crate) fn new(client: &'a EvmClient, max_scan_blocks: u64) -> Self {
        Self {
            client,
            max_scan_blocks,
            exhausted: RefCell::new(HashSet::new()),
        }
    }

    /// Whether a finalized `FundsIn` event confirms the mint within the limit.
    pub(crate) async fn is_confirmed(
        &self,
        contract: &EvmAddress,
        anchor: &ExternalAnchor,
    ) -> Result<bool, Error> {
        let ExternalAnchor::MintEvent {
            opid,
            amount,
            after_block,
        } = anchor;
        let finalized_block = self.client.finalized_block_number().await?;
        let limit_block = after_block.saturating_add(self.max_scan_blocks);
        let last_block = min(finalized_block, limit_block);
        let confirmed = self
            .client
            .find_funds_in(contract, opid, *amount, *after_block, last_block)
            .await?;
        if !confirmed && last_block == limit_block {
            self.exhausted.borrow_mut().insert(*opid);
        }
        Ok(confirmed)
    }

    /// Whether the mint has no event in any block within the limit, all of them finalized. Such a
    /// mint is never confirmed.
    pub(crate) fn is_exhausted(&self, opid: &OpId) -> bool {
        self.exhausted.borrow().contains(opid)
    }
}

/// A resolver of the mint events paying a recipient. Each scan resumes after the last finalized
/// block of the previous scan. A [`BoundedAnchorResolver`] resolves the mints in the history.
/// Both scan only `max_scan_blocks` blocks.
pub(crate) struct MintAnchorResolver<'a> {
    client: &'a EvmClient,
    history: BoundedAnchorResolver<'a>,
    terminal_opids: &'a HashSet<OpId>,
    /// Last finalized block scanned without the event, by terminal mint
    scanned_blocks: RefCell<HashMap<OpId, u64>>,
    max_scan_blocks: u64,
    exhausted: RefCell<HashSet<OpId>>,
}

impl<'a> MintAnchorResolver<'a> {
    pub(crate) fn new(
        client: &'a EvmClient,
        terminal_opids: &'a HashSet<OpId>,
        scanned_blocks: HashMap<OpId, u64>,
        max_scan_blocks: u64,
    ) -> Self {
        Self {
            client,
            history: BoundedAnchorResolver::new(client, max_scan_blocks),
            terminal_opids,
            scanned_blocks: RefCell::new(scanned_blocks),
            max_scan_blocks,
            exhausted: RefCell::new(HashSet::new()),
        }
    }

    /// The last finalized block scanned without the event, by terminal mint.
    pub(crate) fn scanned_blocks(&self) -> HashMap<OpId, u64> {
        self.scanned_blocks.borrow().clone()
    }

    /// Whether a finalized `FundsIn` event confirms the mint within the limit.
    pub(crate) async fn is_confirmed(
        &self,
        contract: &EvmAddress,
        anchor: &ExternalAnchor,
    ) -> Result<bool, Error> {
        let ExternalAnchor::MintEvent {
            opid,
            amount,
            after_block,
        } = anchor;
        if !self.terminal_opids.contains(opid) {
            return self.history.is_confirmed(contract, anchor).await;
        }
        // finalized blocks cannot be reorged, so a scanned block is not scanned again
        let scanned_block = self
            .scanned_blocks
            .borrow()
            .get(opid)
            .copied()
            .unwrap_or(*after_block)
            .max(*after_block);
        let finalized_block = self.client.finalized_block_number().await?;
        let limit_block = after_block.saturating_add(self.max_scan_blocks);
        let last_block = min(finalized_block, limit_block);
        if self
            .client
            .find_funds_in_block(
                contract,
                opid,
                *amount,
                *after_block,
                scanned_block,
                last_block,
            )
            .await?
            .is_some()
        {
            return Ok(true);
        }
        self.scanned_blocks
            .borrow_mut()
            .insert(*opid, scanned_block.max(last_block));
        if last_block == limit_block {
            self.exhausted.borrow_mut().insert(*opid);
        }
        Ok(false)
    }

    /// Whether the mint has no event in any block within the limit, all of them finalized.
    pub(crate) fn is_exhausted(&self, opid: &OpId) -> bool {
        if self.terminal_opids.contains(opid) {
            self.exhausted.borrow().contains(opid)
        } else {
            self.history.is_exhausted(opid)
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    /// An opout derived from `b`, with all its fields different.
    pub(crate) fn test_opout(b: u8) -> Opout {
        Opout {
            op: OpId::from([b; 32]),
            ty: AssignmentType::with(4000 + b as u16),
            no: 100 + b as u16,
        }
    }

    fn test_entry(b: u8, reject: bool) -> RejectListEntry {
        RejectListEntry {
            opout: test_opout(b),
            reject,
        }
    }

    /// ABI-encode reject list entries as `entries` returns them.
    pub(crate) fn encode_entries(entries: &[(u8, bool)]) -> String {
        let mut data = format!("0x{:064x}{:064x}", 32, entries.len());
        for (b, reject) in entries {
            let opout = test_opout(*b);
            data.push_str(&hex::encode(opout.op.to_byte_array()));
            data.push_str(&format!("{:064x}", opout.ty.to_inner()));
            data.push_str(&format!("{:064x}", opout.no));
            data.push_str(&format!("{:064x}", *reject as u8));
        }
        data
    }

    pub(crate) fn mock_rpc(
        server: &mut mockito::Server,
        request: &str,
        body: &str,
    ) -> mockito::Mock {
        server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(request.to_string()))
            .with_body(format!(r#"{{"jsonrpc":"2.0","id":1,{body}}}"#))
            .expect(1)
            .create()
    }

    pub(crate) fn mock_finalized_block(
        server: &mut mockito::Server,
        finalized: u64,
    ) -> mockito::Mock {
        mock_rpc(
            server,
            r#"{"method":"eth_getBlockByNumber","params":["finalized",false]}"#,
            &format!(r#""result":{{"number":"0x{finalized:x}"}}"#),
        )
    }

    pub(crate) fn mock_eth_call(
        server: &mut mockito::Server,
        to: &str,
        block: u64,
        data: &str,
        result: &str,
    ) -> mockito::Mock {
        mock_rpc(
            server,
            &format!(
                r#"{{"method":"eth_call","params":[{{"to":"{to}","data":"{data}"}},"0x{block:x}"]}}"#
            ),
            &format!(r#""result":"{result}""#),
        )
    }

    fn mock_get_logs(
        server: &mut mockito::Server,
        from_block: u64,
        to_block: u64,
        body: &str,
    ) -> mockito::Mock {
        mock_rpc(
            server,
            &format!(
                r#"{{"method":"eth_getLogs","params":[{{"address":"{CONTRACT}","fromBlock":"0x{from_block:x}","toBlock":"0x{to_block:x}","topics":["{FUNDS_IN_TOPIC}",null,"0x{}"]}}]}}"#,
                "00".repeat(32)
            ),
            body,
        )
    }

    /// The topics of a `FundsIn` event for the operation ID made of `opid_byte`.
    fn funds_in_topics(opid_byte: u8) -> Vec<String> {
        vec![
            FUNDS_IN_TOPIC.to_string(),
            format!("0x{}{}", "00".repeat(12), "aa".repeat(20)),
            format!("0x{}", format!("{opid_byte:02x}").repeat(32)),
        ]
    }

    /// A `FundsIn` log (JSON) for the all-zeros operation ID in `block`.
    fn funds_in_log(amount: u64, block: u64) -> String {
        format!(
            r#"{{"topics":{:?},"data":"0x{amount:064x}","blockNumber":"0x{block:x}"}}"#,
            funds_in_topics(0)
        )
    }

    /// A nonzero contract address, as the node expects it.
    const CONTRACT: &str = "0x00000000000000000000000000000000000000b1";

    fn contract() -> EvmAddress {
        EvmAddress::from_str(CONTRACT).unwrap()
    }

    const NO_LOGS: &str = r#""result":[]"#;
    const RANGE_TOO_LARGE: &str =
        r#""error":{"code":-32005,"message":"query exceeds max block range"}"#;

    #[test]
    fn reject_list_entries_decode() {
        let page = encode_entries(&[(1, true), (2, false)]);
        let expected = vec![test_entry(1, true), test_entry(2, false)];
        assert_eq!(decode_reject_list_entries(&page).unwrap(), expected);
        let empty = format!("0x{:064x}{:064x}", 32, 0);
        assert!(decode_reject_list_entries(&empty).unwrap().is_empty());
        // a bool word other than 0 or 1
        let bad_bool = format!(
            "0x{:064x}{:064x}{}{:064x}{:064x}{:064x}",
            32,
            1,
            "01".repeat(32),
            1,
            0,
            2
        );
        assert!(decode_reject_list_entries(&bad_bool).is_err());
        // a type or number that does not fit a uint16
        for (ty, no) in [(1 << 16, 0), (0, 1 << 16)] {
            let bad_u16 = format!(
                "0x{:064x}{:064x}{}{:064x}{:064x}{:064x}",
                32,
                1,
                "01".repeat(32),
                ty,
                no,
                1
            );
            assert!(
                matches!(decode_reject_list_entries(&bad_u16), Err(Error::Network { ref details }) if details.contains("bad uint16"))
            );
        }
        // a truncated page
        assert!(decode_reject_list_entries(&page[..page.len() - 64]).is_err());
        // a multi-byte char across a word boundary
        let page = format!("0x{:063x}é{:063x}", 32, 0);
        assert!(decode_reject_list_entries(&page).is_err());
    }

    #[test]
    fn reject_list_decisions() {
        let log = |entries: &[(u8, bool)], finalized: usize| RejectListLog {
            entries: entries
                .iter()
                .map(|(b, reject)| test_entry(*b, *reject))
                .collect(),
            finalized,
        };
        let set = |bytes: &[u8]| bytes.iter().map(|b| test_opout(*b)).collect::<HashSet<_>>();

        // the latest finalized entry wins
        let (reject, allow) = log(&[(1, true), (2, false), (3, false), (3, true)], 4).decisions();
        assert_eq!(reject, set(&[1, 3]));
        assert_eq!(allow, set(&[2]));

        // a non-finalized reject counts at once
        let (reject, allow) = log(&[(2, false), (1, true), (2, true)], 1).decisions();
        assert_eq!(reject, set(&[1, 2]));
        assert!(allow.is_empty());

        // a non-finalized allow does not count
        let (reject, allow) = log(&[(1, true), (2, false), (1, false)], 1).decisions();
        assert_eq!(reject, set(&[1]));
        assert!(allow.is_empty());
    }

    #[test]
    fn funds_in_layout() {
        let opid_bytes: [u8; 32] = std::array::from_fn(|i| i as u8);
        let mut topics = funds_in_topics(0);
        topics[2] = format!("0x{}", hex::encode(opid_bytes));
        let log = EvmLog {
            topics,
            data: format!("0x{:064x}", 7u64),
            block_number: Some(s!("0x10")),
        };
        let event = log.as_funds_in().unwrap().unwrap();
        assert_eq!(OpId::from(event.operation_id), OpId::from(opid_bytes));
        assert_eq!(event.amount, 7);
        assert_eq!(event.block_number, Some(16));

        // another event
        let log = EvmLog {
            topics: vec![format!("0x{}", "11".repeat(32))],
            data: s!("0x"),
            block_number: None,
        };
        assert!(log.as_funds_in().unwrap().is_none());

        // no indexed operation ID
        let log = EvmLog {
            topics: funds_in_topics(0)[..2].to_vec(),
            data: format!("0x{:064x}", 7u64),
            block_number: None,
        };
        assert!(log.as_funds_in().is_err());

        // data other than one amount word
        let log = EvmLog {
            topics: funds_in_topics(0),
            data: format!("0x{}{:064x}", "00".repeat(32), 7u64),
            block_number: None,
        };
        assert!(log.as_funds_in().is_err());

        // an amount over u64::MAX never matches
        let log = EvmLog {
            topics: funds_in_topics(0),
            data: format!("0x{}1{}", "0".repeat(47), "0".repeat(16)),
            block_number: None,
        };
        assert!(log.as_funds_in().unwrap().is_none());

        assert!(parse_block_number("10").is_err());
    }

    #[test]
    fn funds_in_confirms() {
        let log = EvmLog {
            topics: funds_in_topics(0),
            data: format!("0x{:064x}", 7u64),
            block_number: Some(s!("0x10")),
        };
        let event = log.as_funds_in().unwrap().unwrap();
        let opid = OpId::from([0u8; 32]);
        assert!(event.confirms(&opid, 7, 15));
        // not after the anchor block
        assert!(!event.confirms(&opid, 7, 16));
        // a wrong amount
        assert!(!event.confirms(&opid, 8, 15));
        // another operation
        assert!(!event.confirms(&OpId::from([1u8; 32]), 7, 15));
    }

    #[test]
    fn chain_id() {
        let mut server = mockito::Server::new();
        let mock = mock_rpc(
            &mut server,
            r#"{"method":"eth_chainId"}"#,
            r#""result":"0x7a69""#,
        );
        assert_eq!(
            block_on(EvmClient::new(&server.url()).chain_id()).unwrap(),
            31337
        );
        mock.assert();

        let mut server = mockito::Server::new();
        let _mock = mock_rpc(
            &mut server,
            r#"{"method":"eth_chainId"}"#,
            r#""error":{"code":-32601,"message":"not found"}"#,
        );
        let result = block_on(EvmClient::new(&server.url()).chain_id());
        assert!(
            matches!(result, Err(Error::Network { ref details }) if details == "eth_chainId error -32601: not found")
        );
    }

    #[test]
    fn find_funds_in() {
        let opid = OpId::from([0u8; 32]);
        let address = contract();
        let find = |last: u64, logs: &[String]| {
            let mut server = mockito::Server::new();
            let logs_mock = mock_get_logs(
                &mut server,
                17,
                last,
                &format!(r#""result":[{}]"#, logs.join(",")),
            );
            let found =
                block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16, last))
                    .unwrap();
            logs_mock.assert();
            found
        };
        // the matching event in a finalized block
        assert!(find(0x20, &[funds_in_log(7, 0x11)]));
        // a wrong amount before the right one does not hide it
        assert!(find(0x20, &[funds_in_log(8, 0x11), funds_in_log(7, 0x12)]));
        // a wrong amount
        assert!(!find(0x20, &[funds_in_log(8, 0x11)]));
        // no event
        assert!(!find(0x20, &[]));

        // the last block is not past the anchor block: no log is requested
        let mut server = mockito::Server::new();
        let get_logs = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(s!(
                r#"{"method":"eth_getLogs"}"#
            )))
            .expect(0)
            .create();
        assert!(
            !block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16, 16))
                .unwrap()
        );
        get_logs.assert();
    }

    #[test]
    fn find_funds_in_shrinks_refused_range() {
        let opid = OpId::from([0u8; 32]);
        let address = contract();

        // the refused range is halved
        let find = |server: &mockito::Server, last: u64| {
            block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16, last))
        };
        let mut server = mockito::Server::new();
        let refused = mock_get_logs(&mut server, 17, 20, RANGE_TOO_LARGE);
        let ranges =
            [(17, 18), (19, 20)].map(|(from, to)| mock_get_logs(&mut server, from, to, NO_LOGS));
        assert!(!find(&server, 20).unwrap());
        refused.assert();
        ranges.iter().for_each(|r| r.assert());

        // a range refused with an HTTP 413 is halved too, whatever the answer body
        let mut server = mockito::Server::new();
        let refused = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(format!(
                r#"{{"method":"eth_getLogs","params":[{{"fromBlock":"0x{:x}","toBlock":"0x{:x}"}}]}}"#,
                17, 24
            )))
            .with_status(413)
            .with_body("<html>413 Request Entity Too Large</html>")
            .expect(1)
            .create();
        let ranges =
            [(17, 20), (21, 24)].map(|(from, to)| mock_get_logs(&mut server, from, to, NO_LOGS));
        assert!(!find(&server, 24).unwrap());
        refused.assert();
        ranges.iter().for_each(|r| r.assert());

        // a request refused whatever its range (rate limited, or not authorized) is reported
        // at once, without halving the range
        for status in [429, 401, 403] {
            let mut server = mockito::Server::new();
            let refused = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::PartialJsonString(s!(
                    r#"{"method":"eth_getLogs"}"#
                )))
                .with_status(status)
                .with_body(
                    r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32011,"message":"limited"}}"#,
                )
                .expect(1)
                .create();
            let result = find(&server, 32);
            assert!(
                matches!(result, Err(Error::Network { ref details }) if details.starts_with("eth_getLogs error -32011 (HTTP")),
                "{result:?}"
            );
            refused.assert();
        }

        // a refused single block gives the node error
        let mut server = mockito::Server::new();
        let _refused = [(17, 18), (17, 17)]
            .map(|(from, to)| mock_get_logs(&mut server, from, to, RANGE_TOO_LARGE));
        let result = find(&server, 18);
        assert!(
            matches!(result, Err(Error::Network { ref details }) if details == "eth_getLogs error -32005: query exceeds max block range")
        );
    }

    #[test]
    fn reject_list_entries() {
        let length = |len: u64| format!("0x{len:064x}");
        let entries_data =
            |start: u64, end: u64| format!("{REJECT_LIST_ENTRIES_SELECTOR}{start:064x}{end:064x}");
        // the latest block is 0x10, the finalized block is 8
        let server_with_lengths = |finalized_len: &str, len: &str| {
            let mut server = mockito::Server::new();
            let mocks = vec![
                mock_rpc(
                    &mut server,
                    r#"{"method":"eth_blockNumber"}"#,
                    r#""result":"0x10""#,
                ),
                mock_finalized_block(&mut server, 8),
                mock_eth_call(
                    &mut server,
                    CONTRACT,
                    8,
                    REJECT_LIST_LENGTH_SELECTOR,
                    finalized_len,
                ),
                mock_eth_call(
                    &mut server,
                    CONTRACT,
                    0x10,
                    REJECT_LIST_LENGTH_SELECTOR,
                    len,
                ),
            ];
            (server, mocks)
        };
        let read = |server: &mockito::Server| {
            block_on(EvmClient::new(&server.url()).reject_list_entries(&contract()))
        };

        // two pages
        let all = (0..=REJECT_LIST_PAGE_SIZE)
            .map(|i| (i as u8, i % 2 == 0))
            .collect::<Vec<_>>();
        let split = REJECT_LIST_PAGE_SIZE as usize;
        let (mut server, mut mocks) = server_with_lengths(&length(1), &length(all.len() as u64));
        mocks.push(mock_eth_call(
            &mut server,
            CONTRACT,
            0x10,
            &entries_data(0, REJECT_LIST_PAGE_SIZE),
            &encode_entries(&all[..split]),
        ));
        mocks.push(mock_eth_call(
            &mut server,
            CONTRACT,
            0x10,
            &entries_data(REJECT_LIST_PAGE_SIZE, all.len() as u64),
            &encode_entries(&all[split..]),
        ));
        assert_eq!(
            read(&server).unwrap(),
            RejectListLog {
                entries: all.iter().map(|(b, r)| test_entry(*b, *r)).collect(),
                finalized: 1,
            }
        );
        mocks.iter().for_each(|m| m.assert());

        // a page with fewer entries than requested
        let (mut server, _mocks) = server_with_lengths(&length(1), &length(2));
        let _page = mock_eth_call(
            &mut server,
            CONTRACT,
            0x10,
            &entries_data(0, 2),
            &encode_entries(&[(1, true)]),
        );
        assert!(
            matches!(read(&server), Err(Error::Network { ref details }) if details.contains("unexpected reject list page size"))
        );

        // a finalized list longer than the latest one
        let (server, _mocks) = server_with_lengths(&length(3), &length(2));
        assert!(
            matches!(read(&server), Err(Error::Network { ref details }) if details.contains("shorter at the latest block"))
        );

        // an address with no code is not an empty list
        let (server, _mocks) = server_with_lengths("0x", "0x");
        assert!(
            matches!(read(&server), Err(Error::Network { ref details }) if details.contains("no contract at the reject list address"))
        );

        // a list longer than supported is refused before reading any page
        let (mut server, mocks) = server_with_lengths("0x", &length(MAX_REJECT_LIST_ENTRIES + 1));
        let page = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(format!(
                r#"{{"params":[{{"data":"{}"}}]}}"#,
                entries_data(0, REJECT_LIST_PAGE_SIZE)
            )))
            .expect(0)
            .create();
        let result = read(&server);
        assert!(
            matches!(result, Err(Error::Network { ref details }) if details.contains("reject list too long")),
            "{result:?}"
        );
        mocks.iter().for_each(|m| m.assert());
        page.assert();
    }

    #[test]
    fn post_retries() {
        let chain_id_mock = |server: &mut mockito::Server, status: usize, hits: usize| {
            server
                .mock("POST", "/")
                .match_body(mockito::Matcher::PartialJsonString(s!(
                    r#"{"method":"eth_chainId"}"#
                )))
                .with_status(status)
                .with_body(r#"{"jsonrpc":"2.0","id":1,"result":"0x7a69"}"#)
                .expect(hits)
                .create()
        };

        // a node temporarily unavailable is asked again
        for status in [502, 503, 504] {
            let mut server = mockito::Server::new();
            let unavailable = chain_id_mock(&mut server, status, RPC_RETRIES as usize);
            let available = chain_id_mock(&mut server, 200, 1);
            assert_eq!(
                block_on(EvmClient::new(&server.url()).chain_id()).unwrap(),
                31337
            );
            unavailable.assert();
            available.assert();
        }

        // up to RPC_RETRIES times, then its answer is handled as any other
        let mut server = mockito::Server::new();
        let unavailable = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("<html>503 Service Temporarily Unavailable</html>")
            .expect(RPC_RETRIES as usize + 1)
            .create();
        assert!(matches!(
            block_on(EvmClient::new(&server.url()).chain_id()),
            Err(Error::Network { .. })
        ));
        unavailable.assert();

        // other errors are not retried
        let mut server = mockito::Server::new();
        let refused = chain_id_mock(&mut server, 500, 1);
        block_on(EvmClient::new(&server.url()).chain_id()).unwrap();
        refused.assert();

        // a timed out request is not sent again
        let mut server = mockito::Server::new();
        let slow = server
            .mock("POST", "/")
            .with_body_from_request(|_| {
                std::thread::sleep(std::time::Duration::from_millis(500));
                br#"{"jsonrpc":"2.0","id":1,"result":"0x7a69"}"#.to_vec()
            })
            .expect(1)
            .create();
        let client = EvmClient {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(100))
                .build()
                .unwrap(),
            rpc_url: server.url(),
        };
        let start = std::time::Instant::now();
        assert!(matches!(
            block_on(client.chain_id()),
            Err(Error::Network { .. })
        ));
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        slow.assert();

        // a request that cannot be sent is sent again, then reported
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let start = std::time::Instant::now();
        assert!(matches!(
            block_on(EvmClient::new(&url).chain_id()),
            Err(Error::Network { .. })
        ));
        let pauses: u32 = (1..=RPC_RETRIES).sum();
        assert!(start.elapsed() >= RPC_RETRY_PAUSE * pauses);
    }

    #[test]
    fn bounded_resolver() {
        let opid = OpId::from([0u8; 32]);
        let anchor = |after_block: u64| ExternalAnchor::MintEvent {
            opid,
            amount: 7,
            after_block,
        };
        let resolve = |server: &mockito::Server, after_block: u64| {
            let client = EvmClient::new(&server.url());
            let resolver = BoundedAnchorResolver::new(&client, 10);
            let confirmed =
                block_on(resolver.is_confirmed(&contract(), &anchor(after_block))).unwrap();
            (confirmed, resolver.is_exhausted(&opid))
        };

        // the scan stops at the limit, even if more blocks are finalized: with no event within it,
        // the mint is exhausted
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 1020);
        let range = mock_get_logs(&mut server, 17, 26, NO_LOGS);
        assert_eq!(resolve(&server, 16), (false, true));
        range.assert();

        // with a limit past the finalized block, the scan stops at the finalized block: the event
        // may still be in a later block within the limit, so the mint is not exhausted
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 20);
        let range = mock_get_logs(&mut server, 17, 20, NO_LOGS);
        assert_eq!(resolve(&server, 16), (false, false));
        range.assert();

        // an event within the limit confirms the mint
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 1020);
        let range = mock_get_logs(
            &mut server,
            17,
            26,
            &format!(r#""result":[{}]"#, funds_in_log(7, 26)),
        );
        assert_eq!(resolve(&server, 16), (true, false));
        range.assert();

        // the limit does not overflow: an anchor past the finalized block is not exhausted
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 0x10);
        let get_logs = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(s!(
                r#"{"method":"eth_getLogs"}"#
            )))
            .expect(0)
            .create();
        assert_eq!(resolve(&server, u64::MAX), (false, false));
        get_logs.assert();
    }

    #[test]
    fn mint_anchor_resolver_resumes_scan() {
        let opid = OpId::from([0u8; 32]);
        let terminal_opids = HashSet::from([opid]);
        let anchor = ExternalAnchor::MintEvent {
            opid,
            amount: 7,
            after_block: 16,
        };
        let resolve = |server: &mockito::Server, scanned_block: u64| {
            let client = EvmClient::new(&server.url());
            let resolver = MintAnchorResolver::new(
                &client,
                &terminal_opids,
                HashMap::from([(opid, scanned_block)]),
                10,
            );
            let confirmed = block_on(resolver.is_confirmed(&contract(), &anchor)).unwrap();
            (
                confirmed,
                resolver.scanned_blocks()[&opid],
                resolver.is_exhausted(&opid),
            )
        };

        // the scan resumes after the scanned block and stops at the finalized one
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 22);
        let range = mock_get_logs(&mut server, 21, 22, NO_LOGS);
        assert_eq!(resolve(&server, 20), (false, 22, false));
        range.assert();

        // the next scan resumes from there and stops at the limit: the mint is exhausted
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 1020);
        let range = mock_get_logs(&mut server, 23, 26, NO_LOGS);
        assert_eq!(resolve(&server, 22), (false, 26, true));
        range.assert();

        // an event in the last block within the limit confirms the mint
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 1020);
        let range = mock_get_logs(
            &mut server,
            23,
            26,
            &format!(r#""result":[{}]"#, funds_in_log(7, 26)),
        );
        assert_eq!(resolve(&server, 22), (true, 22, false));
        range.assert();
    }

    #[test]
    fn check_mint_support() {
        // the finalized block is asked, then the logs of block 0 if it is served
        let check = |finalized: &str, logs: &str, logs_hits: usize| {
            let mut server = mockito::Server::new();
            let finalized = mock_rpc(
                &mut server,
                r#"{"method":"eth_getBlockByNumber","params":["finalized",false]}"#,
                finalized,
            );
            let logs = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::PartialJsonString(s!(
                    r#"{"method":"eth_getLogs","params":[{"fromBlock":"0x0","toBlock":"0x0"}]}"#
                )))
                .with_body(format!(r#"{{"jsonrpc":"2.0","id":1,{logs}}}"#))
                .expect(logs_hits)
                .create();
            let result = block_on(EvmClient::new(&server.url()).check_mint_support());
            finalized.assert();
            logs.assert();
            result
        };
        let finalized = r#""result":{"number":"0x10"}"#;
        // no finalized block
        let result = check(
            r#""error":{"code":-32602,"message":"invalid block tag"}"#,
            NO_LOGS,
            0,
        );
        assert!(
            matches!(result, Err(Error::InvalidEvmRpcUrl { ref details }) if details.starts_with("cannot get the latest finalized block")),
            "{result:?}"
        );
        // only the logs of recent blocks
        let result = check(
            finalized,
            r#""error":{"code":-32602,"message":"Archive requests require a personal token"}"#,
            1,
        );
        assert!(
            matches!(result, Err(Error::InvalidEvmRpcUrl { ref details })
                if details.starts_with("cannot get the logs of the oldest blocks")
                    && details.ends_with("Archive requests require a personal token")),
            "{result:?}"
        );
        // serving both
        check(finalized, NO_LOGS, 1).unwrap();
    }
}

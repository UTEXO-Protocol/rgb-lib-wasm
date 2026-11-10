use super::*;

use reqwest::StatusCode;

/// Selector of `getFundsInBlock(uint256,uint256)` on a bridge contract (`MinimalBridge`)
const FUNDS_IN_BLOCK_SELECTOR: &str = "0x2ce06e2d";

/// Selector of `length()` on a reject list contract (`IRgbRejectList`)
pub(crate) const REJECT_LIST_LENGTH_SELECTOR: &str = "0x1f7b6d32";
/// Selector of `entries(uint256,uint256)` on a reject list contract (`IRgbRejectList`)
pub(crate) const REJECT_LIST_ENTRIES_SELECTOR: &str = "0x17b937e6";
/// Maximum number of reject list entries requested with a single `entries` call
const REJECT_LIST_PAGE_SIZE: u64 = 1000;
/// Number of times a JSON-RPC request is sent again when it fails for a temporary reason
const RPC_RETRIES: u32 = 2;
/// Pause before a JSON-RPC request is sent again, multiplied by the number of the retry
#[cfg(not(test))]
const RPC_RETRY_PAUSE: Duration = Duration::from_secs(1);
#[cfg(test)]
const RPC_RETRY_PAUSE: Duration = Duration::from_millis(10);
/// The status of the lock confirming a mint, which has to be included on the external chain
/// before the block the mint commits to (`before_block`), as recorded by the bridge contract as of
/// the latest finalized block (see [`EvmClient::mint_lock_status`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MintLockStatus {
    /// The lock is recorded, included before `before_block`
    Confirmed,
    /// No lock is recorded yet, but one may still be included before `before_block`
    Pending,
    /// The lock is recorded, but not included before `before_block`, or none is and the blocks
    /// before `before_block` are all finalized: the mint will never be confirmed
    Expired,
}

const NO_PARAMS: [u8; 0] = [];

/// JSON-RPC client for an EVM node.
pub(crate) struct EvmClient {
    client: reqwest::Client,
    rpc_url: String,
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

    fn rpc_error(method: &str, err: RpcError) -> Error {
        Error::Network {
            details: format!("{method} error {}: {}", err.code, err.message),
        }
    }

    fn null_result(method: &str) -> Error {
        Error::Network {
            details: format!("{method} returned null result"),
        }
    }

    /// Call a JSON-RPC method. Fail if it reports an error or no result.
    async fn rpc<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<R, Error> {
        self.try_rpc(method, params)
            .await?
            .map_err(|err| Self::rpc_error(method, err))?
            .ok_or_else(|| Self::null_result(method))
    }

    /// Call a JSON-RPC method. Return the error it reports apart from the other failures, and
    /// its result, which can be null.
    async fn try_rpc<P: Serialize, R: serde::de::DeserializeOwned>(
        &self,
        method: &'static str,
        params: P,
    ) -> Result<Result<Option<R>, RpcError>, Error> {
        let body = RpcRequest {
            jsonrpc: "2.0",
            method,
            params,
            id: 1,
        };
        let resp: RpcResponse<R> = self
            .post(&body)
            .await?
            .json()
            .await
            .map_err(Self::req_err)?;
        Ok(match resp.error {
            Some(err) => Err(err),
            None => Ok(resp.result),
        })
    }

    /// EIP-155 chain ID of the chain the node is on (`eth_chainId`).
    pub(crate) async fn chain_id(&self) -> Result<u64, Error> {
        parse_quantity(
            &self.rpc::<_, String>("eth_chainId", NO_PARAMS).await?,
            "chain ID",
        )
    }

    /// Number of the latest block (`eth_blockNumber`).
    pub(crate) async fn block_number(&self) -> Result<u64, Error> {
        parse_block_number(&self.rpc::<_, String>("eth_blockNumber", NO_PARAMS).await?)
    }

    /// Number of the latest finalized block.
    pub(crate) async fn finalized_block_number(&self) -> Result<u64, Error> {
        let header: BlockHeader = self
            .rpc("eth_getBlockByNumber", ("finalized", false))
            .await?;
        parse_block_number(&header.number)
    }

    /// Check that the node serves what confirming mints needs: the latest finalized block and the
    /// contract state as of it. Fail with [`Error::InvalidEvmRpcUrl`] if it does not.
    pub(crate) async fn check_mint_support(&self) -> Result<(), Error> {
        let finalized_block =
            self.finalized_block_number()
                .await
                .map_err(|e| Error::InvalidEvmRpcUrl {
                    details: format!("cannot get the latest finalized block: {e}"),
                })?;
        self.eth_call(&EvmAddress::default(), s!("0x"), finalized_block)
            .await
            .map_err(|e| Error::InvalidEvmRpcUrl {
                details: format!(
                    "cannot read the state as of the latest finalized block, an endpoint keeping \
                     it is needed: {e}"
                ),
            })?;
        Ok(())
    }

    /// The block the lock of `amount` with ID `lock_id` was included in, as recorded by the bridge
    /// contract at `contract` as of `block`, if any.
    ///
    /// An address with no code (calling it returns no data) or a contract without the function
    /// (calling it reverts) cannot have recorded any lock.
    async fn funds_in_block(
        &self,
        contract: &EvmAddress,
        lock_id: &LockId,
        amount: u64,
        block: u64,
    ) -> Result<Option<u64>, Error> {
        let data = format!(
            "{FUNDS_IN_BLOCK_SELECTOR}{}{amount:064x}",
            hex::encode(lock_id.to_byte_array())
        );
        let Some(result) = self.eth_call_unless_reverted(contract, data, block).await? else {
            return Ok(None);
        };
        if result == "0x" {
            return Ok(None);
        }
        // compare the exact hex length: trailing data is refused
        if strip_hex_prefix(&result, "ABI data")?.len() != 64 {
            return Err(abi_error("unexpected getFundsInBlock result size"));
        }
        // the record is the block plus one, zero means there is none
        let block_plus_one = abi_word_u64(&result, 0)?
            .ok_or_else(|| abi_error("lock block doesn't fit in a u64"))?;
        Ok(block_plus_one.checked_sub(1))
    }

    /// The status of the lock of `amount` with ID `lock_id`, which has to be included before
    /// `before_block`, as recorded by the bridge contract at `contract` as of the latest finalized
    /// block.
    pub(crate) async fn mint_lock_status(
        &self,
        contract: &EvmAddress,
        lock_id: &LockId,
        amount: u64,
        before_block: u64,
    ) -> Result<MintLockStatus, Error> {
        let finalized_block = self.finalized_block_number().await?;
        Ok(
            match self
                .funds_in_block(contract, lock_id, amount, finalized_block)
                .await?
            {
                Some(block) if block < before_block => MintLockStatus::Confirmed,
                Some(_) => MintLockStatus::Expired,
                // a lock not recorded as of the finalized block can only be included after it
                None if finalized_block.saturating_add(1) >= before_block => {
                    MintLockStatus::Expired
                }
                None => MintLockStatus::Pending,
            },
        )
    }

    /// Whether the bridge contract at `contract` records, as of the latest block, a lock of
    /// `amount` with ID `lock_id` included before `before_block`, finalized or not.
    pub(crate) async fn mint_lock_recorded(
        &self,
        contract: &EvmAddress,
        lock_id: &LockId,
        amount: u64,
        before_block: u64,
    ) -> Result<bool, Error> {
        let latest_block = self.block_number().await?;
        Ok(matches!(
            self.funds_in_block(contract, lock_id, amount, latest_block).await?,
            Some(block) if block < before_block
        ))
    }

    /// The parameters of an `eth_call` of `contract` with `data` as of `block`.
    fn call_params(contract: &EvmAddress, data: String, block: u64) -> (CallObject, String) {
        let call = CallObject {
            to: contract.to_string(),
            data,
        };
        (call, format!("0x{block:x}"))
    }

    /// Call `contract` with `data` as of `block`. Return `None` if the call reverts.
    async fn eth_call_unless_reverted(
        &self,
        contract: &EvmAddress,
        data: String,
        block: u64,
    ) -> Result<Option<String>, Error> {
        let method = "eth_call";
        match self
            .try_rpc(method, Self::call_params(contract, data, block))
            .await?
        {
            Ok(result) => result.map(Some).ok_or_else(|| Self::null_result(method)),
            // nodes report a revert with this message, then the reason if there is one
            Err(err) if err.message.starts_with("execution reverted") => Ok(None),
            Err(err) => Err(Self::rpc_error(method, err)),
        }
    }

    async fn eth_call(
        &self,
        contract: &EvmAddress,
        data: String,
        block: u64,
    ) -> Result<String, Error> {
        self.rpc("eth_call", Self::call_params(contract, data, block))
            .await
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

/// A claim resolver over the lock statuses fetched from an [`EvmClient`]. It records the mints
/// that will never be confirmed (see [`MintLockStatus::Expired`]).
///
/// [`ResolveClaim`] is synchronous and the browser can only fetch asynchronously, so the statuses
/// of the claims are fetched first.
pub(crate) struct MintClaimResolver {
    chain_id: u64,
    /// Lock statuses, by lock ID, amount and `before_block`
    statuses: HashMap<(LockId, u64, u64), MintLockStatus>,
    /// Operation IDs of the mints that will never be confirmed
    expired: HashSet<OpId>,
}

impl MintClaimResolver {
    /// Fetch the lock status of each of `claims` from the bridge at `location`. Fetch none when
    /// the node is on another chain, which the claim resolution then reports.
    pub(crate) async fn fetch(
        client: &EvmClient,
        location: &BridgeLocation,
        claims: &[Claim],
    ) -> Result<Self, Error> {
        let BridgeLocation::Evm(EvmContract { chain_id, address }) = location;
        let mut resolver = Self {
            chain_id: client.chain_id().await?,
            statuses: HashMap::new(),
            expired: HashSet::new(),
        };
        if resolver.chain_id != *chain_id {
            return Ok(resolver);
        }
        for claim in claims {
            let Claim::Lock {
                opid,
                amount,
                before_block,
                ..
            } = claim;
            let lock_id = LockId::from(claim);
            let status = client
                .mint_lock_status(address, &lock_id, *amount, *before_block)
                .await?;
            if status == MintLockStatus::Expired {
                resolver.expired.insert(*opid);
            }
            resolver
                .statuses
                .insert((lock_id, *amount, *before_block), status);
        }
        Ok(resolver)
    }

    /// Whether the mint with the given operation ID will never be confirmed.
    ///
    /// A mint that is not confirmed but not expired may still be, once its lock is made or
    /// finalized.
    pub(crate) fn is_expired(&self, opid: &OpId) -> bool {
        self.expired.contains(opid)
    }
}

impl ResolveClaim for MintClaimResolver {
    fn evm_chain_id(&self) -> Result<u64, ClaimResolverError> {
        Ok(self.chain_id)
    }

    fn is_confirmed(
        &self,
        _location: &BridgeLocation,
        claim: &Claim,
    ) -> Result<bool, ClaimResolverError> {
        let Claim::Lock {
            amount,
            before_block,
            ..
        } = claim;
        self.statuses
            .get(&(LockId::from(claim), *amount, *before_block))
            .map(|status| *status == MintLockStatus::Confirmed)
            .ok_or_else(|| ClaimResolverError::ResolverIssue(s!("claim not fetched")))
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

    /// A nonzero contract address, as the node expects it.
    const CONTRACT: &str = "0x00000000000000000000000000000000000000b1";

    fn contract() -> EvmAddress {
        EvmAddress::from_str(CONTRACT).unwrap()
    }

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

        // a length that does not fit in a u64
        let overflow = format!("0x{:064x}", u64::MAX as u128 + 1);
        let (server, _mocks) = server_with_lengths("0x", &overflow);
        let result = read(&server);
        assert!(
            matches!(result, Err(Error::Network { ref details }) if details.contains("reject list length doesn't fit in a u64")),
            "{result:?}"
        );

        // a page call that reverts
        let (mut server, _mocks) = server_with_lengths(&length(1), &length(1));
        let _page = mock_rpc(
            &mut server,
            &format!(
                r#"{{"method":"eth_call","params":[{{"to":"{CONTRACT}","data":"{}"}},"0x10"]}}"#,
                entries_data(0, 1)
            ),
            r#""error":{"code":3,"message":"execution reverted"}"#,
        );
        let result = read(&server);
        assert!(
            matches!(result, Err(Error::Network { ref details }) if details.contains("eth_call error")),
            "{result:?}"
        );
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
    fn check_mint_support() {
        // the finalized block is asked, then the state at it if there is one
        let check = |finalized: &str, call: &str, call_hits: usize| {
            let mut server = mockito::Server::new();
            let finalized = mock_rpc(
                &mut server,
                r#"{"method":"eth_getBlockByNumber","params":["finalized",false]}"#,
                finalized,
            );
            let call = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::PartialJsonString(s!(
                    r#"{"method":"eth_call","params":[{"data":"0x"},"0x10"]}"#
                )))
                .with_body(format!(r#"{{"jsonrpc":"2.0","id":1,{call}}}"#))
                .expect(call_hits)
                .create();
            let result = block_on(EvmClient::new(&server.url()).check_mint_support());
            finalized.assert();
            call.assert();
            result
        };
        let finalized = r#""result":{"number":"0x10"}"#;
        let no_code = r#""result":"0x""#;
        let result = check(
            r#""error":{"code":-32602,"message":"invalid block tag"}"#,
            no_code,
            0,
        );
        assert!(
            matches!(result, Err(Error::InvalidEvmRpcUrl { ref details }) if details.contains("cannot get the latest finalized block")),
            "{result:?}"
        );
        let result = check(
            finalized,
            r#""error":{"code":-32000,"message":"missing trie node"}"#,
            1,
        );
        assert!(
            matches!(result, Err(Error::InvalidEvmRpcUrl { ref details })
                if details.contains("cannot read the state") && details.ends_with("missing trie node")),
            "{result:?}"
        );
        check(finalized, no_code, 1).unwrap();
    }

    /// The `getFundsInBlock` call data for the lock with ID `lock_id` and `amount`.
    fn funds_in_call(lock_id: &LockId, amount: u64) -> String {
        format!(
            "{FUNDS_IN_BLOCK_SELECTOR}{}{amount:064x}",
            hex::encode(lock_id.to_byte_array())
        )
    }

    /// Mock an `eth_call` at `block` with the given call `data`, expected `hits` times.
    fn mock_eth_call_hits(
        server: &mut mockito::Server,
        block: u64,
        data: &str,
        result: &str,
        hits: usize,
    ) -> mockito::Mock {
        server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(format!(
                r#"{{"method":"eth_call","params":[{{"data":"{data}"}},"0x{block:x}"]}}"#
            )))
            .with_body(format!(r#"{{"jsonrpc":"2.0","id":1,"result":"{result}"}}"#))
            .expect(hits)
            .create()
    }

    #[test]
    fn funds_in_block() {
        let contract = EvmAddress::default();
        let lock_id = LockId::from([1u8; 32]);
        let funds_in_block = |result: &str| {
            let mut server = mockito::Server::new();
            let call =
                mock_eth_call_hits(&mut server, 0x10, &funds_in_call(&lock_id, 7), result, 1);
            let block = block_on(
                EvmClient::new(&server.url()).funds_in_block(&contract, &lock_id, 7, 0x10),
            );
            call.assert();
            block
        };

        // the record is the block plus one
        assert_eq!(
            funds_in_block(&format!("0x{:064x}", 0x12)).unwrap(),
            Some(0x11)
        );
        assert_eq!(funds_in_block(&format!("0x{:064x}", 1)).unwrap(), Some(0));
        // zero means there is no record
        assert_eq!(funds_in_block(&format!("0x{:064x}", 0)).unwrap(), None);
        // an address with no code has no records
        assert_eq!(funds_in_block("0x").unwrap(), None);
        // a revert means no record, another error is an error
        let call_error = |error: &str| {
            let mut server = mockito::Server::new();
            let call = server
                .mock("POST", "/")
                .match_body(mockito::Matcher::PartialJsonString(s!(
                    r#"{"method":"eth_call"}"#
                )))
                .with_body(format!(r#"{{"jsonrpc":"2.0","id":1,"error":{error}}}"#))
                .create();
            let block = block_on(
                EvmClient::new(&server.url()).funds_in_block(&contract, &lock_id, 7, 0x10),
            );
            call.assert();
            block
        };
        assert_eq!(
            call_error(r#"{"code":3,"message":"execution reverted"}"#).unwrap(),
            None
        );
        assert_eq!(
            call_error(r#"{"code":-32000,"message":"execution reverted: paused"}"#).unwrap(),
            None
        );
        assert!(call_error(r#"{"code":-32000,"message":"missing trie node"}"#).is_err());
        // a result that is not a single word is refused
        assert!(funds_in_block(&format!("0x{:062x}", 1)).is_err());
        assert!(funds_in_block(&format!("0x{:064x}{:064x}", 1, 0)).is_err());
        // a block that does not fit in a u64 is refused
        assert!(funds_in_block(&format!("0x{}1{}", "0".repeat(47), "0".repeat(16))).is_err());
    }

    #[test]
    fn mint_lock_status() {
        let contract = EvmAddress::default();
        let lock_id = LockId::from([1u8; 32]);
        // the status of the lock, with the given record as of the finalized block 0x20
        let status = |record: u64, before_block: u64| {
            let mut server = mockito::Server::new();
            let _finalized = mock_finalized_block(&mut server, 0x20);
            let call = mock_eth_call_hits(
                &mut server,
                0x20,
                &funds_in_call(&lock_id, 7),
                &format!("0x{record:064x}"),
                1,
            );
            let status = block_on(EvmClient::new(&server.url()).mint_lock_status(
                &contract,
                &lock_id,
                7,
                before_block,
            ))
            .unwrap();
            call.assert();
            status
        };

        // a lock in block 0x11 confirms the mint only if that is before the block
        assert_eq!(status(0x12, 0x12), MintLockStatus::Confirmed);
        assert_eq!(status(0x12, 0x11), MintLockStatus::Expired);
        // with no lock as of block 0x20, a lock can only be in block 0x21 or later
        assert_eq!(status(0, 0x22), MintLockStatus::Pending);
        assert_eq!(status(0, 0x21), MintLockStatus::Expired);
        assert_eq!(status(0, 0x10), MintLockStatus::Expired);
    }

    #[test]
    fn is_confirmed_reads_finalized_record() {
        let location = BridgeLocation::Evm(EvmContract {
            chain_id: 31337,
            address: EvmAddress::default(),
        });
        let opid = OpId::from([0u8; 32]);
        let witness_id = RgbTxid::from_byte_array([1u8; 32]);
        let claim_in = |witness_id: RgbTxid, amount: u64, before_block: u64| Claim::Lock {
            opid,
            witness_id,
            amount,
            before_block,
        };
        let claim = |amount: u64, before_block: u64| claim_in(witness_id, amount, before_block);
        let lock_id = LockId::with(opid, witness_id);
        let other_witness_id = RgbTxid::from_byte_array([2u8; 32]);
        let claims = [
            claim(7, 0x30),
            claim(8, 0x30),
            claim_in(other_witness_id, 7, 0x30),
            claim(7, 0x11),
        ];
        // the lock of 7 recorded in block 0x11, read as of the finalized block 0x20
        let mut server = mockito::Server::new();
        let chain_id = mock_rpc(
            &mut server,
            r#"{"method":"eth_chainId"}"#,
            r#""result":"0x7a69""#,
        );
        let _finalized = mock_finalized_block(&mut server, 0x20);
        let record = mock_eth_call_hits(
            &mut server,
            0x20,
            &funds_in_call(&lock_id, 7),
            &format!("0x{:064x}", 0x12),
            2,
        );
        let no_record = mock_eth_call_hits(
            &mut server,
            0x20,
            &funds_in_call(&lock_id, 8),
            &format!("0x{:064x}", 0),
            1,
        );
        let other_tx_no_record = mock_eth_call_hits(
            &mut server,
            0x20,
            &funds_in_call(&LockId::with(opid, other_witness_id), 7),
            &format!("0x{:064x}", 0),
            1,
        );
        let client = EvmClient::new(&server.url());
        let resolver = block_on(MintClaimResolver::fetch(&client, &location, &claims)).unwrap();
        assert!(resolver.is_confirmed(&location, &claims[0]).unwrap());
        // the record is keyed by the amount too
        assert!(!resolver.is_confirmed(&location, &claims[1]).unwrap());
        // and by the witness TX
        assert!(!resolver.is_confirmed(&location, &claims[2]).unwrap());
        // the lock must be in a block before the one the claim commits to
        assert!(!resolver.is_confirmed(&location, &claims[3]).unwrap());
        assert!(resolver.is_expired(&opid));
        chain_id.assert();
        record.assert();
        no_record.assert();
        other_tx_no_record.assert();
        // a claim not fetched is an issue
        assert!(matches!(
            resolver.is_confirmed(&location, &claim(9, 0x30)),
            Err(ClaimResolverError::ResolverIssue(_))
        ));

        // a node on another chain reads no record
        let mut server = mockito::Server::new();
        let _chain_id = mock_rpc(
            &mut server,
            r#"{"method":"eth_chainId"}"#,
            r#""result":"0x1""#,
        );
        let no_call = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(s!(
                r#"{"method":"eth_call"}"#
            )))
            .expect(0)
            .create();
        let client = EvmClient::new(&server.url());
        let resolver = block_on(MintClaimResolver::fetch(&client, &location, &claims)).unwrap();
        assert!(matches!(
            CheckedClaimResolver::with(&resolver, location),
            Err(ClaimResolverError::WrongChainId {
                expected: 31337,
                actual: 1
            })
        ));
        no_call.assert();
    }
}

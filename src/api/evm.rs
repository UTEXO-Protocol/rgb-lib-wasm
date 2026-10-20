use super::*;

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
        let req_err = |e: reqwest::Error| Error::Network {
            details: format!("EVM RPC: {e}"),
        };
        self.client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(req_err)?
            .json()
            .await
            .map_err(req_err)
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

    /// Whether `contract` emitted a `FundsIn` event for `opid` with the exact `amount`, after
    /// `after_block` and in a finalized block.
    /// A log range that the node refuses is halved and requested again.
    pub(crate) async fn find_funds_in(
        &self,
        contract: &EvmAddress,
        opid: &OpId,
        amount: u64,
        after_block: u64,
    ) -> Result<bool, Error> {
        let last_block = self.finalized_block_number().await?;
        let mut block_range = LOGS_BLOCK_RANGE;
        let mut from_block = after_block.saturating_add(1);
        while from_block <= last_block {
            let to_block = min(from_block.saturating_add(block_range - 1), last_block);
            let filter = LogFilter {
                address: contract.to_string(),
                from_block: format!("0x{from_block:x}"),
                to_block: format!("0x{to_block:x}"),
                topics: vec![
                    Some(FUNDS_IN_TOPIC.to_string()),
                    None,
                    Some(format!("0x{}", hex::encode(opid.to_byte_array()))),
                ],
            };
            let resp: RpcResponse<Vec<EvmLog>> = self.call("eth_getLogs", [filter]).await?;
            if let Some(err) = resp.error {
                if to_block > from_block {
                    block_range = (to_block - from_block).div_ceil(2);
                    continue;
                }
                return Err(Error::Network {
                    details: format!("eth_getLogs error {}: {}", err.code, err.message),
                });
            }
            let logs = resp.result.ok_or_else(|| Error::Network {
                details: s!("eth_getLogs returned null result"),
            })?;
            for log in logs {
                if let Some(event) = log.as_funds_in()? {
                    if event.confirms(opid, amount, after_block) {
                        return Ok(true);
                    }
                }
            }
            let Some(next_block) = to_block.checked_add(1) else {
                break;
            };
            from_block = next_block;
        }
        Ok(false)
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
        let find = |finalized: u64, logs: &[String]| {
            let mut server = mockito::Server::new();
            let finalized_mock = mock_finalized_block(&mut server, finalized);
            let logs_mock = mock_get_logs(
                &mut server,
                17,
                finalized,
                &format!(r#""result":[{}]"#, logs.join(",")),
            );
            let found =
                block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16))
                    .unwrap();
            finalized_mock.assert();
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

        // finality is not past the anchor block: no log is requested
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 16);
        let get_logs = server
            .mock("POST", "/")
            .match_body(mockito::Matcher::PartialJsonString(s!(
                r#"{"method":"eth_getLogs"}"#
            )))
            .expect(0)
            .create();
        assert!(
            !block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16)).unwrap()
        );
        get_logs.assert();
    }

    #[test]
    fn find_funds_in_shrinks_refused_range() {
        let opid = OpId::from([0u8; 32]);
        let address = contract();

        // the refused range is halved
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 20);
        let refused = mock_get_logs(&mut server, 17, 20, RANGE_TOO_LARGE);
        let ranges =
            [(17, 18), (19, 20)].map(|(from, to)| mock_get_logs(&mut server, from, to, NO_LOGS));
        assert!(
            !block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16)).unwrap()
        );
        refused.assert();
        ranges.iter().for_each(|r| r.assert());

        // a refused single block gives the node error
        let mut server = mockito::Server::new();
        let _finalized = mock_finalized_block(&mut server, 18);
        let _refused = [(17, 18), (17, 17)]
            .map(|(from, to)| mock_get_logs(&mut server, from, to, RANGE_TOO_LARGE));
        let result = block_on(EvmClient::new(&server.url()).find_funds_in(&address, &opid, 7, 16));
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
    }
}

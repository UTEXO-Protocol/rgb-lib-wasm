//! RGB wallet
//!
//! This module defines the online methods of the [`Wallet`] structure and all its related data.

use super::*;

const SCHEMAS_SUPPORTING_INFLATION: [database::enums::AssetSchema; 1] = [AssetSchema::Ifa];
const SCHEMAS_SUPPORTING_BURN: [database::enums::AssetSchema; 2] =
    [AssetSchema::Ifa, AssetSchema::Bfa];

const TRANSFER_DATA_FILE: &str = "transfer_data.txt";
const SIGNED_PSBT_FILE: &str = "signed.psbt";

pub(crate) const UTXO_SIZE: u32 = 1000;
pub(crate) const UTXO_NUM: u8 = 5;

pub(crate) const MIN_FEE_RATE: u64 = 1;

// how long an operation with no expiration of its own (an inflation) keeps its inputs reserved
pub(crate) const DURATION_SEND_TRANSFER: i64 = 3600;

pub(crate) const MIN_BLOCK_ESTIMATION: u16 = 1;
pub(crate) const MAX_BLOCK_ESTIMATION: u16 = 1008;

const FASCIA_MAX_LEN: usize = amplify::confinement::U32;

struct BeginOperationData {
    psbt: String,
    fascia: Vec<u8>,
    entropy: u64,
    batch_transfer_idx: Option<i32>,
}

enum PrepareTransferPsbtResult {
    Retry,
    Success(BeginOperationData),
}

type TransferEndData = (
    Psbt,
    String,
    PathBuf,
    InfoBatchTransfer,
    BTreeMap<String, InfoAssetTransfer>,
);

/// Collection of different RGB assignments.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssignmentsCollection {
    /// Fungible assignments
    pub fungible: u64,
    /// Non-fungible assignments
    pub non_fungible: bool,
    /// Inflation assignments
    pub inflation: u64,
    /// Mint rights
    pub mint: u64,
}

impl AssignmentsCollection {
    fn add_fungible(&mut self, amt: u64) {
        self.fungible += amt;
    }

    fn add_non_fungible(&mut self) {
        self.non_fungible = true;
    }

    fn add_inflation(&mut self, amt: u64) {
        self.inflation += amt;
    }

    fn add_opout_state(&mut self, opout: &Opout, state: &AllocatedState) {
        match state {
            AllocatedState::Amount(amt) if opout.ty == OS_ASSET => {
                self.add_fungible(amt.as_u64());
            }
            AllocatedState::Amount(amt) if opout.ty == OS_INFLATION => {
                self.add_inflation(amt.as_u64());
            }
            AllocatedState::Data(_) => {
                self.add_non_fungible();
            }
            AllocatedState::Void if opout.ty == OS_MINT => {
                self.mint += 1;
            }
            _ => {}
        }
    }

    fn opout_contributes(&self, opout: &Opout, state: &AllocatedState, needed: &Self) -> bool {
        match (state, opout.ty) {
            (AllocatedState::Amount(_), OS_ASSET) => {
                needed.fungible.saturating_sub(self.fungible) > 0
            }
            (AllocatedState::Amount(_), OS_INFLATION) => {
                needed.inflation.saturating_sub(self.inflation) > 0
            }
            (AllocatedState::Data(_), _) => needed.non_fungible && !self.non_fungible,
            (AllocatedState::Void, OS_MINT) => needed.mint > self.mint,
            _ => false,
        }
    }

    fn change(&self, needed: &Self) -> Self {
        Self {
            fungible: self.fungible - needed.fungible,
            non_fungible: false,
            inflation: self.inflation - needed.inflation,
            mint: self.mint - needed.mint,
        }
    }

    fn enough(&self, needed: &Self) -> bool {
        if self.fungible < needed.fungible {
            return false;
        }
        if self.non_fungible != needed.non_fungible {
            return false;
        }
        if self.inflation < needed.inflation {
            return false;
        }
        if self.mint < needed.mint {
            return false;
        }
        true
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AssetSpend {
    txo_map: HashMap<i32, Outpoint>,
    assignments_collected: AssignmentsCollection,
    input_btc_amt: u64,
}

/// The result of a send operation
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OperationResult {
    /// ID of the transaction
    pub txid: String,
    /// Batch transfer idx
    pub batch_transfer_idx: i32,
    /// Entropy used for the merkle tree construction operation
    pub entropy: u64,
}

/// The result of a burn begin operation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct BurnBeginResult {
    /// PSBT to inspect and sign
    pub psbt: String,
    /// Batch transfer idx, None when `dry_run: true`
    pub batch_transfer_idx: Option<i32>,
    /// Operation details
    pub details: BurnDetails,
}

/// Details for burn operations.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct BurnDetails {
    /// Strict-encoded fascia, for inspection
    pub fascia: Vec<u8>,
    /// Minimum confirmations for the operation
    pub min_confirmations: u8,
    /// Entropy used for the merkle tree construction operation
    pub entropy: u64,
    /// Hex-encoded 32-byte reason committed in the burn transition, if any
    pub burn_reason: Option<String>,
}

/// The result of a mint begin operation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct MintBeginResult {
    /// PSBT to inspect and sign
    pub psbt: String,
    /// Batch transfer idx
    pub batch_transfer_idx: i32,
    /// Operation details
    pub details: MintDetails,
}

/// Details for mint operations.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct MintDetails {
    /// Strict-encoded fascia, for inspection
    pub fascia: Vec<u8>,
    /// Minimum confirmations for the operation
    pub min_confirmations: u8,
    /// Entropy used for the merkle tree construction operation
    pub entropy: u64,
    /// ID of the mint operation
    pub opid: String,
}

/// The result of a send begin operation.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct SendBeginResult {
    /// PSBT to inspect and sign
    pub psbt: String,
    /// Batch transfer idx, None when `dry_run: true`
    pub batch_transfer_idx: Option<i32>,
    /// Operation details
    pub details: SendDetails,
}

/// Details for send operations.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct SendDetails {
    /// Strict-encoded fascia, for inspection
    pub fascia: Vec<u8>,
    /// Minimum confirmations for the operation
    pub min_confirmations: u8,
    /// Entropy used for the merkle tree construction operation
    pub entropy: u64,
    /// Whether this is a donation transfer
    pub is_donation: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BtcChange {
    vout: u32,
    amount: u64,
}

// map txo idx to assignments
type TxoAssignments = HashMap<i32, Vec<Assignment>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct InfoBatchTransfer {
    btc_change: Option<BtcChange>,
    change_utxo_idx: Option<i32>,
    extra_allocations: HashMap<String, TxoAssignments>,
    donation: bool,
    min_confirmations: u8,
    expiration_timestamp: Option<i64>,
    entropy: u64,
    /// the key of the operation, one per operation and shared across all recipients
    operation_secret_key: Option<Vec<u8>>,
    /// the sender's own proxy endpoint, where the recipients post their ACK
    sender_transport_endpoint: Option<String>,
}

/// Where the reject list of an asset is kept.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum RejectListSource {
    /// A list of opouts served at a URL
    Url(String),
    /// A list kept by an `IRgbRejectList` EVM contract
    Evm(EvmContract),
}

impl From<RejectListLocation> for RejectListSource {
    fn from(location: RejectListLocation) -> Self {
        match location {
            RejectListLocation::Url(url) => Self::Url(url.to_string()),
            RejectListLocation::Evm(contract) => Self::Evm(contract),
        }
    }
}

/// The reject and allow opouts of each reject list fetched by an operation.
type RejectLists = HashMap<RejectListSource, (HashSet<Opout>, HashSet<Opout>)>;

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AssetInfo {
    contract_id: ContractId,
    reject_list_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct InfoAssetTransfer {
    asset_info: AssetInfo,
    recipients: Vec<LocalRecipient>,
    asset_spend: AssetSpend,
    change: AssignmentsCollection,
    original_assignments_needed: AssignmentsCollection,
    assignments_needed: AssignmentsCollection,
    assignments_spent: TxoAssignments,
    main_transition: TypeOfTransition,
    burn_reason: Option<BurnReason>,
    /// External chain block the lock of a mint has to be included before
    before_block: Option<u64>,
}

/// The outcome of [`Wallet::_resolve_claims`].
enum ClaimsResolution {
    /// The consignment has no claims to resolve
    NoClaims,
    /// No usable EVM RPC endpoint for the chain of the bridge: retry later
    Retry,
    /// The claims were resolved, except the `unconfirmed` ones
    Resolved {
        evm_client: EvmClient,
        chain_id: u64,
        address: EvmAddress,
        unconfirmed: Vec<OpId>,
    },
}

/// In-memory storage for transfer artifacts (replaces filesystem directories).
#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct TransferArtifacts {
    pub(crate) batch_info: Option<InfoBatchTransfer>,
    pub(crate) asset_infos: BTreeMap<String, InfoAssetTransfer>,
    pub(crate) consignment_bytes: HashMap<String, Vec<u8>>,
    pub(crate) signed_psbt: Option<String>,
    pub(crate) inputs: Vec<Outpoint>,
    pub(crate) expiration: i64,
    pub(crate) fascia: Option<Vec<u8>>,
}

impl TransferArtifacts {
    /// Whether the main transition of every asset transfer of these artifacts is accepted.
    fn is_operation(&self, is_main_transition: impl Fn(&TypeOfTransition) -> bool) -> bool {
        !self.asset_infos.is_empty()
            && self
                .asset_infos
                .values()
                .all(|t| is_main_transition(&t.main_transition))
    }
}

/// Request the ACKs of the operation of `transfer_ids` from the sender proxy (`ack.get`), signing
/// the request with the operation (sender) key.
async fn get_ack_signed(
    proxy_client: &ProxyClient,
    transfer_ids: &[String],
    sender_sk: &SigningKey,
) -> Result<JsonRpcResponse<GetAckResponse>, Error> {
    let timestamp = now().unix_timestamp().to_string();
    let digest = sha256_digest(get_ack_message(transfer_ids, &timestamp).as_bytes());
    let signature = hex::encode(sign_ed25519(sender_sk, &digest));
    proxy_client
        .get_ack(transfer_ids, &signature, &timestamp)
        .await
}

/// The burn transitions of the contract in the fascia.
fn burn_opids(fascia: &Fascia, contract_id: ContractId) -> Vec<OpId> {
    fascia
        .bundles()
        .get(&contract_id)
        .map(|bundle| {
            bundle
                .known_transitions
                .iter()
                .filter(|kt| kt.transition.transition_type == TS_BURN)
                .map(|kt| kt.opid)
                .collect()
        })
        .unwrap_or_default()
}

fn fascia_from_bytes(fascia_bytes: Vec<u8>) -> Result<Fascia, Error> {
    Fascia::from_strict_serialized::<FASCIA_MAX_LEN>(Confined::try_from(fascia_bytes).map_err(
        |e| Error::Internal {
            details: e.to_string(),
        },
    )?)
    .map_err(|e| Error::Internal {
        details: e.to_string(),
    })
}

/// The mint transition of the contract in the fascia.
fn mint_opid(fascia: &Fascia, contract_id: ContractId) -> Option<OpId> {
    fascia
        .bundles()
        .get(&contract_id)?
        .known_transitions
        .iter()
        .find(|kt| kt.transition.transition_type == TS_MINT)
        .map(|kt| kt.opid)
}

/// Whether a received mint pays the amount the invoice requests, if it requests one.
fn mint_pays_requested(transfer: &DbTransfer, received: &HashMap<Opout, Assignment>) -> bool {
    match transfer.requested_assignment {
        Some(Assignment::Fungible(requested)) => {
            received.values().map(|a| a.main_amount()).sum::<u64>() == requested
        }
        _ => true,
    }
}

/// List the transfers to an invoice on a recipient proxy (`transfer.list`), signing the request
/// with the recipient key.
async fn list_invoice_transfers(
    proxy_url: &str,
    invoice_id: &str,
    recipient_seed: &[u8],
) -> Result<JsonRpcResponse<HashMap<String, ProxyTransferState>>, Error> {
    let seed: [u8; 32] = recipient_seed.try_into().map_err(|_| Error::Internal {
        details: s!("invalid recipient secret key"),
    })?;
    let sk = ed25519_from_seed(&seed);
    let timestamp = now().unix_timestamp().to_string();
    let digest = sha256_digest(list_transfers_message(invoice_id, &timestamp).as_bytes());
    let signature = hex::encode(sign_ed25519(&sk, &digest));
    ProxyClient::new(proxy_url)?
        .list_transfers(invoice_id, &signature, &timestamp)
        .await
}

/// Whether the sender proxy knows the operation of `transfer_ids`, registered with the operation
/// (sender) key, i.e. whether an earlier `ack.prepare` for it succeeded.
async fn sender_proxy_knows_operation(
    proxy_client: &ProxyClient,
    transfer_ids: &[String],
    sender_sk: &SigningKey,
) -> Result<bool, Error> {
    match get_ack_signed(proxy_client, transfer_ids, sender_sk)
        .await?
        .error
    {
        None => Ok(true),
        Some(err) => match map_proxy_error(err) {
            Error::OperationNotFound => Ok(false),
            e => Err(e),
        },
    }
}

/// Interpret the sender proxy's response to `ack.clean`, requested before deliberately failing
/// an outgoing operation.
///
/// Any proxy response other than the ones mapped to a [`CleanAckOutcome`] cannot be unambiguously
/// interpreted safely and is surfaced as an error.
fn clean_ack_outcome(error: Option<JsonRpcError>) -> Result<CleanAckOutcome, Error> {
    match error {
        None => Ok(CleanAckOutcome::Revoked),
        Some(err) => match map_proxy_error(err) {
            Error::ProxyTxAlreadyReleased => Ok(CleanAckOutcome::Released),
            Error::OperationNotFound => Ok(CleanAckOutcome::Unknown),
            e => Err(e),
        },
    }
}

/// Whether the expiration of an outgoing operation has passed on the local clock by more than the
/// clock skew margin, so it has passed on the sender proxy's clock as well and the proxy can no
/// longer release the signed TX, whatever it knows about the operation.
fn expired_beyond_skew_margin(batch_transfer: &DbBatchTransfer) -> bool {
    batch_transfer.is_expired_with_margin(now().unix_timestamp(), PROXY_CLOCK_SKEW_MARGIN_SECS)
}

/// Whether the broadcast deadline of an outgoing operation (its expiration plus the sender proxy
/// retention grace, or just its expiration if unknown) has passed on the local clock by more than
/// the clock skew margin, so the sender proxy can no longer hand out the signed TX, not even to
/// recipients that already obtained it, and recipients stopped broadcasting it.
fn broadcast_deadline_passed_beyond_skew_margin(batch_transfer: &DbBatchTransfer) -> bool {
    batch_transfer
        .broadcast_deadline
        .or(batch_transfer.expiration)
        .is_some_and(|deadline| {
            deadline.saturating_add(i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS))
                < now().unix_timestamp()
        })
}

/// Whether the broadcast deadline of an incoming transfer plus the confirmation margin has passed,
/// after which a transfer whose TX is not confirmed is failed. A transfer with no known broadcast
/// deadline is never considered past it.
fn confirmation_deadline_passed(batch_transfer: &DbBatchTransfer) -> bool {
    batch_transfer.broadcast_deadline.is_some_and(|deadline| {
        deadline.saturating_add(i64::from(CONFIRMATION_MARGIN_SECS)) < now().unix_timestamp()
    })
}

/// Whether an incoming transfer is expired, so it can be failed: a transfer waiting for the
/// broadcast of a TX the sender proxy may still release to it (it has a broadcast deadline) only
/// once past the broadcast deadline plus the confirmation margin, a blind receive reserved on
/// proxies (it has a retention deadline) still waiting for the counterparty only once the invoice
/// expiration has passed on the proxies' clocks too, any other once past the invoice expiration.
fn incoming_expired(batch_transfer: &DbBatchTransfer, now: i64) -> bool {
    if batch_transfer.status == TransferStatus::WaitingBroadcast
        && batch_transfer.broadcast_deadline.is_some()
    {
        return confirmation_deadline_passed(batch_transfer);
    }
    if batch_transfer.status == TransferStatus::WaitingCounterparty
        && batch_transfer.retention_deadline.is_some()
    {
        return batch_transfer.is_expired_with_margin(now, PROXY_CLOCK_SKEW_MARGIN_SECS);
    }
    batch_transfer.is_expired(now)
}

/// The retention grace advertised by a sender proxy (`retention_grace_secs`), i.e. how long
/// after an operation expiration it keeps a released signed TX available to the recipients. A
/// proxy not advertising it is considered to grant none.
async fn sender_proxy_retention_grace(proxy_url: &str) -> Result<u32, Error> {
    let res = ProxyClient::new(proxy_url)?.get_info().await?;
    if let Some(err) = res.error {
        return Err(map_proxy_error(err));
    }
    let info = res.result.ok_or_else(|| Error::Proxy {
        details: s!("missing server.info result"),
    })?;
    Ok(crate::utils::proxy_retention_grace(&info))
}

/// The broadcast deadline of an operation from the `expires_at` returned by `ack.post` and the
/// sender proxy retention grace, `None` if the proxy didn't return a valid expiration.
fn broadcast_deadline(expires_at: Option<&str>, retention_grace: u32) -> Option<i64> {
    expires_at
        .and_then(|exp| exp.parse::<i64>().ok())
        .map(|exp| exp + i64::from(retention_grace))
}

/// Interpret an error returned by the sender proxy to `ack.get`, requested while waiting for the
/// recipients' ACKs.
///
/// The only error with a safe interpretation is the proxy no longer knowing the operation (e.g.
/// it dropped its data): the ACKs cannot be retrieved anymore, so the caller falls back to
/// checking whether the TX was broadcast. That case returns `Ok(())`. Any other proxy response
/// cannot be unambiguously interpreted safely and is surfaced as an error.
fn check_get_ack_error(err: JsonRpcError) -> Result<(), Error> {
    match map_proxy_error(err) {
        Error::OperationNotFound => Ok(()),
        e => Err(e),
    }
}

/// The sender proxy's response to `ack.clean`, requested before failing an outgoing operation.
#[derive(Debug)]
pub(crate) enum CleanAckOutcome {
    /// the signed TXs were revoked: nothing can be released anymore
    Revoked,
    /// the TX was already released to a recipient, who may still broadcast it
    Released,
    /// the proxy doesn't know the operation: it was never registered (send_end not completed),
    /// or the proxy lost it, in which case whether the TX was released cannot be known
    Unknown,
}

pub(crate) enum TryFailBatchTransferOutcome {
    Failed,
    Refreshed,
    /// the transfer was not failed: the released signed TX could not be revoked (ack.clean
    /// denied), or the sender proxy no longer knows the operation and it has not expired yet
    Denied,
}

/// The state a transfer's refresh changes, to restore when it fails.
struct TransferSavepoint {
    database: RgbLibDb,
    stock: Option<MemRgbStore>,
}

/// Proxy context needed to receive (and decrypt) a consignment via a proxy server.
#[derive(Clone)]
pub(crate) struct ProxyReceiveData {
    /// endpoint the consignment was downloaded from
    pub(crate) proxy_url: String,
    /// ID of the transfer on the proxies
    pub(crate) transfer_id: String,
    /// sender's proxy endpoint (HTTP URL), where the ACK is posted
    pub(crate) sender_proxy: String,
    /// sender's Ed25519 public key (hex), used to derive the shared secret for decryption
    pub(crate) sender_pubkey: String,
    /// hash of the (plaintext) consignment, signed into the ACK and used for blob verification
    pub(crate) consignment_hash: String,
    /// recipient's encryption preference for this operation
    pub(crate) recipient_encryption: Encryption,
    /// whether the operation is a batch (multiple recipients)
    pub(crate) batch: bool,
}

/// The assignments of a type an operation moves with a transition of the type's default type,
/// beside its main transition (see [`add_or_side_output`]).
#[derive(Default)]
struct SideTransfer {
    inputs: Vec<(Opout, AllocatedState)>,
    outputs: Vec<(BuilderSeal<GraphSeal>, AllocatedState)>,
    /// Set when the main transition can't assign this type but still spends it (a burn), and there
    /// is change: the output of this transition that the main transition then spends, as its seal
    /// and amount. The other outputs are the change.
    split: Option<(GraphSeal, u64)>,
}

/// The amount of `state`, 0 for a state that isn't an amount.
fn state_amount(state: &AllocatedState) -> u64 {
    match state {
        AllocatedState::Amount(amt) => amt.as_u64(),
        _ => 0,
    }
}

/// Assign `state` of type `ty` to `seal` through the main transition when it can assign the type
/// (and set `main_used`), otherwise through `side_transfers`.
fn add_or_side_output(
    builder: TransitionBuilder,
    main_assignment_types: &HashSet<AssignmentType>,
    main_used: &mut bool,
    side_transfers: &mut BTreeMap<AssignmentType, SideTransfer>,
    ty: AssignmentType,
    seal: BuilderSeal<GraphSeal>,
    state: AllocatedState,
) -> Result<TransitionBuilder, Error> {
    if main_assignment_types.contains(&ty) {
        *main_used = true;
        return Ok(builder.add_owned_state_raw(ty, seal, state)?);
    }
    side_transfers
        .entry(ty)
        .or_default()
        .outputs
        .push((seal, state));
    Ok(builder)
}

/// A destination at which a consignment pays this recipient, see `find_payments`.
pub(crate) struct ConsignmentPayment<'c> {
    pub(crate) bundle: &'c WitnessBundle,
    /// for witness receives, the vout of the bundle witness TX being paid
    pub(crate) vout: Option<u32>,
    pub(crate) receiving: HashMap<Opout, Assignment>,
}

pub(crate) enum ReceiveMatcher {
    Blind(SecretSeal),
    /// the script paid and the blinding of the seal, derived from the keys of both parties
    Witness(ScriptBuf, u64),
}

impl DbTransfer {
    pub(crate) fn receive_matcher(&self, blinding: u64) -> Result<ReceiveMatcher, Error> {
        let recipient_id = self
            .recipient_id
            .clone()
            .expect("transfer should have a recipient ID");
        match self
            .recipient_type
            .as_ref()
            .expect("transfer should have a recipient type")
        {
            RecipientTypeFull::Blind { .. } => {
                let beneficiary = XChainNet::<Beneficiary>::from_str(&recipient_id)
                    .expect("saved recipient ID is invalid");
                match beneficiary.into_inner() {
                    Beneficiary::BlindedSeal(secret_seal) => Ok(ReceiveMatcher::Blind(secret_seal)),
                    _ => unreachable!("beneficiary is blinded"),
                }
            }
            RecipientTypeFull::Witness { .. } => {
                let script_pubkey = script_buf_from_recipient_id(recipient_id)?
                    .expect("witness recipient ID should yield a script");
                Ok(ReceiveMatcher::Witness(script_pubkey, blinding))
            }
        }
    }
}

pub(crate) enum Indexer {
    EsploraAsync(Box<esplora_client::AsyncClient<crate::utils::WasmSleeper>>),
}

impl Indexer {
    pub(crate) async fn block_hash(&self, height: usize) -> Result<String, IndexerError> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let hash = client.get_block_hash(height as u32).await?;
                hash.to_string()
            }
        })
    }

    pub(crate) async fn broadcast(&self, tx: &BdkTransaction) -> Result<(), IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                client.broadcast(tx).await?;
                Ok(())
            }
        }
    }

    pub(crate) async fn fee_estimation(&self, blocks: u16) -> Result<f64, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let estimate_map = client
                    .get_fee_estimates()
                    .await
                    .map_err(IndexerError::from)?; // in sat/vB
                if estimate_map.is_empty() {
                    return Err(Error::CannotEstimateFees);
                }
                // map needs to be sorted for interpolation to work
                let estimate_map = BTreeMap::from_iter(estimate_map);
                match estimate_map.get(&blocks) {
                    Some(estimate) => *estimate,
                    None => {
                        // find the two closest keys
                        let mut lower_key = None;
                        let mut upper_key = None;
                        for k in estimate_map.keys() {
                            match k.cmp(&blocks) {
                                Ordering::Less => {
                                    lower_key = Some(k);
                                }
                                Ordering::Greater => {
                                    upper_key = Some(k);
                                    break;
                                }
                                _ => unreachable!("already handled"),
                            }
                        }
                        // use linear interpolation formula
                        match (lower_key, upper_key) {
                            (Some(x1), Some(x2)) => {
                                let y1 = estimate_map[x1];
                                let y2 = estimate_map[x2];
                                y1 + (blocks as f64 - *x1 as f64) / (*x2 as f64 - *x1 as f64)
                                    * (y2 - y1)
                            }
                            _ => {
                                return Err(Error::Internal {
                                    details: s!("esplora map doesn't contain the expected keys"),
                                });
                            }
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn full_scan<K: Ord + Clone + Send, R: Into<FullScanRequest<K>> + Send>(
        &self,
        request: R,
    ) -> Result<FullScanResponse<K>, IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                use bdk_esplora::EsploraAsyncExt;
                client
                    .full_scan(request, INDEXER_STOP_GAP, INDEXER_PARALLEL_REQUESTS)
                    .await
                    .map_err(|e| IndexerError::EsploraAsync(e.to_string()))
            }
        }
    }

    pub(crate) async fn get_tx_confirmations(&self, txid: &str) -> Result<Option<u64>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let txid = Txid::from_str(txid).unwrap();
                let tx_status = client
                    .get_tx_status(&txid)
                    .await
                    .map_err(IndexerError::from)?;
                if let Some(tx_height) = tx_status.block_height {
                    let height = client.get_height().await.map_err(IndexerError::from)?;
                    Some((height - tx_height + 1) as u64)
                } else if client
                    .get_tx(&txid)
                    .await
                    .map_err(IndexerError::from)?
                    .is_none()
                {
                    None
                } else {
                    Some(0)
                }
            }
        })
    }

    pub(crate) async fn get_tx_height(&self, txid: &str) -> Result<Option<u32>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let txid = Txid::from_str(txid).unwrap();
                let tx_status = client
                    .get_tx_status(&txid)
                    .await
                    .map_err(IndexerError::from)?;
                tx_status.block_height
            }
        })
    }

    pub(crate) async fn get_tx_with_status(
        &self,
        txid: &Txid,
    ) -> Result<Option<(BdkTransaction, Option<u32>, Option<u64>)>, Error> {
        Ok(match self {
            Indexer::EsploraAsync(client) => {
                let tx = client.get_tx(txid).await.map_err(IndexerError::from)?;
                if let Some(tx) = tx {
                    let status = client
                        .get_tx_status(txid)
                        .await
                        .map_err(IndexerError::from)?;
                    Some((tx, status.block_height, status.block_time))
                } else {
                    None
                }
            }
        })
    }

    pub(crate) fn populate_tx_cache(
        &self,
        #[allow(unused)] bdk_wallet: &PersistedWallet<super::offline::BdkPersister>,
    ) {
        match self {
            Indexer::EsploraAsync(_) => {}
        }
    }

    pub(crate) async fn sync<I: 'static + Send>(
        &self,
        request: impl Into<SyncRequest<I>> + Send,
    ) -> Result<SyncResponse, IndexerError> {
        match self {
            Indexer::EsploraAsync(client) => {
                use bdk_esplora::EsploraAsyncExt;
                client
                    .sync(request, INDEXER_PARALLEL_REQUESTS)
                    .await
                    .map_err(|e| IndexerError::EsploraAsync(e.to_string()))
            }
        }
    }
}

/// Options for the [`Wallet::go_online`] method.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct OnlineOptions {
    /// Indexer the wallet should use
    pub indexer_config: IndexerConfig,
    /// Whether to skip the consistency check when going online. This wallet runs no consistency
    /// check yet, so the value has no effect.
    pub skip_consistency_check: bool,
    /// Keys that sign calls to proxies that require authentication.
    ///
    /// Maps a transport endpoint (e.g. `rpc://host/json-rpc`) to a hex-encoded 32-byte Ed25519 seed.
    /// The keys stay in memory. Going online again replaces them.
    /// `refresh` uses these keys, so keep a correct key for every endpoint a pending transfer needs.
    pub proxy_auth_keys: HashMap<String, String>,
    /// EVM chains to be used for mint operations on bridged assets, by their EIP-155 chain ID.
    ///
    /// Minting a bridged asset, or receiving a mint of it, needs its chain here. Else
    /// [`Error::NoEvmRpcUrl`] is returned. Going online checks that each endpoint is on its chain
    /// and serves the latest finalized block and the contract state at it. Else
    /// [`Error::InvalidEvmRpcUrl`] is returned. It also checks the mint lock blocks of each chain
    /// ([`Error::InvalidEvmChain`]).
    ///
    /// Only used when the wallet supports the [`AssetSchema::Bfa`] schema.
    #[serde(default, deserialize_with = "deserialize_evm_chains")]
    pub evm_chains: HashMap<u64, EvmChain>,
}

/// An EVM chain mint operations use, see [`OnlineOptions::evm_chains`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct EvmChain {
    /// URL of the EVM JSON-RPC endpoint connected to the chain.
    ///
    /// The wallet trusts this endpoint to report locks and reject lists faithfully. A false
    /// report makes it refuse a valid mint or accept an invalid one. Use an endpoint you trust.
    pub rpc_url: String,
    /// Minimum number of blocks after the latest block that the `before_block` of a mint must be
    /// (see [`Wallet::mint_begin`]). It leaves time for the lock. Received mints skip this check.
    ///
    /// A value below 2 acts as 2. It must be at most
    /// [`max_mint_lock_blocks`](EvmChain::max_mint_lock_blocks), else going online fails with
    /// [`Error::InvalidEvmChain`].
    pub min_mint_lock_blocks: u64,
    /// Maximum number of blocks after the latest block that the `before_block` of a mint can be.
    ///
    /// A received mint further than this is refused, unless its lock is already recorded. Minting
    /// refuses it too (see [`Wallet::mint_begin`]). A value below 2 refuses every mint.
    pub max_mint_lock_blocks: u64,
}

/// Read the chain IDs of [`OnlineOptions::evm_chains`] from object keys, which are strings.
fn deserialize_evm_chains<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<HashMap<u64, EvmChain>, D::Error> {
    HashMap::<String, EvmChain>::deserialize(deserializer)?
        .into_iter()
        .map(|(chain_id, chain)| Ok((chain_id.parse().map_err(de::Error::custom)?, chain)))
        .collect()
}

/// Indexer the wallet connects to, see [`OnlineOptions::indexer_config`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum IndexerConfig {
    /// An indexer implementing the esplora protocol.
    Esplora {
        /// URL of the esplora server
        url: String,
    },
}

impl IndexerConfig {
    /// URL of the indexer.
    pub fn url(&self) -> &str {
        match self {
            Self::Esplora { url } => url,
        }
    }
}

/// Parse and validate the [`OnlineOptions::proxy_auth_keys`], keying them by proxy URL.
fn parse_proxy_auth_keys(
    proxy_auth_keys: &HashMap<String, String>,
) -> Result<HashMap<String, [u8; 32]>, Error> {
    let mut keys = HashMap::new();
    for (endpoint, seed) in proxy_auth_keys {
        let proxy_url = TransportEndpoint::new(endpoint.clone())?.endpoint;
        let seed: [u8; 32] = hex::decode(seed)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| Error::InvalidProxyAuthKey {
                details: format!(
                    "expected a hex-encoded 32-byte Ed25519 seed for endpoint {endpoint}"
                ),
            })?;
        if keys.insert(proxy_url.clone(), seed).is_some() {
            return Err(Error::DuplicateProxyAuthKey {
                details: format!(
                    "endpoint {endpoint} resolves to a proxy URL ({proxy_url}) already covered by another entry"
                ),
            });
        }
    }
    Ok(keys)
}

pub(crate) struct OnlineData {
    pub(crate) id: u64,
    pub(crate) indexer_url: String,
    indexer: Indexer,
    pub(crate) proxy_auth_keys: HashMap<String, [u8; 32]>,
    // EVM chains by chain ID, checked when going online
    evm_chains: HashMap<u64, EvmChain>,
}

impl OnlineData {
    /// Signature authenticating a call to the proxy at `proxy_url`, if a key has been configured
    /// for it. The proxy verifies the Ed25519 signature of the SHA-256 digest of `message` against
    /// its configured pubkeys.
    pub(crate) fn proxy_auth_signature(&self, proxy_url: &str, message: &str) -> Option<String> {
        self.proxy_auth_keys.get(proxy_url).map(|seed| {
            let sk = ed25519_from_seed(seed);
            let digest = sha256_digest(message.as_bytes());
            hex::encode(sign_ed25519(&sk, &digest))
        })
    }
}

#[cfg(test)]
impl OnlineData {
    /// Build online data for a native test. No connection is made.
    pub(crate) fn for_test(indexer_url: &str) -> Self {
        Self {
            id: 1,
            indexer_url: indexer_url.to_string(),
            indexer: crate::utils::build_indexer(indexer_url).unwrap(),
            proxy_auth_keys: HashMap::new(),
            evm_chains: HashMap::new(),
        }
    }
}

/// A transfer refresh filter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RefreshFilter {
    /// Transfer status
    pub status: RefreshTransferStatus,
    /// Whether the transfer is incoming
    pub incoming: bool,
}

/// A refreshed transfer
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RefreshedTransfer {
    /// The updated transfer status, if it has changed
    pub updated_status: Option<TransferStatus>,
    /// Optional failure
    pub failure: Option<Error>,
}

/// The result of a refresh operation
pub type RefreshResult = HashMap<i32, RefreshedTransfer>;

pub(crate) trait RefreshResultTrait {
    fn transfers_changed(&self) -> bool;
}

impl RefreshResultTrait for RefreshResult {
    fn transfers_changed(&self) -> bool {
        self.values().any(|rt| rt.updated_status.is_some())
    }
}

/// The pending status of a [`Transfer`] (eligible for refresh).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum RefreshTransferStatus {
    /// Waiting for the counterparty to take action
    WaitingCounterparty = 1,
    /// Waiting for the transfer transaction to be broadcasted
    WaitingBroadcast = 7,
    /// Waiting for the transfer transaction to reach the minimum number of confirmations
    WaitingConfirmations = 2,
}

impl TryFrom<TransferStatus> for RefreshTransferStatus {
    type Error = &'static str;

    fn try_from(x: TransferStatus) -> Result<Self, Self::Error> {
        match x {
            TransferStatus::WaitingCounterparty => Ok(RefreshTransferStatus::WaitingCounterparty),
            TransferStatus::WaitingBroadcast => Ok(RefreshTransferStatus::WaitingBroadcast),
            TransferStatus::WaitingConfirmations => Ok(RefreshTransferStatus::WaitingConfirmations),
            _ => Err("RefreshTransferStatus only accepts waiting statuses"),
        }
    }
}

impl Wallet {
    pub(crate) fn indexer(&self) -> &Indexer {
        &self.online_data.as_ref().unwrap().indexer
    }

    /// Fetch the status of each witness from the indexer.
    pub(crate) async fn prefetch_witnesses(
        &self,
        witness_ids: impl IntoIterator<Item = RgbTxid>,
    ) -> Result<PreFetchResolver, Error> {
        let mut cache = HashMap::new();
        for witness_id in witness_ids {
            let txid = Txid::from_str(&witness_id.to_string()).map_err(|_| Error::InvalidTxid)?;
            let status = match self.indexer().get_tx_with_status(&txid).await? {
                Some((tx, block_height, block_time)) => {
                    let witness_ord = block_height
                        .zip(block_time)
                        .and_then(|(h, t)| WitnessPos::bitcoin(NonZeroU32::new(h)?, t as i64))
                        .map_or(WitnessOrd::Tentative, WitnessOrd::Mined);
                    WitnessStatus::Resolved(tx, witness_ord)
                }
                None => WitnessStatus::Unresolved,
            };
            cache.insert(witness_id, status);
        }
        Ok(PreFetchResolver::new(cache, self.chain_net()))
    }

    fn _check_fee_rate(&self, fee_rate: u64) -> Result<FeeRate, Error> {
        if fee_rate < MIN_FEE_RATE {
            return Err(Error::InvalidFeeRate {
                details: format!("value under minimum {MIN_FEE_RATE}"),
            });
        }
        let Some(fee_rate) = FeeRate::from_sat_per_vb(fee_rate) else {
            return Err(Error::InvalidFeeRate {
                details: s!("value overflows"),
            });
        };
        Ok(fee_rate)
    }

    pub(crate) fn check_online(&self, online: Online) -> Result<(), Error> {
        if let Some(online_data) = &self.online_data {
            if online_data.id != online.id || online_data.indexer_url != online.indexer_url {
                error!(self.logger, "Cannot change online object");
                return Err(Error::CannotChangeOnline);
            }
        } else {
            error!(self.logger, "Wallet is offline");
            return Err(Error::Offline);
        }
        Ok(())
    }

    fn _check_xprv(&self) -> Result<(), Error> {
        if self.watch_only {
            error!(self.logger, "Invalid operation for a watch only wallet");
            return Err(Error::WatchOnly);
        }
        Ok(())
    }

    fn _create_split_tx(
        &mut self,
        inputs: &[BdkOutPoint],
        addresses: &Vec<ScriptBuf>,
        size: u32,
        fee_rate: FeeRate,
    ) -> Result<Psbt, bdk_wallet::error::CreateTxError> {
        let change_script = if self.wallet_data.reuse_addresses {
            Some(
                self._get_new_address(KeychainKind::Internal)
                    .map_err(|_| bdk_wallet::error::CreateTxError::NoUtxosSelected)?
                    .script_pubkey(),
            )
        } else {
            None
        };

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .add_utxos(inputs)
            .map_err(|_| bdk_wallet::error::CreateTxError::UnknownUtxo)?
            .manually_selected_only()
            .fee_rate(fee_rate);
        for address in addresses {
            tx_builder.add_recipient(address.clone(), BdkAmount::from_sat(size as u64));
        }
        if let Some(change) = change_script {
            tx_builder.drain_to(change);
        }
        let mut psbt = tx_builder.finish()?;
        self.add_non_witness_utxos(&mut psbt);
        Ok(psbt)
    }

    /// Fill missing previous TXs on taproot inputs.
    /// A separate signer needs them when its wallet has no chain data.
    fn add_non_witness_utxos(&self, psbt: &mut Psbt) {
        let tx_graph = self.bdk_wallet.tx_graph();
        for (input, txin) in psbt.inputs.iter_mut().zip(&psbt.unsigned_tx.input) {
            if input.non_witness_utxo.is_none()
                && let Some(prev_tx) = tx_graph.get_tx(txin.previous_output.txid)
            {
                input.non_witness_utxo = Some(prev_tx.as_ref().clone());
            }
        }
    }

    pub(crate) fn get_script_pubkey(&self, address: &str) -> Result<ScriptBuf, Error> {
        Ok(parse_address_str(address, self.bitcoin_network())?.script_pubkey())
    }

    fn _get_unspendable_bdk_outpoints(&self) -> Result<Vec<BdkOutPoint>, Error> {
        Ok(self
            .database
            .iter_txos()?
            .into_iter()
            .map(BdkOutPoint::from)
            .collect())
    }

    fn _transfer_savepoint(&self) -> TransferSavepoint {
        TransferSavepoint {
            database: (*self.database).clone(),
            stock: self
                .rgb_stock
                .borrow()
                .as_ref()
                .map(|s| s.as_store().clone()),
        }
    }

    #[allow(clippy::arc_with_non_send_sync)] // wasm32 is single-threaded; Arc matches native API
    fn _rollback_transfer(&mut self, savepoint: TransferSavepoint) {
        self.database = Arc::new(savepoint.database);
        *self.rgb_stock.borrow_mut() = savepoint.stock.map(Stock::with);
    }

    /// Clear the pending witness flag of the TXOs at `outpoints` or that pay one of `scripts`.
    fn _unflag_pending_witness_txos(
        &self,
        outpoints: &HashSet<Outpoint>,
        scripts: &HashSet<ScriptBuf>,
    ) -> Result<(), Error> {
        for txo in self.database.iter_pending_witness_txos()? {
            let outpoint = txo.outpoint();
            let pays_script = || {
                self.bdk_wallet
                    .tx_graph()
                    .get_txout(outpoint.clone().into())
                    .is_some_and(|txout| scripts.contains(&txout.script_pubkey))
            };
            if !outpoints.contains(&outpoint) && !pays_script() {
                continue;
            }
            let mut txo: DbTxoActMod = txo.into();
            txo.pending_witness = ActiveValue::Set(false);
            self.database.update_txo(txo)?;
        }
        Ok(())
    }

    /// Clear the pending witness flag of the TXOs of the witness receives of a failing batch.
    fn _clean_failed_witness_receives(
        &self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<(), Error> {
        let asset_transfers = self.database.iter_asset_transfers()?;
        let transfers = self.database.iter_transfers()?;
        if !batch_transfer.incoming(&asset_transfers, &transfers)? {
            return Ok(());
        }
        let mut outpoints = HashSet::new();
        let mut scripts = HashSet::new();
        let batch_transfer_data = batch_transfer.get_transfers(&asset_transfers, &transfers)?;
        for transfer in batch_transfer_data
            .asset_transfers_data
            .into_iter()
            .flat_map(|atd| atd.transfers)
        {
            let vout = match transfer.recipient_type {
                Some(RecipientTypeFull::Witness { vout }) => vout,
                _ => continue,
            };
            if let (Some(txid), Some(vout)) = (&batch_transfer.txid, vout) {
                outpoints.insert(Outpoint {
                    txid: txid.clone(),
                    vout,
                });
            }
            if let Some(recipient_id) = transfer.recipient_id
                && let Some(script) = script_buf_from_recipient_id(recipient_id)?
            {
                scripts.insert(script);
            }
        }
        self._unflag_pending_witness_txos(&outpoints, &scripts)
    }

    /// Drop the pending witness state of the witness receives that are not pending anymore and
    /// return the scripts still watched.
    fn _reconcile_pending_witness(&self) -> Result<HashSet<String>, Error> {
        let pending_witness_scripts = self.database.iter_pending_witness_scripts()?;
        let pending_witness_txos = self.database.iter_pending_witness_txos()?;
        if pending_witness_scripts.is_empty() && pending_witness_txos.is_empty() {
            return Ok(HashSet::new());
        }

        let db_data = self.database.get_db_data(false)?;
        let mut pending_scripts = HashSet::new();
        let mut pending_asset_transfers = HashSet::new();
        for batch_transfer in &db_data.batch_transfers {
            if !batch_transfer.status.pending()
                || !batch_transfer.incoming(&db_data.asset_transfers, &db_data.transfers)?
            {
                continue;
            }
            let batch_transfer_data =
                batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
            for transfer in batch_transfer_data
                .asset_transfers_data
                .into_iter()
                .flat_map(|atd| atd.transfers)
            {
                if !matches!(
                    transfer.recipient_type,
                    Some(RecipientTypeFull::Witness { .. })
                ) {
                    continue;
                }
                if let Some(recipient_id) = transfer.recipient_id
                    && let Some(script) = script_buf_from_recipient_id(recipient_id)?
                {
                    pending_scripts.insert(script.to_hex_string());
                    pending_asset_transfers.insert(transfer.asset_transfer_idx);
                }
            }
        }

        let mut kept_scripts = HashSet::new();
        for pending_witness_script in pending_witness_scripts {
            if pending_scripts.contains(&pending_witness_script.script) {
                kept_scripts.insert(pending_witness_script.script);
            } else {
                self.database
                    .del_pending_witness_script(pending_witness_script.script)?;
            }
        }

        for txo in pending_witness_txos {
            let has_pending_coloring = db_data.colorings.iter().any(|c| {
                c.txo_idx == txo.idx
                    && c.r#type == ColoringType::Receive
                    && pending_asset_transfers.contains(&c.asset_transfer_idx)
            });
            let pays_pending_script = || {
                self.bdk_wallet
                    .tx_graph()
                    .get_txout(txo.outpoint().into())
                    .is_some_and(|txout| {
                        pending_scripts.contains(&txout.script_pubkey.to_hex_string())
                    })
            };
            if has_pending_coloring || pays_pending_script() {
                continue;
            }
            let mut txo: DbTxoActMod = txo.into();
            txo.pending_witness = ActiveValue::Set(false);
            self.database.update_txo(txo)?;
        }

        Ok(kept_scripts)
    }

    fn _fail_batch_transfer(
        &mut self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<DbBatchTransfer, Error> {
        self._clean_failed_witness_receives(batch_transfer)?;
        if let Some(artifacts) = batch_transfer
            .txid
            .as_ref()
            .and_then(|txid| self.transfer_artifacts.get_mut(txid))
        {
            artifacts.inputs.clear();
        }
        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Failed);
        updated_batch_transfer.expiration = ActiveValue::Set(Some(now().unix_timestamp()));
        Ok(self
            .database
            .update_batch_transfer(&mut updated_batch_transfer)?)
    }

    fn _fail_batch_transfer_if_no_endpoints(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        transfer_transport_endpoints_data: &[(DbTransferTransportEndpoint, DbTransportEndpoint)],
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if transfer_transport_endpoints_data.is_empty() {
            Ok(Some(self._fail_batch_transfer(batch_transfer)?))
        } else {
            Ok(None)
        }
    }

    pub(crate) fn extract_received_assignments(
        &self,
        consignment: &RgbTransfer,
        witness_id: RgbTxid,
        vout: Option<u32>,
        known_concealed: Option<SecretSeal>,
    ) -> HashMap<Opout, Assignment> {
        consignment
            .bundles
            .iter()
            .find(|bundle| bundle.witness_id() == witness_id)
            .map(|bundle| self.assignments_for_bundle(bundle, vout, known_concealed))
            .unwrap_or_default()
    }

    /// Return the TX when every input is signed.
    fn donation_signed_tx(tx: &BdkTransaction) -> Option<BdkTransaction> {
        (!tx.input.is_empty()
            && tx
                .input
                .iter()
                .all(|i| !i.witness.is_empty() || !i.script_sig.is_empty()))
        .then(|| tx.clone())
    }

    /// The outputs spent within the consignment by its known transitions. The validation checks
    /// the inputs of a known transition, and the witness TX must close their seals, so only this
    /// wallet can spend an output paying it.
    fn known_spent_opouts(consignment: &RgbTransfer) -> BTreeSet<Opout> {
        consignment
            .bundles
            .iter()
            .flat_map(|wb| wb.bundle.known_transitions.iter())
            .flat_map(|kt| kt.transition.inputs.iter().copied())
            .collect()
    }

    /// Locate the destinations (bundle and, for witness receives, vout) that pay this recipient,
    /// with the assignments not in `spent`. Assignments to one vout of one bundle are one
    /// destination. Trust the result only for a validated consignment.
    fn find_payments<'c, 'a>(
        &'a self,
        consignment: &'c RgbTransfer,
        matcher: &'a ReceiveMatcher,
        spent: &'a BTreeSet<Opout>,
    ) -> impl Iterator<Item = ConsignmentPayment<'c>> + 'a
    where
        'c: 'a,
    {
        let known_concealed = match matcher {
            ReceiveMatcher::Blind(secret_seal) => Some(*secret_seal),
            ReceiveMatcher::Witness(..) => None,
        };
        consignment.bundles.iter().flat_map(move |witness_bundle| {
            // the vouts paid by this bundle, `None` for concealed seals
            let mut vouts: BTreeSet<Option<u32>> = BTreeSet::new();
            let seals = witness_bundle
                .bundle
                .known_transitions
                .iter()
                .flat_map(|kt| kt.transition.assignments.values())
                .flat_map(|typed_assigns| typed_assigns.seals());
            match matcher {
                ReceiveMatcher::Blind(secret_seal) => {
                    if seals
                        .into_iter()
                        .any(|seal| *seal == BuilderSeal::Concealed(*secret_seal))
                    {
                        vouts.insert(None);
                    }
                }
                ReceiveMatcher::Witness(script_pubkey, blinding) => {
                    let tx = &witness_bundle.tx;
                    for seal in seals {
                        let BuilderSeal::Revealed(seal) = *seal else {
                            continue;
                        };
                        if seal.txid != TxPtr::WitnessTx {
                            continue;
                        }
                        // the blinding is derived from the keys of both parties, so a seal with a
                        // different one doesn't pay this transfer
                        if *blinding != seal.blinding {
                            continue;
                        }
                        let vout = seal.vout.into_u32();
                        if tx
                            .output
                            .get(vout as usize)
                            .is_some_and(|out| out.script_pubkey == *script_pubkey)
                        {
                            vouts.insert(Some(vout));
                        }
                    }
                }
            }

            vouts.into_iter().filter_map(move |vout| {
                let mut receiving =
                    self.assignments_for_bundle(witness_bundle, vout, known_concealed);
                receiving.retain(|opout, _| !spent.contains(opout));
                (!receiving.is_empty()).then_some(ConsignmentPayment {
                    bundle: witness_bundle,
                    vout,
                    receiving,
                })
            })
        })
    }

    fn assignments_for_bundle(
        &self,
        bundle: &WitnessBundle,
        vout: Option<u32>,
        known_concealed: Option<SecretSeal>,
    ) -> HashMap<Opout, Assignment> {
        let mut received = HashMap::new();
        for KnownTransition { transition, opid } in bundle.bundle.known_transitions.iter() {
            for (ass_type, typed_assigns) in transition.assignments.iter() {
                for (no, fungible_assignment) in typed_assigns.as_fungible().iter().enumerate() {
                    let opout = Opout::new(*opid, *ass_type, no as u16);
                    if let BuilderSeal::Concealed(seal) = fungible_assignment.seal {
                        let state = &fungible_assignment.state;
                        if Some(seal) == known_concealed {
                            match *ass_type {
                                OS_ASSET => {
                                    received.insert(opout, Assignment::Fungible(state.as_u64()));
                                }
                                OS_INFLATION => {
                                    received
                                        .insert(opout, Assignment::InflationRight(state.as_u64()));
                                }
                                _ => {}
                            }
                        }
                    };
                    if let BuilderSeal::Revealed(seal) = fungible_assignment.seal {
                        let state = &fungible_assignment.state;
                        if seal.txid == TxPtr::WitnessTx && Some(seal.vout.into_u32()) == vout {
                            match *ass_type {
                                OS_ASSET => {
                                    received.insert(opout, Assignment::Fungible(state.as_u64()));
                                }
                                OS_INFLATION => {
                                    received
                                        .insert(opout, Assignment::InflationRight(state.as_u64()));
                                }
                                _ => {}
                            }
                        }
                    };
                }
                for (no, structured_assignment) in typed_assigns.as_structured().iter().enumerate()
                {
                    let opout = Opout::new(*opid, *ass_type, no as u16);
                    if let BuilderSeal::Concealed(seal) = structured_assignment.seal
                        && Some(seal) == known_concealed
                    {
                        received.insert(opout, Assignment::NonFungible);
                    }
                    if let BuilderSeal::Revealed(seal) = structured_assignment.seal
                        && seal.txid == TxPtr::WitnessTx
                        && Some(seal.vout.into_u32()) == vout
                    {
                        received.insert(opout, Assignment::NonFungible);
                    };
                }
            }
        }

        received
    }

    fn _select_rgb_inputs(
        &self,
        asset_id: String,
        assignments_needed: &AssignmentsCollection,
        unspents: Vec<LocalUnspent>,
    ) -> Result<AssetSpend, Error> {
        // sort unspents by the sum of main amounts
        fn cmp_localunspent_allocation_sum(a: &LocalUnspent, b: &LocalUnspent) -> Ordering {
            let a_sum: u64 = a
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.main_amount())
                .sum();
            let b_sum: u64 = b
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.main_amount())
                .sum();
            a_sum.cmp(&b_sum)
        }
        // sort unspents by the sum of inflation right amounts
        fn cmp_localunspent_inflation_sum(a: &LocalUnspent, b: &LocalUnspent) -> Ordering {
            let a_sum: u64 = a
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.inflation_amount())
                .sum();
            let b_sum: u64 = b
                .rgb_allocations
                .iter()
                .map(|a| a.assignment.inflation_amount())
                .sum();
            a_sum.cmp(&b_sum)
        }

        debug!(self.logger, "Selecting inputs for asset '{}'...", asset_id);
        let mut txo_map: HashMap<i32, Outpoint> = HashMap::new();

        let mut mut_unspents = unspents;

        // sort unspents first by inflation rights amount, then main amount
        if assignments_needed.inflation > 0 {
            mut_unspents.sort_by(cmp_localunspent_inflation_sum);
        }
        if assignments_needed.fungible > 0 {
            mut_unspents.sort_by(cmp_localunspent_allocation_sum);
        }

        let mut assignments_collected = AssignmentsCollection::default();
        let mut input_btc_amt = 0;
        for unspent in mut_unspents {
            // get spendable allocations for the required asset
            let asset_allocations: Vec<LocalRgbAllocation> = unspent
                .rgb_allocations
                .into_iter()
                .filter(|a| a.asset_id == Some(asset_id.clone()) && a.status.settled())
                .collect();

            // skip UTXOs with no allocations
            if asset_allocations.is_empty() {
                continue;
            }

            // check if the unspent hosts any needed allocations
            let mut needed = false;
            if assignments_collected.fungible < assignments_needed.fungible
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::Fungible(_)))
            {
                needed = true;
            }
            if !assignments_collected.non_fungible & assignments_needed.non_fungible
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::NonFungible))
            {
                needed = true;
            }
            if assignments_collected.inflation < assignments_needed.inflation
                && asset_allocations
                    .iter()
                    .any(|a| matches!(a.assignment, Assignment::InflationRight(_)))
            {
                needed = true;
            }
            if assignments_collected.mint < assignments_needed.mint
                && asset_allocations
                    .iter()
                    .any(|a| a.assignment == Assignment::MintRight)
            {
                needed = true;
            }
            // skip UTXOs with no needed allocations
            if !needed {
                continue;
            }

            // add selected allocations to collected assignments
            asset_allocations
                .iter()
                .for_each(|a| a.assignment.add_to_assignments(&mut assignments_collected));
            txo_map.insert(unspent.utxo.idx, unspent.utxo.outpoint());

            input_btc_amt += unspent.utxo.btc_amount.parse::<u64>().unwrap();

            // stop as soon as we have the needed assignments
            if assignments_collected.enough(assignments_needed) {
                break;
            }
        }
        if !assignments_collected.enough(assignments_needed) {
            return Err(Error::InsufficientAssignments {
                asset_id,
                available: assignments_collected,
            });
        }

        debug!(
            self.logger,
            "Asset input assignments {:?}", assignments_collected
        );
        Ok(AssetSpend {
            txo_map,
            assignments_collected,
            input_btc_amt,
        })
    }

    fn _prepare_psbt(
        &mut self,
        input_outpoints: HashSet<BdkOutPoint>,
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate: FeeRate,
        lock_time: Option<u32>,
    ) -> Result<(Psbt, Option<BtcChange>), Error> {
        let change_addr = self.get_new_address()?.script_pubkey();
        let mut builder = self.bdk_wallet.build_tx();
        builder
            .add_data(&[0; 32])
            .add_utxos(&input_outpoints.into_iter().collect::<Vec<_>>())
            .map_err(InternalError::from)?
            .manually_selected_only()
            .fee_rate(fee_rate)
            .ordering(bdk_wallet::tx_builder::TxOrdering::Untouched);
        // When the caller pins a locktime (e.g. 0 for an LN funding tx that must be final),
        // honor it; otherwise keep BDK's anti-fee-sniping default. LDK's
        // funding_transaction_generated rejects a funding tx whose absolute timelock is non-final.
        if let Some(height) = lock_time {
            builder.nlocktime(
                bdk_wallet::bitcoin::locktime::absolute::LockTime::from_height(height).map_err(
                    |e| Error::Internal {
                        details: e.to_string(),
                    },
                )?,
            );
        }
        for (script_buf, amount_sat) in witness_recipients {
            builder.add_recipient(script_buf.clone(), BdkAmount::from_sat(*amount_sat));
        }
        builder.drain_to(change_addr.clone());

        let mut psbt = builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;
        self.add_non_witness_utxos(&mut psbt);

        let btc_change = psbt
            .unsigned_tx
            .output
            .iter()
            .enumerate()
            .find(|(_, o)| o.script_pubkey == change_addr)
            .map(|(i, o)| BtcChange {
                vout: i as u32,
                amount: o.value.to_sat(),
            });

        Ok((psbt, btc_change))
    }

    fn _try_prepare_psbt(
        &mut self,
        input_unspents: &[LocalUnspent],
        all_inputs: &mut HashSet<BdkOutPoint>,
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate: FeeRate,
        lock_time: Option<u32>,
    ) -> Result<(Psbt, Option<BtcChange>), Error> {
        Ok(loop {
            break match self._prepare_psbt(
                all_inputs.clone(),
                witness_recipients,
                fee_rate,
                lock_time,
            ) {
                Ok(res) => res,
                Err(Error::InsufficientBitcoins { .. }) => {
                    let used_txos: Vec<Outpoint> =
                        all_inputs.clone().into_iter().map(|o| o.into()).collect();
                    let mut free_utxos = self.get_available_allocations(
                        input_unspents,
                        used_txos.as_slice(),
                        Some(0),
                    )?;
                    // sort UTXOs by BTC amount
                    if !free_utxos.is_empty() {
                        // pre-parse BTC amounts to make sure no one will fail
                        for u in &free_utxos {
                            u.utxo
                                .btc_amount
                                .parse::<u64>()
                                .map_err(|e| Error::Internal {
                                    details: e.to_string(),
                                })?;
                        }
                        free_utxos.sort_by_key(|u| u.utxo.btc_amount.parse::<u64>().unwrap());
                    }
                    if let Some(a) = free_utxos.pop() {
                        all_inputs.insert(a.utxo.into());
                        continue;
                    }
                    return Err(self.detect_btc_unspendable_err()?);
                }
                Err(e) => return Err(e),
            };
        })
    }

    async fn _restore_missing_bdk_inputs(
        &mut self,
        input_outpoints: &HashSet<BdkOutPoint>,
    ) -> Result<(), Error> {
        let missing_txids: HashSet<Txid> = input_outpoints
            .iter()
            .filter(|outpoint| self.bdk_wallet.get_utxo(**outpoint).is_none())
            .map(|outpoint| outpoint.txid)
            .collect();
        if missing_txids.is_empty() {
            return Ok(());
        }

        let mut transactions = Vec::with_capacity(missing_txids.len());
        for txid in missing_txids {
            let tx = self
                .bdk_wallet
                .tx_graph()
                .get_tx(txid)
                .ok_or_else(|| Error::Internal {
                    details: format!(
                        "selected RGB input transaction {txid} is missing from the BDK graph"
                    ),
                })?;
            transactions.push(tx);
        }

        // Re-observe local full transactions immediately before PSBT construction so BDK can
        // resolve selected RGB outpoints even after an incremental sync evicted them.
        let last_seen = ((js_sys::Date::now() / 1000.0) as u64).saturating_add(1);
        self.bdk_wallet
            .apply_unconfirmed_txs(transactions.into_iter().map(|tx| (tx, last_seen)));
        self.bdk_wallet.persist(&mut self.bdk_database)?;
        Ok(())
    }

    fn _get_change_seal(
        &self,
        btc_change: &Option<BtcChange>,
        change_utxo_option: &mut Option<DbTxo>,
        change_utxo_idx: &mut Option<i32>,
        input_outpoints: &[Outpoint],
        unspents: &[LocalUnspent],
    ) -> Result<BlindSeal<TxPtr>, Error> {
        let graph_seal = if let Some(btc_change) = btc_change {
            GraphSeal::new_random_vout(btc_change.vout)
        } else {
            if change_utxo_option.is_none() {
                let change_utxo = self.get_utxo(input_outpoints, Some(unspents), true, None)?;
                debug!(
                    self.logger,
                    "Change outpoint '{}'",
                    change_utxo.outpoint().to_string()
                );
                *change_utxo_idx = Some(change_utxo.idx);
                *change_utxo_option = Some(change_utxo);
            }
            let change_utxo = change_utxo_option.clone().unwrap();
            let blind_seal = self.get_blind_seal(change_utxo).transmutate();
            GraphSeal::from(blind_seal)
        };
        Ok(graph_seal)
    }

    fn _check_dag(
        &self,
        ops_dag: &OpsDag,
        reject_opouts: &HashSet<Opout>,
        allow_opouts: &HashSet<Opout>,
        check_opouts: &HashSet<Opout>,
    ) -> Result<HashSet<Opout>, Error> {
        let mut to_reject = HashSet::new();

        // for each opout we are checking, traverse its ancestor chain
        'check: for check_opout in check_opouts {
            // allow shields this path upward: do not traverse this branch further
            if allow_opouts.contains(check_opout) {
                continue;
            }
            // encountering a reject opout on an unshielded path: reject
            if reject_opouts.contains(check_opout) {
                to_reject.insert(*check_opout);
                continue;
            }
            let op_node = ops_dag.op_node(&check_opout.op).ok_or(Error::Internal {
                details: s!("opout not found in DAG"),
            })?;

            // traverse from the operation creating it to its ancestors, depth first, checking the
            // opouts each operation spends
            let mut stack = vec![op_node];
            let mut visited = HashSet::new();
            while let Some(node) = stack.pop() {
                if !visited.insert(node) {
                    continue;
                }
                for (spent_opout, parent) in ops_dag.spent_opouts(node) {
                    if allow_opouts.contains(&spent_opout) {
                        continue;
                    }
                    if reject_opouts.contains(&spent_opout) {
                        to_reject.insert(*check_opout);
                        continue 'check;
                    }
                    stack.push(parent);
                }
            }
        }
        Ok(to_reject)
    }

    fn _save_transfers(
        &self,
        txid: String,
        transfer_info_map: &BTreeMap<String, InfoAssetTransfer>,
        extra_allocations: HashMap<String, TxoAssignments>,
        change_utxo_idx: Option<i32>,
        btc_change: Option<BtcChange>,
        status: TransferStatus,
        min_confirmations: u8,
        expiration_timestamp: Option<i64>,
        operation_secret_key: Option<Vec<u8>>,
        sender_transport_endpoint: Option<String>,
        consignment_hashes: &HashMap<String, String>,
    ) -> Result<i32, Error> {
        let created_at = now().unix_timestamp();

        let batch_transfer = DbBatchTransferActMod {
            txid: ActiveValue::Set(Some(txid.clone())),
            status: ActiveValue::Set(status),
            expiration: ActiveValue::Set(expiration_timestamp),
            created_at: ActiveValue::Set(created_at),
            min_confirmations: ActiveValue::Set(min_confirmations),
            operation_secret_key: ActiveValue::Set(operation_secret_key.clone()),
            ..Default::default()
        };
        let batch_transfer_idx = self.database.set_batch_transfer(batch_transfer)?;
        let sender_pubkey_hex = match &operation_secret_key {
            Some(seed) => {
                let seed: [u8; 32] = seed.clone().try_into().map_err(|_| Error::Internal {
                    details: s!("invalid operation secret key"),
                })?;
                Some(hex::encode(
                    ed25519_from_seed(&seed).verifying_key().to_bytes(),
                ))
            }
            None => None,
        };

        let change_utxo_idx = if let Some(btc_change) = btc_change {
            Some(
                match self.database.get_txo(&Outpoint {
                    txid: txid.clone(),
                    vout: btc_change.vout,
                })? {
                    Some(txo) => txo.idx,
                    None => {
                        let db_utxo = DbTxoActMod {
                            txid: ActiveValue::Set(txid.clone()),
                            vout: ActiveValue::Set(btc_change.vout),
                            btc_amount: ActiveValue::Set(btc_change.amount.to_string()),
                            spent: ActiveValue::Set(false),
                            exists: ActiveValue::Set(false),
                            pending_witness: ActiveValue::Set(false),
                            ..Default::default()
                        };
                        self.database.set_txo(db_utxo)?
                    }
                },
            )
        } else {
            change_utxo_idx
        };

        for (asset_id, transfer_info) in transfer_info_map {
            // hash of the plaintext consignment for this asset, if already generated: a begin
            // saves the transfers before the consignments are final
            let consignment_hash = consignment_hashes.get(asset_id).cloned();
            let asset_transfer = DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
                asset_id: ActiveValue::Set(Some(asset_id.clone())),
                transition_type: ActiveValue::Set(Some(transfer_info.main_transition)),
                ..Default::default()
            };
            let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;

            for (input_idx, assignments) in &transfer_info.assignments_spent {
                for assignment in assignments {
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(*input_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Input),
                        assignment: ActiveValue::Set(assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                }
            }
            if transfer_info.change.fungible > 0 {
                let db_coloring = DbColoringActMod {
                    txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    r#type: ActiveValue::Set(ColoringType::Change),
                    assignment: ActiveValue::Set(Assignment::Fungible(
                        transfer_info.change.fungible,
                    )),
                    ..Default::default()
                };
                self.database.set_coloring(db_coloring)?;
            }
            if transfer_info.change.inflation > 0 {
                let db_coloring = DbColoringActMod {
                    txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    r#type: ActiveValue::Set(ColoringType::Change),
                    assignment: ActiveValue::Set(Assignment::InflationRight(
                        transfer_info.change.inflation,
                    )),
                    ..Default::default()
                };
                self.database.set_coloring(db_coloring)?;
            }
            for _ in 0..transfer_info.change.mint {
                let db_coloring = DbColoringActMod {
                    txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    r#type: ActiveValue::Set(ColoringType::Change),
                    assignment: ActiveValue::Set(Assignment::MintRight),
                    ..Default::default()
                };
                self.database.set_coloring(db_coloring)?;
            }

            for recipient in transfer_info.recipients.clone() {
                if transfer_info.main_transition == TypeOfTransition::Burn {
                    let transfer = DbTransferActMod {
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        requested_assignment: ActiveValue::Set(Some(Assignment::Fungible(
                            transfer_info.original_assignments_needed.fungible,
                        ))),
                        incoming: ActiveValue::Set(false),
                        recipient_encryption: ActiveValue::Set(recipient.encryption),
                        burn_reason: ActiveValue::Set(
                            transfer_info.burn_reason.map(|r| r.to_string()),
                        ),
                        ..Default::default()
                    };
                    self.database.set_transfer(transfer)?;
                    continue;
                }
                // the transfer ID on the proxies, only for a proxy recipient
                let proxy_transfer_id = match &sender_pubkey_hex {
                    Some(sender_pk) if !recipient.public_key.is_empty() => {
                        Some(recipient.proxy_ids(sender_pk).1)
                    }
                    _ => None,
                };
                let recipient_pubkey = if recipient.public_key.is_empty() {
                    None
                } else {
                    Some(recipient.public_key.clone())
                };
                // a test mint, with no invoice, pays outputs of the wallet like an inflation
                #[cfg(test)]
                let own_outputs = transfer_info.main_transition == TypeOfTransition::Mint
                    && recipient.transport_endpoints.is_empty();
                #[cfg(not(test))]
                let own_outputs = false;
                let recipient_type = if own_outputs
                    || !matches!(
                        transfer_info.main_transition,
                        TypeOfTransition::Transfer | TypeOfTransition::Mint
                    ) {
                    let vout = if let LocalRecipientData::Witness(local_witness_data) =
                        recipient.local_recipient_data
                    {
                        local_witness_data.vout
                    } else {
                        unreachable!("inflation uses witness recipients")
                    };
                    let txo_idx = self
                        .database
                        .get_txo(&Outpoint {
                            txid: txid.clone(),
                            vout,
                        })?
                        .expect("outpoint should be in the DB")
                        .idx;
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(txo_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Issue),
                        assignment: ActiveValue::Set(recipient.assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                    Some(RecipientTypeFull::Witness { vout: Some(vout) })
                } else {
                    None
                };

                let transfer = DbTransferActMod {
                    asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                    requested_assignment: ActiveValue::Set(Some(recipient.assignment)),
                    incoming: ActiveValue::Set(false),
                    recipient_id: ActiveValue::Set(Some(recipient.recipient_id.clone())),
                    recipient_type: ActiveValue::Set(recipient_type),
                    recipient_pubkey: ActiveValue::Set(recipient_pubkey),
                    consignment_hash: ActiveValue::Set(consignment_hash.clone()),
                    recipient_encryption: ActiveValue::Set(recipient.encryption),
                    proxy_transfer_id: ActiveValue::Set(proxy_transfer_id),
                    invoice_expiration: ActiveValue::Set(recipient.invoice_expiration),
                    ..Default::default()
                };
                let transfer_idx = self.database.set_transfer(transfer)?;
                let has_endpoints = !recipient.transport_endpoints.is_empty();
                for transport_endpoint in recipient.transport_endpoints {
                    self.save_transfer_transport_endpoint(transfer_idx, &transport_endpoint)?;
                }
                // the sender proxy endpoint (where the ACK is collected and the signed TX
                // released) only concerns proxy recipients
                if let (Some(endpoint), true) = (&sender_transport_endpoint, has_endpoints) {
                    let url = self.convert_transport_endpoint_url(endpoint)?;
                    self.save_sender_transport_endpoint(transfer_idx, &url)?;
                }
            }
        }

        for (asset_id, txo_assignments) in extra_allocations {
            let asset_transfer = DbAssetTransferActMod {
                user_driven: ActiveValue::Set(false),
                batch_transfer_idx: ActiveValue::Set(batch_transfer_idx),
                asset_id: ActiveValue::Set(Some(asset_id.clone())),
                transition_type: ActiveValue::Set(Some(TypeOfTransition::Transfer)),
                ..Default::default()
            };
            let asset_transfer_idx = self.database.set_asset_transfer(asset_transfer)?;
            for (input_idx, assignments) in txo_assignments {
                for assignment in assignments {
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(input_idx),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Input),
                        assignment: ActiveValue::Set(assignment.clone()),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                    let db_coloring = DbColoringActMod {
                        txo_idx: ActiveValue::Set(change_utxo_idx.unwrap()),
                        asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                        r#type: ActiveValue::Set(ColoringType::Change),
                        assignment: ActiveValue::Set(assignment),
                        ..Default::default()
                    };
                    self.database.set_coloring(db_coloring)?;
                }
            }
        }

        Ok(batch_transfer_idx)
    }

    fn _get_outgoing_batch_transfer(&self, txid: &str) -> Result<Option<DbBatchTransfer>, Error> {
        let db_data = self.database.get_db_data(false)?;
        // a send to oneself shares its txid with the receive transfers: use only the outgoing batch
        for bt in db_data.batch_transfers {
            if bt.txid.as_deref() == Some(txid)
                && !bt.incoming(&db_data.asset_transfers, &db_data.transfers)?
            {
                return Ok(Some(bt));
            }
        }
        Ok(None)
    }

    fn _update_or_save_transfers(
        &self,
        txid: String,
        transfer_info_map: &BTreeMap<String, InfoAssetTransfer>,
        info: &InfoBatchTransfer,
        status: TransferStatus,
        consignment_hashes: &HashMap<String, String>,
    ) -> Result<i32, Error> {
        let Some(existing) = self._get_outgoing_batch_transfer(&txid)? else {
            return self._save_transfers(
                txid,
                transfer_info_map,
                info.extra_allocations.clone(),
                info.change_utxo_idx,
                info.btc_change.clone(),
                status,
                info.min_confirmations,
                info.expiration_timestamp,
                info.operation_secret_key.clone(),
                info.sender_transport_endpoint.clone(),
                consignment_hashes,
            );
        };
        let mut updated: DbBatchTransferActMod = existing.clone().into();
        updated.status = ActiveValue::Set(status);
        self.database.update_batch_transfer(&mut updated)?;

        // a begin saves the transfers before the consignments are final, so their
        // consignment_hash is None: backfill it now
        for (asset_id, consignment_hash) in consignment_hashes {
            for transfer in self
                .database
                .get_transfers_for_asset(existing.idx, asset_id)?
            {
                if transfer.consignment_hash.is_none() {
                    let mut updated_transfer: DbTransferActMod = transfer.into();
                    updated_transfer.consignment_hash =
                        ActiveValue::Set(Some(consignment_hash.clone()));
                    self.database.update_transfer(&mut updated_transfer)?;
                }
            }
        }

        // copy the used flags of the posts to the rows saved at begin; extra allocations have no
        // recipients
        let db_data = self.database.get_db_data(false)?;
        let batch_data = existing.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let db_transfers: Vec<&DbTransfer> = batch_data
            .asset_transfers_data
            .iter()
            .flat_map(|a| a.transfers.iter())
            .collect();
        for recipient in transfer_info_map.values().flat_map(|t| &t.recipients) {
            let db_transfer = db_transfers
                .iter()
                .find(|t| t.recipient_id.as_deref() == Some(recipient.recipient_id.as_str()))
                .expect("transfer should be set");
            for (tte, te) in self
                .database
                .get_transfer_transport_endpoints_data(db_transfer.idx)?
                .into_iter()
                .filter(|(tte, _)| tte.kind == TransportEndpointKind::RecipientProxy)
            {
                let local_used = recipient
                    .transport_endpoints
                    .iter()
                    .find(|l| l.endpoint == te.endpoint)
                    .is_some_and(|l| l.used);
                if tte.used != local_used {
                    let mut updated_tte: DbTransferTransportEndpointActMod = tte.into();
                    updated_tte.used = ActiveValue::Set(local_used);
                    self.database
                        .update_transfer_transport_endpoint(&mut updated_tte)?;
                }
            }
        }
        Ok(existing.idx)
    }

    /// Refuse to end the PSBT of an RGB operation with a vanilla end method.
    pub(crate) fn get_input_unspents(
        &self,
        unspents: &[LocalUnspent],
    ) -> Result<Vec<LocalUnspent>, Error> {
        let mut input_unspents = unspents.to_vec();
        // consider the following UTXOs unspendable:
        // - incoming and pending
        // - outgoing and in initiated or waiting counterparty status
        // - pending incoming witness
        // - pending incoming blinded
        // - inexistent
        input_unspents.retain(|u| {
            !(u.rgb_allocations
                .iter()
                .any(|a| a.incoming && a.status.pending()))
                && !(u.rgb_allocations.iter().any(|a| {
                    !a.incoming && (a.status.initiated() || a.status.waiting_counterparty())
                }))
                && !u.utxo.pending_witness
                && u.pending_blinded == 0
                && u.utxo.exists
        });
        Ok(input_unspents)
    }

    fn _get_transfer_begin_data(
        &mut self,
        online: Online,
        fee_rate: u64,
    ) -> Result<(FeeRate, Vec<LocalUnspent>, Vec<LocalUnspent>, RgbRuntime), Error> {
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        let db_data = self.database.get_db_data(false)?;

        let utxos = self.database.get_unspent_txos(db_data.txos.clone())?;

        let mut unspents = self.database.get_rgb_allocations(
            utxos,
            Some(db_data.colorings.clone()),
            Some(db_data.batch_transfers.clone()),
            Some(db_data.asset_transfers.clone()),
            Some(db_data.transfers.clone()),
        )?;
        let reserved = self.get_reserved_outpoints()?;
        unspents.retain(|u| !reserved.contains(&u.utxo.outpoint()));

        let input_unspents = self.get_input_unspents(&unspents)?;

        let runtime = self.rgb_runtime()?;

        Ok((fee_rate_checked, unspents, input_unspents, runtime))
    }
}

impl Wallet {
    async fn _go_online_wasm(
        &self,
        online_options: &OnlineOptions,
    ) -> Result<(Online, OnlineData), Error> {
        let proxy_auth_keys = parse_proxy_auth_keys(&online_options.proxy_auth_keys)?;
        let indexer_url = online_options.indexer_config.url().to_string();
        let online_id = now().unix_timestamp_nanos() as u64;
        let online = Online {
            id: online_id,
            indexer_url: indexer_url.clone(),
        };
        let indexer =
            crate::utils::build_indexer(&indexer_url).ok_or_else(|| Error::InvalidIndexer {
                details: s!("failed to build esplora async client"),
            })?;
        let genesis_hash = indexer
            .block_hash(0)
            .await
            .map_err(|_| Error::InvalidIndexer {
                details: s!("not a valid esplora server"),
            })?;
        crate::utils::check_indexer_network(self.bitcoin_network(), &genesis_hash)?;
        indexer.populate_tx_cache(&self.bdk_wallet);
        let online_data = OnlineData {
            id: online.id,
            indexer_url,
            indexer,
            proxy_auth_keys,
            evm_chains: HashMap::new(),
        };
        Ok((online, online_data))
    }

    /// Return the existing or freshly generated set of wallet [`Online`] data (wasm32 async).
    ///
    /// See [`OnlineOptions`] for details on the available options.
    pub async fn go_online(&mut self, online_options: OnlineOptions) -> Result<Online, Error> {
        info!(self.logger, "Going online...");
        let mut evm_chains = online_options.evm_chains.clone();
        if self.supports_schema(&AssetSchema::Bfa) {
            for (chain_id, evm_chain) in &evm_chains {
                // with a higher minimum than maximum, no block could be committed to when minting
                if evm_chain.min_mint_lock_blocks > evm_chain.max_mint_lock_blocks {
                    return Err(Error::InvalidEvmChain {
                        details: format!(
                            "chain ID {chain_id}: min_mint_lock_blocks ({}) is greater than \
                             max_mint_lock_blocks ({})",
                            evm_chain.min_mint_lock_blocks, evm_chain.max_mint_lock_blocks
                        ),
                    });
                }
                let evm_client = EvmClient::new(&evm_chain.rpc_url);
                let actual = evm_client
                    .chain_id()
                    .await
                    .map_err(|e| Error::InvalidEvmRpcUrl {
                        details: e.to_string(),
                    })?;
                if actual != *chain_id {
                    return Err(Error::EvmChainIdMismatch {
                        expected: *chain_id,
                        actual,
                    });
                }
                evm_client.check_mint_support().await?;
            }
        } else {
            evm_chains.clear();
        }
        let online = if let Some(online_data) = &self.online_data {
            let online = Online {
                id: online_data.id,
                indexer_url: online_options.indexer_config.url().to_string(),
            };
            if online_data.indexer_url != online.indexer_url {
                let (online, online_data) = self._go_online_wasm(&online_options).await?;
                self.online_data = Some(online_data);
                info!(self.logger, "Went online with new indexer");
                online
            } else {
                self.check_online(online.clone())?;
                // No `..`: a new option must be handled here to compile.
                let OnlineOptions {
                    indexer_config: _,         // unchanged here
                    skip_consistency_check: _, // no effect
                    proxy_auth_keys,
                    evm_chains: _, // set below
                } = &online_options;
                let proxy_auth_keys = parse_proxy_auth_keys(proxy_auth_keys)?;
                self.online_data.as_mut().unwrap().proxy_auth_keys = proxy_auth_keys;
                online
            }
        } else {
            let (online, online_data) = self._go_online_wasm(&online_options).await?;
            self.online_data = Some(online_data);
            online
        };
        self.online_data.as_mut().unwrap().evm_chains = evm_chains;
        info!(self.logger, "Go online completed");
        Ok(online)
    }

    pub(crate) async fn sync_db_txos(&mut self, full_scan: bool) -> Result<(), Error> {
        debug!(self.logger, "Syncing TXOs...");

        // Use js_sys::Date::now() for WASM-compatible timestamp
        // (std::time::UNIX_EPOCH.elapsed() panics on wasm32-unknown-unknown)
        let start_time = (js_sys::Date::now() / 1000.0) as u64;
        let update: Update = if full_scan {
            let request = self.bdk_wallet.start_full_scan_at(start_time);
            self.indexer().full_scan(request).await?.into()
        } else {
            let request = self.bdk_wallet.start_sync_with_revealed_spks_at(start_time);
            self.indexer().sync(request).await?.into()
        };
        self.bdk_wallet
            .apply_update(update)
            .map_err(|e| Error::FailedBdkSync {
                details: e.to_string(),
            })?;
        self.bdk_wallet.persist(&mut self.bdk_database)?;

        let pending_witness_scripts = self._reconcile_pending_witness()?;

        let db_txos = self.database.iter_txos()?;

        let db_outpoints: HashSet<String> = db_txos
            .clone()
            .into_iter()
            .filter(|t| !t.spent && t.exists)
            .map(|u| u.outpoint().to_string())
            .collect();
        let bdk_utxos = self.bdk_wallet.list_unspent();
        let external_bdk_utxos: Vec<LocalOutput> = bdk_utxos
            .filter(|u| u.keychain == KeychainKind::External)
            .collect();

        let new_utxos: Vec<LocalOutput> = external_bdk_utxos
            .clone()
            .into_iter()
            .filter(|u| !db_outpoints.contains(&u.outpoint.to_string()))
            .collect();

        for new_utxo in new_utxos.iter().cloned() {
            let mut new_db_utxo: DbTxoActMod = new_utxo.clone().into();
            let pending_witness_script = new_utxo.txout.script_pubkey.to_hex_string();
            if pending_witness_scripts.contains(&pending_witness_script) {
                new_db_utxo.pending_witness = ActiveValue::Set(true);
                self.database
                    .del_pending_witness_script(pending_witness_script)?;
            }
            self.database.set_txo(new_db_utxo.clone())?;
        }

        debug!(self.logger, "Synced TXOs");

        Ok(())
    }

    /// Incrementally sync the wallet and save new RGB UTXOs to the DB (wasm32 async).
    ///
    /// Re-queries only already-revealed SPKs. To rediscover UTXOs for unrevealed SPKs (e.g. a thin
    /// restored state) use [`Wallet::full_scan`].
    pub async fn sync(&mut self, online: Online) -> Result<(), Error> {
        info!(self.logger, "Syncing...");
        self.check_online(online)?;
        self.sync_db_txos(false).await?;
        info!(self.logger, "Sync completed");
        Ok(())
    }

    /// Run a BIP44 stop-gap full scan and save new RGB UTXOs to the DB (wasm32 async).
    ///
    /// Derives and queries SPKs up to the indexer stop-gap, rebuilding a thin BDK state (no
    /// revealed SPKs) from the indexer when an incremental [`Wallet::sync`] cannot.
    pub async fn full_scan(&mut self, online: Online) -> Result<(), Error> {
        info!(self.logger, "Full scanning...");
        self.check_online(online)?;
        self.sync_db_txos(true).await?;
        info!(self.logger, "Full scan completed");
        Ok(())
    }

    async fn _broadcast_tx(&self, tx: BdkTransaction) -> Result<BdkTransaction, Error> {
        let txid = tx.compute_txid().to_string();
        let indexer = self.indexer();
        match indexer.broadcast(&tx).await {
            Ok(_) => {
                debug!(self.logger, "Broadcasted TX with ID '{}'", txid);
                Ok(tx)
            }
            Err(e) => {
                match e {
                    IndexerError::EsploraAsync(ref msg) => {
                        if msg.contains("min relay fee not met") {
                            return Err(Error::MinFeeNotMet { txid: txid.clone() });
                        } else if msg.contains("Fee exceeds maximum configured") {
                            return Err(Error::MaxFeeExceeded { txid: txid.clone() });
                        }
                    }
                }
                if indexer.get_tx_confirmations(&txid).await?.is_none() {
                    return Err(Error::FailedBroadcast {
                        details: e.to_string(),
                    });
                }
                Ok(tx)
            }
        }
    }

    async fn _broadcast_psbt(
        &mut self,
        signed_psbt: Psbt,
        skip_sync: bool,
    ) -> Result<BdkTransaction, Error> {
        let tx = self
            ._broadcast_tx(signed_psbt.extract_tx().map_err(InternalError::from)?)
            .await?;

        let internal_unspents_outpoints: Vec<(String, u32)> = self
            .internal_unspents()
            .map(|u| (u.outpoint.txid.to_string(), u.outpoint.vout))
            .collect();

        for input in tx.clone().input {
            let txid = input.previous_output.txid.to_string();
            let vout = input.previous_output.vout;
            if internal_unspents_outpoints.contains(&(txid.clone(), vout)) {
                continue;
            }
            let mut db_txo: DbTxoActMod = self
                .database
                .get_txo(&Outpoint { txid, vout })?
                .expect("outpoint should be in the DB")
                .into();
            db_txo.spent = ActiveValue::Set(true);
            self.database.update_txo(db_txo)?;
        }

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        // Keep the transaction in BDK's graph after the optional sync. Esplora may not have
        // indexed a freshly broadcast transaction yet and can mark it evicted during that sync,
        // leaving rgb-lib's TXO database aware of its outputs while BDK cannot later use them
        // as PSBT inputs.
        // Esplora's sync uses second-resolution timestamps for evictions. Advance the local
        // observation by one second so a same-second "not indexed yet" eviction cannot make the
        // transaction non-canonical.
        let last_seen = ((js_sys::Date::now() / 1000.0) as u64).saturating_add(1);
        self.bdk_wallet
            .apply_unconfirmed_txs([(tx.clone(), last_seen)]);
        self.bdk_wallet.persist(&mut self.bdk_database)?;

        Ok(tx)
    }

    fn _record_vanilla_transaction(
        &self,
        txid: String,
        r#type: WalletTransactionType,
    ) -> Result<(), Error> {
        self.database
            .set_wallet_transaction(DbWalletTransactionActMod {
                txid: ActiveValue::Set(txid),
                r#type: ActiveValue::Set(r#type),
                ..Default::default()
            })?;
        Ok(())
    }

    async fn _record_vanilla_begin(
        &self,
        psbt: &Psbt,
        r#type: WalletTransactionType,
    ) -> Result<(), Error> {
        self._record_vanilla_transaction(psbt.unsigned_tx.compute_txid().to_string(), r#type)?;
        self.update_backup_info(false)?;
        // the PSBT leaves the wallet only after its operation is saved
        self.flush().await
    }

    /// Check that the PSBT is of the given vanilla operation and is not already ended.
    fn _check_vanilla_end(&self, psbt: &Psbt, r#type: WalletTransactionType) -> Result<(), Error> {
        let bdk_txid = psbt.unsigned_tx.compute_txid();
        let txid = bdk_txid.to_string();
        // an end or a refresh removes the artifacts of an RGB operation and keeps its batch
        if self.transfer_artifacts.contains_key(&txid)
            || self._get_outgoing_batch_transfer(&txid)?.is_some()
            || self
                .database
                .iter_wallet_transactions()?
                .iter()
                .any(|t| t.txid == txid && t.r#type != r#type)
        {
            return Err(Error::WrongOperation { txid });
        }
        // a broadcast always adds the TX to the graph, a TX listed by the wallet can be evicted
        if self.bdk_wallet.tx_graph().get_tx(bdk_txid).is_some() {
            return Err(Error::CannotEndTransfer { txid });
        }
        Ok(())
    }

    /// Prepare a transaction to create new UTXOs for RGB allocations (wasm32 async).
    ///
    /// Signing of the returned PSBT needs to be carried out separately. The signed PSBT then needs
    /// to be fed to the [`create_utxos_end`](Wallet::create_utxos_end) function.
    ///
    /// This doesn't require the wallet to have private keys.
    ///
    /// Returns a PSBT ready to be signed.
    pub async fn create_utxos_begin(
        &mut self,
        online: Online,
        up_to: bool,
        num: Option<u8>,
        size: Option<u32>,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Creating UTXOs (begin)...");
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let unspent_txos = self.database.get_unspent_txos(vec![])?;
        let unspents = self
            .database
            .get_rgb_allocations(unspent_txos, None, None, None, None)?;

        let mut utxos_to_create = num.unwrap_or(UTXO_NUM);
        if up_to {
            let allocatable = self.get_available_allocations(unspents, &[], None)?.len() as u8;
            if allocatable >= utxos_to_create {
                return Err(Error::AllocationsAlreadyAvailable);
            }
            utxos_to_create -= allocatable
        }
        debug!(self.logger, "Will try to create {} UTXOs", utxos_to_create);

        let inputs: Vec<BdkOutPoint> = self.internal_unspents().map(|u| u.outpoint).collect();
        let inputs: &[BdkOutPoint] = &inputs;
        let usable_btc_amount = self.get_uncolorable_btc_sum()?;
        let utxo_size = size.unwrap_or(UTXO_SIZE);
        if utxo_size == 0 {
            return Err(Error::InvalidAmountZero);
        }
        let possible_utxos = usable_btc_amount / utxo_size as u64;
        let max_possible_utxos: u8 = if possible_utxos > u8::MAX as u64 {
            u8::MAX
        } else {
            possible_utxos as u8
        };
        let mut btc_needed: u64 = (utxo_size as u64 * utxos_to_create as u64) + 1000;
        let mut btc_available: u64 = 0;
        let num_try_creating = min(utxos_to_create, max_possible_utxos);
        let mut addresses = vec![];
        for _i in 0..num_try_creating {
            addresses.push(self.get_new_address()?.script_pubkey());
        }
        while !addresses.is_empty() {
            match self._create_split_tx(inputs, &addresses, utxo_size, fee_rate_checked) {
                Ok(psbt) => {
                    self._record_vanilla_begin(&psbt, WalletTransactionType::CreateUtxos)
                        .await?;
                    info!(self.logger, "Create UTXOs (begin) completed");
                    return Ok(psbt.to_string());
                }
                Err(e) => {
                    (btc_needed, btc_available) = match e {
                        bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                            needed,
                            available,
                        }) => (needed.to_sat(), available.to_sat()),
                        bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                            return Err(Error::OutputBelowDustLimit);
                        }
                        _ => {
                            return Err(Error::Internal {
                                details: e.to_string(),
                            });
                        }
                    };
                    addresses.pop()
                }
            };
        }
        Err(Error::InsufficientBitcoins {
            needed: btc_needed,
            available: btc_available,
        })
    }

    /// Broadcast the provided PSBT to create new UTXOs (wasm32 async).
    ///
    /// The provided PSBT, prepared with the [`create_utxos_begin`](Wallet::create_utxos_begin)
    /// function, needs to have already been signed.
    ///
    /// This doesn't require the wallet to have private keys.
    ///
    /// Returns the number of created UTXOs, if `skip_sync` is set to true this will be 0.
    pub async fn create_utxos_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<u8, Error> {
        info!(self.logger, "Creating UTXOs (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        self._check_vanilla_end(&signed_psbt, WalletTransactionType::CreateUtxos)?;
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;

        let mut num_utxos_created = 0;
        if !skip_sync {
            let bdk_utxos: Vec<LocalOutput> = self.bdk_wallet.list_unspent().collect();
            let txid = tx.compute_txid();
            for utxo in bdk_utxos.into_iter() {
                if utxo.outpoint.txid == txid && utxo.keychain == KeychainKind::External {
                    num_utxos_created += 1
                }
            }
        }

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Create UTXOs (end) completed");
        Ok(num_utxos_created)
    }

    /// Async version of `_prepare_rgb_psbt` for wasm32.
    ///
    /// Mirrors the native `_prepare_rgb_psbt` but stores consignment bytes and transfer info
    /// in `self.transfer_artifacts` instead of the filesystem, and uses
    /// `_get_reject_list_async` instead of `_get_reject_list`.
    async fn _prepare_rgb_psbt_wasm(
        &mut self,
        psbt: &mut Psbt,
        transfer_info_map: &mut BTreeMap<String, InfoAssetTransfer>,
        txid_key: &str,
        donation: bool,
        unspents: Vec<LocalUnspent>,
        runtime: &mut RgbRuntime,
        min_confirmations: u8,
        expiration_timestamp: Option<i64>,
        btc_change: Option<BtcChange>,
        rejected: &mut HashSet<Opout>,
        reject_lists: &mut RejectLists,
    ) -> Result<Option<Vec<u8>>, Error> {
        let mut change_utxo_option = None;
        let mut change_utxo_idx = None;

        let prev_outputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|txin| txin.previous_output)
            .collect::<HashSet<OutPoint>>();

        let input_outpoints: Vec<Outpoint> =
            prev_outputs.iter().map(|o| Outpoint::from(*o)).collect();

        let mut asset_beneficiaries = bmap![];
        let mut extra_state = HashMap::<ContractId, Vec<(i32, Opout, AllocatedState)>>::new();
        let mut input_opouts: HashMap<ContractId, HashMap<Opout, AllocatedState>> = HashMap::new();
        for (asset_id, transfer_info) in transfer_info_map.iter_mut() {
            let asset_utxos = transfer_info.asset_spend.txo_map.values().cloned();
            let mut all_opout_state_vec = Vec::new();
            for (explicit_seal, opout_state_map) in runtime.contract_assignments_for(
                transfer_info.asset_info.contract_id,
                asset_utxos.clone(),
            )? {
                let txo_idx = self
                    .database
                    .get_txo(&explicit_seal.to_outpoint().into())?
                    .expect("outpoint should be in the DB")
                    .idx;
                all_opout_state_vec
                    .extend(opout_state_map.into_iter().map(|(o, s)| (txo_idx, o, s)));
            }

            all_opout_state_vec.sort_by_key(|(_, _, state)| state_amount(state));

            let mut inputs_added = AssignmentsCollection::default();
            let mut asset_transition_builder = runtime.transition_builder(
                transfer_info.asset_info.contract_id,
                transfer_info.main_transition.clone().type_name(),
            )?;
            // the main transition only takes some assignment types as inputs and assignments:
            // the others are moved by a transition of their default type in the same bundle
            let schema = runtime.contract_schema(transfer_info.asset_info.contract_id)?;
            let main_schema = &schema
                .transitions
                .get(&asset_transition_builder.transition_type())
                .expect("main transition missing from the schema")
                .transition_schema;
            let main_input_types: HashSet<AssignmentType> =
                main_schema.inputs.keys().copied().collect();
            let main_assignment_types: HashSet<AssignmentType> =
                main_schema.assignments.keys().copied().collect();
            let mut main_inputs = vec![];
            let mut main_used = false;
            let mut side_transfers: BTreeMap<AssignmentType, SideTransfer> = BTreeMap::new();
            // a retry raises the need, but the transition spends no more mint rights than asked
            let inputs_needed = AssignmentsCollection {
                mint: transfer_info.original_assignments_needed.mint,
                ..transfer_info.assignments_needed.clone()
            };
            for (txo_idx, opout, state) in all_opout_state_vec {
                let mut should_add_as_input = !rejected.contains(&opout);
                if should_add_as_input {
                    should_add_as_input =
                        inputs_added.opout_contributes(&opout, &state, &inputs_needed);
                }
                if !should_add_as_input {
                    extra_state
                        .entry(transfer_info.asset_info.contract_id)
                        .or_default()
                        .push((txo_idx, opout, state.clone()));
                    continue;
                }

                inputs_added.add_opout_state(&opout, &state);
                transfer_info
                    .assignments_spent
                    .entry(txo_idx)
                    .or_default()
                    .push(Assignment::from_opout_and_state(opout, &state));
                if main_input_types.contains(&opout.ty) {
                    main_inputs.push((opout, state.clone()));
                } else {
                    side_transfers
                        .entry(opout.ty)
                        .or_default()
                        .inputs
                        .push((opout, state.clone()));
                }
                input_opouts
                    .entry(transfer_info.asset_info.contract_id)
                    .or_default()
                    .insert(opout, state);
            }

            let mut beneficiaries = vec![];
            for recipient in &transfer_info.recipients {
                if matches!(recipient.assignment, Assignment::Fungible(0)) {
                    continue;
                }
                let seal: BuilderSeal<GraphSeal> = match &recipient.local_recipient_data {
                    LocalRecipientData::Blind(secret_seal) => BuilderSeal::Concealed(*secret_seal),
                    LocalRecipientData::Witness(witness_data) => {
                        let graph_seal = if let Some(blinding) = witness_data.blinding {
                            GraphSeal::with_blinded_vout(witness_data.vout, blinding)
                        } else {
                            GraphSeal::new_random_vout(witness_data.vout)
                        };
                        BuilderSeal::Revealed(graph_seal)
                    }
                };

                beneficiaries.push(seal);

                let (ty, amt) = match &recipient.assignment {
                    Assignment::Fungible(amt) => (OS_ASSET, *amt),
                    Assignment::InflationRight(amt) => (OS_INFLATION, *amt),
                    _ => unreachable!(),
                };
                asset_transition_builder = add_or_side_output(
                    asset_transition_builder,
                    &main_assignment_types,
                    &mut main_used,
                    &mut side_transfers,
                    ty,
                    seal,
                    Amount::from(amt).into(),
                )?;
            }

            let change = inputs_added.change(&transfer_info.original_assignments_needed);

            if change != AssignmentsCollection::default() {
                transfer_info.change = change.clone();
                let seal = self._get_change_seal(
                    &btc_change,
                    &mut change_utxo_option,
                    &mut change_utxo_idx,
                    &input_outpoints,
                    unspents.as_slice(),
                )?;
                let mut change_outputs: Vec<(AssignmentType, AllocatedState)> = vec![];
                if change.fungible > 0 {
                    change_outputs.push((OS_ASSET, Amount::from(change.fungible).into()));
                }
                if change.inflation > 0 {
                    change_outputs.push((OS_INFLATION, Amount::from(change.inflation).into()));
                }
                for _ in 0..change.mint {
                    change_outputs.push((OS_MINT, AllocatedState::Void));
                }
                for (ty, state) in change_outputs {
                    // split the inputs into change and the burn amount, sealed to an outpoint
                    // this TX spends; the main transition spends that split output
                    if !main_assignment_types.contains(&ty) && main_input_types.contains(&ty) {
                        let side = side_transfers.entry(ty).or_default();
                        if side.split.is_none() {
                            let (split_inputs, rest): (Vec<_>, Vec<_>) =
                                main_inputs.into_iter().partition(|(o, _)| o.ty == ty);
                            main_inputs = rest;
                            let inputs_amount: u64 =
                                split_inputs.iter().map(|(_, s)| state_amount(s)).sum();
                            let needed = inputs_amount
                                .checked_sub(state_amount(&state))
                                .ok_or(InternalError::Unexpected)?;
                            side.inputs.extend(split_inputs);
                            let split_seal =
                                GraphSeal::rand_from(psbt.unsigned_tx.input[0].previous_output);
                            side.split = Some((split_seal, needed));
                            side.outputs.push((
                                BuilderSeal::Revealed(split_seal),
                                Amount::from(needed).into(),
                            ));
                        }
                    }
                    asset_transition_builder = add_or_side_output(
                        asset_transition_builder,
                        &main_assignment_types,
                        &mut main_used,
                        &mut side_transfers,
                        ty,
                        BuilderSeal::Revealed(seal),
                        state,
                    )?;
                }
            };

            // the transitions moving the assignment types the main transition doesn't, before
            // the main one, which may spend the output of a split
            for (ty, side) in side_transfers {
                if side.outputs.is_empty() {
                    return Err(Error::Internal {
                        details: format!("not enough outputs to move assignments of type {ty}"),
                    });
                }
                let mut side_builder = runtime.transition_builder_raw(
                    transfer_info.asset_info.contract_id,
                    schema.default_transition_for_assignment(&ty),
                )?;
                for (opout, state) in side.inputs {
                    side_builder = side_builder.add_input(opout, state)?;
                }
                for (seal, state) in side.outputs {
                    side_builder = side_builder.add_owned_state_raw(ty, seal, state)?;
                }
                let side_transition = side_builder.complete_transition()?;
                if let Some((split_seal, needed)) = side.split
                    && let Some(no) = side_transition.assignments.get(&ty).and_then(|a| {
                        a.as_fungible()
                            .iter()
                            .position(|a| a.revealed_seal() == Some(split_seal))
                    })
                {
                    main_inputs.push((
                        Opout::new(side_transition.id(), ty, no as u16),
                        Amount::from(needed).into(),
                    ));
                }
                psbt.push_rgb_transition(side_transition)
                    .map_err(InternalError::from)?;
            }
            for (opout, state) in main_inputs {
                asset_transition_builder = asset_transition_builder.add_input(opout, state)?;
            }

            if transfer_info.main_transition == TypeOfTransition::Inflate {
                let inflation = transfer_info.original_assignments_needed.inflation;
                main_used = true;
                asset_transition_builder = asset_transition_builder
                    .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(inflation))
                    .unwrap()
                    .add_metadata(
                        RGB_METADATA_ALLOWED_INFLATION,
                        Amount::from(change.inflation),
                    )
                    .unwrap();
            }
            if transfer_info.main_transition == TypeOfTransition::Burn {
                let burn = transfer_info.original_assignments_needed.fungible;
                main_used = true;
                asset_transition_builder = asset_transition_builder
                    .add_global_state(RGB_GLOBAL_BURNED_ASSET, Amount::from(burn))
                    .unwrap();
                if let Some(burn_reason) = transfer_info.burn_reason {
                    asset_transition_builder = asset_transition_builder
                        .add_global_state(RGB_GLOBAL_BURN_REASON, burn_reason)?;
                }
            }
            if transfer_info.main_transition == TypeOfTransition::Mint {
                let before_block = transfer_info.before_block.ok_or(Error::Internal {
                    details: s!("mint transfer is missing the external chain block"),
                })?;
                let minted: u64 = transfer_info
                    .recipients
                    .iter()
                    .map(|r| r.assignment.main_amount())
                    .sum();
                main_used = true;
                asset_transition_builder = asset_transition_builder
                    .add_global_state(RGB_GLOBAL_ISSUED_SUPPLY, Amount::from(minted))
                    .unwrap()
                    .add_metadata("beforeBlock", rgbstd::stl::BlockNumber::from(before_block))
                    .unwrap();
            }

            // a transition must spend something: skip a main transition that spends and does
            // nothing (a transfer of rights only, which side transitions move)
            if asset_transition_builder.has_inputs() {
                let transition = asset_transition_builder.complete_transition()?;
                psbt.push_rgb_transition(transition)
                    .map_err(InternalError::from)?;
            } else if main_used {
                return Err(Error::Internal {
                    details: s!("main transition assigning or recording state without inputs"),
                });
            }
            asset_beneficiaries.insert(asset_id.clone(), beneficiaries);
        }

        for id in runtime.contracts_assigning(prev_outputs.clone())? {
            if transfer_info_map.contains_key(&id.to_string()) {
                continue;
            }
            let state = runtime.contract_assignments_for(id, prev_outputs.clone())?;
            let entry = extra_state.entry(id).or_default();
            for (explicit_seal, opout_state_map) in state {
                let txo_idx = self
                    .database
                    .get_txo(&explicit_seal.to_outpoint().into())?
                    .expect("outpoint should be in the DB")
                    .idx;
                entry.extend(opout_state_map.into_iter().map(|(o, s)| (txo_idx, o, s)));
            }
        }

        let mut extra_allocations: HashMap<String, TxoAssignments> = HashMap::new();
        for (cid, opout_state_map) in extra_state {
            let schema = runtime.contract_schema(cid)?;
            for (txo_idx, opout, state) in opout_state_map {
                let transition_type = schema.default_transition_for_assignment(&opout.ty);
                let mut extra_builder = runtime.transition_builder_raw(cid, transition_type)?;
                let assignment = Assignment::from_opout_and_state(opout, &state);
                let seal = self._get_change_seal(
                    &btc_change,
                    &mut change_utxo_option,
                    &mut change_utxo_idx,
                    &input_outpoints,
                    unspents.as_slice(),
                )?;
                extra_builder = extra_builder
                    .add_input(opout, state.clone())?
                    .add_owned_state_raw(opout.ty, seal, state)?;
                let extra_transition = extra_builder.complete_transition()?;
                extra_allocations
                    .entry(cid.to_string())
                    .or_default()
                    .entry(txo_idx)
                    .or_default()
                    .push(assignment);
                psbt.push_rgb_transition(extra_transition)
                    .map_err(InternalError::from)?;
            }
        }

        psbt.set_opret_host();
        let entropy = SysRng
            .try_next_u64()
            .expect("OS randomness source unavailable");
        let opret_idx = psbt
            .unsigned_tx
            .output
            .iter()
            .position(|o| o.script_pubkey.is_op_return())
            .expect("psbt should have an op_return output");
        psbt.outputs[opret_idx]
            .set_mpc_entropy(entropy)
            .map_err(InternalError::from)?;

        psbt.set_rgb_close_method(CloseMethod::OpretFirst);
        let fascia = psbt.rgb_commit().map_err(|e| Error::Internal {
            details: e.to_string(),
        })?;
        let entropy = psbt
            .outputs
            .iter()
            .find_map(|o| o.mpc_entropy())
            .expect("commit sets the entropy");

        let witness_txid = psbt.get_txid();

        let fascia_bytes = fascia
            .to_strict_serialized::<FASCIA_MAX_LEN>()
            .map_err(|e| Error::Internal {
                details: format!("cannot serialize fascia: {e}"),
            })?
            .release();
        // a burn enters the store at burn_end: a dry run or a failed begin leaves no split output
        let burning = transfer_info_map
            .values()
            .all(|ti| ti.main_transition == TypeOfTransition::Burn);
        if !burning {
            runtime.consume_fascia(fascia.clone(), None)?;
        }

        for (asset_id, transfer_info) in transfer_info_map.iter_mut() {
            let beneficiaries = asset_beneficiaries[asset_id].clone();
            let (beneficiaries_witness, beneficiaries_blinded) = beneficiaries.into_iter().fold(
                (Vec::new(), Vec::new()),
                |(mut witness, mut blinded), builder_seal| {
                    match builder_seal {
                        BuilderSeal::Revealed(seal) => {
                            let explicit_seal = ExplicitSeal::with(witness_txid, seal.vout);
                            witness.push(explicit_seal);
                        }
                        BuilderSeal::Concealed(secret_seal) => {
                            blinded.push(secret_seal);
                        }
                    }
                    (witness, blinded)
                },
            );

            let contract_id = transfer_info.asset_info.contract_id;
            let reject_list_source = match transfer_info.asset_info.reject_list_url.clone() {
                Some(url) => Some(RejectListSource::Url(url)),
                None if AssetSchema::get_from_contract_id(contract_id, runtime)?
                    == AssetSchema::Bfa =>
                {
                    runtime
                        .contract_wrapper::<BridgedFungibleAsset>(contract_id)?
                        .reject_list()
                        .map(RejectListSource::from)
                }
                None => None,
            };
            let is_burn = transfer_info.main_transition == TypeOfTransition::Burn;
            let keeps_fascia = matches!(
                transfer_info.main_transition,
                TypeOfTransition::Burn | TypeOfTransition::Mint
            );
            // check every operation, so that none spends a rejected allocation
            let should_build_dag = reject_list_source.is_some();

            let consignment = if should_build_dag {
                let (consignment, ops_dag) = if burning {
                    runtime.transfer_from_fascia_with_dag(
                        contract_id,
                        beneficiaries_witness,
                        beneficiaries_blinded,
                        burn_opids(&fascia, contract_id),
                        &fascia,
                    )?
                } else {
                    runtime.transfer_with_dag(
                        contract_id,
                        beneficiaries_witness,
                        beneficiaries_blinded,
                        burn_opids(&fascia, contract_id),
                        Some(witness_txid),
                    )?
                };

                // fetch a list once per operation, so that all retries use the same version
                let source = reject_list_source.as_ref().unwrap();
                if !reject_lists.contains_key(source) {
                    let decisions = self._get_reject_list_async(source).await?;
                    reject_lists.insert(source.clone(), decisions);
                }
                let (reject_opouts, allow_opouts) = &reject_lists[source];
                let asset_opouts = input_opouts
                    .get(&transfer_info.asset_info.contract_id)
                    .unwrap();
                let asset_input_opouts = asset_opouts.keys().cloned().collect();
                let to_reject =
                    self._check_dag(&ops_dag, reject_opouts, allow_opouts, &asset_input_opouts)?;
                if !to_reject.is_empty() {
                    warn!(
                        self.logger,
                        "Found {} rejected input opout(s), retrying transfer",
                        to_reject.len()
                    );
                    for rejected_opout in &to_reject {
                        if let Some(state) = asset_opouts.get(rejected_opout) {
                            transfer_info
                                .assignments_needed
                                .add_opout_state(rejected_opout, state);
                        }
                        rejected.insert(*rejected_opout);
                    }
                    return Ok(None);
                }

                Some(consignment)
            } else if is_burn {
                // a burn has no recipient: burn_end builds its consignment
                None
            } else {
                Some(runtime.transfer(
                    transfer_info.asset_info.contract_id,
                    beneficiaries_witness,
                    beneficiaries_blinded,
                    Some(witness_txid),
                )?)
            };

            let artifacts = self
                .transfer_artifacts
                .entry(txid_key.to_string())
                .or_default();
            if keeps_fascia {
                artifacts.fascia = Some(fascia_bytes.clone());
            }
            if let Some(consignment) = consignment {
                // Store consignment bytes in memory instead of filesystem
                let mut consignment_bytes = Vec::new();
                consignment.save(&mut consignment_bytes)?;
                artifacts
                    .consignment_bytes
                    .insert(asset_id.clone(), consignment_bytes);
            }

            // Store asset transfer info in memory
            self.transfer_artifacts
                .entry(txid_key.to_string())
                .or_default()
                .asset_infos
                .insert(asset_id.clone(), transfer_info.clone());
        }

        if !burning {
            runtime.upsert_witness(witness_txid, WitnessOrd::Tentative)?;
        }

        // Store batch transfer info in memory instead of filesystem
        let info_contents = InfoBatchTransfer {
            btc_change,
            change_utxo_idx,
            extra_allocations,
            donation,
            min_confirmations,
            expiration_timestamp,
            entropy,
            operation_secret_key: None,
            sender_transport_endpoint: None,
        };
        self.transfer_artifacts
            .entry(txid_key.to_string())
            .or_default()
            .batch_info = Some(info_contents);

        Ok(Some(fascia_bytes))
    }

    async fn _prepare_transfer_psbt_wasm(
        &mut self,
        transfer_info_map: &mut BTreeMap<String, InfoAssetTransfer>,
        donation: bool,
        unspents: Vec<LocalUnspent>,
        input_unspents: &[LocalUnspent],
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate_checked: FeeRate,
        min_confirmations: u8,
        expiration_timestamp: Option<i64>,
        lock_time: Option<u32>,
        operation_secret_key: Option<Vec<u8>>,
        sender_transport_endpoint: Option<String>,
        runtime: &mut RgbRuntime,
        rejected: &mut HashSet<Opout>,
        reject_lists: &mut RejectLists,
        dry_run: bool,
    ) -> Result<PrepareTransferPsbtResult, Error> {
        // prepare BDK PSBT
        let mut all_inputs: HashSet<BdkOutPoint> = transfer_info_map
            .values()
            .flat_map(|ti| ti.asset_spend.txo_map.values().map(|o| o.clone().into()))
            .collect();
        self._restore_missing_bdk_inputs(&all_inputs).await?;
        let (mut psbt, btc_change) = self._try_prepare_psbt(
            input_unspents,
            &mut all_inputs,
            witness_recipients,
            fee_rate_checked,
            lock_time,
        )?;
        psbt.unsigned_tx.output[0].script_pubkey = ScriptBuf::new_op_return([]);

        // Get txid for in-memory storage key
        let txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();

        // prepare RGB PSBT (stores artifacts in self.transfer_artifacts)
        let fascia = match self
            ._prepare_rgb_psbt_wasm(
                &mut psbt,
                transfer_info_map,
                &txid,
                donation,
                unspents,
                runtime,
                min_confirmations,
                expiration_timestamp,
                btc_change,
                rejected,
                reject_lists,
            )
            .await?
        {
            Some(fascia) => fascia,
            None => return Ok(PrepareTransferPsbtResult::Retry),
        };

        // Recompute txid after rgb_commit changed the OP_RETURN output
        let final_txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();
        if final_txid != txid
            && let Some(artifacts) = self.transfer_artifacts.remove(&txid)
        {
            self.transfer_artifacts
                .insert(final_txid.clone(), artifacts);
        }
        let batch_info = self
            .transfer_artifacts
            .get_mut(&final_txid)
            .and_then(|a| a.batch_info.as_mut())
            .expect("set by _prepare_rgb_psbt_wasm");
        batch_info.operation_secret_key = operation_secret_key;
        batch_info.sender_transport_endpoint = sender_transport_endpoint;
        let info = batch_info.clone();
        // reserve all inputs, also the ones with no allocations, until the operation ends, fails or
        // expires
        let artifacts = self
            .transfer_artifacts
            .get_mut(&final_txid)
            .expect("set by _prepare_rgb_psbt_wasm");
        artifacts.inputs = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|i| i.previous_output.into())
            .collect();
        artifacts.expiration = info
            .expiration_timestamp
            .unwrap_or_else(|| now().unix_timestamp() + DURATION_SEND_TRANSFER);

        let batch_transfer_idx = if dry_run {
            None
        } else {
            // save the send as Initiated to reserve its inputs
            Some(self._save_transfers(
                final_txid,
                transfer_info_map,
                info.extra_allocations,
                info.change_utxo_idx,
                info.btc_change,
                TransferStatus::Initiated,
                min_confirmations,
                expiration_timestamp,
                info.operation_secret_key,
                info.sender_transport_endpoint,
                &HashMap::new(),
            )?)
        };

        Ok(PrepareTransferPsbtResult::Success(BeginOperationData {
            psbt: psbt.to_string(),
            fascia,
            entropy: info.entropy,
            batch_transfer_idx,
        }))
    }

    fn _evm_chain(&self, chain_id: u64) -> Option<&EvmChain> {
        self.online_data.as_ref().unwrap().evm_chains.get(&chain_id)
    }

    fn _evm_client(&self, chain_id: u64) -> Option<EvmClient> {
        self._evm_chain(chain_id)
            .map(|evm_chain| EvmClient::new(&evm_chain.rpc_url))
    }

    /// Confirm the claims of a consignment that have a lock recorded by its bridge as of the
    /// latest finalized block of the external chain, included before the block they commit to.
    ///
    /// Return the claims not confirmed yet. A claim whose lock will never be confirmed gives
    /// [`Error::MintLockExpired`]. Without a usable EVM RPC endpoint for the chain, return
    /// [`ClaimsResolution::Retry`].
    async fn _resolve_claims(
        &self,
        pending: &mut PendingConsignment<true>,
    ) -> Result<ClaimsResolution, Error> {
        let claims: Vec<Claim> = pending.pending().unresolved_claims().cloned().collect();
        if claims.is_empty() {
            return Ok(ClaimsResolution::NoClaims);
        }
        let location = BridgedFungibleAsset::bridge_location(&pending.consignment().genesis)
            .map_err(|e| Error::Internal {
                details: format!("unexpected bridge location error: {e:?}"),
            })?;
        let BridgeLocation::Evm(EvmContract { chain_id, address }) = location.clone();
        let Some(evm_client) = self._evm_client(chain_id) else {
            warn!(
                self.logger,
                "Cannot resolve mint locks: no EVM RPC URL configured for chain ID {chain_id}, \
                 will retry"
            );
            return Ok(ClaimsResolution::Retry);
        };
        let resolver = MintClaimResolver::fetch(&evm_client, &location, &claims).await?;
        let checked = match CheckedClaimResolver::with(&resolver, location) {
            Ok(checked) => checked,
            Err(ClaimResolverError::WrongChainId { expected, actual }) => {
                warn!(
                    self.logger,
                    "Cannot resolve mint locks: the EVM RPC endpoint configured for chain ID \
                     {expected} is on chain ID {actual}, will retry"
                );
                return Ok(ClaimsResolution::Retry);
            }
            Err(e) => return Err(e.into()),
        };
        let mut unconfirmed = vec![];
        for claim in &claims {
            match pending.resolve_claim(&checked, claim) {
                Ok(()) => {}
                Err(ClaimResolverError::Unconfirmed(opid)) if resolver.is_expired(&opid) => {
                    let Claim::Lock { before_block, .. } = claim;
                    return Err(Error::MintLockExpired {
                        opid: opid.to_string(),
                        before_block: *before_block,
                    });
                }
                Err(ClaimResolverError::Unconfirmed(opid)) => unconfirmed.push(opid),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(ClaimsResolution::Resolved {
            evm_client,
            chain_id,
            address,
            unconfirmed,
        })
    }

    async fn _get_reject_list_async(
        &self,
        source: &RejectListSource,
    ) -> Result<(HashSet<Opout>, HashSet<Opout>), Error> {
        let reject_list_url = match source {
            RejectListSource::Url(url) => url,
            RejectListSource::Evm(EvmContract { chain_id, address }) => {
                let evm_client = self._evm_client(*chain_id).ok_or(Error::NoEvmRpcUrl {
                    chain_id: *chain_id,
                })?;
                return Ok(evm_client.reject_list_entries(address).await?.decisions());
            }
        };
        let list = RejectListClient::new(reject_list_url)?
            .get_reject_list()
            .await?;
        // trim each line and skip empty ones, count the invalid ones and warn once
        let mut opout_map = HashMap::new();
        let mut invalid = 0;
        for line in list.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let (is_allow, opout_str) = line.strip_prefix("!").map_or((false, line), |s| (true, s));
            match Opout::from_str(opout_str) {
                Ok(opout) => {
                    opout_map.insert(opout, is_allow);
                }
                Err(_) => invalid += 1,
            }
        }
        if invalid > 0 {
            warn!(
                self.logger,
                "Ignoring {invalid} invalid opout(s) in reject list"
            );
        }
        let (allow_opouts, reject_opouts) = opout_map.into_iter().fold(
            (HashSet::new(), HashSet::new()),
            |(mut allow, mut reject), (o, allowed)| {
                if allowed {
                    allow.insert(o);
                } else {
                    reject.insert(o);
                }
                (allow, reject)
            },
        );

        Ok((reject_opouts, allow_opouts))
    }

    /// The canonical ACK/NACK content message digest used both when signing (recipient) and
    /// verifying (sender) an ACK (SHA-256 of `transfer_id || true|false || consignment_hash`).
    fn ack_digest(transfer_id: &str, ack: bool, consignment_hash: &str) -> [u8; 32] {
        let msg = format!(
            "{transfer_id}{}{consignment_hash}",
            if ack { "true" } else { "false" }
        );
        sha256_digest(msg.as_bytes())
    }

    /// Symmetric key shared with the counterparty (from the own operation seed and the
    /// counterparty's hex-encoded Ed25519 pubkey) and the raw bytes of the expected plaintext hash.
    fn shared_key_and_hash(
        own_seed: &[u8],
        counterparty_pubkey_hex: &str,
        hash_hex: &str,
    ) -> Result<([u8; 32], [u8; 32]), Error> {
        let seed: [u8; 32] = own_seed.try_into().map_err(|_| Error::Internal {
            details: s!("invalid own secret key"),
        })?;
        let own_sk = ed25519_from_seed(&seed);
        let counterparty_vk =
            verifying_key_from_bytes(&hex::decode(counterparty_pubkey_hex).map_err(|_| {
                Error::Internal {
                    details: s!("invalid counterparty pubkey"),
                }
            })?)?;
        let derived = derive_shared(&own_sk, &counterparty_vk)?;
        let hash_raw = decode_hash_hex(hash_hex)?;
        Ok((derived.sym_key, hash_raw))
    }

    /// Verify a downloaded blob against its expected plaintext hash, first decrypting it with the
    /// key shared with the counterparty as dictated by `encryption`, the mode the payload is
    /// expected in: the recipient's invoice preference for consignments and media, always
    /// `Required` for the signed TX. For `Optional` the encrypted form is tried first, falling back
    /// to cleartext; for `Unsupported` the blob is taken as cleartext. Anything that doesn't match
    /// the hash afterwards is rejected.
    fn decrypt_blob_with_shared_key(
        raw: Vec<u8>,
        encryption: Encryption,
        own_seed: &[u8],
        counterparty_pubkey_hex: &str,
        hash_hex: &str,
    ) -> Result<Vec<u8>, Error> {
        let (sym_key, hash_raw) =
            Self::shared_key_and_hash(own_seed, counterparty_pubkey_hex, hash_hex)?;
        let plaintext = match encryption {
            Encryption::Unsupported => raw,
            Encryption::Required => decrypt_blob(&raw, &sym_key, &hash_raw)?,
            Encryption::Optional => match decrypt_blob(&raw, &sym_key, &hash_raw) {
                Ok(p) => p,
                Err(_) => raw,
            },
        };
        if hex::encode(sha256_digest(&plaintext)) != hash_hex {
            return Err(Error::HashMismatch);
        }
        Ok(plaintext)
    }

    /// [`decrypt_blob_with_shared_key`](Self::decrypt_blob_with_shared_key) for a payload as the
    /// proxy delivers it: base64-encoded in its JSON-RPC response.
    fn decrypt_payload_with_shared_key(
        payload_b64: &str,
        encryption: Encryption,
        own_seed: &[u8],
        counterparty_pubkey_hex: &str,
        hash_hex: &str,
    ) -> Result<Vec<u8>, Error> {
        let raw = general_purpose::STANDARD.decode(payload_b64).map_err(|e| {
            Error::InvalidEncryptedBlob {
                details: e.to_string(),
            }
        })?;
        Self::decrypt_blob_with_shared_key(
            raw,
            encryption,
            own_seed,
            counterparty_pubkey_hex,
            hash_hex,
        )
    }

    /// Sign an ACK/NACK with the recipient's operation key and post it to the sender's proxy.
    async fn sign_and_post_ack(
        &self,
        sender_proxy: &str,
        transfer_id: &str,
        ack: bool,
        consignment_hash: &str,
        recipient_seed: &[u8],
    ) -> Result<PostAckResponse, Error> {
        let seed: [u8; 32] = recipient_seed.try_into().map_err(|_| Error::Internal {
            details: s!("invalid recipient secret key"),
        })?;
        let sk = ed25519_from_seed(&seed);
        let digest = Self::ack_digest(transfer_id, ack, consignment_hash);
        let sig = hex::encode(sign_ed25519(&sk, &digest));
        let proxy_client = ProxyClient::new(sender_proxy)?;
        let res = proxy_client.post_ack(transfer_id, ack, &sig).await?;
        if let Some(err) = res.error {
            return Err(map_proxy_error(err));
        }
        res.result.ok_or_else(|| Error::Proxy {
            details: s!("missing ack.post result"),
        })
    }

    /// Decrypt the released signed TX, verify it, save it so it can be broadcast again until the
    /// broadcast deadline and broadcast it.
    async fn broadcast_received_tx(
        &mut self,
        recipient_id: &str,
        signed_tx_b64: &str,
        signed_tx_hash: &str,
        sender_pubkey_hex: &str,
        recipient_seed: &[u8],
    ) -> Result<(), Error> {
        let tx_bytes = Self::decrypt_payload_with_shared_key(
            signed_tx_b64,
            Encryption::Required,
            recipient_seed,
            sender_pubkey_hex,
            signed_tx_hash,
        )?;
        let tx: BdkTransaction = bdk_wallet::bitcoin::consensus::encode::deserialize(&tx_bytes)
            .map_err(|e| Error::Internal {
                details: e.to_string(),
            })?;
        self.received_signed_txs
            .insert(recipient_id.to_string(), tx_bytes);
        self._broadcast_tx(tx).await?;
        Ok(())
    }

    /// Broadcast again the signed TX released to a recipient, if saved, e.g. after it was evicted
    /// from the mempool. Failures are only logged, as the TX may e.g. have been replaced.
    async fn rebroadcast_received_tx(&self, recipient_id: &str) {
        let Some(tx_bytes) = self.received_signed_txs.get(recipient_id) else {
            return;
        };
        let tx: BdkTransaction = match bdk_wallet::bitcoin::consensus::encode::deserialize(tx_bytes)
        {
            Ok(tx) => tx,
            Err(e) => {
                warn!(self.logger, "Cannot load saved signed TX: {e}");
                return;
            }
        };
        debug!(self.logger, "Broadcasting the saved signed TX again");
        if let Err(e) = self._broadcast_tx(tx).await {
            warn!(
                self.logger,
                "Failed to broadcast the saved signed TX again: {e}"
            );
        }
    }

    /// Forget the data kept for a received consignment.
    fn clear_received_data(&mut self, recipient_id: &str) {
        self.received_consignments.remove(recipient_id);
        self.received_consignment_meta.remove(recipient_id);
        self.received_signed_txs.remove(recipient_id);
    }

    async fn _refuse_consignment_async(
        &mut self,
        data: &ProxyReceiveData,
        recipient_id: &str,
        recipient_seed: &[u8],
        updated_batch_transfer: &mut DbBatchTransferActMod,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(
            self.logger,
            "Refusing invalid consignment for {recipient_id}"
        );
        match self
            .sign_and_post_ack(
                &data.sender_proxy,
                &data.transfer_id,
                false,
                &data.consignment_hash,
                recipient_seed,
            )
            .await
        {
            Ok(r) => debug!(self.logger, "Consignment NACK response: {:?}", r),
            Err(Error::ProxyCannotChange { .. }) => {
                warn!(self.logger, "Found an ACK when trying NACK");
            }
            // the sender proxy may have no operation (e.g. donation): just fail locally
            Err(Error::OperationNotFound) => {}
            Err(e) => {
                error!(self.logger, "Failed to post NACK: {e}");
                return Err(e);
            }
        };
        // a sync may have flagged a TXO paying the script of the refused receive
        let scripts = script_buf_from_recipient_id(recipient_id.to_string())?
            .into_iter()
            .collect();
        self._unflag_pending_witness_txos(&HashSet::new(), &scripts)?;
        self.clear_received_data(recipient_id);
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Failed);
        Ok(Some(
            self.database
                .update_batch_transfer(updated_batch_transfer)?,
        ))
    }

    async fn _get_consignment_async(
        &self,
        proxy_url: &str,
        transfer_id: &str,
    ) -> Result<GetConsignmentResponse, Error> {
        let consignment_res = ProxyClient::new(proxy_url)?
            .get_consignment(transfer_id)
            .await;
        match consignment_res {
            Ok(res) => match res.result {
                Some(r) => Ok(r),
                None => {
                    debug!(
                        self.logger,
                        "Consignment GET response error: {:?}", res.error
                    );
                    Err(Error::NoConsignment)
                }
            },
            Err(e) => {
                debug!(self.logger, "Consignment GET response error: {e}");
                Err(Error::NoConsignment)
            }
        }
    }

    /// Find the transfer to an invoice on a recipient proxy and download its consignment.
    ///
    /// The proxy is asked for the pending transfers of the invoice, the first one whose transfer
    /// ID matches the one derived from the invoice ID and the sender pubkey is returned. Proxy
    /// errors (e.g. an unreachable proxy or an invoice it doesn't know) make it skip the proxy.
    async fn find_invoice_transfer(
        &self,
        proxy_url: &str,
        invoice_id: &str,
        recipient_seed: &[u8],
    ) -> Result<Option<(String, GetConsignmentResponse)>, Error> {
        let transfers = match list_invoice_transfers(proxy_url, invoice_id, recipient_seed).await {
            Ok(res) => match (res.result, res.error) {
                (Some(transfers), None) => transfers,
                (_, err) => {
                    debug!(self.logger, "Transfer list error: {:?}", err);
                    return Ok(None);
                }
            },
            Err(e) => {
                debug!(self.logger, "Transfer list error: {e}");
                return Ok(None);
            }
        };
        let mut pending: Vec<String> = transfers
            .into_iter()
            .filter(|(_, state)| *state == ProxyTransferState::Pending)
            .map(|(tid, _)| tid)
            .collect();
        pending.sort();
        for tid in pending {
            let result = match self._get_consignment_async(proxy_url, &tid).await {
                Err(Error::NoConsignment) => continue,
                Err(e) => return Err(e),
                Ok(r) => r,
            };
            // a transfer whose ID doesn't match the sender pubkey was not posted by its sender
            if transfer_id(invoice_id, &result.sender_pk) != tid {
                warn!(
                    self.logger,
                    "Ignoring transfer {tid}, which doesn't match its sender pubkey"
                );
                continue;
            }
            return Ok(Some((tid, result)));
        }
        Ok(None)
    }

    /// Whether a blind receive still waiting for the counterparty may be failed, which releases its
    /// blinded UTXO. Before the invoice expiration the receive can be cancelled. After it, the
    /// blinded UTXO stays reserved until the invoice expiration has passed on the proxies' clocks
    /// too and then, on each recipient proxy, either a `transfer.list` shows no pending transfer
    /// or the proxy retention grace has passed, since a transfer created before the expiration
    /// can be downloaded until then and would otherwise assign its assets to a spent UTXO. An
    /// "invoice not reserved" answer is taken as no pending transfer.
    async fn blind_receive_may_be_failed(
        &self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
    ) -> Result<bool, Error> {
        let Some(retention_deadline) = batch_transfer.retention_deadline else {
            return Ok(true);
        };
        let now = now().unix_timestamp();
        if !batch_transfer.is_expired(now) {
            return Ok(true);
        }
        let skew_margin = i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS);
        if !batch_transfer.is_expired_with_margin(now, PROXY_CLOCK_SKEW_MARGIN_SECS) {
            return Ok(false);
        }
        if retention_deadline + skew_margin < now {
            return Ok(true);
        }
        let batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let (_, transfer) = self.database.get_incoming_transfer(&batch_transfer_data)?;
        let (Some(recipient_id), Some(recipient_pubkey), Some(recipient_seed)) = (
            &transfer.recipient_id,
            &transfer.recipient_pubkey,
            &batch_transfer.operation_secret_key,
        ) else {
            return Ok(true);
        };
        let invoice_id = invoice_id(recipient_id, &hex::encode(recipient_pubkey));
        for (tte, te) in self
            .database
            .get_transfer_transport_endpoints_data(transfer.idx)?
        {
            if tte.kind != TransportEndpointKind::RecipientProxy {
                continue;
            }
            let res = list_invoice_transfers(&te.endpoint, &invoice_id, recipient_seed).await;
            let nothing_pending = match res {
                Ok(res) => match (res.result, res.error) {
                    (Some(transfers), None) => transfers
                        .values()
                        .all(|state| *state != ProxyTransferState::Pending),
                    (_, Some(err)) => matches!(map_proxy_error(err), Error::InvoiceNotReserved),
                    (None, None) => false,
                },
                Err(_) => false,
            };
            if !nothing_pending {
                debug!(
                    self.logger,
                    "Recipient proxy {} may still hold a transfer to the invoice", te.endpoint
                );
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Let the recipient proxy delete the data of a transfer that has been handled. Failures are
    /// only logged, as the proxy deletes the data anyway once the invoice expires.
    async fn mark_transfer_retrieved(
        &self,
        proxy_url: &str,
        transfer_id: &str,
        recipient_seed: &[u8],
    ) {
        let Ok(seed) = <[u8; 32]>::try_from(recipient_seed) else {
            return;
        };
        let sk = ed25519_from_seed(&seed);
        let digest = sha256_digest(transfer_retrieved_message(transfer_id).as_bytes());
        let signature = hex::encode(sign_ed25519(&sk, &digest));
        let res = match ProxyClient::new(proxy_url) {
            Ok(client) => client.transfer_retrieved(transfer_id, &signature).await,
            Err(e) => Err(e),
        };
        match res {
            Ok(r) if r.error.is_none() => {}
            Ok(r) => warn!(
                self.logger,
                "Failed to mark transfer as retrieved: {:?}", r.error
            ),
            Err(e) => warn!(self.logger, "Failed to mark transfer as retrieved: {e}"),
        }
    }

    async fn ack_consignment(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        transfer: &DbTransfer,
        updated_batch_transfer: &mut DbBatchTransferActMod,
        data: &ProxyReceiveData,
        recipient_seed: &[u8],
        // the sender proxy retention grace, for a transfer whose TX the sender proxy releases
        retention_grace: Option<u32>,
        // signed TX embedded in the consignment (donation flow), broadcast directly if present
        donation_tx: Option<BdkTransaction>,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "ACKing consignment...");

        let recipient_id = transfer
            .recipient_id
            .clone()
            .expect("transfer should have a recipient ID");

        // record our own ACK on the transfer
        let mut updated_transfer: DbTransferActMod = transfer.clone().into();
        updated_transfer.ack = ActiveValue::Set(Some(true));

        let res = match self
            .sign_and_post_ack(
                &data.sender_proxy,
                &data.transfer_id,
                true,
                &data.consignment_hash,
                recipient_seed,
            )
            .await
        {
            Ok(r) => r,
            // the sender proxy may have no operation (e.g. donation): broadcast locally
            Err(Error::OperationNotFound) if donation_tx.is_some() => {
                self.database.update_transfer(&mut updated_transfer)?;
                self._broadcast_tx(donation_tx.expect("checked to be some"))
                    .await?;
                updated_batch_transfer.status =
                    ActiveValue::Set(TransferStatus::WaitingConfirmations);
                return Ok(Some(
                    self.database
                        .update_batch_transfer(updated_batch_transfer)?,
                ));
            }
            // a NACK is already recorded for this recipient (only this wallet could have posted
            // it, as ACKs are signed with its operation key): the sender fails the operation on
            // seeing it, so there is nothing left to wait for
            Err(Error::ProxyCannotChange { .. }) => {
                warn!(
                    self.logger,
                    "A NACK was already posted for this recipient, failing the transfer"
                );
                return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
            }
            Err(e) => return Err(e),
        };
        debug!(self.logger, "Consignment ACK response: {:?}", res);

        self.database.update_transfer(&mut updated_transfer)?;

        // the shared deadline for broadcasting the TX, after which (plus a confirmation margin)
        // the transfer is failed if the TX is not confirmed
        if let Some(grace) = retention_grace {
            updated_batch_transfer.broadcast_deadline =
                ActiveValue::Set(broadcast_deadline(res.expires_at.as_deref(), grace));
        }

        match (res.signed_tx, res.signed_tx_hash, res.sender_pk) {
            (Some(signed_tx), Some(signed_tx_hash), Some(sender_pubkey)) => {
                // the TX has been released: decrypt and broadcast it
                self.broadcast_received_tx(
                    &recipient_id,
                    &signed_tx,
                    &signed_tx_hash,
                    &sender_pubkey,
                    recipient_seed,
                )
                .await?;
                updated_batch_transfer.status =
                    ActiveValue::Set(TransferStatus::WaitingConfirmations);
            }
            _ if donation_tx.is_some() => {
                // donation: the signed TX travels inside the consignment, broadcast it
                self._broadcast_tx(donation_tx.expect("checked to be some"))
                    .await?;
                updated_batch_transfer.status =
                    ActiveValue::Set(TransferStatus::WaitingConfirmations);
            }
            _ => {
                // the TX has not been released (yet): for a batch operation not all recipients
                // have ACKed yet, for a non-batch one the proxy is not compliant. Either way wait
                // for the broadcast, posting the ACK again until the TX is returned or the
                // operation expires
                if !data.batch {
                    warn!(
                        self.logger,
                        "No signed TX released for a non-batch operation"
                    );
                }
                updated_batch_transfer.status = ActiveValue::Set(TransferStatus::WaitingBroadcast);
            }
        }

        Ok(Some(
            self.database
                .update_batch_transfer(updated_batch_transfer)?,
        ))
    }

    /// Fetch the media of a contract from the proxy and check it against the contract. Return
    /// `false` if it is missing or does not match.
    #[allow(clippy::too_many_arguments)]
    async fn _contract_media_verified(
        &self,
        runtime: &RgbRuntime,
        valid_contract: &ValidContract,
        asset_schema: AssetSchema,
        data: &ProxyReceiveData,
        recipient_seed: &[u8],
        seed: &[u8; 32],
        sender_vk: &VerifyingKey,
    ) -> Result<bool, Error> {
        let mut attachments = vec![];
        match asset_schema {
            AssetSchema::Nia => {
                let contract_data = runtime.consignment_data(valid_contract)?;
                let contract = NiaWrapper::with(contract_data);
                if let Some(attachment) = contract.contract_terms().media {
                    attachments.push(attachment)
                }
            }
            AssetSchema::Ifa => {
                let contract_data = runtime.consignment_data(valid_contract)?;
                let contract = IfaWrapper::with(contract_data);
                if let Some(attachment) = contract.contract_terms().media {
                    attachments.push(attachment)
                }
            }
            AssetSchema::Bfa => {
                let contract_data = runtime.consignment_data(valid_contract)?;
                let contract = BfaWrapper::with(contract_data);
                if let Some(attachment) = contract.contract_terms().media {
                    attachments.push(attachment)
                }
            }
        };
        for attachment in attachments {
            let digest = hex::encode(attachment.digest);
            // on wasm32, skip media file storage (no filesystem): the media is fetched
            // and checked against the digest in the contract
            let derived = derive_shared(&ed25519_from_seed(seed), sender_vk)?;
            let media_id = derived.media_id(&digest);
            let media_res = ProxyClient::new(&data.proxy_url)?
                .get_media(&data.transfer_id, &media_id)
                .await?;
            let Some(media_b64) = media_res.result else {
                error!(
                    self.logger,
                    "Cannot find the media file but the contract defines one"
                );
                return Ok(false);
            };
            // also checks the plaintext against the digest in the contract
            if Self::decrypt_payload_with_shared_key(
                &media_b64,
                data.recipient_encryption,
                recipient_seed,
                &data.sender_pubkey,
                &digest,
            )
            .is_err()
            {
                error!(
                    self.logger,
                    "Attached file has a different hash than the one in the contract"
                );
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Validate what a consignment pays and its history. `None` means refuse the consignment.
    /// `mint_transfer` is the transfer of a mint, which must pay what its invoice requests.
    /// The history is checked against the reject list only when `check_reject_list` is true.
    #[allow(clippy::too_many_arguments)]
    async fn _check_received_consignment(
        &self,
        runtime: &RgbRuntime,
        pending: PendingConsignment<true>,
        receiving: &HashMap<Opout, Assignment>,
        asset_schema: AssetSchema,
        asset_id: &str,
        mint_transfer: Option<&DbTransfer>,
        check_reject_list: bool,
    ) -> Result<Option<(ValidConsignment<true>, HashMap<Opout, Assignment>)>, Error> {
        let valid_consignment = pending.finalize();
        let validation_status = valid_consignment.validation_status();
        let validity = validation_status.validity();
        debug!(self.logger, "Consignment validity: {:?}", validity);

        let received = receiving.clone();
        let reject_list_source = match asset_schema {
            _ if !check_reject_list => None,
            AssetSchema::Ifa => {
                let url = if let Ok(ass) = self.database.check_asset_exists(asset_id.to_string()) {
                    ass.reject_list_url
                } else {
                    let contract = IfaWrapper::with(runtime.consignment_data(&valid_consignment)?);
                    contract.reject_list_url().map(|u| u.to_string())
                };
                url.map(RejectListSource::Url)
            }
            AssetSchema::Bfa => BfaWrapper::with(runtime.consignment_data(&valid_consignment)?)
                .reject_list()
                .map(RejectListSource::from),
            AssetSchema::Nia => None,
        };
        if let Some(source) = &reject_list_source {
            let (reject_opouts, allow_opouts) = self._get_reject_list_async(source).await?;

            let to_reject = self._check_dag(
                validation_status
                    .ops_dag
                    .as_ref()
                    .expect("build_ops_dag is true"),
                &reject_opouts,
                &allow_opouts,
                &received.clone().into_keys().collect(),
            )?;

            if !to_reject.is_empty() {
                error!(
                    self.logger,
                    "Found {} opout(s) that must be rejected",
                    to_reject.len()
                );
                return Ok(None);
            } else {
                info!(
                    self.logger,
                    "Didn't find any opout(s) that should be rejected"
                );
            }
        }

        if let Some(transfer) = mint_transfer
            && !mint_pays_requested(transfer, &received)
        {
            error!(self.logger, "Mint does not pay the invoice");
            return Ok(None);
        }

        Ok(Some((valid_consignment, received)))
    }

    /// Prepare the PSBT of an operation on one asset (an inflation, a burn or a mint).
    /// Retry without the inputs that the reject list rejects, and select more inputs to replace them.
    #[allow(clippy::too_many_arguments)]
    async fn _prepare_single_asset_psbt(
        &mut self,
        transfer_info: InfoAssetTransfer,
        unspents: Vec<LocalUnspent>,
        input_unspents: &[LocalUnspent],
        witness_recipients: &Vec<(ScriptBuf, u64)>,
        fee_rate_checked: FeeRate,
        min_confirmations: u8,
        operation_secret_key: Option<Vec<u8>>,
        sender_transport_endpoint: Option<String>,
        runtime: &mut RgbRuntime,
        dry_run: bool,
    ) -> Result<BeginOperationData, Error> {
        let asset_id = transfer_info.asset_info.contract_id.to_string();
        let mut rejected = HashSet::new();
        let mut reject_lists = RejectLists::new();
        let mut transfer_info_map = BTreeMap::from([(asset_id.clone(), transfer_info.clone())]);
        loop {
            match self
                ._prepare_transfer_psbt_wasm(
                    &mut transfer_info_map,
                    false,
                    unspents.clone(),
                    input_unspents,
                    witness_recipients,
                    fee_rate_checked,
                    min_confirmations,
                    None,
                    None,
                    operation_secret_key.clone(),
                    sender_transport_endpoint.clone(),
                    runtime,
                    &mut rejected,
                    &mut reject_lists,
                    dry_run,
                )
                .await?
            {
                PrepareTransferPsbtResult::Retry => {
                    // select the inputs again, without the rejected ones
                    let assignments_needed =
                        transfer_info_map[&asset_id].assignments_needed.clone();
                    let asset_spend = self._select_rgb_inputs(
                        asset_id.clone(),
                        &assignments_needed,
                        input_unspents.to_vec(),
                    )?;
                    transfer_info_map = BTreeMap::from([(
                        asset_id.clone(),
                        InfoAssetTransfer {
                            asset_spend,
                            assignments_needed,
                            ..transfer_info.clone()
                        },
                    )]);
                }
                PrepareTransferPsbtResult::Success(begin_data) => return Ok(begin_data),
            }
        }
    }

    async fn _wait_consignment_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting consignment...");

        let batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let (asset_transfer, transfer) =
            self.database.get_incoming_transfer(&batch_transfer_data)?;
        let recipient_id = transfer
            .recipient_id
            .clone()
            .expect("transfer should have a recipient ID");
        debug!(self.logger, "Recipient ID: {recipient_id}");

        let tte_data = self
            .database
            .get_transfer_transport_endpoints_data(transfer.idx)?;

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();

        // the recipient's per-operation Ed25519 secret seed (used to derive the shared secret)
        let recipient_seed =
            batch_transfer
                .operation_secret_key
                .clone()
                .ok_or_else(|| Error::Internal {
                    details: s!("missing operation secret key"),
                })?;

        // if the consignment was downloaded in a previous attempt that failed for a transient
        // reason, reuse it instead of hitting the proxy again
        let cached = match (
            self.received_consignments.contains_key(&recipient_id),
            self.received_consignment_meta.get(&recipient_id),
        ) {
            (true, Some(meta)) => meta.transfer_id.clone().and_then(|cached_transfer_id| {
                tte_data
                    .iter()
                    .find(|(tte, te)| {
                        tte.kind == TransportEndpointKind::RecipientProxy
                            && meta
                                .proxy_url
                                .as_ref()
                                .map_or(tte.used, |url| url == &te.endpoint)
                    })
                    .map(|(tte, te)| {
                        (
                            tte.clone(),
                            ProxyReceiveData {
                                proxy_url: te.endpoint.clone(),
                                transfer_id: cached_transfer_id,
                                sender_proxy: meta.sender_transport_endpoint.clone(),
                                sender_pubkey: meta.sender_pubkey.clone(),
                                consignment_hash: meta.consignment_hash.clone(),
                                recipient_encryption: transfer.recipient_encryption,
                                batch: meta.batch,
                            },
                        )
                    })
            }),
            _ => None,
        };
        let data = if let Some((cached_tte, data)) = cached {
            debug!(
                self.logger,
                "Reusing previously-downloaded consignment for {recipient_id}"
            );
            if !cached_tte.used {
                let mut updated_tte: DbTransferTransportEndpointActMod = cached_tte.into();
                updated_tte.used = ActiveValue::Set(true);
                self.database
                    .update_transfer_transport_endpoint(&mut updated_tte)?;
            }
            if transfer.consignment_hash.is_none() || transfer.proxy_transfer_id.is_none() {
                let mut ut: DbTransferActMod = transfer.clone().into();
                ut.consignment_hash = ActiveValue::Set(Some(data.consignment_hash.clone()));
                ut.proxy_transfer_id = ActiveValue::Set(Some(data.transfer_id.clone()));
                self.database.update_transfer(&mut ut)?;
            }
            if !tte_data
                .iter()
                .any(|(tte, _)| tte.kind == TransportEndpointKind::SenderProxy)
            {
                self.save_sender_transport_endpoint(transfer.idx, &data.sender_proxy)?;
            }
            data
        } else {
            // discover the transfer to the invoice and download its consignment
            let recipient_pk = hex::encode(transfer.recipient_pubkey.as_ref().ok_or_else(
                || Error::Internal {
                    details: s!("missing recipient pubkey"),
                },
            )?);
            let invoice_id = invoice_id(&recipient_id, &recipient_pk);
            let mut proxy_res = None;
            for (transfer_transport_endpoint, transport_endpoint) in tte_data {
                if transfer_transport_endpoint.kind != TransportEndpointKind::RecipientProxy {
                    continue;
                }
                let Some(found) = self
                    .find_invoice_transfer(
                        &transport_endpoint.endpoint,
                        &invoice_id,
                        &recipient_seed,
                    )
                    .await?
                else {
                    info!(
                        self.logger,
                        "Skipping transport endpoint: {:?}", &transport_endpoint
                    );
                    continue;
                };

                proxy_res = Some((transport_endpoint.endpoint, found));
                let mut updated_tte: DbTransferTransportEndpointActMod =
                    transfer_transport_endpoint.into();
                updated_tte.used = ActiveValue::Set(true);
                self.database
                    .update_transfer_transport_endpoint(&mut updated_tte)?;
                break;
            }
            let Some((proxy_url, (transfer_id, result))) = proxy_res else {
                return Ok(None);
            };
            // the sender endpoint arrives in RgbTransport (rpc://) format; convert to an HTTP URL
            let sender_proxy =
                self.convert_transport_endpoint_url(&result.sender_transport_endpoint)?;
            let data = ProxyReceiveData {
                proxy_url,
                transfer_id,
                sender_proxy,
                sender_pubkey: result.sender_pk.clone(),
                consignment_hash: result.consignment_hash.clone(),
                recipient_encryption: transfer.recipient_encryption,
                batch: result.batch,
            };

            // decode and (if needed) decrypt the consignment; a malformed payload is refused like
            // one that fails decryption or verification
            match Self::decrypt_payload_with_shared_key(
                &result.consignment,
                data.recipient_encryption,
                &recipient_seed,
                &data.sender_pubkey,
                &data.consignment_hash,
            ) {
                Ok(bytes) => {
                    self.received_consignments
                        .insert(recipient_id.clone(), bytes);
                }
                Err(_) => {
                    error!(self.logger, "Failed to decrypt/verify consignment");
                    return self
                        ._refuse_consignment_async(
                            &data,
                            &recipient_id,
                            &recipient_seed,
                            &mut updated_batch_transfer,
                        )
                        .await;
                }
            }
            self.received_consignment_meta.insert(
                recipient_id.clone(),
                ReceivedConsignmentMeta {
                    proxy_url: Some(data.proxy_url.clone()),
                    transfer_id: Some(data.transfer_id.clone()),
                    sender_transport_endpoint: data.sender_proxy.clone(),
                    sender_pubkey: data.sender_pubkey.clone(),
                    consignment_hash: data.consignment_hash.clone(),
                    batch: data.batch,
                },
            );

            // persist the transfer ID, the consignment hash and the sender proxy endpoint (for the
            // ACK)
            let mut ut: DbTransferActMod = transfer.clone().into();
            ut.consignment_hash = ActiveValue::Set(Some(data.consignment_hash.clone()));
            ut.proxy_transfer_id = ActiveValue::Set(Some(data.transfer_id.clone()));
            self.database.update_transfer(&mut ut)?;
            self.save_sender_transport_endpoint(transfer.idx, &data.sender_proxy)?;

            data
        };

        let res = self
            ._process_received_consignment_async(
                batch_transfer,
                &asset_transfer,
                &transfer,
                recipient_id,
                &data,
                &recipient_seed,
                &mut updated_batch_transfer,
            )
            .await?;
        // once the consignment has been handled (ACKed or refused), its data is saved locally and
        // the proxy can delete it, while the transfer keeps counting towards the invoice quota
        if res.is_some() {
            self.mark_transfer_retrieved(&data.proxy_url, &data.transfer_id, &recipient_seed)
                .await;
        }
        Ok(res)
    }

    #[allow(clippy::too_many_arguments)]
    async fn _process_received_consignment_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        asset_transfer: &DbAssetTransfer,
        transfer: &DbTransfer,
        recipient_id: String,
        data: &ProxyReceiveData,
        recipient_seed: &[u8],
        updated_batch_transfer: &mut DbBatchTransferActMod,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        let consignment_bytes = self
            .received_consignments
            .get(&recipient_id)
            .ok_or_else(|| Error::Internal {
                details: s!("consignment not found in memory"),
            })?
            .clone();

        let mut runtime = self.rgb_runtime()?;
        let consignment = match RgbTransfer::load(&consignment_bytes[..]) {
            Ok(c) => c,
            Err(e) => {
                error!(self.logger, "Failed to deserialize consignment: {e}");
                return self
                    ._refuse_consignment_async(
                        data,
                        &recipient_id,
                        recipient_seed,
                        updated_batch_transfer,
                    )
                    .await;
            }
        };
        let contract_id = consignment.contract_id();
        let asset_id = contract_id.to_string();
        let asset_schema: AssetSchema = consignment.schema_id().try_into()?;

        if !self.supports_schema(&asset_schema) {
            error!(
                self.logger,
                "The wallet doesn't support the provided schema: {}", asset_schema
            );
            return self
                ._refuse_consignment_async(
                    data,
                    &recipient_id,
                    recipient_seed,
                    updated_batch_transfer,
                )
                .await;
        }

        // validate consignment
        if let Some(aid) = asset_transfer.asset_id.clone()
            && aid != asset_id
        {
            error!(
                self.logger,
                "Received a different asset than the expected one"
            );
            return self
                ._refuse_consignment_async(
                    data,
                    &recipient_id,
                    recipient_seed,
                    updated_batch_transfer,
                )
                .await;
        }

        // the seal blinding is derived from the keys of both parties
        let seed: [u8; 32] = recipient_seed.try_into().map_err(|_| Error::Internal {
            details: s!("invalid recipient secret key"),
        })?;
        let sender_vk = hex::decode(&data.sender_pubkey)
            .ok()
            .and_then(|pk| verifying_key_from_bytes(&pk).ok());
        let Some(sender_vk) = sender_vk else {
            error!(self.logger, "Invalid sender pubkey");
            return self
                ._refuse_consignment_async(
                    data,
                    &recipient_id,
                    recipient_seed,
                    updated_batch_transfer,
                )
                .await;
        };
        let blinding = derive_shared(&ed25519_from_seed(&seed), &sender_vk)?.seal_blinding;
        let matcher = transfer.receive_matcher(blinding)?;
        let spent = Self::known_spent_opouts(&consignment);
        let mut payments: Vec<_> = self.find_payments(&consignment, &matcher, &spent).collect();
        if payments.len() != 1 {
            if payments.is_empty() {
                error!(
                    self.logger,
                    "Cannot find any unspent assignment paying this recipient"
                );
            } else {
                error!(
                    self.logger,
                    "Consignment pays this recipient at {} destinations, refusing it as ambiguous",
                    payments.len()
                );
            }
            return self
                ._refuse_consignment_async(
                    data,
                    &recipient_id,
                    recipient_seed,
                    updated_batch_transfer,
                )
                .await;
        }
        let ConsignmentPayment {
            bundle: paying_bundle,
            vout,
            receiving,
        } = payments.remove(0);
        let witness_id = paying_bundle.witness_id();
        let txid = witness_id.to_string();
        let donation_tx = Self::donation_signed_tx(&paying_bundle.tx);

        let chain_resolver = self
            .prefetch_witnesses(consignment.bundled_witnesses().map(|bw| bw.witness_id()))
            .await
            .map_err(|e| Error::Network {
                details: e.to_string(),
            })?;
        let resolver = OffchainResolverWasm {
            witness_id,
            consignment: &consignment,
            fallback: &chain_resolver,
        };

        debug!(self.logger, "Validating consignment...");
        let schema_rules = asset_schema.schema_rules();
        let validation_config = ValidationConfig {
            chain_net: self.chain_net(),
            build_ops_dag: true,
            ..Default::default()
        };
        let mut pending = match consignment
            .clone()
            .validate_deterministic(&schema_rules, &validation_config)
            .and_then(|mut p| p.pending().resolve_all(&resolver).map(|_| p))
        {
            Ok(pending) => pending,
            Err(ValidationError::InvalidConsignment(e)) => {
                error!(self.logger, "Consignment is invalid: {}", e);
                return self
                    ._refuse_consignment_async(
                        data,
                        &recipient_id,
                        recipient_seed,
                        updated_batch_transfer,
                    )
                    .await;
            }
            Err(ValidationError::ResolverError(e)) => {
                warn!(self.logger, "Network error during consignment validation");
                return Err(Error::Network {
                    details: e.to_string(),
                });
            }
            Err(e) => {
                return Err(Error::Internal {
                    details: format!("unexpected consignment validation error: {e}"),
                });
            }
        };
        // the mint paying this recipient, as opposed to those in its history
        let terminal_opids: HashSet<OpId> = consignment
            .bundles
            .iter()
            .filter(|ab| ab.witness_id() == witness_id)
            .flat_map(|ab| ab.bundle.known_transitions.iter().map(|kt| kt.opid))
            .collect();
        let paying_claim = pending
            .pending()
            .unresolved_claims()
            .find(|Claim::Lock { opid, .. }| terminal_opids.contains(opid))
            .cloned();
        let received_mint = paying_claim
            .as_ref()
            .map(|claim| {
                let Claim::Lock {
                    opid,
                    amount,
                    before_block,
                    ..
                } = claim;
                let BridgeLocation::Evm(EvmContract { chain_id, address }) =
                    BridgedFungibleAsset::bridge_location(&consignment.genesis).map_err(|e| {
                        Error::Internal {
                            details: format!("unexpected bridge location error: {e:?}"),
                        }
                    })?;
                Ok::<_, Error>(ReceivedMint {
                    opid: opid.to_string(),
                    lock_id: LockId::from(claim).to_string(),
                    amount: *amount,
                    before_block: *before_block,
                    evm_chain_id: chain_id,
                    bridge_contract: address.to_string(),
                })
            })
            .transpose()?;
        // a mint whose lock may be made completes whatever the wallet does: the reject list is
        // not checked again for a mint already checked, nor for one whose lock is recorded
        let checked_mint = self.received_mints.contains_key(&recipient_id);
        let mut committed_mint = checked_mint;
        match self._resolve_claims(&mut pending).await {
            Err(e @ Error::MintLockExpired { .. }) => {
                error!(self.logger, "{e}");
                return self
                    ._refuse_consignment_async(
                        data,
                        &recipient_id,
                        recipient_seed,
                        updated_batch_transfer,
                    )
                    .await;
            }
            Err(e) => return Err(e),
            Ok(ClaimsResolution::Retry) => return Ok(None),
            Ok(ClaimsResolution::NoClaims) => {}
            Ok(ClaimsResolution::Resolved {
                evm_client,
                chain_id,
                address,
                unconfirmed,
            }) => {
                let paying_opid = paying_claim.as_ref().map(|Claim::Lock { opid, .. }| *opid);
                if let Some(opid) = &paying_opid
                    && !unconfirmed.contains(opid)
                {
                    debug!(self.logger, "Mint lock already finalized");
                    committed_mint = true;
                }
                if !unconfirmed.is_empty() {
                    let others_unconfirmed =
                        unconfirmed.iter().any(|opid| Some(*opid) != paying_opid);
                    // check the mint once before the lock, so the user knows what to lock
                    if let Some(mint) = &received_mint
                        && let Some(claim) = &paying_claim
                        && !checked_mint
                        && !others_unconfirmed
                    {
                        if !committed_mint
                            && evm_client
                                .mint_lock_recorded(
                                    &address,
                                    &LockId::from(claim),
                                    mint.amount,
                                    mint.before_block,
                                )
                                .await?
                        {
                            debug!(self.logger, "Mint lock already recorded");
                            committed_mint = true;
                        }
                        if !committed_mint {
                            let max_mint_lock_blocks = self
                                ._evm_chain(chain_id)
                                .expect("configured along with the EVM RPC URL")
                                .max_mint_lock_blocks;
                            let latest_block = evm_client.block_number().await?;
                            if mint.before_block > latest_block.saturating_add(max_mint_lock_blocks)
                            {
                                error!(
                                    self.logger,
                                    "Mint lock has to be included before block {}, more than {} \
                                     blocks after the latest external chain block {}",
                                    mint.before_block,
                                    max_mint_lock_blocks,
                                    latest_block
                                );
                                return self
                                    ._refuse_consignment_async(
                                        data,
                                        &recipient_id,
                                        recipient_seed,
                                        updated_batch_transfer,
                                    )
                                    .await;
                            }
                        }
                        let mut assumed = pending.clone();
                        let unresolved: Vec<Claim> =
                            assumed.pending().unresolved_claims().cloned().collect();
                        for claim in &unresolved {
                            assumed.pending().resolve_claim(claim).map_err(|e| {
                                Error::Internal {
                                    details: format!("unexpected claim resolution error: {e}"),
                                }
                            })?;
                        }
                        let checked = self
                            ._check_received_consignment(
                                &runtime,
                                assumed,
                                &receiving,
                                asset_schema,
                                &asset_id,
                                Some(transfer),
                                !committed_mint,
                            )
                            .await?;
                        let Some((valid_consignment, _)) = checked else {
                            return self
                                ._refuse_consignment_async(
                                    data,
                                    &recipient_id,
                                    recipient_seed,
                                    updated_batch_transfer,
                                )
                                .await;
                        };
                        // the proxy can delete the media before the mint settles: verify it now
                        if asset_transfer.asset_id.is_none()
                            && self.database.check_asset_exists(asset_id.clone()).is_err()
                            && !self
                                ._contract_media_verified(
                                    &runtime,
                                    &valid_consignment.into_valid_contract(),
                                    asset_schema,
                                    data,
                                    recipient_seed,
                                    &seed,
                                    &sender_vk,
                                )
                                .await?
                        {
                            return self
                                ._refuse_consignment_async(
                                    data,
                                    &recipient_id,
                                    recipient_seed,
                                    updated_batch_transfer,
                                )
                                .await;
                        }
                        self.received_mints.insert(recipient_id, mint.clone());
                    }
                    info!(
                        self.logger,
                        "{} mint lock(s) not found or not confirmed on the external chain yet, \
                         will retry",
                        unconfirmed.len()
                    );
                    return Ok(None);
                }
            }
        }
        let Some((valid_consignment, received)) = self
            ._check_received_consignment(
                &runtime,
                pending,
                &receiving,
                asset_schema,
                &asset_id,
                received_mint.as_ref().map(|_| transfer),
                !committed_mint,
            )
            .await?
        else {
            return self
                ._refuse_consignment_async(
                    data,
                    &recipient_id,
                    recipient_seed,
                    updated_batch_transfer,
                )
                .await;
        };
        let is_mint = received_mint.is_some();
        if let Some(mint) = received_mint {
            self.received_mints.insert(recipient_id.clone(), mint);
        }

        // add asset info to transfer if missing
        if asset_transfer.asset_id.is_none() {
            let exists_check = self.database.check_asset_exists(asset_id.clone());
            if exists_check.is_err() {
                // unknown asset
                debug!(self.logger, "Receiving unknown contract...");
                let valid_contract = valid_consignment.clone().into_valid_contract();

                // a checked mint has its media verified by the check
                if !checked_mint
                    && !self
                        ._contract_media_verified(
                            &runtime,
                            &valid_contract,
                            asset_schema,
                            data,
                            recipient_seed,
                            &seed,
                            &sender_vk,
                        )
                        .await?
                {
                    return self
                        ._refuse_consignment_async(
                            data,
                            &recipient_id,
                            recipient_seed,
                            updated_batch_transfer,
                        )
                        .await;
                }

                runtime
                    .import_contract(valid_contract.clone(), &chain_resolver)
                    .expect("failure importing received contract");
                debug!(self.logger, "Contract registered");
                self.save_new_asset_internal(
                    &runtime,
                    contract_id,
                    asset_schema,
                    valid_contract,
                    valid_consignment,
                )?;
            }

            let mut updated_asset_transfer: DbAssetTransferActMod = asset_transfer.clone().into();
            updated_asset_transfer.asset_id = ActiveValue::Set(Some(asset_id.clone()));
            self.database
                .update_asset_transfer(&mut updated_asset_transfer)?;
        }

        debug!(
            self.logger,
            "Consignment is valid. Received '{:?}' of contract '{}'", received, asset_id
        );

        // the sender proxy retention grace is how long after the operation expiration a released
        // signed TX can still be retrieved: a non-donation transfer whose sender proxy grants
        // less than the acceptable minimum is NACKed
        let mut retention_grace = None;
        if donation_tx.is_none() && !is_mint {
            let grace = sender_proxy_retention_grace(&data.sender_proxy).await?;
            if grace < MIN_SENDER_RETENTION_GRACE_SECS {
                warn!(
                    self.logger,
                    "Sender proxy retention grace ({grace}s) is below the acceptable minimum"
                );
                return self
                    ._refuse_consignment_async(
                        data,
                        &recipient_id,
                        recipient_seed,
                        updated_batch_transfer,
                    )
                    .await;
            }
            retention_grace = Some(grace);
        }

        updated_batch_transfer.txid = ActiveValue::Set(Some(txid.clone()));

        let utxo_idx = match transfer.recipient_type {
            Some(RecipientTypeFull::Blind { ref unblinded_utxo }) => {
                self.database
                    .get_txo(unblinded_utxo)?
                    .expect("utxo must exist")
                    .idx
            }
            Some(RecipientTypeFull::Witness { .. }) => {
                let mut updated_transfer: DbTransferActMod = transfer.clone().into();
                updated_transfer.recipient_type =
                    ActiveValue::Set(Some(RecipientTypeFull::Witness { vout }));
                self.database.update_transfer(&mut updated_transfer)?;
                let outpoint = Outpoint {
                    txid: txid.clone(),
                    vout: vout.unwrap(),
                };
                match self.database.get_txo(&outpoint)? {
                    Some(txo) => txo.idx,
                    None => {
                        let db_utxo = DbTxoActMod {
                            txid: ActiveValue::Set(txid.clone()),
                            vout: ActiveValue::Set(vout.unwrap()),
                            btc_amount: ActiveValue::Set(s!("0")),
                            spent: ActiveValue::Set(false),
                            exists: ActiveValue::Set(false),
                            pending_witness: ActiveValue::Set(true),
                            ..Default::default()
                        };
                        self.database.set_txo(db_utxo)?
                    }
                }
            }
            _ => return Err(InternalError::Unexpected.into()),
        };
        for assignment in received.into_values() {
            let db_coloring = DbColoringActMod {
                txo_idx: ActiveValue::Set(utxo_idx),
                asset_transfer_idx: ActiveValue::Set(asset_transfer.idx),
                r#type: ActiveValue::Set(ColoringType::Receive),
                assignment: ActiveValue::Set(assignment),
                ..Default::default()
            };
            self.database.set_coloring(db_coloring)?;
        }

        // release the stock before awaiting the proxy
        drop(runtime);

        // a mint has no operation on the sender proxy and its lock is final: wait for its TX
        if is_mint {
            let mut updated_transfer: DbTransferActMod = transfer.clone().into();
            updated_transfer.ack = ActiveValue::Set(Some(true));
            self.database.update_transfer(&mut updated_transfer)?;
            updated_batch_transfer.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
            return Ok(Some(
                self.database
                    .update_batch_transfer(updated_batch_transfer)?,
            ));
        }

        self.ack_consignment(
            batch_transfer,
            transfer,
            updated_batch_transfer,
            data,
            recipient_seed,
            retention_grace,
            donation_tx,
        )
        .await
    }

    async fn _broadcast_and_update_rgb_async(
        &mut self,
        runtime: &mut RgbRuntime,
        witness_id: RgbTxid,
        signed_psbt: Psbt,
        skip_sync: bool,
    ) -> Result<BdkTransaction, Error> {
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;
        runtime.upsert_witness(witness_id, WitnessOrd::Tentative)?;
        Ok(tx)
    }

    async fn _wait_ack_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting ACK...");

        let mut batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;

        // collect the (shared) sender proxy endpoint and the IDs of the transfers registered on it
        let mut transfer_ids: Vec<String> = vec![];
        let mut sender_proxy: Option<String> = None;
        for asset_transfer_data in &batch_transfer_data.asset_transfers_data {
            for transfer in &asset_transfer_data.transfers {
                let transfer_sender_proxy = self
                    .database
                    .get_transfer_transport_endpoints_data(transfer.idx)?
                    .into_iter()
                    .find(|(tte, _)| tte.kind == TransportEndpointKind::SenderProxy)
                    .map(|(_, ce)| ce.endpoint);
                let Some(transfer_sender_proxy) = transfer_sender_proxy else {
                    continue;
                };
                // a SenderProxy endpoint is only ever saved for a proxy recipient transfer, which
                // always has a transfer ID: a missing one is corrupted data
                let tid = transfer
                    .proxy_transfer_id
                    .clone()
                    .ok_or_else(|| Error::Internal {
                        details: format!(
                            "transfer {} has a sender proxy endpoint but no transfer ID",
                            transfer.idx
                        ),
                    })?;
                transfer_ids.push(tid);
                if sender_proxy.is_none() {
                    sender_proxy = Some(transfer_sender_proxy);
                }
            }
        }
        let Some(sender_proxy) = sender_proxy else {
            // no sender proxy registered (e.g. donation): nothing to wait for here
            return Ok(None);
        };

        // ack.get is authenticated: sign the request with the operation (sender) key
        let seed: [u8; 32] = batch_transfer
            .operation_secret_key
            .clone()
            .unwrap_or_default()
            .try_into()
            .map_err(|_| Error::Internal {
                details: s!("invalid operation secret key"),
            })?;
        let sk = ed25519_from_seed(&seed);
        let proxy_client = ProxyClient::new(&sender_proxy)?;
        let ack_res = get_ack_signed(&proxy_client, &transfer_ids, &sk).await?;
        debug!(self.logger, "ACK get response: {:?}", ack_res);
        if let Some(err) = ack_res.error {
            // the sender proxy no longer knows the operation: the ACKs cannot be retrieved anymore
            // and whether the TX was released cannot be known, so settle via the chain if a
            // recipient broadcast it, otherwise keep waiting until the broadcast deadline has
            // passed on the proxy's clock too
            check_get_ack_error(err)?;
            return self
                .settle_untrusted_batch(
                    batch_transfer,
                    "the sender proxy no longer knows the operation",
                    Error::OperationNotFound,
                    skip_sync,
                )
                .await
                .map(Some);
        }
        let ack_data = ack_res.result.ok_or_else(|| Error::Proxy {
            details: s!("missing ack.get result"),
        })?;

        for asset_transfer_data in batch_transfer_data.asset_transfers_data.iter_mut() {
            for transfer in asset_transfer_data.transfers.iter_mut() {
                if transfer.ack.is_some() {
                    continue;
                }
                let tte_data = self
                    .database
                    .get_transfer_transport_endpoints_data(transfer.idx)?;
                // a transfer with no transport endpoints at all has nothing to poll
                if tte_data.is_empty() {
                    continue;
                }
                let transfer_id =
                    transfer
                        .proxy_transfer_id
                        .clone()
                        .ok_or_else(|| Error::Internal {
                            details: format!(
                                "transfer {} has transport endpoints but no transfer ID",
                                transfer.idx
                            ),
                        })?;
                debug!(self.logger, "Transfer ID: {transfer_id}");
                // a transfer absent from the response has not been responded to yet
                let ack = ack_data.acks.get(&transfer_id).copied();
                let sig = ack_data.signatures.get(&transfer_id).cloned();
                let (Some(ack), Some(sig_hex)) = (ack, sig) else {
                    continue;
                };

                // verify the recipient's ACK signature
                let pubkey = transfer
                    .recipient_pubkey
                    .as_ref()
                    .ok_or_else(|| Error::Internal {
                        details: s!("missing recipient pubkey"),
                    })?;
                let vk = verifying_key_from_bytes(pubkey)?;
                let consignment_hash =
                    transfer
                        .consignment_hash
                        .clone()
                        .ok_or_else(|| Error::Internal {
                            details: s!("missing consignment hash"),
                        })?;
                let digest = Self::ack_digest(&transfer_id, ack, &consignment_hash);
                let sig_bytes =
                    hex::decode(&sig_hex).map_err(|_| Error::InvalidContentSignature)?;
                if !verify_ed25519(&vk, &digest, &sig_bytes) {
                    // the proxy served an ACK the recipient never signed, so it is corrupt or
                    // forging ACKs and nothing it says can be trusted. The batch cannot be failed
                    // right away either, as the proxy may have released the TX, so it is settled
                    // like one the proxy lost, surfacing the error while waiting
                    return self
                        .settle_untrusted_batch(
                            batch_transfer,
                            "the sender proxy serves an invalid ACK signature",
                            Error::InvalidContentSignature,
                            skip_sync,
                        )
                        .await
                        .map(Some);
                }

                let mut updated_transfer: DbTransferActMod = transfer.clone().into();
                updated_transfer.ack = ActiveValue::Set(Some(ack));
                updated_transfer.ack_signature = ActiveValue::Set(Some(sig_bytes.clone()));
                self.database.update_transfer(&mut updated_transfer)?;
                transfer.ack = Some(ack);
                transfer.ack_signature = Some(sig_bytes);
            }
        }

        let mut batch_transfer_transfers: Vec<DbTransfer> = vec![];
        batch_transfer_data
            .asset_transfers_data
            .iter()
            .for_each(|atd| batch_transfer_transfers.extend(atd.transfers.clone()));
        self.try_complete_batch(batch_transfer, &batch_transfer_transfers, skip_sync)
            .await
    }

    async fn try_complete_batch(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        transfers: &[DbTransfer],
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if transfers.iter().any(|t| t.ack == Some(false)) {
            return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
        }
        if !transfers.iter().all(|t| t.ack == Some(true)) {
            return Ok(None);
        }

        // every recipient has ACKed: broadcast the transaction if transfer hasn't expired

        // don't broadcast a transfer that has expired: past its expiration the recipient is
        // allowed to fail it. When every ACK came (signed) from the sender proxy, the proxy
        // released the TX to the recipients, so the transfer cannot simply be failed anymore: the
        // TX is broadcast while no invoice has expired, while past that a recipient that didn't
        // obtain the TX may have failed the transfer, so the transfer only progresses if the TX is
        // found on-chain and can otherwise only be failed after spending one of its inputs
        let released = transfers.iter().all(|t| t.ack_signature.is_some());
        let now = now().unix_timestamp();
        if !released && batch_transfer.is_expired(now) {
            debug!(
                self.logger,
                "Transfer expired before broadcast, failing it instead of broadcasting"
            );
            return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
        }
        if released && Self::any_invoice_expired(transfers, now) {
            if let Some(updated) = self
                .complete_batch_if_tx_on_chain(batch_transfer, skip_sync)
                .await?
            {
                return Ok(Some(updated));
            }
            // the TX can never confirm once one of its inputs has been spent by another confirmed
            // transaction, which is the only way such an operation can fail
            if self.tx_inputs_spent_elsewhere(batch_transfer) {
                info!(
                    self.logger,
                    "An input of the TX has been spent by another confirmed transaction, failing the transfer"
                );
                return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
            }
            warn!(
                self.logger,
                "Every recipient ACKed but an invoice has expired: not broadcasting the TX"
            );
            return Ok(None);
        }
        Ok(Some(self.broadcast_batch(batch_transfer, skip_sync).await?))
    }

    /// Whether the invoice expiration of any recipient has passed, accounting for the clock skew
    /// with the recipient.
    fn any_invoice_expired(transfers: &[DbTransfer], now: i64) -> bool {
        transfers
            .iter()
            .filter_map(|t| t.invoice_expiration)
            .any(|exp| now + PROXY_CLOCK_SKEW_MARGIN_SECS as i64 >= exp)
    }

    /// Whether one of the inputs of the TX of an outgoing transfer has been spent by another
    /// confirmed transaction, so that the TX can never confirm.
    fn tx_inputs_spent_elsewhere(&self, batch_transfer: &DbBatchTransfer) -> bool {
        let Some(txid) = &batch_transfer.txid else {
            return false;
        };
        let Some(signed_psbt) = self
            .transfer_artifacts
            .get(txid)
            .and_then(|a| a.signed_psbt.as_ref())
        else {
            return false;
        };
        let Ok(psbt) = Psbt::from_str(signed_psbt) else {
            return false;
        };
        let inputs: HashSet<BdkOutPoint> = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect();
        self.bdk_wallet.transactions().any(|wallet_tx| {
            wallet_tx.chain_position.is_confirmed()
                && wallet_tx.tx_node.txid.to_string() != *txid
                && wallet_tx
                    .tx_node
                    .tx
                    .input
                    .iter()
                    .any(|i| inputs.contains(&i.previous_output))
        })
    }

    /// Broadcast the TX of an outgoing batch transfer (a no-op on the network if a recipient
    /// already did), update the RGB state and move the transfer to `WaitingConfirmations`.
    async fn broadcast_batch(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        skip_sync: bool,
    ) -> Result<DbBatchTransfer, Error> {
        let txid = batch_transfer
            .txid
            .as_ref()
            .expect("batch transfer should have a TXID");
        // read signed PSBT from in-memory transfer artifacts
        let Some(signed_psbt) = self
            .transfer_artifacts
            .get(txid)
            .and_then(|a| a.signed_psbt.as_ref())
        else {
            let txid = txid.clone();
            self._fail_batch_transfer(batch_transfer)?;
            return Err(Error::MissingTransferArtifacts { txid });
        };
        let signed_psbt = Psbt::from_str(signed_psbt)?;
        let mut runtime = self.rgb_runtime()?;
        let witness_id = RgbTxid::from_str(txid).unwrap();
        self._broadcast_and_update_rgb_async(&mut runtime, witness_id, signed_psbt, skip_sync)
            .await?;
        // Clean up: signed PSBT no longer needed after broadcast
        self.transfer_artifacts.remove(txid);
        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
        Ok(self
            .database
            .update_batch_transfer(&mut updated_batch_transfer)?)
    }

    /// Settle an outgoing batch transfer whose sender proxy can no longer be relied upon to tell
    /// whether the signed TX was released (it lost the operation, or serves an ACK with an invalid
    /// signature): complete it if its TX is on-chain, otherwise fail it once its broadcast
    /// deadline (expiration plus the sender proxy retention grace) has passed on the proxy's clock
    /// too, accepting the residual risk of a TX broadcast later anyway.
    ///
    /// While the transfer has to keep waiting, `waiting_error` is returned, so that the refresh
    /// reports why the transfer cannot progress and a fail attempt is denied.
    async fn settle_untrusted_batch(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        reason: &str,
        waiting_error: Error,
        skip_sync: bool,
    ) -> Result<DbBatchTransfer, Error> {
        if let Some(updated) = self
            .complete_batch_if_tx_on_chain(batch_transfer, skip_sync)
            .await?
        {
            return Ok(updated);
        }
        if !broadcast_deadline_passed_beyond_skew_margin(batch_transfer) {
            return Err(waiting_error);
        }
        warn!(
            self.logger,
            "Failing a transfer past its broadcast deadline as {reason}: its TX can still be broadcast if it was released"
        );
        self._fail_batch_transfer(batch_transfer)
    }

    /// Complete an outgoing batch transfer waiting for its counterparty if its TX is on-chain
    /// (mempool included), which happens when a recipient obtained the TX from the sender proxy
    /// and broadcast it without the sender learning about the ACKs.
    async fn complete_batch_if_tx_on_chain(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        let Some(txid) = &batch_transfer.txid else {
            return Ok(None);
        };
        if self.indexer().get_tx_confirmations(txid).await?.is_none() {
            return Ok(None);
        }
        info!(
            self.logger,
            "TX of a transfer waiting for the counterparty found on-chain"
        );
        Ok(Some(self.broadcast_batch(batch_transfer, skip_sync).await?))
    }

    async fn _wait_confirmations_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
        incoming: bool,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting confirmations...");
        let txid = batch_transfer
            .txid
            .clone()
            .expect("batch transfer should have a TXID");
        debug!(
            self.logger,
            "Getting details of transaction with ID '{}'...", txid
        );
        let confirmations = self.indexer().get_tx_confirmations(&txid).await?;
        debug!(self.logger, "Confirmations: {:?}", confirmations);

        // an incoming transfer with a broadcast deadline (released via the sender proxy) whose TX
        // is not confirmed is failed once past the deadline plus the confirmation margin, while
        // until the deadline its TX is broadcast again if missing
        if incoming
            && batch_transfer.broadcast_deadline.is_some()
            && confirmations.unwrap_or(0) == 0
        {
            if confirmation_deadline_passed(batch_transfer) {
                info!(
                    self.logger,
                    "TX not confirmed past the broadcast deadline, failing the transfer"
                );
                return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
            }
            if confirmations.is_none()
                && batch_transfer
                    .broadcast_deadline
                    .is_some_and(|deadline| now().unix_timestamp() <= deadline)
            {
                let batch_transfer_data =
                    batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
                let (_, transfer) = self.database.get_incoming_transfer(&batch_transfer_data)?;
                if let Some(recipient_id) = &transfer.recipient_id {
                    self.rebroadcast_received_tx(recipient_id).await;
                }
            }
        }

        let Some(confirmations) = confirmations else {
            return Ok(None);
        };

        if confirmations < batch_transfer.min_confirmations as u64 {
            if batch_transfer.status == TransferStatus::WaitingBroadcast {
                let mut updated_batch_transfer: DbBatchTransferActMod =
                    batch_transfer.clone().into();
                updated_batch_transfer.status =
                    ActiveValue::Set(TransferStatus::WaitingConfirmations);
                return Ok(Some(
                    self.database
                        .update_batch_transfer(&mut updated_batch_transfer)?,
                ));
            }
            return Ok(None);
        }

        if incoming {
            let batch_transfer_data =
                batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
            let (asset_transfer, transfer) =
                self.database.get_incoming_transfer(&batch_transfer_data)?;
            let recipient_id = transfer
                .clone()
                .recipient_id
                .expect("transfer should have a recipient ID");
            debug!(self.logger, "Recipient ID: {recipient_id}");

            // load consignment from in-memory storage
            let consignment_bytes = self
                .received_consignments
                .get(&recipient_id)
                .ok_or_else(|| Error::Internal {
                    details: s!("consignment not found in memory"),
                })?
                .clone();
            let consignment =
                RgbTransfer::load(&consignment_bytes[..]).map_err(InternalError::from)?;

            if let Some(RecipientTypeFull::Witness { vout }) = transfer.recipient_type {
                if !skip_sync {
                    self.sync_db_txos(false).await?;
                }
                let outpoint = Outpoint {
                    txid: txid.clone(),
                    vout: vout.unwrap(),
                };
                let txo = self.database.get_txo(&outpoint)?.expect("txo must exist");
                let mut txo: DbTxoActMod = txo.into();
                txo.pending_witness = ActiveValue::Set(false);
                self.database.update_txo(txo)?;
            }

            let wasm_resolver = self
                .prefetch_witnesses(consignment.bundled_witnesses().map(|bw| bw.witness_id()))
                .await?;
            let asset_schema: AssetSchema = consignment.schema_id().try_into()?;
            let validation_config = ValidationConfig {
                chain_net: self.chain_net(),
                ..Default::default()
            };
            let mut pending = consignment
                .validate_deterministic(&asset_schema.schema_rules(), &validation_config)
                .and_then(|mut p| p.pending().resolve_all(&wasm_resolver).map(|_| p))
                .map_err(|_| InternalError::Unexpected)?;
            match self._resolve_claims(&mut pending).await? {
                ClaimsResolution::NoClaims => {}
                ClaimsResolution::Resolved { unconfirmed, .. } if unconfirmed.is_empty() => {}
                _ => {
                    warn!(self.logger, "Mint locks not confirmed yet, will retry");
                    return Ok(None);
                }
            }
            let valid_consignment = pending.finalize();
            let validation_status = valid_consignment.validation_status();
            let mut runtime = self.rgb_runtime()?;
            runtime.accept_transfer(valid_consignment.clone(), &wasm_resolver)?;
            if asset_schema == AssetSchema::Ifa {
                let contract_id = valid_consignment.contract_id();
                let contract_wrapper =
                    runtime.contract_wrapper::<InflatableFungibleAsset>(contract_id)?;
                let known_circulating_supply = contract_wrapper.total_issued_supply().into();
                let asset_id = asset_transfer.asset_id.unwrap();
                let db_asset = self.database.get_asset(asset_id).unwrap().unwrap();
                let db_known_circulating_supply = db_asset
                    .known_circulating_supply
                    .as_ref()
                    .unwrap()
                    .parse::<u64>()
                    .unwrap();
                if db_known_circulating_supply < known_circulating_supply {
                    let mut updated_asset: DbAssetActMod = db_asset.into();
                    updated_asset.known_circulating_supply =
                        ActiveValue::Set(Some(known_circulating_supply.to_string()));
                    self.database.update_asset(&mut updated_asset)?;
                }
            }

            match validation_status.validity() {
                Validity::Valid => {}
                Validity::Warnings => {
                    if let Warning::UnsafeHistory(ref unsafe_history) =
                        validation_status.warnings[0]
                    {
                        warn!(
                            self.logger,
                            "Cannot accept transfer because of unsafe history: {unsafe_history:?}"
                        );
                        return Ok(None);
                    }
                }
            }
        }

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        updated_batch_transfer.status = ActiveValue::Set(TransferStatus::Settled);
        let updated = self
            .database
            .update_batch_transfer(&mut updated_batch_transfer)?;

        // Clean up: received data no longer needed after settle
        if incoming {
            let batch_transfer_data =
                batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
            if let Ok((_, transfer)) = self.database.get_incoming_transfer(&batch_transfer_data)
                && let Some(recipient_id) = &transfer.recipient_id
            {
                self.clear_received_data(recipient_id);
            }
        }

        Ok(Some(updated))
    }

    async fn _wait_counterparty_async(
        &mut self,
        transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        incoming: bool,
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if incoming {
            self._wait_consignment_async(transfer, db_data).await
        } else {
            self._wait_ack_async(transfer, db_data, skip_sync).await
        }
    }

    /// A proxy recipient that has ACKed polls the sender proxy until the signed TX is released,
    /// then decrypts and broadcasts it.
    async fn _wait_broadcast_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Waiting broadcast...");

        let batch_transfer_data =
            batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let (_, transfer) = self.database.get_incoming_transfer(&batch_transfer_data)?;

        // sender proxy endpoint (where the signed TX is released); without one there is nothing
        // to ask for the TX, the sender broadcasts it and wait_confirmations detects it on-chain
        let mut sender_proxy = None;
        for (tte, ce) in self
            .database
            .get_transfer_transport_endpoints_data(transfer.idx)?
        {
            if tte.kind == TransportEndpointKind::SenderProxy {
                sender_proxy = Some(ce.endpoint);
                break;
            }
        }
        let Some(sender_proxy) = sender_proxy else {
            debug!(
                self.logger,
                "No sender proxy, waiting for the sender to broadcast"
            );
            return Ok(None);
        };

        let consignment_hash =
            transfer
                .consignment_hash
                .clone()
                .ok_or_else(|| Error::Internal {
                    details: s!("missing consignment hash"),
                })?;
        let recipient_seed = batch_transfer
            .operation_secret_key
            .clone()
            .unwrap_or_default();
        let transfer_id = transfer
            .proxy_transfer_id
            .clone()
            .ok_or_else(|| Error::Internal {
                details: s!("missing transfer ID"),
            })?;
        let res = match self
            .sign_and_post_ack(
                &sender_proxy,
                &transfer_id,
                true,
                &consignment_hash,
                &recipient_seed,
            )
            .await
        {
            Ok(r) => r,
            // the operation expired without releasing the TX, which never will be: fail the
            // transfer
            Err(Error::OperationExpired) => {
                return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
            }
            // the sender proxy no longer knows the operation: a TX released before can still be
            // broadcast (and detected on-chain) until the broadcast deadline
            Err(Error::OperationNotFound) => return self.fail_if_past_deadline(batch_transfer),
            // no definite answer: fail the transfer once past the broadcast deadline
            Err(e) => {
                if let Some(updated) = self.fail_if_past_deadline(batch_transfer)? {
                    return Ok(Some(updated));
                }
                return Err(e);
            }
        };

        let mut updated_batch_transfer: DbBatchTransferActMod = batch_transfer.clone().into();
        // a transfer ACKed via a proxy not returning the operation expiration yet has no
        // broadcast deadline, set it as soon as it's known
        let mut deadline_changed = false;
        if batch_transfer.broadcast_deadline.is_none()
            && let Some(deadline) = broadcast_deadline(
                res.expires_at.as_deref(),
                sender_proxy_retention_grace(&sender_proxy).await?,
            )
        {
            updated_batch_transfer.broadcast_deadline = ActiveValue::Set(Some(deadline));
            deadline_changed = true;
        }

        match (res.signed_tx, res.signed_tx_hash, res.sender_pk) {
            (Some(signed_tx), Some(signed_tx_hash), Some(sender_pubkey)) => {
                let recipient_id = transfer
                    .recipient_id
                    .clone()
                    .expect("transfer should have a recipient ID");
                self.broadcast_received_tx(
                    &recipient_id,
                    &signed_tx,
                    &signed_tx_hash,
                    &sender_pubkey,
                    &recipient_seed,
                )
                .await?;
                updated_batch_transfer.status =
                    ActiveValue::Set(TransferStatus::WaitingConfirmations);
                Ok(Some(
                    self.database
                        .update_batch_transfer(&mut updated_batch_transfer)?,
                ))
            }
            // still waiting on the other recipients (or a non-compliant proxy)
            _ => {
                if deadline_changed {
                    // the status is unchanged, the deadline is checked on the next refresh
                    self.database
                        .update_batch_transfer(&mut updated_batch_transfer)?;
                    return Ok(None);
                }
                self.fail_if_past_deadline(batch_transfer)
            }
        }
    }

    /// The TX is not on-chain (the confirmations are checked before this on every refresh): past
    /// the broadcast deadline plus the confirmation margin the transfer is failed.
    fn fail_if_past_deadline(
        &mut self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        if confirmation_deadline_passed(batch_transfer) {
            info!(
                self.logger,
                "TX not confirmed past the broadcast deadline, failing the transfer"
            );
            return Ok(Some(self._fail_batch_transfer(batch_transfer)?));
        }
        Ok(None)
    }

    async fn _refresh_transfer_async(
        &mut self,
        transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        filter: &[RefreshFilter],
        skip_sync: bool,
    ) -> Result<Option<DbBatchTransfer>, Error> {
        debug!(self.logger, "Refreshing transfer: {:?}", transfer);
        let incoming = transfer.incoming(&db_data.asset_transfers, &db_data.transfers)?;
        if !filter.is_empty() {
            let requested = RefreshFilter {
                status: RefreshTransferStatus::try_from(transfer.status).expect("pending status"),
                incoming,
            };
            if !filter.contains(&requested) {
                return Ok(None);
            }
        }
        match transfer.status {
            TransferStatus::WaitingCounterparty => {
                self._wait_counterparty_async(transfer, db_data, incoming, skip_sync)
                    .await
            }
            // a proxy recipient that ACKed polls the sender proxy for the TX (or expiration), if
            // the TX is not on-chain already (broadcast by the sender when the batch completed,
            // or by an earlier poll)
            TransferStatus::WaitingBroadcast => {
                match self
                    ._wait_confirmations_async(transfer, db_data, incoming, skip_sync)
                    .await?
                {
                    Some(updated) => Ok(Some(updated)),
                    None => self._wait_broadcast_async(transfer, db_data).await,
                }
            }
            TransferStatus::WaitingConfirmations => {
                self._wait_confirmations_async(transfer, db_data, incoming, skip_sync)
                    .await
            }
            _ => Ok(None),
        }
    }

    /// Update pending RGB transfers (wasm32 async).
    ///
    /// See native [`refresh`](Wallet::refresh) for full documentation.
    pub async fn refresh(
        &mut self,
        online: Online,
        asset_id: Option<String>,
        filter: Vec<RefreshFilter>,
        skip_sync: bool,
    ) -> Result<RefreshResult, Error> {
        if let Some(aid) = asset_id.clone() {
            info!(self.logger, "Refreshing asset {}...", aid);
            self.database.check_asset_exists(aid)?;
        } else {
            info!(self.logger, "Refreshing assets...");
        }
        self.check_online(online)?;

        let mut db_data = self.database.get_db_data(false)?;

        if asset_id.is_some() {
            let batch_transfers_ids: Vec<i32> = db_data
                .asset_transfers
                .iter()
                .filter(|t| t.asset_id == asset_id)
                .map(|t| t.batch_transfer_idx)
                .collect();
            db_data
                .batch_transfers
                .retain(|t| batch_transfers_ids.contains(&t.idx));
        };
        db_data.batch_transfers.retain(|t| t.waiting());

        let mut refresh_result = HashMap::new();
        for transfer in db_data.batch_transfers.clone().into_iter() {
            let mut failure = None;
            let mut updated_status = None;
            let savepoint = self._transfer_savepoint();
            match self
                ._refresh_transfer_async(&transfer, &mut db_data, &filter, skip_sync)
                .await
            {
                Ok(Some(updated_transfer)) => updated_status = Some(updated_transfer.status),
                Err(e) => {
                    // this error means the refresh failed the batch on purpose
                    if matches!(e, Error::MissingTransferArtifacts { .. }) {
                        updated_status = Some(TransferStatus::Failed);
                    } else {
                        self._rollback_transfer(savepoint);
                    }
                    failure = Some(e);
                }
                _ => {}
            }
            refresh_result.insert(
                transfer.idx,
                RefreshedTransfer {
                    updated_status,
                    failure,
                },
            );
        }

        info!(self.logger, "Refresh completed");
        Ok(refresh_result)
    }

    /// Revoke the stored signed TXs for an outgoing operation on the sender proxy (`ack.clean`).
    /// An operation with nothing registered on a sender proxy has nothing to revoke and is
    /// reported as [`CleanAckOutcome::Revoked`].
    async fn clean_sender_ack(
        &self,
        batch_transfer: &DbBatchTransfer,
        db_data: &DbData,
    ) -> Result<CleanAckOutcome, Error> {
        let Some(seed) = &batch_transfer.operation_secret_key else {
            return Ok(CleanAckOutcome::Revoked);
        };
        let bt_data = batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?;
        let mut transfer_ids: Vec<String> = vec![];
        let mut sender_proxy = None;
        for atd in &bt_data.asset_transfers_data {
            for t in &atd.transfers {
                let transfer_sender_proxy = self
                    .database
                    .get_transfer_transport_endpoints_data(t.idx)?
                    .into_iter()
                    .find(|(tte, _)| tte.kind == TransportEndpointKind::SenderProxy)
                    .map(|(_, ce)| ce.endpoint);
                let Some(transfer_sender_proxy) = transfer_sender_proxy else {
                    continue;
                };
                if let Some(tid) = &t.proxy_transfer_id {
                    transfer_ids.push(tid.clone());
                }
                if sender_proxy.is_none() {
                    sender_proxy = Some(transfer_sender_proxy);
                }
            }
        }
        let Some(sender_proxy) = sender_proxy else {
            // no sender proxy: nothing to clean
            return Ok(CleanAckOutcome::Revoked);
        };
        transfer_ids.sort();
        let seed: [u8; 32] = seed.clone().try_into().map_err(|_| Error::Internal {
            details: s!("invalid operation secret key"),
        })?;
        let sk = ed25519_from_seed(&seed);
        let digest = sha256_digest(transfer_ids.concat().as_bytes());
        let sig = hex::encode(sign_ed25519(&sk, &digest));
        let proxy_client = ProxyClient::new(&sender_proxy)?;
        let res = proxy_client.clean_ack(&transfer_ids, &sig).await?;
        clean_ack_outcome(res.error)
    }

    async fn _try_fail_batch_transfer_async(
        &mut self,
        batch_transfer: &DbBatchTransfer,
        db_data: &mut DbData,
        revoke_released: bool,
    ) -> Result<TryFailBatchTransferOutcome, Error> {
        let incoming = batch_transfer.incoming(&db_data.asset_transfers, &db_data.transfers)?;
        // an outgoing transfer is expired once its expiration has passed on the sender proxy's
        // clock too, as the proxy may release its TX until then
        let expired = if incoming {
            incoming_expired(batch_transfer, now().unix_timestamp())
        } else {
            expired_beyond_skew_margin(batch_transfer)
        };
        let savepoint = self._transfer_savepoint();
        let refreshed = self
            ._refresh_transfer_async(batch_transfer, db_data, &[], true)
            .await;
        let updated_batch_transfer = match refreshed {
            Err(Error::MinFeeNotMet { txid: _ }) | Err(Error::MaxFeeExceeded { txid: _ }) => {
                // an outgoing transfer keeps the ACKs the refresh stored
                if incoming {
                    self._rollback_transfer(savepoint);
                }
                Ok(None)
            }
            // the sender proxy lost the operation or serves an invalid ACK signature: the
            // refresh fails the transfer itself once its broadcast deadline has passed, until
            // then the signed TX may still be broadcast by a recipient, so the fail is denied
            Err(Error::OperationNotFound) | Err(Error::InvalidContentSignature) if !incoming => {
                info!(
                    self.logger,
                    "Sender proxy cannot be relied upon, fail denied until the broadcast deadline"
                );
                self._rollback_transfer(savepoint);
                return Ok(TryFailBatchTransferOutcome::Denied);
            }
            // a transfer that keeps failing to refresh would otherwise never be failed; once
            // expired its TX should no longer be broadcast by the sender, so it is safe to fail
            // it, while before that the error could be transient
            Err(e) if expired => {
                warn!(
                    self.logger,
                    "Failing expired transfer that failed to refresh: {e}"
                );
                // an outgoing transfer keeps the ACKs the refresh stored
                if incoming {
                    self._rollback_transfer(savepoint);
                }
                Ok(None)
            }
            // the refresh failed the batch on purpose
            Err(e @ Error::MissingTransferArtifacts { .. }) => Err(e),
            Err(e) => {
                self._rollback_transfer(savepoint);
                Err(e)
            }
            Ok(v) => Ok(v),
        }?;
        // if the status changed after the refresh, the transfer progressed
        if let Some(updated) = updated_batch_transfer {
            // the refresh may have failed the batch itself: that is exactly the outcome a
            // deliberate fail wanted
            if updated.status == TransferStatus::Failed {
                return Ok(TryFailBatchTransferOutcome::Failed);
            }
            return Ok(TryFailBatchTransferOutcome::Refreshed);
        }
        // the refresh above may have been the first to check a received mint: deny the fail, as
        // a mint expires only by its lock, which may already be made
        if incoming && self._is_mint(batch_transfer, db_data)? {
            return Ok(TryFailBatchTransferOutcome::Denied);
        }
        // the status didn't change: before failing an outgoing transfer make sure its TX is not
        // on-chain already (a recipient holding the TX may have broadcast it), in which case the
        // transfer progresses instead
        if !incoming
            && batch_transfer.status == TransferStatus::WaitingCounterparty
            && self
                .complete_batch_if_tx_on_chain(batch_transfer, true)
                .await?
                .is_some()
        {
            return Ok(TryFailBatchTransferOutcome::Refreshed);
        }
        // an outgoing transfer every recipient ACKed has had its TX released: if it's still
        // waiting for the counterparty it can only be failed once one of the TX inputs has been
        // spent by another confirmed transaction, which the refresh above detects
        if !incoming && batch_transfer.status == TransferStatus::WaitingCounterparty {
            // read from the DB, as the refresh above may have just stored the ACKs
            let batch_transfer_data = batch_transfer.get_transfers(
                &self.database.iter_asset_transfers()?,
                &self.database.iter_transfers()?,
            )?;
            let transfers: Vec<DbTransfer> = batch_transfer_data
                .asset_transfers_data
                .into_iter()
                .flat_map(|atd| atd.transfers)
                .collect();
            if !transfers.is_empty() && transfers.iter().all(|t| t.ack == Some(true)) {
                info!(
                    self.logger,
                    "Every recipient ACKed the transfer, fail denied"
                );
                return Ok(TryFailBatchTransferOutcome::Denied);
            }
        }
        // for a deliberate fail of an outgoing operation, revoke the released signed TXs on the
        // sender proxy first
        if revoke_released && !incoming {
            match self.clean_sender_ack(batch_transfer, db_data).await? {
                CleanAckOutcome::Revoked => {}
                // the TX was already released to a recipient, who may still broadcast it: deny
                CleanAckOutcome::Released => return Ok(TryFailBatchTransferOutcome::Denied),
                // the proxy no longer knows the operation, so whether the TX was released cannot
                // be known. Only an operation that completed send_end had a signed TX registered:
                // it is failed only once its broadcast deadline has passed on the proxy's clock
                // too, since until then the proxy may recover the operation and hand out the TX,
                // while recipients that obtained the TX have time to broadcast it, which the chain
                // check above detects
                CleanAckOutcome::Unknown
                    if batch_transfer.status == TransferStatus::WaitingCounterparty =>
                {
                    if !broadcast_deadline_passed_beyond_skew_margin(batch_transfer) {
                        info!(
                            self.logger,
                            "Operation unknown to the sender proxy, fail denied until its broadcast deadline"
                        );
                        return Ok(TryFailBatchTransferOutcome::Denied);
                    }
                    warn!(
                        self.logger,
                        "Failing an operation unknown to the sender proxy past its broadcast deadline: its TX can still be broadcast if it was released before the proxy lost the operation"
                    );
                }
                // an operation that didn't complete send_end (still Initiated) never had a signed
                // TX registered, so nothing can have been released
                CleanAckOutcome::Unknown => {}
            }
        }
        // a blind receive keeps its blinded UTXO reserved while a transfer to the invoice can
        // still be downloaded
        if incoming
            && batch_transfer.status == TransferStatus::WaitingCounterparty
            && !self
                .blind_receive_may_be_failed(batch_transfer, db_data)
                .await?
        {
            info!(
                self.logger,
                "A transfer to the blind invoice may still be downloaded, fail denied"
            );
            return Ok(TryFailBatchTransferOutcome::Denied);
        }
        self._fail_batch_transfer(batch_transfer)?;
        Ok(TryFailBatchTransferOutcome::Failed)
    }

    /// Fail pending RGB transfers (wasm32 async).
    ///
    /// See native [`fail_transfers`](Wallet::fail_transfers) for full documentation.
    ///
    /// A transfer in status [`TransferStatus::WaitingBroadcast`] can be failed only once past its
    /// broadcast deadline plus the confirmation margin, because its TX can still be broadcast
    /// before then.
    pub async fn fail_transfers(
        &mut self,
        online: Online,
        batch_transfer_idx: Option<i32>,
        no_asset_only: bool,
        skip_sync: bool,
    ) -> Result<bool, Error> {
        info!(
            self.logger,
            "Failing batch transfer with idx {:?}...", batch_transfer_idx
        );
        self.check_online(online)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let mut db_data = self.database.get_db_data(false)?;
        let mut transfers_changed = false;
        let now = now().unix_timestamp();

        if let Some(batch_transfer_idx) = batch_transfer_idx {
            let batch_transfer = &self
                .database
                .get_batch_transfer_or_fail(batch_transfer_idx, &db_data.batch_transfers)?;

            if !batch_transfer.is_fallible() {
                return Err(Error::CannotFailBatchTransfer);
            }

            // a mint expires by its lock, not by a timestamp: a sent one can be failed once its
            // lock has expired, the refresh fails a received one
            if self._is_mint(batch_transfer, &db_data)? {
                if !batch_transfer.incoming(&db_data.asset_transfers, &db_data.transfers)?
                    && !no_asset_only
                    && self._fail_expired_mint(batch_transfer).await?
                {
                    info!(self.logger, "Fail transfers completed");
                    return Ok(true);
                }
                return Err(Error::CannotFailBatchTransfer);
            }

            // a transfer waiting for the counterparty's broadcast can only be failed once it has
            // expired, since the TX may still be broadcast before then
            if batch_transfer.status == TransferStatus::WaitingBroadcast
                && !incoming_expired(batch_transfer, now)
            {
                return Err(Error::CannotFailBatchTransfer);
            }

            if no_asset_only {
                let asset_transfers =
                    batch_transfer.get_asset_transfers(&db_data.asset_transfers)?;
                let connected_assets = asset_transfers.iter().any(|t| t.asset_id.is_some());
                if connected_assets {
                    return Err(Error::CannotFailBatchTransfer);
                }
            }

            transfers_changed = true;
            // a deliberate fail (explicit batch idx) revokes any released signed TXs first
            if !matches!(
                self._try_fail_batch_transfer_async(batch_transfer, &mut db_data, true)
                    .await?,
                TryFailBatchTransferOutcome::Failed
            ) {
                return Err(Error::CannotFailBatchTransfer);
            }
        } else {
            // fail all expired transfers in a fallible status; an outgoing one is expired once
            // its expiration has passed on the sender proxy's clock too, as the proxy may release
            // its TX until then
            // a mint never expires by a timestamp: the refresh fails a received one, and a sent
            // one is failed below once its lock has expired
            let mut expired_batch_transfers: Vec<DbBatchTransfer> = vec![];
            let mut sent_mints = vec![];
            for t in db_data.batch_transfers.clone() {
                if !t.is_fallible() {
                    continue;
                }
                let incoming = t.incoming(&db_data.asset_transfers, &db_data.transfers)?;
                if self._is_mint(&t, &db_data)? {
                    if !incoming {
                        sent_mints.push(t);
                    }
                    continue;
                }
                let expired = if incoming {
                    incoming_expired(&t, now)
                } else {
                    expired_beyond_skew_margin(&t)
                };
                if expired {
                    expired_batch_transfers.push(t);
                }
            }
            for batch_transfer in expired_batch_transfers.iter_mut() {
                if no_asset_only {
                    let connected_assets = batch_transfer
                        .get_asset_transfers(&db_data.asset_transfers)?
                        .iter()
                        .any(|t| t.asset_id.is_some());
                    if connected_assets {
                        continue;
                    }
                }
                // expired transfers: the proxy operation has lapsed, no need to revoke released TXs
                if !matches!(
                    self._try_fail_batch_transfer_async(batch_transfer, &mut db_data, false)
                        .await?,
                    TryFailBatchTransferOutcome::Denied
                ) {
                    transfers_changed = true;
                }
            }
            // a mint is always of an asset. A mint whose lock cannot be checked now is left for a
            // later call
            if !no_asset_only {
                for batch_transfer in &sent_mints {
                    match self._fail_expired_mint(batch_transfer).await {
                        Ok(failed) => transfers_changed |= failed,
                        Err(e) => warn!(
                            self.logger,
                            "Cannot check the lock of mint {}: {e}", batch_transfer.idx
                        ),
                    }
                }
            }
        }

        info!(self.logger, "Fail transfers completed");
        Ok(transfers_changed)
    }

    /// Check a recipient and turn it into a local one.
    #[allow(clippy::too_many_arguments)]
    async fn _parse_recipient(
        &self,
        recipient: &Recipient,
        schema: AssetSchema,
        chainnet: ChainNet,
        operation_sk: &SigningKey,
        now_ts: i64,
        witness_recipients: &mut Vec<(ScriptBuf, u64)>,
        recipient_vout: &mut u32,
        mint: bool,
    ) -> Result<LocalRecipient, Error> {
        self.check_transport_endpoints(&recipient.transport_endpoints)?;
        match (&recipient.assignment, schema) {
            (Assignment::Fungible(amt), AssetSchema::Nia | AssetSchema::Ifa | AssetSchema::Bfa) => {
                if *amt == 0 {
                    return Err(Error::InvalidAmountZero);
                }
            }
            (Assignment::InflationRight(amt), AssetSchema::Ifa) => {
                if *amt == 0 {
                    return Err(Error::InvalidAmountZero);
                }
            }
            _ => {
                return Err(Error::InvalidAssignment);
            }
        }
        let mut transport_endpoints: Vec<LocalTransportEndpoint> = vec![];
        let mut found_valid = false;
        for endpoint_str in &recipient.transport_endpoints {
            let transport_endpoint = TransportEndpoint::new(endpoint_str.clone())?;
            let mut local_transport_endpoint = LocalTransportEndpoint {
                transport_type: transport_endpoint.transport_type,
                endpoint: transport_endpoint.endpoint.clone(),
                used: false,
                usable: false,
            };
            if crate::utils::check_proxy(&transport_endpoint.endpoint)
                .await
                .is_ok()
            {
                local_transport_endpoint.usable = true;
                found_valid = true;
            }
            transport_endpoints.push(local_transport_endpoint);
        }

        if !found_valid {
            return Err(Error::InvalidTransportEndpoints {
                details: s!("no valid transport endpoints"),
            });
        }

        let xchainnet_beneficiary = XChainNet::<Beneficiary>::from_str(&recipient.recipient_id)
            .map_err(|_| Error::InvalidRecipientID)?;

        if xchainnet_beneficiary.chain_network() != chainnet {
            return Err(Error::InvalidRecipientNetwork);
        }

        // decode and validate the recipient's Ed25519 public key, needed to encrypt data for and
        // verify the ACK from the recipient
        let public_key_hex =
            recipient
                .public_key
                .as_deref()
                .ok_or_else(|| Error::InvalidPubkey {
                    details: s!("missing recipient public key"),
                })?;
        let public_key = hex::decode(public_key_hex).map_err(|_| Error::InvalidPubkey {
            details: s!("recipient public key is not valid hex"),
        })?;
        let recipient_vk = verifying_key_from_bytes(&public_key)?;

        // a mint expires by the external chain block its lock has to be included before, never
        // by a timestamp, so its recipient must have no expiration
        let invoice_expiration = if mint {
            if recipient.expiration_timestamp.is_some() {
                return Err(Error::InvalidRecipientData {
                    details: s!("a mint recipient cannot have an expiration"),
                });
            }
            None
        } else {
            // a transfer cannot be created after the invoice expiration
            let invoice_expiration =
                recipient
                    .expiration_timestamp
                    .ok_or_else(|| Error::InvalidRecipientData {
                        details: s!("missing invoice expiration"),
                    })? as i64;
            if invoice_expiration <= now_ts {
                return Err(Error::InvoiceExpired);
            }
            Some(invoice_expiration)
        };

        let local_recipient_data = match xchainnet_beneficiary.into_inner() {
            Beneficiary::BlindedSeal(secret_seal) => {
                if recipient.witness_data.is_some() {
                    return Err(Error::InvalidRecipientData {
                        details: s!("cannot provide witness data for a blinded recipient"),
                    });
                }
                LocalRecipientData::Blind(secret_seal)
            }
            Beneficiary::WitnessVout(pay_2_vout, _) => {
                if let Some(ref witness_data) = recipient.witness_data {
                    let script_buf = pay_2_vout.to_script();
                    witness_recipients.push((script_buf.clone(), witness_data.amount_sat));
                    // the blinding of the recipient seal is derived from the keys of both
                    // parties, so the sender cannot choose it
                    if witness_data.blinding.is_some() {
                        return Err(Error::InvalidRecipientData {
                            details: s!(
                                "cannot provide a blinding for a witness recipient with transport endpoints"
                            ),
                        });
                    }
                    let blinding = Some(derive_shared(operation_sk, &recipient_vk)?.seal_blinding);
                    let local_witness_data = LocalWitnessData {
                        amount_sat: witness_data.amount_sat,
                        blinding,
                        vout: *recipient_vout,
                    };
                    *recipient_vout += 1;
                    LocalRecipientData::Witness(local_witness_data)
                } else {
                    return Err(Error::InvalidRecipientData {
                        details: s!("missing witness data for a witness recipient"),
                    });
                }
            }
        };

        Ok(LocalRecipient {
            recipient_id: recipient.recipient_id.clone(),
            local_recipient_data,
            assignment: recipient.assignment.clone(),
            transport_endpoints,
            public_key,
            encryption: recipient.encryption,
            invoice_expiration,
        })
    }

    /// Prepare the PSBT to send RGB assets (wasm32 async).
    ///
    /// See native [`send_begin`](Wallet::send_begin) for full documentation.
    ///
    /// An expiration UTC timestamp must be specified, which will set the expiration of the
    /// transfer. This should be set to the same value specified by the recipient's invoice. After it
    /// passes, the recipient can fail the transfer and the sender does not broadcast it, even when
    /// a late ACK arrives. In case of a batch transfer, set it to the minimum (earliest) expiration
    /// across the recipients' invoices.
    ///
    /// If `dry_run` is true, the wallet does not save the transfer in
    /// [`TransferStatus::Initiated`]. The returned `batch_transfer_idx` is then None.
    /// [`send_end`](Wallet::send_end) can still complete the operation and save the transfer.
    pub async fn send_begin(
        &mut self,
        online: Online,
        recipient_map: HashMap<String, Vec<Recipient>>,
        donation: bool,
        fee_rate: u64,
        min_confirmations: u8,
        expiration_timestamp: u64,
        sender_transport_endpoint: Option<String>,
        lock_time: Option<u32>,
        dry_run: bool,
    ) -> Result<SendBeginResult, Error> {
        info!(self.logger, "Sending (begin) to: {:?}...", recipient_map);

        if recipient_map.is_empty() || recipient_map.values().any(|v| v.is_empty()) {
            return Err(Error::InvalidRecipientMap);
        }
        // the recipients post their ACK to (and get the signed TX from) the sender proxy
        let Some(sender_transport_endpoint) = sender_transport_endpoint else {
            return Err(Error::InvalidTransportEndpoint {
                details: s!("missing sender transport endpoint"),
            });
        };
        self.convert_transport_endpoint_url(&sender_transport_endpoint)?;
        let operation_sk = crate::encryption::gen_ed25519();
        let operation_secret_key = operation_sk.to_bytes().to_vec();
        let now_ts = now().unix_timestamp();
        // earliest invoice expiration among the recipients, bounding the operation one
        let mut min_invoice_expiration: Option<i64> = None;

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        let chainnet: ChainNet = self.bitcoin_network().into();
        let mut witness_recipients: Vec<(ScriptBuf, u64)> = vec![];
        let mut recipient_vout = 1;
        let main_transition = TypeOfTransition::Transfer;
        let mut assets_data: BTreeMap<String, (AssetInfo, AssignmentsCollection)> = BTreeMap::new();
        let mut local_recipients: BTreeMap<String, Vec<LocalRecipient>> = BTreeMap::new();
        for (asset_id, recipients) in &recipient_map {
            let asset = self.database.check_asset_exists(asset_id.clone())?;
            let schema = asset.schema;
            self.check_schema_support(&schema)?;

            let mut original_assignments_needed = AssignmentsCollection::default();
            for recipient in recipients.clone() {
                let local_recipient = self
                    ._parse_recipient(
                        &recipient,
                        schema,
                        chainnet,
                        &operation_sk,
                        now_ts,
                        &mut witness_recipients,
                        &mut recipient_vout,
                        false,
                    )
                    .await?;
                let invoice_expiration = local_recipient
                    .invoice_expiration
                    .expect("set by _parse_recipient");
                min_invoice_expiration = Some(
                    min_invoice_expiration
                        .map_or(invoice_expiration, |m| std::cmp::min(m, invoice_expiration)),
                );
                local_recipients
                    .entry(asset_id.clone())
                    .or_default()
                    .push(local_recipient);

                recipient
                    .assignment
                    .add_to_assignments(&mut original_assignments_needed);
            }
            let contract_id = ContractId::from_str(asset_id).expect("invalid contract ID");
            assets_data.insert(
                asset_id.clone(),
                (
                    AssetInfo {
                        contract_id,
                        reject_list_url: asset.reject_list_url,
                    },
                    original_assignments_needed,
                ),
            );
        }

        // the operation must expire sufficiently before every invoice, so its TX can only be
        // released while the recipients are still waiting for it
        if !donation
            && let Some(min_invoice_expiration) = min_invoice_expiration
            && expiration_timestamp as i64
                > min_invoice_expiration - i64::from(INVOICE_EXPIRATION_MARGIN_SECS)
        {
            return Err(Error::InvalidExpiration);
        }

        // Check for duplicate recipient IDs
        let receive_ids: Vec<String> = recipient_map
            .values()
            .flatten()
            .map(|r| r.recipient_id.clone())
            .collect();
        let mut receive_ids_dedup = receive_ids.clone();
        receive_ids_dedup.sort();
        receive_ids_dedup.dedup();
        if receive_ids.len() != receive_ids_dedup.len() {
            return Err(Error::RecipientIDDuplicated);
        }

        let mut rejected = HashSet::new();
        let mut reject_lists = RejectLists::new();
        let mut transfer_info_map: BTreeMap<String, InfoAssetTransfer> = BTreeMap::new();
        let mut invoices_checked = false;

        let begin_data = loop {
            for asset_id in recipient_map.keys() {
                let (asset_info, original_assignments_needed) =
                    assets_data.get(asset_id).unwrap().clone();
                let assignments_needed =
                    if let Some(existing_info) = transfer_info_map.get(asset_id) {
                        existing_info.assignments_needed.clone()
                    } else {
                        original_assignments_needed.clone()
                    };

                let asset_spend = self._select_rgb_inputs(
                    asset_id.clone(),
                    &assignments_needed,
                    input_unspents.clone(),
                )?;

                let transfer_info = InfoAssetTransfer {
                    asset_info,
                    recipients: local_recipients[asset_id].clone(),
                    asset_spend,
                    change: AssignmentsCollection::default(),
                    original_assignments_needed,
                    assignments_needed,
                    assignments_spent: HashMap::new(),
                    main_transition,
                    burn_reason: None,
                    before_block: None,
                };
                transfer_info_map.insert(asset_id.clone(), transfer_info);
            }

            // now that the assignments are confirmed available, ensure each invoice can still
            // accept a transfer on (one of) its proxies, proceeding only if a proxy reports it as
            // available. Done here (not earlier) so an InsufficientAssignments error takes
            // precedence
            if !invoices_checked {
                invoices_checked = true;
                for recipients in local_recipients.values() {
                    for lr in recipients {
                        let (invoice_id, _) = lr.proxy_ids("");
                        let mut full = false;
                        let mut unknown = false;
                        let mut available = false;
                        for lte in lr.transport_endpoints.iter().filter(|e| e.usable) {
                            // a proxy that cannot be asked is skipped, as another one may still
                            // have the invoice available
                            let result = match ProxyClient::new(&lte.endpoint)?
                                .check_invoices(std::slice::from_ref(&invoice_id))
                                .await
                            {
                                Ok(res) => res.result,
                                Err(e) => {
                                    debug!(self.logger, "Invoice check error: {e}");
                                    continue;
                                }
                            };
                            match result.and_then(|r| r.get(&invoice_id).map(|r| r.status)) {
                                Some(InvoiceStatus::Available) => {
                                    available = true;
                                    break;
                                }
                                Some(InvoiceStatus::Full) => full = true,
                                Some(InvoiceStatus::Unknown) => unknown = true,
                                None => {}
                            }
                        }
                        if !available {
                            return Err(if full {
                                Error::InvoiceQuotaExceeded
                            } else if unknown {
                                Error::InvoiceNotReserved
                            } else {
                                Error::NoValidTransportEndpoint
                            });
                        }
                    }
                }
            }

            match self
                ._prepare_transfer_psbt_wasm(
                    &mut transfer_info_map,
                    donation,
                    unspents.clone(),
                    &input_unspents,
                    &witness_recipients,
                    fee_rate_checked,
                    min_confirmations,
                    Some(expiration_timestamp as i64),
                    lock_time,
                    Some(operation_secret_key.clone()),
                    Some(sender_transport_endpoint.clone()),
                    &mut runtime,
                    &mut rejected,
                    &mut reject_lists,
                    dry_run,
                )
                .await?
            {
                PrepareTransferPsbtResult::Retry => continue,
                PrepareTransferPsbtResult::Success(begin_data) => break begin_data,
            }
        };

        // release the RGB stock so the backup can snapshot it
        drop(runtime);
        if dry_run {
            // a dry run reserves nothing: send_end reserves the inputs once it saves the transfer
            let txid = Psbt::from_str(&begin_data.psbt)?
                .unsigned_tx
                .compute_txid()
                .to_string();
            if let Some(artifacts) = self.transfer_artifacts.get_mut(&txid) {
                artifacts.inputs.clear();
            }
        } else {
            self.update_backup_info(false)?;
        }
        // the PSBT leaves the wallet only after its operation is saved
        self.flush().await?;

        info!(self.logger, "Send (begin) completed");
        Ok(SendBeginResult {
            psbt: begin_data.psbt,
            batch_transfer_idx: begin_data.batch_transfer_idx,
            details: SendDetails {
                fascia: begin_data.fascia,
                min_confirmations,
                entropy: begin_data.entropy,
                is_donation: donation,
            },
        })
    }

    /// Post a consignment, computing the proof of work the recipient proxy requires: a proxy under
    /// attack can raise the difficulty at any time, in which case it reports the current one and
    /// the post is retried with a new proof of work, a limited number of times.
    ///
    /// The outer result is for errors that should stop the operation, the inner one for errors
    /// specific to the proxy, which allow trying another transport endpoint.
    async fn post_consignment_with_pow(
        &self,
        proxy_client: &ProxyClient,
        data: &PostConsignmentData,
        consignment: &[u8],
    ) -> Result<Result<(), Error>, Error> {
        const MAX_POW_ATTEMPTS: u8 = 3;
        let mut difficulty = 0;
        for _ in 0..MAX_POW_ATTEMPTS {
            let pow_timestamp = now().unix_timestamp().to_string();
            let pow_nonce = solve_pow_nonce(&data.transfer_id, &pow_timestamp, difficulty).await;
            let res = proxy_client
                .post_consignment(data, consignment, &pow_timestamp, &pow_nonce)
                .await?;
            debug!(self.logger, "Consignment POST response: {:?}", res);
            let Some(err) = res.error else {
                // skip proxies with a non-compliant API
                return Ok(match res.result {
                    Some(_) => Ok(()),
                    None => Err(Error::Proxy {
                        details: s!("missing consignment.post result"),
                    }),
                });
            };
            match pow_difficulty_required(&err) {
                Some(required) if required <= MAX_POW_DIFFICULTY => {
                    debug!(
                        self.logger,
                        "Proof of work difficulty {required} required, retrying"
                    );
                    difficulty = required;
                }
                Some(required) => {
                    warn!(
                        self.logger,
                        "Proof of work difficulty {required} exceeds the maximum accepted"
                    );
                    return Ok(Err(map_proxy_error(err)));
                }
                None => return Ok(Err(map_proxy_error(err))),
            }
        }
        Ok(Err(Error::Proxy {
            details: s!("proof of work not accepted"),
        }))
    }

    /// Post the consignment of an asset to each recipient, on the first of its transport
    /// endpoints that accepts it, marking the endpoint as used.
    #[allow(clippy::too_many_arguments)]
    async fn post_transfer_data(
        &self,
        recipients: &mut [LocalRecipient],
        consignment: &[u8],
        sender_sk: &SigningKey,
        sender_pubkey_hex: &str,
        sender_transport_endpoint: &str,
        consignment_hash: &str,
        batch: bool,
    ) -> Result<(), Error> {
        for recipient in recipients {
            let recipient_id = &recipient.recipient_id;
            let recipient_vk = verifying_key_from_bytes(&recipient.public_key)?;
            let derived = derive_shared(sender_sk, &recipient_vk)?;
            let (invoice_id, transfer_id) = recipient.proxy_ids(sender_pubkey_hex);

            // the (optionally encrypted) consignment, signed along with the hash of the uploaded
            // bytes, is the same for every transport endpoint
            let posted = if recipient.encryption != Encryption::Unsupported {
                encrypt_blob(
                    consignment,
                    &derived.sym_key,
                    &decode_hash_hex(consignment_hash)?,
                )?
            } else {
                consignment.to_vec()
            };
            let blob_hash = hex::encode(sha256_digest(&posted));
            let consignment_data = PostConsignmentData {
                invoice_id,
                transfer_id: transfer_id.clone(),
                sender_transport_endpoint: sender_transport_endpoint.to_string(),
                sender_pk: sender_pubkey_hex.to_string(),
                consignment_hash: consignment_hash.to_string(),
                batch,
                signature: hex::encode(sign_ed25519(
                    sender_sk,
                    &sha256_digest(
                        post_consignment_message(&transfer_id, consignment_hash, &blob_hash)
                            .as_bytes(),
                    ),
                )),
            };

            let mut found_valid = false;
            // the error of the last endpoint that refused to create the transfer, if any
            let mut refusal = None;
            for transport_endpoint in recipient.transport_endpoints.iter_mut() {
                if transport_endpoint.used {
                    found_valid = true;
                    break;
                }
                if transport_endpoint.transport_type != TransportType::JsonRpc
                    || !transport_endpoint.usable
                {
                    debug!(
                        self.logger,
                        "Skipping transport endpoint {:?}", transport_endpoint
                    );
                    continue;
                }
                let proxy_url = transport_endpoint.endpoint.clone();
                debug!(
                    self.logger,
                    "Posting consignment for recipient ID: {recipient_id}"
                );
                let proxy_client = ProxyClient::new(&proxy_url)?;

                match self
                    .post_consignment_with_pow(&proxy_client, &consignment_data, &posted)
                    .await?
                {
                    Ok(()) => {}
                    // the transfer cannot be created on this transport endpoint
                    Err(
                        e @ (Error::InvoiceNotReserved
                        | Error::InvoiceExpired
                        | Error::InvoiceQuotaExceeded
                        | Error::TransferIDDeleted
                        | Error::ProxyDraining),
                    ) => {
                        refusal = Some(e);
                        continue;
                    }
                    Err(e @ Error::ProxyCannotChange { .. }) => return Err(e),
                    Err(_) => continue,
                }

                transport_endpoint.used = true;
                found_valid = true;
                break;
            }
            if !found_valid {
                return Err(refusal.unwrap_or(Error::NoValidTransportEndpoint));
            }
        }

        Ok(())
    }

    /// Complete the send operation (wasm32 async).
    ///
    /// See native [`send_end`](Wallet::send_end) for full documentation.
    pub async fn send_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Sending (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();

        // a failed or progressed send must not end again
        if let Some(bt) = self._get_outgoing_batch_transfer(&txid)?
            && !matches!(
                bt.status,
                TransferStatus::Initiated | TransferStatus::WaitingCounterparty
            )
        {
            return Err(Error::CannotEndTransfer { txid });
        }

        let artifacts = self
            .transfer_artifacts
            .get(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        if !artifacts.is_operation(|t| *t == TypeOfTransition::Transfer) {
            return Err(Error::WrongOperation { txid });
        }
        let info_contents = artifacts.batch_info.clone().ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let mut asset_infos = artifacts.asset_infos.clone();
        let mut consignment_bytes = artifacts.consignment_bytes.clone();

        // extract the signed transaction (needed for its hash and, for donations, to embed it)
        let tx = psbt.clone().extract_tx().map_err(InternalError::from)?;
        let tx_bytes = bdk_wallet::bitcoin::consensus::encode::serialize(&tx);
        let signed_tx_hash_raw = sha256_digest(&tx_bytes);
        let signed_tx_hash = hex::encode(signed_tx_hash_raw);

        // for a donation, put the signed TX in the consignments so the recipients can broadcast it
        if info_contents.donation {
            let witness_id = RgbTxid::from_str(&txid).unwrap();
            for bytes in consignment_bytes.values_mut() {
                let mut consignment = RgbTransfer::load(&bytes[..]).map_err(InternalError::from)?;
                if !consignment.modify_bundle(witness_id, |b| b.tx = tx.clone()) {
                    return Err(InternalError::Unexpected.into());
                }
                let mut v = vec![];
                consignment.save(&mut v)?;
                *bytes = v;
            }
            self.transfer_artifacts
                .get_mut(&txid)
                .expect("artifacts checked above")
                .consignment_bytes = consignment_bytes.clone();
        }

        // sender operation key (one per operation, shared across all recipients)
        let seed: [u8; 32] = info_contents
            .operation_secret_key
            .clone()
            .and_then(|s| s.try_into().ok())
            .ok_or_else(|| Error::Internal {
                details: s!("missing operation secret key"),
            })?;
        let sender_sk = ed25519_from_seed(&seed);
        let sender_pubkey_hex = hex::encode(sender_sk.verifying_key().to_bytes());
        let sender_transport_endpoint = info_contents
            .sender_transport_endpoint
            .clone()
            .ok_or_else(|| Error::Internal {
                details: s!("missing sender transport endpoint"),
            })?;

        let total_recipients: usize = asset_infos.values().map(|t| t.recipients.len()).sum();
        let batch = total_recipients > 1;

        let mut consignment_hashes: HashMap<String, String> = HashMap::new();
        for asset_id in asset_infos.keys() {
            let bytes = consignment_bytes.get(asset_id).ok_or(Error::Internal {
                details: s!("missing consignment bytes"),
            })?;
            consignment_hashes.insert(asset_id.clone(), hex::encode(sha256_digest(bytes)));
        }

        // expiration plus the sender proxy retention grace, until which recipients may still
        // obtain and broadcast the signed TX
        let mut broadcast_deadline = None;
        // whether the operation was registered earlier and has expired since, so there is nothing
        // left to post
        let mut operation_expired = false;
        let sender_proxy_url = self.convert_transport_endpoint_url(&sender_transport_endpoint)?;
        // ACK preparation on the sender's own proxy (non-donation only), before posting
        // consignments
        if !info_contents.donation {
            let mut transfer_data: HashMap<String, AckTransferData> = HashMap::new();
            for (asset_id, info_asset) in &asset_infos {
                let consignment_hash = consignment_hashes[asset_id].clone();
                for recipient in &info_asset.recipients {
                    let vk = verifying_key_from_bytes(&recipient.public_key)?;
                    let derived = derive_shared(&sender_sk, &vk)?;
                    let enc_tx = encrypt_blob(&tx_bytes, &derived.sym_key, &signed_tx_hash_raw)?;
                    let (_, transfer_id) = recipient.proxy_ids(&sender_pubkey_hex);
                    transfer_data.insert(
                        transfer_id,
                        AckTransferData {
                            recipient_pk: hex::encode(&recipient.public_key),
                            consignment_hash: consignment_hash.clone(),
                            signed_tx: general_purpose::STANDARD.encode(&enc_tx),
                        },
                    );
                }
            }
            // a send with a counterparty always has an expiration (required by the public API)
            let expiration = info_contents
                .expiration_timestamp
                .ok_or_else(|| Error::Internal {
                    details: s!("missing expiration"),
                })?;
            let retention_grace = sender_proxy_retention_grace(&sender_proxy_url).await?;
            broadcast_deadline = Some(expiration.saturating_add(i64::from(retention_grace)));
            let proxy_client = ProxyClient::new(&sender_proxy_url)?;
            let transfer_ids: Vec<String> = transfer_data.keys().cloned().collect();
            let expiration = expiration.to_string();
            // only needed by proxies with proxy-level authentication enabled
            let signature = self.online_data.as_ref().and_then(|online_data| {
                online_data.proxy_auth_signature(
                    &sender_proxy_url,
                    &prepare_ack_auth_message(
                        &sender_pubkey_hex,
                        &expiration,
                        &signed_tx_hash,
                        &transfer_data,
                    ),
                )
            });
            let res = proxy_client
                .prepare_ack(
                    transfer_data,
                    &expiration,
                    &signed_tx_hash,
                    &sender_pubkey_hex,
                    signature,
                )
                .await?;
            if let Some(err) = res.error {
                match map_proxy_error(err) {
                    // a prior ACK clean nulled the signed TX for this operation; re-supplying it
                    // is expected and ignored, the consignment(s) are posted and the local state
                    // is updated
                    Error::SignedTxCleared => {}
                    // the operation has expired: if it was registered earlier (by this wallet
                    // before a crash) only the local state is left to update, from which the
                    // refresh settles or fails the operation, otherwise it can no longer be
                    // started
                    e @ Error::ExpirationInvalid => {
                        if !sender_proxy_knows_operation(&proxy_client, &transfer_ids, &sender_sk)
                            .await?
                        {
                            return Err(e);
                        }
                        operation_expired = true;
                    }
                    e => return Err(e),
                }
            }
        }

        // an expired operation can no longer progress, so its consignments are not posted
        let mut post_result = Ok(());
        if !operation_expired {
            for (asset_id, info_contents_asset) in asset_infos.iter_mut() {
                post_result = self
                    .post_transfer_data(
                        &mut info_contents_asset.recipients,
                        &consignment_bytes[asset_id],
                        &sender_sk,
                        &sender_pubkey_hex,
                        &sender_transport_endpoint,
                        &consignment_hashes[asset_id],
                        batch,
                    )
                    .await;
                if post_result.is_err() {
                    break;
                }
            }
        }

        // keep the used flags, so a retry doesn't post to the same endpoints again
        let artifacts = self
            .transfer_artifacts
            .get_mut(&txid)
            .expect("artifacts checked above");
        artifacts.asset_infos = asset_infos.clone();
        post_result?;
        // keep the signed PSBT for the refresh
        artifacts.signed_psbt = Some(signed_psbt.clone());
        if artifacts.inputs.is_empty() {
            artifacts.inputs = psbt
                .unsigned_tx
                .input
                .iter()
                .map(|i| i.previous_output.into())
                .collect();
        }

        let batch_transfer_idx = if info_contents.donation {
            // Broadcast immediately for donations
            let mut runtime = self.rgb_runtime()?;
            self._broadcast_psbt(psbt, skip_sync).await?;
            runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
            let idx = self._update_or_save_transfers(
                txid.clone(),
                &asset_infos,
                &info_contents,
                TransferStatus::WaitingConfirmations,
                &consignment_hashes,
            )?;
            self.transfer_artifacts.remove(&txid);
            idx
        } else {
            let idx = self._update_or_save_transfers(
                txid.clone(),
                &asset_infos,
                &info_contents,
                TransferStatus::WaitingCounterparty,
                &consignment_hashes,
            )?;
            if broadcast_deadline.is_some() {
                let batch_transfer = self
                    .database
                    .iter_batch_transfers()?
                    .into_iter()
                    .find(|bt| bt.idx == idx)
                    .expect("batch transfer has just been saved");
                let mut updated: DbBatchTransferActMod = batch_transfer.into();
                updated.broadcast_deadline = ActiveValue::Set(broadcast_deadline);
                self.database.update_batch_transfer(&mut updated)?;
            }
            idx
        };

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Send (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
            entropy: info_contents.entropy,
        })
    }

    /// Prepare the PSBT to send bitcoins (wasm32 async).
    ///
    /// See native [`send_btc_begin`](Wallet::send_btc_begin) for full documentation.
    pub async fn send_btc_begin(
        &mut self,
        online: Online,
        address: String,
        amount: u64,
        fee_rate: u64,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Sending BTC (begin)...");
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let script_pubkey = self.get_script_pubkey(&address)?;

        let unspendable = self._get_unspendable_bdk_outpoints()?;

        let change_script = if self.wallet_data.reuse_addresses {
            Some(
                self._get_new_address(KeychainKind::Internal)?
                    .script_pubkey(),
            )
        } else {
            None
        };

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .unspendable(unspendable)
            .add_recipient(script_pubkey, BdkAmount::from_sat(amount))
            .fee_rate(fee_rate_checked);

        if let Some(change) = change_script {
            tx_builder.drain_to(change);
        }

        let mut psbt = tx_builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;
        self.add_non_witness_utxos(&mut psbt);

        self._record_vanilla_begin(&psbt, WalletTransactionType::SendBtc)
            .await?;

        info!(self.logger, "Send BTC (begin) completed");
        Ok(psbt.to_string())
    }

    /// Broadcast the PSBT to send bitcoins (wasm32 async).
    ///
    /// See native [`send_btc_end`](Wallet::send_btc_end) for full documentation.
    pub async fn send_btc_end(
        &mut self,
        online: Online,
        signed_psbt: String,
        skip_sync: bool,
    ) -> Result<String, Error> {
        info!(self.logger, "Sending BTC (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        self._check_vanilla_end(&signed_psbt, WalletTransactionType::SendBtc)?;
        let tx = self._broadcast_psbt(signed_psbt, skip_sync).await?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Send BTC (end) completed");
        Ok(tx.compute_txid().to_string())
    }

    /// Get a fee estimation for the given number of blocks.
    pub async fn get_fee_estimation(&self, online: Online, blocks: u16) -> Result<f64, Error> {
        info!(self.logger, "Getting fee estimation...");
        self.check_online(online)?;

        if !(MIN_BLOCK_ESTIMATION..=MAX_BLOCK_ESTIMATION).contains(&blocks) {
            return Err(Error::InvalidEstimationBlocks);
        }

        let estimation = self.indexer().fee_estimation(blocks).await?;

        info!(self.logger, "Get fee estimation completed");
        Ok(estimation)
    }

    /// Prepare the PSBT to drain all wallet funds.
    pub async fn drain_to_begin(
        &mut self,
        online: Online,
        address: String,
        destroy_assets: bool,
        fee_rate: u64,
    ) -> Result<String, Error> {
        info!(
            self.logger,
            "Draining (begin) to '{}' destroying assets '{}'...", address, destroy_assets
        );
        self.check_online(online)?;
        let fee_rate_checked = self._check_fee_rate(fee_rate)?;

        self.sync_db_txos(false).await?;

        let script_pubkey = self.get_script_pubkey(&address)?;

        let mut unspendable = None;
        if !destroy_assets {
            unspendable = Some(self._get_unspendable_bdk_outpoints()?);
        }

        let mut tx_builder = self.bdk_wallet.build_tx();
        tx_builder
            .drain_wallet()
            .drain_to(script_pubkey)
            .fee_rate(fee_rate_checked);

        if let Some(unspendable) = unspendable {
            tx_builder.unspendable(unspendable);
        }

        let mut psbt = tx_builder.finish().map_err(|e| match e {
            bdk_wallet::error::CreateTxError::CoinSelection(InsufficientFunds {
                needed,
                available,
            }) => Error::InsufficientBitcoins {
                needed: needed.to_sat(),
                available: available.to_sat(),
            },
            bdk_wallet::error::CreateTxError::OutputBelowDustLimit(_) => {
                Error::OutputBelowDustLimit
            }
            _ => Error::Internal {
                details: e.to_string(),
            },
        })?;
        self.add_non_witness_utxos(&mut psbt);

        self._record_vanilla_begin(&psbt, WalletTransactionType::Drain)
            .await?;

        info!(self.logger, "Drain (begin) completed");
        Ok(psbt.to_string())
    }

    /// Broadcast the PSBT to drain wallet funds.
    pub async fn drain_to_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<String, Error> {
        info!(self.logger, "Draining (end)...");
        self.check_online(online)?;

        let signed_psbt = Psbt::from_str(&signed_psbt)?;
        self._check_vanilla_end(&signed_psbt, WalletTransactionType::Drain)?;
        let tx = self._broadcast_psbt(signed_psbt, false).await?;

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Drain (end) completed");
        Ok(tx.compute_txid().to_string())
    }

    /// Prepare the PSBT to inflate an IFA asset.
    pub async fn inflate_begin(
        &mut self,
        online: Online,
        asset_id: String,
        inflation_amounts: Vec<u64>,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<String, Error> {
        info!(
            self.logger,
            "Inflating (begin) amounts: {:?}...", inflation_amounts
        );

        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let schema = asset.schema;
        self.check_schema_support(&schema)?;
        if !SCHEMAS_SUPPORTING_INFLATION.contains(&schema) {
            return Err(Error::UnsupportedInflation {
                asset_schema: schema,
            });
        }

        let known_circulating_supply = asset
            .known_circulating_supply
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let inflation =
            self.get_total_inflation_amount(&inflation_amounts, known_circulating_supply)?;
        if inflation == 0 {
            return Err(Error::NoInflationAmounts);
        }

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        let assignments_needed = AssignmentsCollection {
            inflation,
            ..Default::default()
        };
        let asset_spend = self._select_rgb_inputs(
            asset_id.clone(),
            &assignments_needed,
            input_unspents.clone(),
        )?;

        let network: ChainNet = self.bitcoin_network().into();
        let amount_sat = asset_spend.input_btc_amt / inflation_amounts.len() as u64;
        let dust = self
            .bdk_wallet
            .public_descriptor(KeychainKind::External)
            .dust_value()
            .to_sat();
        let amount_sat = max(amount_sat, dust);
        let mut local_recipients = vec![];
        let mut witness_recipients: Vec<(ScriptBuf, u64)> = vec![];
        for (idx, amt) in inflation_amounts.iter().enumerate() {
            let script_pubkey = self
                ._get_new_address(KeychainKind::External)?
                .script_pubkey();
            let beneficiary = beneficiary_from_script_buf(script_pubkey.clone());
            let beneficiary = XChainNet::with(network, beneficiary);
            let recipient_id = beneficiary.to_string();
            witness_recipients.push((script_pubkey, amount_sat));
            let vout = idx as u32 + 1;
            local_recipients.push(LocalRecipient {
                recipient_id,
                local_recipient_data: LocalRecipientData::Witness(LocalWitnessData {
                    amount_sat,
                    blinding: None,
                    vout,
                }),
                assignment: Assignment::Fungible(*amt),
                transport_endpoints: vec![],
                public_key: vec![],
                encryption: Encryption::Required,
                invoice_expiration: None,
            })
        }

        let contract_id = ContractId::from_str(&asset_id).expect("invalid contract ID");
        let asset_info = AssetInfo {
            contract_id,
            reject_list_url: asset.reject_list_url,
        };
        let transfer_info = InfoAssetTransfer {
            asset_info,
            recipients: local_recipients,
            asset_spend,
            change: AssignmentsCollection::default(),
            original_assignments_needed: assignments_needed.clone(),
            assignments_needed,
            assignments_spent: HashMap::new(),
            main_transition: TypeOfTransition::Inflate,
            burn_reason: None,
            before_block: None,
        };
        let psbt_string = self
            ._prepare_single_asset_psbt(
                transfer_info,
                unspents,
                &input_unspents,
                &witness_recipients,
                fee_rate_checked,
                min_confirmations,
                None,
                None,
                &mut runtime,
                true,
            )
            .await?
            .psbt;

        // release the RGB stock so the wallet state can be saved
        drop(runtime);
        // the PSBT leaves the wallet only after its operation is saved
        self.flush().await?;

        info!(self.logger, "Inflation (begin) completed");
        Ok(psbt_string)
    }

    /// Complete the inflate operation and broadcast.
    pub async fn inflate_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Inflating (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt
            .clone()
            .extract_tx()
            .map_err(InternalError::from)?
            .compute_txid()
            .to_string();

        // a failed or progressed operation must not end again
        if let Some(bt) = self._get_outgoing_batch_transfer(&txid)?
            && bt.status != TransferStatus::Initiated
        {
            return Err(Error::CannotEndTransfer { txid });
        }

        let artifacts = self
            .transfer_artifacts
            .get(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        if !artifacts.is_operation(|t| match t {
            TypeOfTransition::Inflate => true,
            #[cfg(test)]
            TypeOfTransition::Mint => true,
            _ => false,
        }) {
            return Err(Error::WrongOperation { txid });
        }
        // an expired inflation no longer reserves its inputs, so it cannot be ended
        if now().unix_timestamp() >= artifacts.expiration {
            return Err(Error::CannotEndTransfer { txid });
        }
        let artifacts = self.transfer_artifacts.remove(&txid).unwrap();
        let info_contents = artifacts.batch_info.ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let transfer_info_map = artifacts.asset_infos;

        // Store signed PSBT for refresh() persistence
        self.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(signed_psbt.clone()),
                ..Default::default()
            },
        );

        let mut runtime = self.rgb_runtime()?;
        let tx = self._broadcast_psbt(psbt, false).await?;
        let _ = tx;
        runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let batch_transfer_idx = self._save_transfers(
            txid.clone(),
            &transfer_info_map,
            info_contents.extra_allocations,
            info_contents.change_utxo_idx,
            info_contents.btc_change,
            TransferStatus::WaitingConfirmations,
            info_contents.min_confirmations,
            info_contents.expiration_timestamp,
            None,
            None,
            &HashMap::new(),
        )?;

        let (asset_id, transfer_info) = transfer_info_map.into_iter().next().unwrap();
        // a test BFA mint has no known circulating supply
        if transfer_info.main_transition == TypeOfTransition::Inflate {
            let inflation = transfer_info.original_assignments_needed.inflation;
            let db_asset = self.database.get_asset(asset_id).unwrap().unwrap();
            let updated_known_circulating_supply = db_asset
                .known_circulating_supply
                .as_ref()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                + inflation;
            let mut updated_asset: DbAssetActMod = db_asset.into();
            updated_asset.known_circulating_supply =
                ActiveValue::Set(Some(updated_known_circulating_supply.to_string()));
            self.database.update_asset(&mut updated_asset)?;
        }

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Inflate (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
            entropy: info_contents.entropy,
        })
    }

    /// Burn RGB assets.
    ///
    /// This calls [`burn_begin`](Wallet::burn_begin), signs the resulting PSBT and finally
    /// calls [`burn_end`](Wallet::burn_end).
    ///
    /// A wallet with private keys is required.
    pub async fn burn(
        &mut self,
        online: Online,
        asset_id: String,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Burning amount: {}...", amount);
        self._check_xprv()?;
        let begin = self
            .burn_begin(
                online.clone(),
                asset_id,
                amount,
                burn_reason,
                fee_rate,
                min_confirmations,
                true,
            )
            .await?;
        let signed_psbt = self.sign_psbt(begin.psbt, None)?;
        let res = self.burn_end(online, signed_psbt).await?;
        info!(self.logger, "Burn completed");
        Ok(res)
    }

    /// Prepare the PSBT to burn `amount` of an RGB asset, with the provided `fee_rate` (in
    /// sat/vB).
    ///
    /// The optional `burn_reason` is a hex-encoded 32-byte value committed in the burn
    /// transition.
    ///
    /// If `dry_run` is false, the wallet saves the transfer in [`TransferStatus::Initiated`] and
    /// returns its batch transfer idx.
    ///
    /// Sign the returned PSBT and give it to [`burn_end`](Wallet::burn_end).
    ///
    /// This doesn't require the wallet to have private keys.
    pub async fn burn_begin(
        &mut self,
        online: Online,
        asset_id: String,
        amount: u64,
        burn_reason: Option<String>,
        fee_rate: u64,
        min_confirmations: u8,
        dry_run: bool,
    ) -> Result<BurnBeginResult, Error> {
        info!(self.logger, "Burning (begin) amount: {}...", amount);
        self.check_online(online.clone())?;

        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let schema = asset.schema;
        self.check_schema_support(&schema)?;
        if !SCHEMAS_SUPPORTING_BURN.contains(&schema) {
            return Err(Error::UnsupportedBurn {
                asset_schema: schema,
            });
        }
        if amount == 0 {
            return Err(Error::NoBurnAmount);
        }
        let burn_reason = burn_reason
            .map(|reason| self.check_burn_reason(reason))
            .transpose()?;

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        let assignments_needed = AssignmentsCollection {
            fungible: amount,
            ..Default::default()
        };
        let asset_spend = self._select_rgb_inputs(
            asset_id.clone(),
            &assignments_needed,
            input_unspents.clone(),
        )?;

        let network: ChainNet = self.bitcoin_network().into();
        let script_pubkey = self
            ._get_new_address(KeychainKind::External)?
            .script_pubkey();
        let dust = self
            .bdk_wallet
            .public_descriptor(KeychainKind::External)
            .dust_value()
            .to_sat();
        let witness_recipients = vec![(script_pubkey.clone(), dust)];
        let beneficiary = beneficiary_from_script_buf(script_pubkey);
        let local_recipient = LocalRecipient {
            recipient_id: XChainNet::with(network, beneficiary).to_string(),
            local_recipient_data: LocalRecipientData::Witness(LocalWitnessData {
                amount_sat: dust,
                blinding: None,
                vout: 1,
            }),
            assignment: Assignment::Fungible(0),
            transport_endpoints: vec![],
            public_key: vec![],
            encryption: Encryption::Required,
            invoice_expiration: None,
        };

        let contract_id = ContractId::from_str(&asset_id).expect("invalid contract ID");
        let transfer_info = InfoAssetTransfer {
            asset_info: AssetInfo {
                contract_id,
                reject_list_url: asset.reject_list_url,
            },
            recipients: vec![local_recipient],
            asset_spend,
            change: AssignmentsCollection::default(),
            original_assignments_needed: assignments_needed.clone(),
            assignments_needed,
            assignments_spent: HashMap::new(),
            main_transition: TypeOfTransition::Burn,
            burn_reason,
            before_block: None,
        };
        let begin_data = self
            ._prepare_single_asset_psbt(
                transfer_info,
                unspents,
                &input_unspents,
                &witness_recipients,
                fee_rate_checked,
                min_confirmations,
                None,
                None,
                &mut runtime,
                dry_run,
            )
            .await?;
        drop(runtime);

        if dry_run {
            // a dry run reserves nothing: burn_end saves the transfer
            let txid = Psbt::from_str(&begin_data.psbt)?
                .unsigned_tx
                .compute_txid()
                .to_string();
            if let Some(artifacts) = self.transfer_artifacts.get_mut(&txid) {
                artifacts.inputs.clear();
            }
        } else {
            self.update_backup_info(false)?;
            self.trigger_auto_backup();
        }

        info!(self.logger, "Burn (begin) completed");
        Ok(BurnBeginResult {
            psbt: begin_data.psbt,
            batch_transfer_idx: begin_data.batch_transfer_idx,
            details: BurnDetails {
                fascia: begin_data.fascia,
                min_confirmations,
                entropy: begin_data.entropy,
                burn_reason: burn_reason.map(|r| r.to_string()),
            },
        })
    }

    /// Complete the burn operation by broadcasting the provided PSBT and saving the transfer.
    ///
    /// The provided PSBT, prepared with the [`burn_begin`](Wallet::burn_begin) function, needs to
    /// have already been signed.
    ///
    /// The wallet keeps a consignment that proves the burn, with its full history.
    ///
    /// This doesn't require the wallet to have private keys.
    pub async fn burn_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Burning (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();

        // only a burn begun without dry run, and not failed or ended, has a batch transfer
        let batch_transfer = self._check_end_initiated(&txid)?;

        let artifacts = self
            .transfer_artifacts
            .get(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        if !artifacts.is_operation(|t| *t == TypeOfTransition::Burn) {
            return Err(Error::WrongOperation { txid });
        }
        let info_contents = artifacts.batch_info.clone().ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let transfer_info_map = artifacts.asset_infos.clone();
        let fascia_bytes = artifacts.fascia.clone().ok_or(Error::Internal {
            details: s!("missing fascia in transfer artifacts"),
        })?;
        let mut fascia = fascia_from_bytes(fascia_bytes)?;

        // the consignment carries the signed transaction
        fascia.update_witness_tx(psbt.clone().extract_tx().map_err(InternalError::from)?);
        let mut runtime = self.rgb_runtime()?;
        let mut consignment_bytes = HashMap::new();
        for (asset_id, transfer_info) in &transfer_info_map {
            let contract_id = transfer_info.asset_info.contract_id;
            let consignment = runtime.transfer_from_fascia(
                contract_id,
                [],
                [],
                burn_opids(&fascia, contract_id),
                &fascia,
            )?;
            let mut bytes = Vec::new();
            consignment.save(&mut bytes)?;
            consignment_bytes.insert(asset_id.clone(), bytes);
        }

        self._broadcast_psbt(psbt, false).await?;
        runtime.consume_fascia(fascia, None)?;
        runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
        // Drop runtime so Stock returns to RefCell before auto-save
        drop(runtime);

        let batch_transfer_idx = match batch_transfer {
            Some(batch_transfer) => {
                let mut updated: DbBatchTransferActMod = batch_transfer.into();
                updated.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
                self.database.update_batch_transfer(&mut updated)?.idx
            }
            None => self._save_transfers(
                txid.clone(),
                &transfer_info_map,
                info_contents.extra_allocations,
                info_contents.change_utxo_idx,
                info_contents.btc_change,
                TransferStatus::WaitingConfirmations,
                info_contents.min_confirmations,
                None,
                None,
                None,
                &HashMap::new(),
            )?,
        };
        self.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(signed_psbt),
                consignment_bytes,
                ..Default::default()
            },
        );

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Burn (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
            entropy: info_contents.entropy,
        })
    }

    /// The outgoing batch transfer of `txid`, if any. Fail with [`Error::CannotEndTransfer`] if
    /// it is no longer initiated.
    fn _check_end_initiated(&self, txid: &str) -> Result<Option<DbBatchTransfer>, Error> {
        let batch_transfer = self._get_outgoing_batch_transfer(txid)?;
        if batch_transfer
            .as_ref()
            .is_some_and(|bt| !bt.status.initiated())
        {
            return Err(Error::CannotEndTransfer {
                txid: txid.to_string(),
            });
        }
        Ok(batch_transfer)
    }

    /// The mint the artifacts of an outgoing mint carry: what its lock has to match.
    fn _sent_mint(&self, artifacts: &TransferArtifacts) -> Result<ReceivedMint, Error> {
        let transfer_info = artifacts
            .asset_infos
            .values()
            .find(|t| t.main_transition == TypeOfTransition::Mint)
            .ok_or(Error::Internal {
                details: s!("not a mint transfer"),
            })?;
        let before_block = transfer_info.before_block.ok_or(Error::Internal {
            details: s!("mint transfer is missing the external chain block"),
        })?;
        let contract_id = transfer_info.asset_info.contract_id;
        let fascia = fascia_from_bytes(artifacts.fascia.clone().ok_or(Error::Internal {
            details: s!("missing fascia in transfer artifacts"),
        })?)?;
        let opid = mint_opid(&fascia, contract_id).ok_or(Error::Internal {
            details: s!("mint transition missing from fascia"),
        })?;
        let BridgeLocation::Evm(EvmContract { chain_id, address }) = self
            .rgb_runtime()?
            .contract_wrapper::<BridgedFungibleAsset>(contract_id)?
            .bridge_location();
        Ok(ReceivedMint {
            opid: opid.to_string(),
            lock_id: LockId::with(opid, fascia.witness_id()).to_string(),
            amount: transfer_info
                .recipients
                .iter()
                .map(|r| r.assignment.main_amount())
                .sum(),
            before_block,
            evm_chain_id: chain_id,
            bridge_contract: address.to_string(),
        })
    }

    /// Check that the bridge records a finalized lock for `mint`, included before the block the
    /// mint commits to. Give [`Error::MintLockNotFound`] if there is none yet and
    /// [`Error::MintLockExpired`] if there never will be.
    async fn _check_mint_lock(&self, mint: &ReceivedMint) -> Result<(), Error> {
        let evm_client = self
            ._evm_client(mint.evm_chain_id)
            .ok_or(Error::NoEvmRpcUrl {
                chain_id: mint.evm_chain_id,
            })?;
        let address = EvmAddress::from_str(&mint.bridge_contract).map_err(|e| Error::Internal {
            details: format!("invalid bridge contract address: {e}"),
        })?;
        let lock_id = LockId::from_str(&mint.lock_id).map_err(|e| Error::Internal {
            details: format!("invalid lock ID: {e}"),
        })?;
        match evm_client
            .mint_lock_status(&address, &lock_id, mint.amount, mint.before_block)
            .await?
        {
            MintLockStatus::Confirmed => Ok(()),
            MintLockStatus::Pending => Err(Error::MintLockNotFound {
                opid: mint.opid.clone(),
            }),
            MintLockStatus::Expired => Err(Error::MintLockExpired {
                opid: mint.opid.clone(),
                before_block: mint.before_block,
            }),
        }
    }

    /// Fail a sent mint, not ended yet, if its lock has expired. Return whether it was failed.
    async fn _fail_expired_mint(
        &mut self,
        batch_transfer: &DbBatchTransfer,
    ) -> Result<bool, Error> {
        let txid = batch_transfer
            .txid
            .as_ref()
            .expect("a pending mint has a TXID");
        let artifacts = self.transfer_artifacts.get(txid).ok_or(Error::Internal {
            details: s!("missing transfer artifacts of a pending mint"),
        })?;
        let mint = self._sent_mint(artifacts)?;
        match self._check_mint_lock(&mint).await {
            Err(Error::MintLockExpired { .. }) => {
                info!(self.logger, "Mint lock expired, failing the transfer");
                self._fail_batch_transfer(batch_transfer)?;
                Ok(true)
            }
            // a confirmed lock completes the mint at mint_end, a pending one may still
            Ok(()) | Err(Error::MintLockNotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Whether the batch transfer is an outgoing mint that has not reached `mint_end` yet.
    fn _is_pending_mint(batch_transfer: &DbBatchTransfer, db_data: &DbData) -> bool {
        batch_transfer.status == TransferStatus::Initiated
            && batch_transfer.txid.is_some()
            && db_data.asset_transfers.iter().any(|at| {
                at.batch_transfer_idx == batch_transfer.idx
                    && at.transition_type == Some(TypeOfTransition::Mint)
            })
    }

    /// Whether the batch transfer is a mint: a sent one before `mint_end`, or a received one
    /// that the wallet has checked. `fail_transfers` never fails a mint because of its expiration.
    fn _is_mint(&self, batch_transfer: &DbBatchTransfer, db_data: &DbData) -> Result<bool, Error> {
        if !batch_transfer.incoming(&db_data.asset_transfers, &db_data.transfers)? {
            return Ok(Self::_is_pending_mint(batch_transfer, db_data));
        }
        let (_, transfer) = self.database.get_incoming_transfer(
            &batch_transfer.get_transfers(&db_data.asset_transfers, &db_data.transfers)?,
        )?;
        Ok(transfer
            .recipient_id
            .as_ref()
            .is_some_and(|r| self.received_mints.contains_key(r)))
    }

    /// Begin a BFA mint: spend one mint right to mint `recipient`'s amount, backed by a lock of
    /// tokens on the external chain.
    ///
    /// The consignment is posted before the PSBT is signed, so the recipient can check the mint
    /// before the lock is made. The last mint right of the wallet is never spent.
    ///
    /// The lock has to be included before `before_block`. A lock at or after it never confirms the
    /// mint. The block must be at least [`EvmChain::min_mint_lock_blocks`] and at most
    /// [`EvmChain::max_mint_lock_blocks`] after the latest block, and leave a block for the lock.
    /// Else [`Error::InvalidBeforeBlock`] is returned.
    ///
    /// `before_block` is the only expiration of the mint. The `recipient` must have none, else
    /// [`Error::InvalidRecipientData`] is returned. The mint expires once its lock can no longer
    /// be included before `before_block`, as of the latest finalized block.
    ///
    /// Complete the mint with [`mint_end`](Wallet::mint_end) once the lock is finalized.
    #[allow(clippy::too_many_arguments)]
    pub async fn mint_begin(
        &mut self,
        online: Online,
        asset_id: String,
        recipient: Recipient,
        before_block: u64,
        fee_rate: u64,
        min_confirmations: u8,
        sender_transport_endpoint: String,
    ) -> Result<MintBeginResult, Error> {
        info!(self.logger, "Minting (begin) to: {:?}...", recipient);
        self.check_online(online.clone())?;

        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let schema = asset.schema;
        self.check_schema_support(&schema)?;
        if schema != AssetSchema::Bfa {
            return Err(Error::UnsupportedMint {
                asset_schema: schema,
            });
        }
        match recipient.assignment {
            Assignment::Fungible(0) => return Err(Error::InvalidAmountZero),
            Assignment::Fungible(_) => {}
            _ => return Err(Error::InvalidAssignment),
        }
        TransportEndpoint::new(sender_transport_endpoint.clone())?;
        let operation_sk = crate::encryption::gen_ed25519();

        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, fee_rate)?;

        // refuse to spend the last mint right, so the wallet can still mint
        let mint_rights = input_unspents
            .iter()
            .flat_map(|u| &u.rgb_allocations)
            .filter(|a| {
                a.asset_id.as_ref() == Some(&asset_id)
                    && a.status.settled()
                    && a.assignment == Assignment::MintRight
            })
            .count();
        if mint_rights == 1 {
            return Err(Error::LastMintRight);
        }

        let assignments_needed = AssignmentsCollection {
            mint: 1,
            ..Default::default()
        };
        let asset_spend = self._select_rgb_inputs(
            asset_id.clone(),
            &assignments_needed,
            input_unspents.clone(),
        )?;

        let now_ts = now().unix_timestamp();
        let mut witness_recipients: Vec<(ScriptBuf, u64)> = vec![];
        let local_recipient = self
            ._parse_recipient(
                &recipient,
                schema,
                self.bitcoin_network().into(),
                &operation_sk,
                now_ts,
                &mut witness_recipients,
                &mut 1,
                true,
            )
            .await?;

        // the lock can only be included after the latest block: refuse a block that leaves no
        // block for it, or one closer or further than configured. This block is the only
        // expiration of the mint
        let contract_id = ContractId::from_str(&asset_id).expect("invalid contract ID");
        let BridgeLocation::Evm(EvmContract { chain_id, .. }) = runtime
            .contract_wrapper::<BridgedFungibleAsset>(contract_id)?
            .bridge_location();
        let EvmChain {
            rpc_url,
            min_mint_lock_blocks,
            max_mint_lock_blocks,
        } = self
            ._evm_chain(chain_id)
            .ok_or(Error::NoEvmRpcUrl { chain_id })?
            .clone();
        let evm_client = EvmClient::new(&rpc_url);
        let latest_block = evm_client.block_number().await?;
        if before_block <= latest_block.saturating_add(1) {
            return Err(Error::InvalidBeforeBlock {
                details: format!(
                    "the lock cannot be included before block {before_block}, the latest \
                     external chain block being {latest_block}"
                ),
            });
        }
        if before_block < latest_block.saturating_add(min_mint_lock_blocks) {
            return Err(Error::InvalidBeforeBlock {
                details: format!(
                    "block {before_block} is less than {min_mint_lock_blocks} blocks after the \
                     latest external chain block {latest_block}"
                ),
            });
        }
        if before_block > latest_block.saturating_add(max_mint_lock_blocks) {
            return Err(Error::InvalidBeforeBlock {
                details: format!(
                    "block {before_block} is more than {max_mint_lock_blocks} blocks after the \
                     latest external chain block {latest_block}"
                ),
            });
        }

        let transfer_info = InfoAssetTransfer {
            asset_info: AssetInfo {
                contract_id,
                reject_list_url: asset.reject_list_url,
            },
            recipients: vec![local_recipient],
            asset_spend,
            change: AssignmentsCollection::default(),
            original_assignments_needed: assignments_needed.clone(),
            assignments_needed,
            assignments_spent: HashMap::new(),
            main_transition: TypeOfTransition::Mint,
            burn_reason: None,
            before_block: Some(before_block),
        };
        let begin_data = self
            ._prepare_single_asset_psbt(
                transfer_info,
                unspents,
                &input_unspents,
                &witness_recipients,
                fee_rate_checked,
                min_confirmations,
                Some(operation_sk.to_bytes().to_vec()),
                Some(sender_transport_endpoint.clone()),
                &mut runtime,
                false,
            )
            .await?;
        drop(runtime);

        let txid = Psbt::from_str(&begin_data.psbt)?
            .unsigned_tx
            .compute_txid()
            .to_string();
        let artifacts = &self.transfer_artifacts[&txid];
        let info_contents = artifacts.batch_info.clone().ok_or(Error::Internal {
            details: s!("missing batch info in transfer artifacts"),
        })?;
        let mut asset_infos = artifacts.asset_infos.clone();
        let consignment = artifacts.consignment_bytes[&asset_id].clone();
        let consignment_hash = hex::encode(sha256_digest(&consignment));
        let sender_pubkey_hex = hex::encode(operation_sk.verifying_key().to_bytes());

        // post the consignment now, so the recipient can check the mint before the lock. No ACK
        // follows, as the TX is signed only after the lock
        let post_result = self
            .post_transfer_data(
                &mut asset_infos
                    .get_mut(&asset_id)
                    .expect("saved by _prepare_transfer_psbt_wasm")
                    .recipients,
                &consignment,
                &operation_sk,
                &sender_pubkey_hex,
                &sender_transport_endpoint,
                &consignment_hash,
                false,
            )
            .await;
        if let Err(e) = post_result {
            let batch_transfer = self
                ._get_outgoing_batch_transfer(&txid)?
                .expect("saved by _prepare_transfer_psbt_wasm");
            self._fail_batch_transfer(&batch_transfer)?;
            self.update_backup_info(false)?;
            self.trigger_auto_backup();
            return Err(e);
        }
        self.transfer_artifacts
            .get_mut(&txid)
            .expect("artifacts read above")
            .asset_infos = asset_infos.clone();
        let batch_transfer_idx = self._update_or_save_transfers(
            txid,
            &asset_infos,
            &info_contents,
            TransferStatus::Initiated,
            &HashMap::from([(asset_id, consignment_hash)]),
        )?;
        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        let opid = mint_opid(&fascia_from_bytes(begin_data.fascia.clone())?, contract_id).ok_or(
            Error::Internal {
                details: s!("mint transition missing from fascia"),
            },
        )?;
        info!(self.logger, "Mint (begin) completed");
        Ok(MintBeginResult {
            psbt: begin_data.psbt,
            batch_transfer_idx,
            details: MintDetails {
                fascia: begin_data.fascia,
                min_confirmations,
                entropy: begin_data.entropy,
                opid: opid.to_string(),
            },
        })
    }

    /// Complete the mint operation by broadcasting the provided PSBT and saving the transfer.
    ///
    /// The provided PSBT, prepared with the [`mint_begin`](Wallet::mint_begin) function, needs to
    /// have already been signed. The bridge must record a finalized lock for the mint, included
    /// before the block the mint commits to (see [`mint_begin`](Wallet::mint_begin)). Else
    /// [`Error::MintLockNotFound`] is returned, or [`Error::MintLockExpired`] if there never will
    /// be one. An expired mint is failed, which releases its inputs, before the error is returned.
    ///
    /// This doesn't require the wallet to have private keys.
    pub async fn mint_end(
        &mut self,
        online: Online,
        signed_psbt: String,
    ) -> Result<OperationResult, Error> {
        info!(self.logger, "Minting (end)...");
        self.check_online(online)?;

        let psbt = Psbt::from_str(&signed_psbt)?;
        let txid = psbt.unsigned_tx.compute_txid().to_string();
        let batch_transfer = self._check_end_initiated(&txid)?;
        let artifacts = self
            .transfer_artifacts
            .get(&txid)
            .ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        if !artifacts.is_operation(|t| *t == TypeOfTransition::Mint) {
            return Err(Error::WrongOperation { txid });
        }
        let batch_transfer = batch_transfer.ok_or(Error::UnknownTransfer { txid: txid.clone() })?;
        let entropy = artifacts
            .batch_info
            .as_ref()
            .ok_or(Error::Internal {
                details: s!("missing batch info in transfer artifacts"),
            })?
            .entropy;
        let consignment_bytes = artifacts.consignment_bytes.clone();
        let mint = self._sent_mint(artifacts)?;

        match self._check_mint_lock(&mint).await {
            Ok(()) => {}
            Err(e @ Error::MintLockExpired { .. }) => {
                self._fail_batch_transfer(&batch_transfer)?;
                self.update_backup_info(false)?;
                self.trigger_auto_backup();
                return Err(e);
            }
            Err(e) => return Err(e),
        }

        let mut runtime = self.rgb_runtime()?;
        self._broadcast_psbt(psbt, false).await?;
        runtime.upsert_witness(RgbTxid::from_str(&txid).unwrap(), WitnessOrd::Tentative)?;
        drop(runtime);

        let mut updated: DbBatchTransferActMod = batch_transfer.into();
        updated.status = ActiveValue::Set(TransferStatus::WaitingConfirmations);
        let batch_transfer_idx = self.database.update_batch_transfer(&mut updated)?.idx;
        self.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(signed_psbt),
                consignment_bytes,
                ..Default::default()
            },
        );

        self.update_backup_info(false)?;
        self.trigger_auto_backup();

        info!(self.logger, "Mint (end) completed");
        Ok(OperationResult {
            txid,
            batch_transfer_idx,
            entropy,
        })
    }

    /// Begin a BFA mint of `amount` on one new UTXO, with one mint right as input, for tests only.
    ///
    /// Return the unsigned PSBT and the mint opid. Complete it with `inflate_end`.
    #[cfg(test)]
    pub(crate) async fn test_mint_begin(
        &mut self,
        online: Online,
        asset_id: String,
        amount: u64,
        before_block: u64,
    ) -> Result<(String, OpId), Error> {
        let asset = self.database.check_asset_exists(asset_id.clone())?;
        let (fee_rate_checked, unspents, input_unspents, mut runtime) =
            self._get_transfer_begin_data(online, 1)?;
        let assignments_needed = AssignmentsCollection {
            mint: 1,
            ..Default::default()
        };
        let asset_spend = self._select_rgb_inputs(
            asset_id.clone(),
            &assignments_needed,
            input_unspents.clone(),
        )?;
        let network: ChainNet = self.bitcoin_network().into();
        let script_pubkey = self
            ._get_new_address(KeychainKind::External)?
            .script_pubkey();
        let beneficiary = beneficiary_from_script_buf(script_pubkey.clone());
        let witness_recipients: Vec<(ScriptBuf, u64)> = vec![(script_pubkey, 1000)];
        let local_recipients = vec![LocalRecipient {
            recipient_id: XChainNet::with(network, beneficiary).to_string(),
            local_recipient_data: LocalRecipientData::Witness(LocalWitnessData {
                amount_sat: 1000,
                blinding: None,
                vout: 1,
            }),
            assignment: Assignment::Fungible(amount),
            transport_endpoints: vec![],
            public_key: vec![],
            encryption: Encryption::Required,
            invoice_expiration: None,
        }];
        let contract_id = ContractId::from_str(&asset_id).expect("invalid contract ID");
        let transfer_info = InfoAssetTransfer {
            asset_info: AssetInfo {
                contract_id,
                reject_list_url: asset.reject_list_url,
            },
            recipients: local_recipients,
            asset_spend,
            change: AssignmentsCollection::default(),
            original_assignments_needed: assignments_needed.clone(),
            assignments_needed,
            assignments_spent: HashMap::new(),
            main_transition: TypeOfTransition::Mint,
            burn_reason: None,
            before_block: Some(before_block),
        };
        let mut transfer_info_map = BTreeMap::from([(asset_id, transfer_info)]);
        let PrepareTransferPsbtResult::Success(begin_data) = self
            ._prepare_transfer_psbt_wasm(
                &mut transfer_info_map,
                false,
                unspents,
                &input_unspents,
                &witness_recipients,
                fee_rate_checked,
                1,
                None,
                None,
                None,
                None,
                &mut runtime,
                &mut HashSet::new(),
                &mut RejectLists::new(),
                true,
            )
            .await?
        else {
            unreachable!("a mint has no retry")
        };
        let psbt_string = begin_data.psbt;
        let opid = Psbt::from_str(&psbt_string)?
            .rgb_contract_consumers(contract_id)
            .unwrap()
            .into_iter()
            .find_map(|(opout, opid)| (opout.ty == OS_MINT).then_some(opid))
            .unwrap();
        Ok((psbt_string, opid))
    }

    /// List vanilla (non-colored) unspent outputs.
    pub async fn list_unspents_vanilla(
        &mut self,
        online: Online,
        min_confirmations: u8,
        skip_sync: bool,
    ) -> Result<Vec<LocalOutput>, Error> {
        info!(self.logger, "Listing unspents vanilla...");
        self.check_online(online)?;

        if !skip_sync {
            self.sync_db_txos(false).await?;
        }

        let all_unspents: Vec<LocalOutput> = self.internal_unspents().collect();

        let res = if min_confirmations > 0 {
            let mut filtered = Vec::new();
            for u in all_unspents {
                let confirmations = self
                    .indexer()
                    .get_tx_confirmations(&u.outpoint.txid.to_string())
                    .await?;
                if let Some(confs) = confirmations
                    && confs >= min_confirmations as u64
                {
                    filtered.push(u);
                }
            }
            filtered
        } else {
            all_unspents
        };

        info!(self.logger, "List unspents vanilla completed");
        Ok(res)
    }
}

// Serve an esplora: `route` maps a request path to a status line and a body.
#[cfg(test)]
fn fake_esplora(route: impl Fn(&str) -> (&'static str, Vec<u8>) + Send + 'static) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let n = std::io::Read::read(&mut stream, &mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let (code, body) = route(req.split_whitespace().nth(1).unwrap_or_default());
            let head = format!(
                "HTTP/1.1 {code}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = std::io::Write::write_all(&mut stream, &[head.as_bytes(), &body].concat());
        }
    });
    url
}

#[cfg(test)]
mod go_online_tests {
    use super::*;
    use crate::wallet::offline::address_reuse_tests::make_test_wallet;

    const INDEXER_URL: &str = "http://127.0.0.1:1";
    const PROXY_ENDPOINT: &str = "rpc://127.0.0.1:3000/json-rpc";
    const PROXY_URL: &str = "http://127.0.0.1:3000/json-rpc";

    fn options(url: &str, keys: &[(&str, &str)]) -> OnlineOptions {
        OnlineOptions {
            indexer_config: IndexerConfig::Esplora {
                url: url.to_string(),
            },
            skip_consistency_check: true,
            proxy_auth_keys: keys
                .iter()
                .map(|(e, k)| (e.to_string(), k.to_string()))
                .collect(),
            evm_chains: HashMap::new(),
        }
    }

    fn go_online(wallet: &mut Wallet, options: OnlineOptions) -> Result<Online, Error> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(wallet.go_online(options))
    }

    fn stored_keys(wallet: &Wallet) -> HashMap<String, [u8; 32]> {
        wallet.online_data.as_ref().unwrap().proxy_auth_keys.clone()
    }

    #[test]
    fn go_online_replaces_proxy_auth_keys() {
        let mut wallet = make_test_wallet(false);
        wallet.online_data = Some(OnlineData::for_test(INDEXER_URL));
        let online = Online {
            id: 1,
            indexer_url: INDEXER_URL.to_string(),
        };
        let seed1 = "11".repeat(32);
        let seed2 = "22".repeat(32);

        let res = go_online(
            &mut wallet,
            options(INDEXER_URL, &[(PROXY_ENDPOINT, &seed1)]),
        );
        assert_eq!(res, Ok(online.clone()));
        assert_eq!(
            stored_keys(&wallet),
            HashMap::from([(PROXY_URL.to_string(), [0x11; 32])])
        );

        let res = go_online(
            &mut wallet,
            options(INDEXER_URL, &[(PROXY_ENDPOINT, &seed2)]),
        );
        assert_eq!(res, Ok(online.clone()));
        assert_eq!(
            stored_keys(&wallet),
            HashMap::from([(PROXY_URL.to_string(), [0x22; 32])])
        );

        let res = go_online(&mut wallet, options(INDEXER_URL, &[]));
        assert_eq!(res, Ok(online));
        assert!(stored_keys(&wallet).is_empty());
    }

    #[test]
    fn go_online_stores_proxy_auth_keys() {
        let mut wallet = make_test_wallet(false);
        // the regtest genesis hash, so the network check passes
        let block_hash = |_: &str| {
            (
                "200 OK",
                b"0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206".to_vec(),
            )
        };
        let url = fake_esplora(block_hash);
        let other_url = fake_esplora(block_hash);
        let seed1 = "11".repeat(32);
        let seed2 = "22".repeat(32);

        let res = go_online(&mut wallet, options(&url, &[(PROXY_ENDPOINT, &seed1)]));
        assert!(res.is_ok());
        assert_eq!(
            stored_keys(&wallet),
            HashMap::from([(PROXY_URL.to_string(), [0x11; 32])])
        );

        let res = go_online(
            &mut wallet,
            options(&other_url, &[(PROXY_ENDPOINT, &seed2)]),
        );
        assert!(res.is_ok());
        assert_eq!(
            stored_keys(&wallet),
            HashMap::from([(PROXY_URL.to_string(), [0x22; 32])])
        );
    }

    #[test]
    fn go_online_rejects_invalid_proxy_auth_keys() {
        let mut wallet = make_test_wallet(false);
        let seed1 = "11".repeat(32);

        let res = go_online(
            &mut wallet,
            options(INDEXER_URL, &[("127.0.0.1:3000/json-rpc", &seed1)]),
        );
        assert!(matches!(res, Err(Error::InvalidTransportEndpoint { .. })));
        let res = go_online(
            &mut wallet,
            options(INDEXER_URL, &[("ws://127.0.0.1:3000/json-rpc", &seed1)]),
        );
        assert_eq!(res, Err(Error::UnsupportedTransportType));
        for seed in ["not hex", &seed1[..62], &format!("{seed1}00")] {
            let res = go_online(&mut wallet, options(INDEXER_URL, &[(PROXY_ENDPOINT, seed)]));
            assert!(matches!(res, Err(Error::InvalidProxyAuthKey { .. })));
        }
        assert!(wallet.online_data.is_none());

        wallet.online_data = Some(OnlineData::for_test(INDEXER_URL));
        let res = go_online(
            &mut wallet,
            options(INDEXER_URL, &[(PROXY_ENDPOINT, &seed1)]),
        );
        assert!(res.is_ok());
        let keys = HashMap::from([(PROXY_URL.to_string(), [0x11; 32])]);
        for url in [INDEXER_URL, "http://127.0.0.1:2"] {
            let res = go_online(&mut wallet, options(url, &[(PROXY_ENDPOINT, "not hex")]));
            assert!(matches!(res, Err(Error::InvalidProxyAuthKey { .. })));
            let online_data = wallet.online_data.as_ref().unwrap();
            assert_eq!(online_data.id, 1);
            assert_eq!(online_data.indexer_url, INDEXER_URL);
            assert_eq!(online_data.proxy_auth_keys, keys);
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod separate_signer_tests {
    use bdk_wallet::bitcoin::{Transaction, TxIn, transaction::Version};

    use super::*;
    use crate::keys::generate_keys;
    use crate::wallet::offline::address_reuse_tests::make_test_wallet_with_keys;

    fn funded_wallet_and_signer() -> (Wallet, Wallet, OutPoint) {
        let keys = generate_keys(BitcoinNetwork::Regtest);
        let mut wallet = make_test_wallet_with_keys(keys.clone(), false);
        let signer = make_test_wallet_with_keys(keys, false);

        let funding_tx = Transaction {
            version: Version::TWO,
            lock_time: bdk_wallet::bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([1; 32]), 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: BdkAmount::from_sat(100_000),
                script_pubkey: wallet
                    ._get_new_address(KeychainKind::External)
                    .unwrap()
                    .script_pubkey(),
            }],
        };
        let outpoint = OutPoint::new(funding_tx.compute_txid(), 0);
        wallet.bdk_wallet.apply_unconfirmed_txs([(funding_tx, 1)]);
        (wallet, signer, outpoint)
    }

    fn assert_signed(signer: &Wallet, psbt: &Psbt) {
        let signed = signer.sign_psbt(psbt.to_string(), None).unwrap();
        let signed = Psbt::from_str(&signed).unwrap();
        assert!(signed.inputs[0].final_script_witness.is_some());
    }

    #[test]
    fn signer_without_chain_data_signs_split_tx() {
        let (mut wallet, signer, outpoint) = funded_wallet_and_signer();
        let recipient = wallet
            ._get_new_address(KeychainKind::External)
            .unwrap()
            .script_pubkey();
        let psbt = wallet
            ._create_split_tx(
                &[outpoint],
                &vec![recipient],
                1000,
                FeeRate::from_sat_per_vb(1).unwrap(),
            )
            .unwrap();
        assert_signed(&signer, &psbt);
    }

    #[test]
    fn signer_without_chain_data_signs_rgb_psbt() {
        let (mut wallet, signer, outpoint) = funded_wallet_and_signer();
        let recipient = wallet
            ._get_new_address(KeychainKind::External)
            .unwrap()
            .script_pubkey();
        let (psbt, _) = wallet
            ._prepare_psbt(
                HashSet::from([outpoint]),
                &vec![(recipient, 1000)],
                FeeRate::from_sat_per_vb(1).unwrap(),
                None,
            )
            .unwrap();
        assert_signed(&signer, &psbt);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod send_end_tests {
    use super::*;
    use crate::utils::recipient_id_from_script_buf;
    use crate::wallet::offline::address_reuse_tests::make_test_wallet;

    const INDEXER_URL: &str = "http://127.0.0.1:1";
    const ENDPOINT: &str = "http://127.0.0.1:1/json-rpc";

    fn setup() -> (Wallet, Online) {
        setup_at(INDEXER_URL)
    }

    fn setup_at(url: &str) -> (Wallet, Online) {
        let mut wallet = make_test_wallet(false);
        wallet.online_data = Some(OnlineData::for_test(url));
        let online = Online {
            id: 1,
            indexer_url: url.to_string(),
        };
        (wallet, online)
    }

    // The TX is unconfirmed when `raw_tx` is given, unknown otherwise.
    fn esplora_route(raw_tx: Option<Vec<u8>>) -> impl Fn(&str) -> (&'static str, Vec<u8>) {
        move |path| match (&raw_tx, path) {
            (_, p) if p.ends_with("/status") => ("200 OK", b"{\"confirmed\":false}".to_vec()),
            (Some(raw), p) if p.ends_with("/raw") => ("200 OK", raw.clone()),
            _ => ("404 Not Found", vec![]),
        }
    }

    fn raw_tx(psbt: &str) -> Vec<u8> {
        bdk_wallet::bitcoin::consensus::serialize(&Psbt::from_str(psbt).unwrap().unsigned_tx)
    }

    fn dummy_psbt() -> (String, String) {
        dummy_psbt_to(ScriptBuf::new())
    }

    fn dummy_psbt_to(script_pubkey: ScriptBuf) -> (String, String) {
        use bdk_wallet::bitcoin::{Transaction, TxIn, TxOut};
        let tx = Transaction {
            version: bdk_wallet::bitcoin::transaction::Version::TWO,
            lock_time: bdk_wallet::bitcoin::absolute::LockTime::ZERO,
            input: vec![TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([1; 32]), 0),
                ..Default::default()
            }],
            output: vec![TxOut {
                value: BdkAmount::from_sat(1000),
                script_pubkey,
            }],
        };
        let txid = tx.compute_txid().to_string();
        let mut psbt = Psbt::from_unsigned_tx(tx).unwrap();
        psbt.inputs[0].witness_utxo = Some(TxOut {
            value: BdkAmount::from_sat(2000),
            script_pubkey: ScriptBuf::new(),
        });
        (psbt.to_string(), txid)
    }

    fn seed_batch(
        wallet: &Wallet,
        txid: &str,
        status: TransferStatus,
        incoming: bool,
        expiration: Option<i64>,
    ) -> i32 {
        let batch_idx = wallet
            .database
            .set_batch_transfer(DbBatchTransferActMod {
                txid: ActiveValue::Set(Some(txid.to_string())),
                status: ActiveValue::Set(status),
                expiration: ActiveValue::Set(expiration),
                min_confirmations: ActiveValue::Set(1),
                ..Default::default()
            })
            .unwrap();
        let asset_transfer_idx = wallet
            .database
            .set_asset_transfer(DbAssetTransferActMod {
                user_driven: ActiveValue::Set(true),
                batch_transfer_idx: ActiveValue::Set(batch_idx),
                asset_id: ActiveValue::Set(Some(s!("asset"))),
                ..Default::default()
            })
            .unwrap();
        wallet
            .database
            .set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(asset_transfer_idx),
                incoming: ActiveValue::Set(incoming),
                recipient_id: ActiveValue::Set(Some(s!("recipient"))),
                ..Default::default()
            })
            .unwrap();
        batch_idx
    }

    // An outgoing batch of one proxy recipient (`recipient_pubkey()`), registered on the sender
    // proxy `sender_proxy` with the operation key `OPERATION_SEED`.
    fn seed_outgoing(
        wallet: &Wallet,
        txid: &str,
        status: TransferStatus,
        expiration: Option<i64>,
        broadcast_deadline: Option<i64>,
        sender_proxy: &str,
        ack: Option<bool>,
    ) -> i32 {
        let batch_idx = seed_batch(wallet, txid, status, false, expiration);
        let mut batch: DbBatchTransferActMod = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|bt| bt.idx == batch_idx)
            .unwrap()
            .into();
        batch.operation_secret_key = ActiveValue::Set(Some(OPERATION_SEED.to_vec()));
        batch.broadcast_deadline = ActiveValue::Set(broadcast_deadline);
        wallet.database.update_batch_transfer(&mut batch).unwrap();
        let transfer = wallet.database.iter_transfers().unwrap().pop().unwrap();
        let mut updated: DbTransferActMod = transfer.clone().into();
        updated.ack = ActiveValue::Set(ack);
        updated.proxy_transfer_id = ActiveValue::Set(Some(s!("transfer")));
        updated.consignment_hash = ActiveValue::Set(Some("00".repeat(32)));
        updated.recipient_pubkey = ActiveValue::Set(Some(recipient_pubkey()));
        wallet.database.update_transfer(&mut updated).unwrap();
        wallet
            .save_sender_transport_endpoint(transfer.idx, sender_proxy)
            .unwrap();
        batch_idx
    }

    fn status(wallet: &Wallet, idx: i32) -> TransferStatus {
        wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|bt| bt.idx == idx)
            .unwrap()
            .status
    }

    // the keys of the recipient and of the operation
    const RECIPIENT_SEED: [u8; 32] = [8; 32];
    const OPERATION_SEED: [u8; 32] = [7; 32];

    fn recipient_pubkey() -> Vec<u8> {
        ed25519_from_seed(&RECIPIENT_SEED)
            .verifying_key()
            .to_bytes()
            .to_vec()
    }

    const INFO_BODY: &str = r#"{"jsonrpc":"2.0","id":null,"result":{"protocol_version":"1.0","version":"0.4.0","uptime":1,"retention_grace_secs":"100"}}"#;
    const TRUE_BODY: &str = r#"{"jsonrpc":"2.0","id":null,"result":true}"#;
    const ACK_GET_EMPTY: &str =
        r#"{"jsonrpc":"2.0","id":null,"result":{"acks":{},"signatures":{}}}"#;

    // answer the JSON-RPC calls of `method`
    fn rpc_mock(server: &mut mockito::ServerGuard, method: &str, body: &str) -> mockito::Mock {
        server
            .mock("POST", "/")
            .match_body(mockito::Matcher::Regex(method.to_string()))
            .with_header("content-type", "application/json")
            .with_body(body)
            .create()
    }

    fn recipient(id: &str, used: bool, usable: bool) -> LocalRecipient {
        LocalRecipient {
            recipient_id: id.to_string(),
            local_recipient_data: LocalRecipientData::Witness(LocalWitnessData {
                amount_sat: 1000,
                blinding: None,
                vout: 1,
            }),
            assignment: Assignment::Fungible(10),
            transport_endpoints: vec![LocalTransportEndpoint {
                transport_type: TransportType::JsonRpc,
                endpoint: ENDPOINT.to_string(),
                used,
                usable,
            }],
            public_key: recipient_pubkey(),
            encryption: Encryption::Required,
            invoice_expiration: None,
        }
    }

    fn info_asset(
        main_transition: TypeOfTransition,
        recipients: Vec<LocalRecipient>,
        assignments_spent: TxoAssignments,
    ) -> InfoAssetTransfer {
        InfoAssetTransfer {
            asset_info: AssetInfo {
                contract_id: ContractId::copy_from_slice([9; 32]).unwrap(),
                reject_list_url: None,
            },
            recipients,
            asset_spend: AssetSpend {
                txo_map: HashMap::new(),
                assignments_collected: AssignmentsCollection::default(),
                input_btc_amt: 0,
            },
            change: AssignmentsCollection::default(),
            original_assignments_needed: AssignmentsCollection::default(),
            assignments_needed: AssignmentsCollection::default(),
            assignments_spent,
            main_transition,
            burn_reason: None,
            before_block: None,
        }
    }

    fn batch_info(
        change_utxo_idx: Option<i32>,
        extra_allocations: HashMap<String, TxoAssignments>,
    ) -> InfoBatchTransfer {
        InfoBatchTransfer {
            btc_change: None,
            change_utxo_idx,
            extra_allocations,
            donation: false,
            min_confirmations: 1,
            expiration_timestamp: Some(i64::MAX),
            entropy: 0,
            operation_secret_key: Some(OPERATION_SEED.to_vec()),
            sender_transport_endpoint: Some(s!("rpc://127.0.0.1:1")),
        }
    }

    fn artifacts(asset_infos: BTreeMap<String, InfoAssetTransfer>) -> TransferArtifacts {
        TransferArtifacts {
            batch_info: Some(batch_info(None, HashMap::new())),
            consignment_bytes: asset_infos.keys().map(|k| (k.clone(), vec![1])).collect(),
            asset_infos,
            signed_psbt: None,
            ..Default::default()
        }
    }

    fn send_end_err(wallet: &mut Wallet, online: &Online, psbt: &str) -> Error {
        block_on(wallet.send_end(online.clone(), psbt.to_string(), true)).unwrap_err()
    }

    fn inflate_end_err(wallet: &mut Wallet, online: &Online, psbt: &str) -> Error {
        block_on(wallet.inflate_end(online.clone(), psbt.to_string())).unwrap_err()
    }

    const VANILLA_TYPES: [WalletTransactionType; 3] = [
        WalletTransactionType::CreateUtxos,
        WalletTransactionType::SendBtc,
        WalletTransactionType::Drain,
    ];

    // The errors of the vanilla end methods, in the order of `VANILLA_TYPES`.
    fn vanilla_end_errs(wallet: &mut Wallet, online: &Online, psbt: &str) -> [Error; 3] {
        [
            block_on(wallet.create_utxos_end(online.clone(), psbt.to_string(), true)).unwrap_err(),
            block_on(wallet.send_btc_end(online.clone(), psbt.to_string(), true)).unwrap_err(),
            block_on(wallet.drain_to_end(online.clone(), psbt.to_string())).unwrap_err(),
        ]
    }

    fn inflate_artifacts() -> TransferArtifacts {
        let inflate = info_asset(TypeOfTransition::Inflate, vec![], HashMap::new());
        TransferArtifacts {
            expiration: i64::MAX,
            ..artifacts(BTreeMap::from([(s!("asset"), inflate)]))
        }
    }

    fn seed_txo(wallet: &Wallet, txid: &str, vout: u32) -> i32 {
        wallet
            .database
            .set_txo(DbTxoActMod {
                txid: ActiveValue::Set(txid.to_string()),
                vout: ActiveValue::Set(vout),
                btc_amount: ActiveValue::Set(s!("1000")),
                spent: ActiveValue::Set(false),
                exists: ActiveValue::Set(true),
                pending_witness: ActiveValue::Set(false),
                ..Default::default()
            })
            .unwrap()
    }

    // A flagged TXO, as a sync or an accepted consignment leaves it.
    fn seed_pending_witness_txo(wallet: &Wallet, txid: &str, vout: u32) -> i32 {
        let idx = seed_txo(wallet, txid, vout);
        let txo = DbTxoActMod {
            idx: ActiveValue::Set(idx),
            pending_witness: ActiveValue::Set(true),
            ..Default::default()
        };
        wallet.database.update_txo(txo).unwrap();
        idx
    }

    fn pending_witness(wallet: &Wallet, txo_idx: i32) -> bool {
        wallet
            .database
            .iter_txos()
            .unwrap()
            .into_iter()
            .find(|t| t.idx == txo_idx)
            .unwrap()
            .pending_witness
    }

    fn new_script(wallet: &mut Wallet) -> ScriptBuf {
        wallet
            .bdk_wallet
            .reveal_next_address(KeychainKind::External)
            .address
            .script_pubkey()
    }

    // Make the wallet know a TX that pays `script` and return its TXID.
    fn apply_tx_paying(wallet: &mut Wallet, script: ScriptBuf) -> String {
        let tx = BdkTransaction {
            version: bdk_wallet::bitcoin::transaction::Version::TWO,
            lock_time: bdk_wallet::bitcoin::absolute::LockTime::ZERO,
            input: vec![bdk_wallet::bitcoin::TxIn {
                previous_output: OutPoint::new(Txid::from_byte_array([2; 32]), 0),
                ..Default::default()
            }],
            output: vec![bdk_wallet::bitcoin::TxOut {
                value: BdkAmount::from_sat(1000),
                script_pubkey: script,
            }],
        };
        let txid = tx.compute_txid().to_string();
        wallet.bdk_wallet.apply_unconfirmed_txs([(tx, 0)]);
        txid
    }

    fn seed_witness_receive(
        wallet: &Wallet,
        txid: &str,
        status: TransferStatus,
        vout: Option<u32>,
        script: &ScriptBuf,
    ) -> i32 {
        let batch_idx = seed_batch(wallet, txid, status, true, None);
        let mut transfer: DbTransferActMod = batch_transfer_row(wallet, batch_idx).into();
        transfer.recipient_type = ActiveValue::Set(Some(RecipientTypeFull::Witness { vout }));
        transfer.recipient_id = ActiveValue::Set(Some(recipient_id_from_script_buf(
            script.clone(),
            BitcoinNetwork::Regtest,
        )));
        wallet.database.update_transfer(&mut transfer).unwrap();
        batch_idx
    }

    // An incoming batch with a downloaded consignment that cannot be parsed, so the refresh
    // changes the transfer and then posts a NACK to `proxy_url`.
    fn seed_receive_with_consignment(wallet: &mut Wallet, proxy_url: &str, expiration: i64) -> i32 {
        let txid = "33".repeat(32);
        let status = TransferStatus::WaitingCounterparty;
        let batch_idx = seed_batch(wallet, &txid, status, true, Some(expiration));
        let mut batch = DbBatchTransferActMod {
            idx: ActiveValue::Set(batch_idx),
            operation_secret_key: ActiveValue::Set(Some(RECIPIENT_SEED.to_vec())),
            ..Default::default()
        };
        wallet.database.update_batch_transfer(&mut batch).unwrap();
        wallet
            .save_transfer_transport_endpoint(
                batch_transfer_row(wallet, batch_idx).idx,
                &LocalTransportEndpoint {
                    transport_type: TransportType::JsonRpc,
                    endpoint: proxy_url.to_string(),
                    used: false,
                    usable: true,
                },
            )
            .unwrap();
        wallet
            .received_consignments
            .insert(s!("recipient"), b"garbage".to_vec());
        wallet.received_consignment_meta.insert(
            s!("recipient"),
            ReceivedConsignmentMeta {
                proxy_url: Some(proxy_url.to_string()),
                transfer_id: Some(s!("transfer")),
                sender_transport_endpoint: proxy_url.to_string(),
                sender_pubkey: hex::encode(
                    ed25519_from_seed(&OPERATION_SEED)
                        .verifying_key()
                        .to_bytes(),
                ),
                consignment_hash: "00".repeat(32),
                batch: false,
            },
        );
        batch_idx
    }

    // The receive of `seed_receive_with_consignment` is as it was seeded, with its download kept.
    fn assert_receive_rolled_back(wallet: &Wallet, batch_idx: i32) {
        assert_eq!(ttes_used(wallet, batch_idx), vec![false]);
        let transfer = batch_transfer_row(wallet, batch_idx);
        assert_eq!(transfer.consignment_hash, None);
        assert_eq!(transfer.proxy_transfer_id, None);
        let endpoints = wallet
            .database
            .get_transfer_transport_endpoints_data(transfer.idx)
            .unwrap();
        assert!(
            endpoints
                .iter()
                .all(|(tte, _)| tte.kind != TransportEndpointKind::SenderProxy)
        );
        assert!(wallet.received_consignments.contains_key("recipient"));
        assert!(wallet.received_consignment_meta.contains_key("recipient"));
    }

    fn set_expiration(wallet: &Wallet, batch_idx: i32, expiration: i64) {
        let mut batch = DbBatchTransferActMod {
            idx: ActiveValue::Set(batch_idx),
            expiration: ActiveValue::Set(Some(expiration)),
            ..Default::default()
        };
        wallet.database.update_batch_transfer(&mut batch).unwrap();
    }

    fn batch_transfer_row(wallet: &Wallet, batch_idx: i32) -> DbTransfer {
        let db_data = wallet.database.get_db_data(false).unwrap();
        db_data
            .batch_transfers
            .iter()
            .find(|bt| bt.idx == batch_idx)
            .unwrap()
            .get_transfers(&db_data.asset_transfers, &db_data.transfers)
            .unwrap()
            .asset_transfers_data
            .pop()
            .unwrap()
            .transfers
            .pop()
            .unwrap()
    }

    fn ttes_used(wallet: &Wallet, batch_idx: i32) -> Vec<bool> {
        let db_data = wallet.database.get_db_data(false).unwrap();
        let bt = db_data
            .batch_transfers
            .iter()
            .find(|bt| bt.idx == batch_idx)
            .unwrap();
        let data = bt
            .get_transfers(&db_data.asset_transfers, &db_data.transfers)
            .unwrap();
        data.asset_transfers_data
            .iter()
            .flat_map(|a| a.transfers.iter())
            .flat_map(|t| {
                wallet
                    .database
                    .get_transfer_transport_endpoints_data(t.idx)
                    .unwrap()
            })
            .filter(|(tte, _)| tte.kind == TransportEndpointKind::RecipientProxy)
            .map(|(tte, _)| tte.used)
            .collect()
    }

    #[test]
    fn refresh_skips_initiated_send() {
        let (mut wallet, online) = setup();
        let idx = seed_batch(&wallet, "t", TransferStatus::Initiated, false, None);
        let filter = vec![RefreshFilter {
            status: RefreshTransferStatus::WaitingCounterparty,
            incoming: false,
        }];
        let res = block_on(wallet.refresh(online, None, filter, true)).unwrap();
        assert!(!res.contains_key(&idx));
        assert_eq!(status(&wallet, idx), TransferStatus::Initiated);
    }

    #[test]
    fn fail_transfers_fails_initiated_send() {
        let (mut wallet, online) = setup();
        let idx = seed_batch(&wallet, "t", TransferStatus::Initiated, false, None);
        let res = block_on(wallet.fail_transfers(online, Some(idx), false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);
    }

    #[test]
    fn fail_transfers_fails_expired_initiated_send() {
        let (mut wallet, online) = setup();
        let now = now().unix_timestamp();
        // an outgoing transfer is expired once its expiration passed on the sender proxy's clock too
        let expired = seed_batch(
            &wallet,
            "a",
            TransferStatus::Initiated,
            false,
            Some(now - i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS) - 1),
        );
        let valid = seed_batch(
            &wallet,
            "b",
            TransferStatus::Initiated,
            false,
            Some(i64::MAX),
        );
        let res = block_on(wallet.fail_transfers(online, None, false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, expired), TransferStatus::Failed);
        assert_eq!(status(&wallet, valid), TransferStatus::Initiated);
    }

    #[test]
    fn send_end_after_fail_or_end() {
        let (psbt, txid) = dummy_psbt();
        for st in [
            TransferStatus::Failed,
            TransferStatus::WaitingConfirmations,
            TransferStatus::Settled,
        ] {
            let (mut wallet, online) = setup();
            let idx = seed_batch(&wallet, &txid, st, false, None);
            let res = send_end_err(&mut wallet, &online, &psbt);
            assert_eq!(res, Error::CannotEndTransfer { txid: txid.clone() });
            assert_eq!(status(&wallet, idx), st);
        }
    }

    #[test]
    fn send_end_accepts_initiated_or_waiting_counterparty() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        seed_batch(&wallet, &txid, TransferStatus::Settled, true, None);
        let idx = seed_batch(&wallet, &txid, TransferStatus::Initiated, false, None);
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::UnknownTransfer { txid: txid.clone() });
        let mut updated: DbBatchTransferActMod = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|bt| bt.idx == idx)
            .unwrap()
            .into();
        updated.status = ActiveValue::Set(TransferStatus::WaitingCounterparty);
        wallet.database.update_batch_transfer(&mut updated).unwrap();
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::UnknownTransfer { txid });
    }

    #[test]
    fn send_end_without_send_begin() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::UnknownTransfer { txid });
    }

    #[test]
    fn send_end_refuses_other_operation() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let inflate = info_asset(TypeOfTransition::Inflate, vec![], HashMap::new());
        wallet.transfer_artifacts.insert(
            txid.clone(),
            artifacts(BTreeMap::from([(s!("asset"), inflate)])),
        );
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::WrongOperation { txid: txid.clone() });
        wallet
            .transfer_artifacts
            .insert(txid.clone(), artifacts(BTreeMap::new()));
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::WrongOperation { txid });
    }

    #[test]
    fn vanilla_end_refuses_rgb_operation() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        wallet
            .transfer_artifacts
            .insert(txid.clone(), inflate_artifacts());
        for err in vanilla_end_errs(&mut wallet, &online, &psbt) {
            assert_eq!(err, Error::WrongOperation { txid: txid.clone() });
        }
        for st in [TransferStatus::Failed, TransferStatus::Settled] {
            let (mut wallet, online) = setup();
            seed_batch(&wallet, &txid, st, false, None);
            for err in vanilla_end_errs(&mut wallet, &online, &psbt) {
                assert_eq!(err, Error::WrongOperation { txid: txid.clone() });
            }
        }
    }

    #[test]
    fn vanilla_end_refuses_other_vanilla_operation() {
        let (psbt, txid) = dummy_psbt();
        let parsed = Psbt::from_str(&psbt).unwrap();
        for recorded in VANILLA_TYPES {
            let (mut wallet, online) = setup();
            wallet
                ._record_vanilla_transaction(txid.clone(), recorded)
                .unwrap();
            let errs = vanilla_end_errs(&mut wallet, &online, &psbt);
            for (err, r#type) in errs.into_iter().zip(VANILLA_TYPES) {
                if r#type != recorded {
                    assert_eq!(err, Error::WrongOperation { txid: txid.clone() });
                }
            }
            assert_eq!(wallet._check_vanilla_end(&parsed, recorded), Ok(()));
            let res = send_end_err(&mut wallet, &online, &psbt);
            assert_eq!(res, Error::UnknownTransfer { txid: txid.clone() });
            let res = inflate_end_err(&mut wallet, &online, &psbt);
            assert_eq!(res, Error::UnknownTransfer { txid: txid.clone() });
        }
    }

    #[test]
    fn vanilla_end_after_end() {
        let (mut wallet, online) = setup();
        let script_pubkey = wallet
            .bdk_wallet
            .reveal_next_address(KeychainKind::External)
            .script_pubkey();
        let (psbt, txid) = dummy_psbt_to(script_pubkey);
        let tx = Psbt::from_str(&psbt).unwrap().unsigned_tx;
        wallet.bdk_wallet.apply_unconfirmed_txs([(tx, 1)]);
        for err in vanilla_end_errs(&mut wallet, &online, &psbt) {
            assert_eq!(err, Error::CannotEndTransfer { txid: txid.clone() });
        }
    }

    #[test]
    fn inflate_end_after_fail_or_end() {
        let (psbt, txid) = dummy_psbt();
        for st in [
            TransferStatus::Failed,
            TransferStatus::WaitingConfirmations,
            TransferStatus::Settled,
        ] {
            let (mut wallet, online) = setup();
            let idx = seed_batch(&wallet, &txid, st, false, None);
            wallet
                .transfer_artifacts
                .insert(txid.clone(), inflate_artifacts());
            let res = inflate_end_err(&mut wallet, &online, &psbt);
            assert_eq!(res, Error::CannotEndTransfer { txid: txid.clone() });
            assert_eq!(status(&wallet, idx), st);
            assert!(wallet.transfer_artifacts.contains_key(&txid));
        }
    }

    #[test]
    fn inflate_end_refuses_other_operation() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let transfer = info_asset(TypeOfTransition::Transfer, vec![], HashMap::new());
        wallet.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                expiration: i64::MAX,
                ..artifacts(BTreeMap::from([(s!("asset"), transfer)]))
            },
        );
        let res = inflate_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::WrongOperation { txid: txid.clone() });
        assert!(wallet.transfer_artifacts.contains_key(&txid));
        seed_batch(&wallet, &txid, TransferStatus::Initiated, false, None);
        let res = inflate_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::WrongOperation { txid: txid.clone() });
        assert!(wallet.transfer_artifacts.contains_key(&txid));
    }

    #[test]
    fn send_end_keeps_artifacts_when_a_post_fails() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let mut server = mockito::Server::new();
        let _info = rpc_mock(&mut server, "server.info", INFO_BODY);
        let _prepare = rpc_mock(&mut server, "ack.prepare", TRUE_BODY);
        let transfer = info_asset(
            TypeOfTransition::Transfer,
            vec![recipient("r1", true, true), recipient("r2", false, false)],
            HashMap::new(),
        );
        let mut transfer_artifacts = artifacts(BTreeMap::from([(s!("asset"), transfer)]));
        transfer_artifacts
            .batch_info
            .as_mut()
            .unwrap()
            .sender_transport_endpoint = Some(format!("rpc://{}", server.host_with_port()));
        transfer_artifacts
            .consignment_bytes
            .insert(s!("asset"), vec![1, 2, 3]);
        wallet
            .transfer_artifacts
            .insert(txid.clone(), transfer_artifacts);
        let res = send_end_err(&mut wallet, &online, &psbt);
        assert_eq!(res, Error::NoValidTransportEndpoint);
        let kept = &wallet.transfer_artifacts[&txid];
        assert!(kept.batch_info.is_some());
        assert!(kept.asset_infos["asset"].recipients[0].transport_endpoints[0].used);
        assert!(kept.signed_psbt.is_none());
    }

    #[test]
    fn update_or_save_transfers_updates_outgoing_batch() {
        let (wallet, _) = setup();
        let txid = s!("t");
        let input_idx = seed_txo(&wallet, "i", 0);
        let change_idx = seed_txo(&wallet, &txid, 2);
        let incoming = seed_batch(&wallet, &txid, TransferStatus::Settled, true, None);
        let mut map = BTreeMap::from([(
            s!("asset"),
            info_asset(
                TypeOfTransition::Transfer,
                vec![recipient("r1", false, true)],
                HashMap::from([(input_idx, vec![Assignment::Fungible(10)])]),
            ),
        )]);
        let extra = HashMap::from([(
            s!("other"),
            HashMap::from([(input_idx, vec![Assignment::Fungible(5)])]),
        )]);
        let info = batch_info(Some(change_idx), extra);
        let outgoing = wallet
            ._save_transfers(
                txid.clone(),
                &map,
                info.extra_allocations.clone(),
                info.change_utxo_idx,
                None,
                TransferStatus::Initiated,
                1,
                None,
                None,
                None,
                &HashMap::new(),
            )
            .unwrap();
        // the Initiated send reserves its input
        let free_idx = seed_txo(&wallet, "f", 0);
        let db_data = wallet.database.get_db_data(false).unwrap();
        let unspents = wallet
            .database
            .get_rgb_allocations(
                wallet.database.get_unspent_txos(db_data.txos).unwrap(),
                Some(db_data.colorings),
                Some(db_data.batch_transfers),
                Some(db_data.asset_transfers),
                Some(db_data.transfers),
            )
            .unwrap();
        let idxs = |u: Vec<LocalUnspent>| u.iter().map(|u| u.utxo.idx).collect::<Vec<_>>();
        let inputs = idxs(wallet.get_input_unspents(&unspents).unwrap());
        let available = idxs(
            wallet
                .get_available_allocations(unspents, &[], None)
                .unwrap(),
        );
        for found in [inputs, available] {
            assert!(found.contains(&free_idx));
            assert!(!found.contains(&input_idx));
        }
        map.get_mut("asset").unwrap().recipients[0].transport_endpoints[0].used = true;
        let idx = wallet
            ._update_or_save_transfers(
                txid.clone(),
                &map,
                &info,
                TransferStatus::WaitingCounterparty,
                &HashMap::new(),
            )
            .unwrap();
        assert_eq!(idx, outgoing);
        assert_eq!(
            status(&wallet, outgoing),
            TransferStatus::WaitingCounterparty
        );
        assert_eq!(status(&wallet, incoming), TransferStatus::Settled);
        let batches = wallet.database.iter_batch_transfers().unwrap();
        assert_eq!(
            batches
                .iter()
                .filter(|bt| bt.txid.as_deref() == Some(&txid))
                .count(),
            2
        );
        assert_eq!(ttes_used(&wallet, outgoing), vec![true]);
    }

    #[test]
    fn update_or_save_transfers_saves_beside_incoming_batch() {
        let (wallet, _) = setup();
        let txid = s!("t");
        let input_idx = seed_txo(&wallet, "i", 0);
        let change_idx = seed_txo(&wallet, &txid, 2);
        let incoming = seed_batch(&wallet, &txid, TransferStatus::Settled, true, None);
        let map = BTreeMap::from([(
            s!("asset"),
            info_asset(
                TypeOfTransition::Transfer,
                vec![recipient("r1", true, true)],
                HashMap::from([(input_idx, vec![Assignment::Fungible(10)])]),
            ),
        )]);
        let extra = HashMap::from([(
            s!("other"),
            HashMap::from([(input_idx, vec![Assignment::Fungible(5)])]),
        )]);
        let info = batch_info(Some(change_idx), extra);
        let idx = wallet
            ._update_or_save_transfers(
                txid.clone(),
                &map,
                &info,
                TransferStatus::WaitingCounterparty,
                &HashMap::new(),
            )
            .unwrap();
        assert_ne!(idx, incoming);
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
        let db_data = wallet.database.get_db_data(false).unwrap();
        let bt = db_data
            .batch_transfers
            .iter()
            .find(|bt| bt.idx == idx)
            .unwrap();
        let mut data: Vec<(bool, usize)> = bt
            .get_transfers(&db_data.asset_transfers, &db_data.transfers)
            .unwrap()
            .asset_transfers_data
            .iter()
            .map(|a| (a.asset_transfer.user_driven, a.transfers.len()))
            .collect();
        data.sort();
        assert_eq!(data, vec![(false, 0), (true, 1)]);
        assert_eq!(ttes_used(&wallet, idx), vec![true]);
    }

    #[test]
    fn is_batch_transfer_sent_ignores_initiated() {
        let (wallet, _) = setup();
        let idx = seed_batch(&wallet, "t", TransferStatus::Initiated, false, None);
        assert_eq!(wallet.is_batch_transfer_sent("t"), Ok(false));
        let mut updated: DbBatchTransferActMod = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|bt| bt.idx == idx)
            .unwrap()
            .into();
        updated.status = ActiveValue::Set(TransferStatus::WaitingCounterparty);
        wallet.database.update_batch_transfer(&mut updated).unwrap();
        assert_eq!(wallet.is_batch_transfer_sent("t"), Ok(true));
    }

    #[test]
    fn refresh_moves_waiting_broadcast_to_waiting_confirmations() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(Some(raw_tx(&psbt)))));
        let idx = seed_batch(
            &wallet,
            &txid,
            TransferStatus::WaitingBroadcast,
            true,
            Some(i64::MAX),
        );

        let filter = |incoming| {
            vec![RefreshFilter {
                status: RefreshTransferStatus::WaitingBroadcast,
                incoming,
            }]
        };
        block_on(wallet.refresh(online.clone(), None, filter(false), true)).unwrap();
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingBroadcast);

        let res = block_on(wallet.refresh(online.clone(), None, filter(true), true)).unwrap();
        assert_eq!(
            res[&idx].updated_status,
            Some(TransferStatus::WaitingConfirmations)
        );
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingConfirmations);

        let now = now().unix_timestamp();
        let idx2 = seed_batch(
            &wallet,
            &txid,
            TransferStatus::WaitingBroadcast,
            true,
            Some(now - 1),
        );
        let res = block_on(wallet.fail_transfers(online, Some(idx2), false, true));
        assert_eq!(res, Err(Error::CannotFailBatchTransfer));
        assert_eq!(status(&wallet, idx2), TransferStatus::WaitingConfirmations);
    }

    #[test]
    fn fail_transfers_fails_waiting_broadcast_only_once_expired() {
        let (_, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let now = now().unix_timestamp();
        let seed = |wallet: &Wallet, expiration| {
            seed_batch(
                wallet,
                &txid,
                TransferStatus::WaitingBroadcast,
                true,
                Some(expiration),
            )
        };

        let live = seed(&wallet, i64::MAX);
        let res = block_on(wallet.fail_transfers(online.clone(), Some(live), false, true));
        assert_eq!(res, Err(Error::CannotFailBatchTransfer));
        let res = block_on(wallet.fail_transfers(online.clone(), None, false, true));
        assert_eq!(res, Ok(false));
        assert_eq!(status(&wallet, live), TransferStatus::WaitingBroadcast);

        let expired = seed(&wallet, now - 1);
        let res = block_on(wallet.fail_transfers(online.clone(), Some(expired), false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, expired), TransferStatus::Failed);

        let expired = seed(&wallet, now - 1);
        let res = block_on(wallet.fail_transfers(online, None, false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, expired), TransferStatus::Failed);
        assert_eq!(status(&wallet, live), TransferStatus::WaitingBroadcast);
    }

    #[test]
    fn wait_ack_fails_expired_send_without_broadcast() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", ACK_GET_EMPTY);
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now - 1),
            None,
            &server.url(),
            Some(true),
        );
        wallet.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );
        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();
        assert_eq!(
            res[&idx],
            RefreshedTransfer {
                updated_status: Some(TransferStatus::Failed),
                failure: None,
            }
        );
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);
    }

    #[test]
    fn donation_signed_tx_needs_every_input_signed() {
        use bdk_wallet::bitcoin::{ScriptBuf, TxIn, Witness};
        let (psbt, _) = dummy_psbt();
        let mut tx = Psbt::from_str(&psbt).unwrap().unsigned_tx;
        let signed = TxIn {
            witness: Witness::from_slice(&[[1u8]]),
            ..Default::default()
        };

        tx.input = vec![];
        assert_eq!(Wallet::donation_signed_tx(&tx), None);
        tx.input = vec![TxIn::default()];
        assert_eq!(Wallet::donation_signed_tx(&tx), None);
        tx.input = vec![signed.clone()];
        assert_eq!(Wallet::donation_signed_tx(&tx), Some(tx.clone()));
        tx.input = vec![TxIn {
            script_sig: ScriptBuf::from_bytes(vec![1]),
            ..Default::default()
        }];
        assert_eq!(Wallet::donation_signed_tx(&tx), Some(tx.clone()));
        tx.input = vec![signed, TxIn::default()];
        assert_eq!(Wallet::donation_signed_tx(&tx), None);
    }

    const ERROR_BODY: &str =
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-104,"message":"not found"}}"#;

    fn proxy_error(code: i64) -> JsonRpcError {
        JsonRpcError {
            code,
            message: s!("msg"),
            data: None,
        }
    }

    fn ack_get_body(seed: [u8; 32], ack: bool) -> String {
        let sk = ed25519_from_seed(&seed);
        let digest = Wallet::ack_digest("transfer", ack, &"00".repeat(32));
        let sig = hex::encode(sign_ed25519(&sk, &digest));
        format!(
            r#"{{"jsonrpc":"2.0","id":null,"result":{{"acks":{{"transfer":{ack}}},"signatures":{{"transfer":"{sig}"}}}}}}"#
        )
    }

    // the sender proxy's ack.clean response decides whether a deliberate fail can proceed
    #[test]
    fn clean_ack_outcome_decisions() {
        // the signed TXs were revoked
        assert!(matches!(
            clean_ack_outcome(None).unwrap(),
            CleanAckOutcome::Revoked
        ));
        // the proxy never had or no longer has the operation
        assert!(matches!(
            clean_ack_outcome(Some(proxy_error(-104))).unwrap(),
            CleanAckOutcome::Unknown
        ));
        // all the recipients already ACKed (-107): the TX may be in a recipient's hands
        assert!(matches!(
            clean_ack_outcome(Some(proxy_error(-107))).unwrap(),
            CleanAckOutcome::Released
        ));
        // any other response is surfaced rather than read as "nothing to revoke"
        assert!(matches!(
            clean_ack_outcome(Some(proxy_error(-502))).unwrap_err(),
            Error::InvalidEndpointAuthSignature
        ));
        assert!(matches!(
            clean_ack_outcome(Some(proxy_error(-500))).unwrap_err(),
            Error::Proxy { .. }
        ));
    }

    // only a lost operation is tolerated when polling ack.get
    #[test]
    fn check_get_ack_error_decisions() {
        assert!(check_get_ack_error(proxy_error(-104)).is_ok());
        assert!(matches!(
            check_get_ack_error(proxy_error(-502)).unwrap_err(),
            Error::InvalidEndpointAuthSignature
        ));
        assert!(matches!(
            check_get_ack_error(proxy_error(-500)).unwrap_err(),
            Error::Proxy { .. }
        ));
    }

    #[test]
    fn decrypt_blob_with_shared_key_checks_the_hash() {
        let plaintext = b"consignment".to_vec();
        let hash = hex::encode(sha256_digest(&plaintext));
        let wrong_hash = "00".repeat(32);
        let pk = hex::encode(recipient_pubkey());
        let (sym_key, hash_raw) = Wallet::shared_key_and_hash(&OPERATION_SEED, &pk, &hash).unwrap();
        let encrypted = encrypt_blob(&plaintext, &sym_key, &hash_raw).unwrap();
        let decrypt = |raw: &[u8], encryption, hash: &str| {
            Wallet::decrypt_blob_with_shared_key(
                raw.to_vec(),
                encryption,
                &OPERATION_SEED,
                &pk,
                hash,
            )
        };
        // Optional takes the encrypted form and falls back to plaintext
        assert_eq!(
            decrypt(&encrypted, Encryption::Optional, &hash),
            Ok(plaintext.clone())
        );
        assert_eq!(
            decrypt(&plaintext, Encryption::Optional, &hash),
            Ok(plaintext.clone())
        );
        assert!(decrypt(&plaintext, Encryption::Required, &hash).is_err());
        // a plaintext that does not match the hash is refused
        for encryption in [Encryption::Unsupported, Encryption::Optional] {
            assert_eq!(
                decrypt(&plaintext, encryption, &wrong_hash),
                Err(Error::HashMismatch)
            );
        }
    }

    #[test]
    fn find_invoice_transfer_checks_the_sender_pubkey() {
        let (wallet, _) = setup();
        let mut server = mockito::Server::new();
        let invoice = invoice_id("recipient", &hex::encode(recipient_pubkey()));
        let sender_pk = hex::encode(
            ed25519_from_seed(&OPERATION_SEED)
                .verifying_key()
                .to_bytes(),
        );
        let valid = transfer_id(&invoice, &sender_pk);
        let _list = rpc_mock(
            &mut server,
            "transfer.list",
            &format!(
                r#"{{"jsonrpc":"2.0","id":null,"result":{{"0":"pending","{valid}":"pending"}}}}"#
            ),
        );
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":null,"result":{{"consignment":"","sender_transport_endpoint":"","sender_pk":"{sender_pk}","consignment_hash":"","batch":false}}}}"#
        );
        // transfer "0" is checked first and does not match its sender pubkey
        let forged = rpc_mock(&mut server, r#""transfer_id":"0""#, &body);
        let genuine = rpc_mock(&mut server, &format!(r#""transfer_id":"{valid}""#), &body);
        let (tid, res) =
            block_on(wallet.find_invoice_transfer(&server.url(), &invoice, &RECIPIENT_SEED))
                .unwrap()
                .unwrap();
        assert_eq!(tid, valid);
        assert_eq!(res.sender_pk, sender_pk);
        forged.assert();
        genuine.assert();
    }

    #[test]
    fn post_consignment_with_pow_follows_the_difficulty() {
        let (wallet, _) = setup();
        let data = PostConsignmentData {
            invoice_id: s!("invoice"),
            transfer_id: s!("transfer"),
            sender_transport_endpoint: s!(""),
            sender_pk: s!(""),
            consignment_hash: s!(""),
            batch: false,
            signature: s!(""),
        };
        let pow_body = |difficulty: u32| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-114,"message":"pow","data":{{"pow_difficulty":"{difficulty}"}}}}}}"#
            )
        };
        let pow_mock = |server: &mut mockito::ServerGuard, difficulty: u32, hits: usize| {
            server
                .mock("POST", "/")
                .match_body(mockito::Matcher::Regex(s!("consignment.post")))
                .with_header("content-type", "application/json")
                .with_body(pow_body(difficulty))
                .expect(hits)
                .create()
        };
        let post = |server: &mockito::ServerGuard| {
            let client = ProxyClient::new(&server.url()).unwrap();
            block_on(wallet.post_consignment_with_pow(&client, &data, b"consignment")).unwrap()
        };

        // the proxy raises the difficulty, then accepts the retry
        let mut server = mockito::Server::new();
        let raise = pow_mock(&mut server, 1, 1);
        let accept = rpc_mock(&mut server, "consignment.post", TRUE_BODY);
        assert_eq!(post(&server), Ok(()));
        raise.assert();
        accept.assert();

        // a difficulty above the maximum is not tried
        let mut server = mockito::Server::new();
        let too_hard = pow_mock(&mut server, MAX_POW_DIFFICULTY + 1, 1);
        assert!(matches!(post(&server), Err(Error::Proxy { .. })));
        too_hard.assert();

        // the proxy never accepts the proof of work
        let mut server = mockito::Server::new();
        let always = pow_mock(&mut server, 1, 3);
        assert_eq!(
            post(&server),
            Err(Error::Proxy {
                details: s!("proof of work not accepted")
            })
        );
        always.assert();
    }

    #[test]
    fn ack_digest_binds_transfer_ack_and_hash() {
        let hash = "00".repeat(32);
        let digest = Wallet::ack_digest("transfer", true, &hash);
        assert_ne!(digest, Wallet::ack_digest("other", true, &hash));
        assert_ne!(digest, Wallet::ack_digest("transfer", false, &hash));
        assert_ne!(
            digest,
            Wallet::ack_digest("transfer", true, &"11".repeat(32))
        );
    }

    #[test]
    fn deadline_helpers() {
        let (wallet, _) = setup();
        let now = now().unix_timestamp();
        let skew = i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS);
        let confirmation = i64::from(CONFIRMATION_MARGIN_SECS);
        let batch = |expiration, broadcast_deadline, status, retention_deadline| {
            let idx = seed_batch(&wallet, "t", status, false, expiration);
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .into_iter()
                .find(|bt| bt.idx == idx)
                .unwrap()
                .into();
            updated.broadcast_deadline = ActiveValue::Set(broadcast_deadline);
            updated.retention_deadline = ActiveValue::Set(retention_deadline);
            wallet.database.update_batch_transfer(&mut updated).unwrap();
            wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .into_iter()
                .find(|bt| bt.idx == idx)
                .unwrap()
        };
        let waiting = TransferStatus::WaitingCounterparty;

        // an outgoing operation is expired once the skew margin has passed too
        assert!(!expired_beyond_skew_margin(&batch(
            Some(now - skew + 5),
            None,
            waiting,
            None
        )));
        assert!(expired_beyond_skew_margin(&batch(
            Some(now - skew - 5),
            None,
            waiting,
            None
        )));
        assert!(!expired_beyond_skew_margin(&batch(
            None, None, waiting, None
        )));

        // the broadcast deadline defaults to the expiration
        assert!(!broadcast_deadline_passed_beyond_skew_margin(&batch(
            Some(now - skew - 5),
            Some(now + 100),
            waiting,
            None
        )));
        assert!(broadcast_deadline_passed_beyond_skew_margin(&batch(
            Some(now + 100),
            Some(now - skew - 5),
            waiting,
            None
        )));
        assert!(broadcast_deadline_passed_beyond_skew_margin(&batch(
            Some(now - skew - 5),
            None,
            waiting,
            None
        )));

        // an unknown broadcast deadline is never past
        assert!(!confirmation_deadline_passed(&batch(
            None, None, waiting, None
        )));
        assert!(!confirmation_deadline_passed(&batch(
            None,
            Some(now - confirmation + 5),
            waiting,
            None
        )));
        assert!(confirmation_deadline_passed(&batch(
            None,
            Some(now - confirmation - 5),
            waiting,
            None
        )));

        // a transfer waiting for the broadcast is expired past the confirmation margin
        let waiting_broadcast = TransferStatus::WaitingBroadcast;
        assert!(!incoming_expired(
            &batch(Some(now - 10), Some(now + 5), waiting_broadcast, None),
            now
        ));
        assert!(incoming_expired(
            &batch(
                Some(now + 100),
                Some(now - confirmation - 5),
                waiting_broadcast,
                None
            ),
            now
        ));
        // a blind receive reserved on proxies waits for the skew margin too
        assert!(!incoming_expired(
            &batch(Some(now - 10), None, waiting, Some(now + 100)),
            now
        ));
        assert!(incoming_expired(
            &batch(Some(now - skew - 5), None, waiting, Some(now + 100)),
            now
        ));
        // any other is expired past its expiration
        assert!(incoming_expired(
            &batch(Some(now - 1), None, waiting, None),
            now
        ));
        assert!(!incoming_expired(
            &batch(Some(now + 1), None, waiting, None),
            now
        ));

        assert_eq!(broadcast_deadline(Some("1000"), 50), Some(1050));
        assert_eq!(broadcast_deadline(Some("soon"), 50), None);
        assert_eq!(broadcast_deadline(None, 50), None);
    }

    #[test]
    fn send_end_sets_broadcast_deadline() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup();
        let mut server = mockito::Server::new();
        let _info = rpc_mock(&mut server, "server.info", INFO_BODY);
        let _prepare = rpc_mock(&mut server, "ack.prepare", TRUE_BODY);
        let _post = rpc_mock(&mut server, "consignment.post", TRUE_BODY);
        let transfer = info_asset(
            TypeOfTransition::Transfer,
            vec![recipient("r1", false, true)],
            HashMap::new(),
        );
        let expiration = now().unix_timestamp() + 1000;
        let mut transfer_artifacts = artifacts(BTreeMap::from([(s!("asset"), transfer)]));
        let info = transfer_artifacts.batch_info.as_mut().unwrap();
        info.expiration_timestamp = Some(expiration);
        info.sender_transport_endpoint = Some(format!("rpc://{}", server.host_with_port()));
        transfer_artifacts
            .consignment_bytes
            .insert(s!("asset"), vec![1, 2, 3]);
        wallet
            .transfer_artifacts
            .insert(txid.clone(), transfer_artifacts);
        // the recipient proxy is the mock too
        let endpoint = server.url();
        let artifact = wallet.transfer_artifacts.get_mut(&txid).unwrap();
        artifact.asset_infos.get_mut("asset").unwrap().recipients[0].transport_endpoints[0]
            .endpoint = endpoint;

        // the end panics on native in the backup info update, which needs JS time, once the
        // transfers are saved
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            block_on(wallet.send_end(online, psbt, true))
        }));
        let batch = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .into_iter()
            .find(|bt| bt.txid.as_deref() == Some(txid.as_str()))
            .unwrap();
        // the expiration plus the retention grace advertised by the sender proxy
        assert_eq!(batch.broadcast_deadline, Some(expiration + 100));
        assert_eq!(batch.status, TransferStatus::WaitingCounterparty);
    }

    #[test]
    fn invalid_ack_signature_fails_once_expired() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        // an ACK signed by another key than the one of the recipient
        let _ack = rpc_mock(&mut server, "ack.get", &ack_get_body([9; 32], true));
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now + 1000),
            None,
            &server.url(),
            None,
        );
        wallet.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );

        let res = block_on(wallet.refresh(online.clone(), None, vec![], true)).unwrap();
        assert!(matches!(
            res[&idx].failure,
            Some(Error::InvalidContentSignature)
        ));
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
        let res = block_on(wallet.fail_transfers(online.clone(), Some(idx), false, true));
        assert_eq!(res, Err(Error::CannotFailBatchTransfer));

        // once the broadcast deadline passed, the transfer fails
        let mut updated: DbBatchTransferActMod = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .pop()
            .unwrap()
            .into();
        updated.expiration = ActiveValue::Set(Some(now - 1000));
        updated.broadcast_deadline = ActiveValue::Set(Some(now - 1000));
        wallet.database.update_batch_transfer(&mut updated).unwrap();
        let res = block_on(wallet.fail_transfers(online.clone(), Some(idx), false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);

        // a denied fail does not keep the valid ACK read before the invalid signature
        let sign = |seed, id| {
            let digest = Wallet::ack_digest(id, true, &"00".repeat(32));
            hex::encode(sign_ed25519(&ed25519_from_seed(&seed), &digest))
        };
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":null,"result":{{"acks":{{"transfer":true,"second":true}},"signatures":{{"transfer":"{}","second":"{}"}}}}}}"#,
            sign(RECIPIENT_SEED, "transfer"),
            sign([9; 32], "second"),
        );
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", &body);
        let idx = seed_outgoing(
            &wallet,
            &"22".repeat(32),
            TransferStatus::WaitingCounterparty,
            Some(now + 1000),
            None,
            &server.url(),
            None,
        );
        let first = wallet.database.iter_transfers().unwrap().pop().unwrap();
        let second = wallet
            .database
            .set_transfer(DbTransferActMod {
                asset_transfer_idx: ActiveValue::Set(first.asset_transfer_idx),
                incoming: ActiveValue::Set(false),
                recipient_id: ActiveValue::Set(Some(s!("second"))),
                proxy_transfer_id: ActiveValue::Set(Some(s!("second"))),
                consignment_hash: ActiveValue::Set(Some("00".repeat(32))),
                recipient_pubkey: ActiveValue::Set(Some(recipient_pubkey())),
                ..Default::default()
            })
            .unwrap();
        wallet
            .save_sender_transport_endpoint(second, &server.url())
            .unwrap();

        let res = block_on(wallet.fail_transfers(online, Some(idx), false, true));

        assert_eq!(res, Err(Error::CannotFailBatchTransfer));
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
        let first = wallet
            .database
            .iter_transfers()
            .unwrap()
            .into_iter()
            .find(|t| t.idx == first.idx)
            .unwrap();
        assert_eq!(first.ack, None);
        assert_eq!(first.ack_signature, None);
    }

    #[test]
    fn wait_ack_stores_a_valid_signature() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        // a NACK signed by the recipient
        let _ack = rpc_mock(&mut server, "ack.get", &ack_get_body(RECIPIENT_SEED, false));
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now + 1000),
            None,
            &server.url(),
            None,
        );
        wallet.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );
        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();
        assert_eq!(res[&idx].failure, None);
        assert_eq!(res[&idx].updated_status, Some(TransferStatus::Failed));
        let transfer = wallet.database.iter_transfers().unwrap().pop().unwrap();
        assert_eq!(transfer.ack, Some(false));
        assert!(transfer.ack_signature.is_some());
    }

    // the recipient ACKed but the transfer expired on the sender side: the TX was released, so
    // the transfer cannot be failed
    #[test]
    fn sender_expired_after_recipient_ack() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", &ack_get_body(RECIPIENT_SEED, true));
        let _clean = rpc_mock(&mut server, "ack.clean", ERROR_BODY);
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now - 400),
            Some(now + 1000),
            &server.url(),
            None,
        );
        let mut transfer: DbTransferActMod = wallet
            .database
            .iter_transfers()
            .unwrap()
            .pop()
            .unwrap()
            .into();
        transfer.invoice_expiration = ActiveValue::Set(Some(now - 1000));
        wallet.database.update_transfer(&mut transfer).unwrap();
        wallet.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );
        let res = block_on(wallet.fail_transfers(online, None, false, true));
        // the ACK is signed, so the TX was released: nothing is failed, even though the TX is not
        // on-chain and the transfer has expired
        assert_eq!(res, Ok(false));
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
    }

    // the sender proxy lost the operation
    #[test]
    fn sender_proxy_lost_operation() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", ERROR_BODY);
        let _clean = rpc_mock(&mut server, "ack.clean", ERROR_BODY);
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now + 1000),
            None,
            &server.url(),
            None,
        );
        wallet.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );
        let set = |wallet: &Wallet, expiration, deadline| {
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .pop()
                .unwrap()
                .into();
            updated.expiration = ActiveValue::Set(Some(expiration));
            updated.broadcast_deadline = ActiveValue::Set(deadline);
            wallet.database.update_batch_transfer(&mut updated).unwrap();
        };

        // not expired: the refresh reports it and the fail is denied
        let res = block_on(wallet.refresh(online.clone(), None, vec![], true)).unwrap();
        assert!(matches!(res[&idx].failure, Some(Error::OperationNotFound)));
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
        let res = block_on(wallet.fail_transfers(online.clone(), Some(idx), false, true));
        assert_eq!(res, Err(Error::CannotFailBatchTransfer));

        // expired, but the broadcast deadline is not past
        set(&wallet, now - 400, Some(now + 1000));
        let res = block_on(wallet.fail_transfers(online.clone(), Some(idx), false, true));
        assert_eq!(res, Err(Error::CannotFailBatchTransfer));
        assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);

        // past the broadcast deadline
        set(&wallet, now - 1000, Some(now - 400));
        let res = block_on(wallet.fail_transfers(online, Some(idx), false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);
    }

    #[test]
    fn recipient_waits_for_the_broadcast_deadline() {
        let (_, txid) = dummy_psbt();
        let paths = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
        let seen = paths.clone();
        let inner = esplora_route(None);
        let url = fake_esplora(move |path| {
            seen.lock().unwrap().push(path.to_string());
            inner(path)
        });
        let (mut wallet, online) = setup_at(&url);
        let now = now().unix_timestamp();
        wallet
            .received_signed_txs
            .insert(s!("recipient"), raw_tx(&dummy_psbt().0));
        for expected in [
            TransferStatus::WaitingBroadcast,
            TransferStatus::WaitingConfirmations,
        ] {
            let idx = seed_batch(&wallet, &txid, expected, true, Some(now + 5000));
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .into_iter()
                .find(|bt| bt.idx == idx)
                .unwrap()
                .into();
            updated.broadcast_deadline = ActiveValue::Set(Some(now + 1000));
            wallet.database.update_batch_transfer(&mut updated).unwrap();
            paths.lock().unwrap().clear();
            let res = block_on(wallet.refresh(online.clone(), None, vec![], true)).unwrap();
            assert_eq!(res[&idx].updated_status, None);
            assert_eq!(status(&wallet, idx), expected);
            // the TX is unknown to the indexer, so it is broadcast again
            assert!(paths.lock().unwrap().iter().any(|p| p.ends_with("/tx")));
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .into_iter()
                .find(|bt| bt.idx == idx)
                .unwrap()
                .into();
            updated.status = ActiveValue::Set(TransferStatus::Failed);
            wallet.database.update_batch_transfer(&mut updated).unwrap();
        }
    }

    #[test]
    fn recipient_fails_unconfirmed_past_the_deadline() {
        let (_, txid) = dummy_psbt();
        for (initial, route_raw) in [
            (TransferStatus::WaitingBroadcast, false),
            (TransferStatus::WaitingConfirmations, true),
        ] {
            let raw = route_raw.then(|| raw_tx(&dummy_psbt().0));
            let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(raw)));
            let now = now().unix_timestamp();
            let idx = seed_batch(&wallet, &txid, initial, true, Some(now - 100_000));
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .pop()
                .unwrap()
                .into();
            updated.broadcast_deadline =
                ActiveValue::Set(Some(now - i64::from(CONFIRMATION_MARGIN_SECS) - 10));
            wallet.database.update_batch_transfer(&mut updated).unwrap();
            let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();
            assert_eq!(res[&idx].updated_status, Some(TransferStatus::Failed));
            assert_eq!(status(&wallet, idx), TransferStatus::Failed);
        }
    }

    #[test]
    fn blind_receive_waits_for_the_retention_grace() {
        let (wallet, _) = setup();
        let now = now().unix_timestamp();
        let idx = seed_batch(
            &wallet,
            "t",
            TransferStatus::WaitingCounterparty,
            true,
            Some(now - 100),
        );
        let transfer = wallet.database.iter_transfers().unwrap().pop().unwrap();
        let mut updated_transfer: DbTransferActMod = transfer.clone().into();
        updated_transfer.recipient_pubkey = ActiveValue::Set(Some(recipient_pubkey()));
        wallet
            .database
            .update_transfer(&mut updated_transfer)
            .unwrap();
        // the recipient proxy is unavailable
        wallet
            .save_transfer_transport_endpoint(
                transfer.idx,
                &LocalTransportEndpoint {
                    transport_type: TransportType::JsonRpc,
                    endpoint: ENDPOINT.to_string(),
                    used: false,
                    usable: true,
                },
            )
            .unwrap();
        let set = |expiration: i64, retention_deadline: i64| {
            let mut updated: DbBatchTransferActMod = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .pop()
                .unwrap()
                .into();
            updated.expiration = ActiveValue::Set(Some(expiration));
            updated.retention_deadline = ActiveValue::Set(Some(retention_deadline));
            updated.operation_secret_key = ActiveValue::Set(Some(OPERATION_SEED.to_vec()));
            wallet.database.update_batch_transfer(&mut updated).unwrap();
        };
        let check = || {
            let db_data = wallet.database.get_db_data(false).unwrap();
            let batch = db_data
                .batch_transfers
                .iter()
                .find(|bt| bt.idx == idx)
                .unwrap();
            block_on(wallet.blind_receive_may_be_failed(batch, &db_data)).unwrap()
        };
        // the invoice expired, but not on the clocks of the proxies
        set(now - 100, now + 1000);
        assert!(!check());
        // expired beyond the skew margin, the proxy cannot answer before the retention deadline
        set(now - 400, now + 1000);
        assert!(!check());
        // past the retention deadline, but not beyond the skew margin
        set(now - 1000, now - 200);
        assert!(!check());
        // past the retention deadline beyond the skew margin
        set(now - 1000, now - 400);
        assert!(check());
    }

    #[test]
    fn blind_receive_waits_for_the_proxy() {
        let (wallet, _) = setup();
        let now = now().unix_timestamp();
        let mut server = mockito::Server::new();
        let idx = seed_batch(
            &wallet,
            "t",
            TransferStatus::WaitingCounterparty,
            true,
            Some(now - 400),
        );
        let mut updated: DbBatchTransferActMod = wallet
            .database
            .iter_batch_transfers()
            .unwrap()
            .pop()
            .unwrap()
            .into();
        updated.retention_deadline = ActiveValue::Set(Some(now + 1000));
        updated.operation_secret_key = ActiveValue::Set(Some(OPERATION_SEED.to_vec()));
        wallet.database.update_batch_transfer(&mut updated).unwrap();
        let transfer = wallet.database.iter_transfers().unwrap().pop().unwrap();
        let mut updated_transfer: DbTransferActMod = transfer.clone().into();
        updated_transfer.recipient_pubkey = ActiveValue::Set(Some(recipient_pubkey()));
        wallet
            .database
            .update_transfer(&mut updated_transfer)
            .unwrap();
        wallet
            .save_transfer_transport_endpoint(
                transfer.idx,
                &LocalTransportEndpoint {
                    transport_type: TransportType::JsonRpc,
                    endpoint: server.url(),
                    used: false,
                    usable: true,
                },
            )
            .unwrap();
        let check = |wallet: &Wallet| {
            let db_data = wallet.database.get_db_data(false).unwrap();
            let batch = db_data
                .batch_transfers
                .iter()
                .find(|bt| bt.idx == idx)
                .unwrap();
            block_on(wallet.blind_receive_may_be_failed(batch, &db_data)).unwrap()
        };
        // a transfer to the invoice is pending on the proxy
        let pending = rpc_mock(
            &mut server,
            "transfer.list",
            r#"{"jsonrpc":"2.0","id":null,"result":{"transfer":"pending"}}"#,
        );
        assert!(!check(&wallet));
        pending.remove();
        // nothing is pending
        let _empty = rpc_mock(
            &mut server,
            "transfer.list",
            r#"{"jsonrpc":"2.0","id":null,"result":{}}"#,
        );
        assert!(check(&wallet));
    }

    #[test]
    fn transport_endpoint_kinds() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut sender_proxy = mockito::Server::new();
        let ack = rpc_mock(&mut sender_proxy, "ack.get", ACK_GET_EMPTY);
        let now = now().unix_timestamp();
        let idx = seed_outgoing(
            &wallet,
            &txid,
            TransferStatus::WaitingCounterparty,
            Some(now + 1000),
            None,
            &sender_proxy.url(),
            None,
        );
        let transfer = wallet.database.iter_transfers().unwrap().pop().unwrap();
        wallet
            .save_transfer_transport_endpoint(
                transfer.idx,
                &LocalTransportEndpoint {
                    transport_type: TransportType::JsonRpc,
                    endpoint: ENDPOINT.to_string(),
                    used: true,
                    usable: true,
                },
            )
            .unwrap();
        wallet.transfer_artifacts.insert(
            txid,
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );

        // the listing tells the two apart
        let listed = wallet
            .list_transfers(AssetFilter::Any, None)
            .unwrap()
            .into_iter()
            .find(|t| t.batch_transfer_idx == idx)
            .unwrap();
        assert_eq!(listed.transport_endpoints.len(), 1);
        assert_eq!(listed.transport_endpoints[0].endpoint, ENDPOINT);
        assert_eq!(listed.sender_transport_endpoint, Some(sender_proxy.url()));

        // the ACKs are polled on the sender proxy, not on the recipient one
        block_on(wallet.refresh(online, None, vec![], true)).unwrap();
        ack.assert();
    }

    #[test]
    fn refresh_rolls_back_only_the_failed_transfer() {
        let (psbt, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut nack_server = mockito::Server::new();
        let _nack = rpc_mock(
            &mut nack_server,
            "ack.get",
            &ack_get_body(RECIPIENT_SEED, false),
        );
        let mut ack_server = mockito::Server::new();
        let _ack = rpc_mock(
            &mut ack_server,
            "ack.get",
            &ack_get_body(RECIPIENT_SEED, true),
        );
        let expiration = Some(now().unix_timestamp() + 1000);
        let status_before = TransferStatus::WaitingCounterparty;
        let seed = |txid: &str, url: &str| {
            seed_outgoing(&wallet, txid, status_before, expiration, None, url, None)
        };
        let first = seed(&"11".repeat(32), &nack_server.url());
        let second = seed(&txid, &ack_server.url());
        let third = seed(&"22".repeat(32), &nack_server.url());
        wallet.transfer_artifacts.insert(
            txid.clone(),
            TransferArtifacts {
                signed_psbt: Some(psbt),
                ..artifacts(BTreeMap::new())
            },
        );

        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();

        assert!(matches!(
            res[&second].failure,
            Some(Error::FailedBroadcast { .. })
        ));
        assert_eq!(status(&wallet, second), status_before);
        let transfer = batch_transfer_row(&wallet, second);
        assert_eq!(transfer.ack, None);
        assert_eq!(transfer.ack_signature, None);
        assert!(wallet.transfer_artifacts.contains_key(&txid));
        for idx in [first, third] {
            assert_eq!(res[&idx].failure, None);
            assert_eq!(status(&wallet, idx), TransferStatus::Failed);
            assert_eq!(batch_transfer_row(&wallet, idx).ack, Some(false));
        }
    }

    #[test]
    fn refresh_rolls_back_a_receive_and_keeps_its_consignment() {
        let (mut wallet, online) = setup();
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.post", "not valid json");
        let expiration = now().unix_timestamp() + 3600;
        let idx = seed_receive_with_consignment(&mut wallet, &server.url(), expiration);

        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();

        assert!(matches!(res[&idx].failure, Some(Error::Proxy { .. })));
        assert_receive_rolled_back(&wallet, idx);
    }

    #[test]
    fn fail_transfers_expired_send_with_failing_ack() {
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", "not valid json");
        let _clean = rpc_mock(&mut server, "ack.clean", TRUE_BODY);
        let now = now().unix_timestamp();
        let status_before = TransferStatus::WaitingCounterparty;
        let seed = |txid: &str| {
            seed_outgoing(
                &wallet,
                txid,
                status_before,
                Some(now + 1000),
                None,
                &server.url(),
                None,
            )
        };
        let single = seed(&"11".repeat(32));
        let all = seed(&"22".repeat(32));

        // before the expiration the error can be transient
        let res = block_on(wallet.refresh(online.clone(), None, vec![], true)).unwrap();
        assert!(matches!(res[&single].failure, Some(Error::Proxy { .. })));
        assert!(matches!(res[&all].failure, Some(Error::Proxy { .. })));
        let res = block_on(wallet.fail_transfers(online.clone(), Some(single), false, true));
        assert!(matches!(res, Err(Error::Proxy { .. })));
        let res = block_on(wallet.fail_transfers(online.clone(), None, false, true));
        assert_eq!(res, Ok(false));
        assert_eq!(status(&wallet, single), status_before);
        assert_eq!(status(&wallet, all), status_before);

        let expired = now - i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS) - 1;
        set_expiration(&wallet, single, expired);
        set_expiration(&wallet, all, expired);
        let res = block_on(wallet.fail_transfers(online.clone(), Some(single), false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, single), TransferStatus::Failed);
        assert_eq!(status(&wallet, all), status_before);
        let res = block_on(wallet.fail_transfers(online, None, false, true));
        assert_eq!(res, Ok(true));
        assert_eq!(status(&wallet, all), TransferStatus::Failed);
    }

    #[test]
    fn fail_transfers_expired_receive_with_failing_ack() {
        for all in [false, true] {
            let (mut wallet, online) = setup();
            let mut server = mockito::Server::new();
            let _ack = rpc_mock(&mut server, "ack.post", "not valid json");
            let now = now().unix_timestamp();
            let idx = seed_receive_with_consignment(&mut wallet, &server.url(), now + 3600);
            let target = (!all).then_some(idx);

            // before the expiration the error can be transient
            let res = block_on(wallet.fail_transfers(online.clone(), target, false, true));
            if all {
                assert_eq!(res, Ok(false));
            } else {
                assert!(matches!(res, Err(Error::Proxy { .. })));
            }
            assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
            assert_receive_rolled_back(&wallet, idx);

            set_expiration(&wallet, idx, now - 1);
            let res = block_on(wallet.fail_transfers(online, target, false, true));
            assert_eq!(res, Ok(true));
            assert_eq!(status(&wallet, idx), TransferStatus::Failed);
            assert_eq!(ttes_used(&wallet, idx), vec![false]);
        }
    }

    // the ACKs stored by the refresh tell that the TX was released
    #[test]
    fn fail_transfers_keeps_acks_of_a_failed_broadcast() {
        for rejection in ["", "min relay fee not met"] {
            let (psbt, txid) = dummy_psbt();
            let route = esplora_route(None);
            let url = fake_esplora(move |path| {
                if path.ends_with("/tx") {
                    return ("400 Bad Request", rejection.as_bytes().to_vec());
                }
                route(path)
            });
            let (mut wallet, online) = setup_at(&url);
            let mut server = mockito::Server::new();
            let _ack = rpc_mock(&mut server, "ack.get", &ack_get_body(RECIPIENT_SEED, true));
            let expired = now().unix_timestamp() - i64::from(PROXY_CLOCK_SKEW_MARGIN_SECS) - 1;
            let idx = seed_outgoing(
                &wallet,
                &txid,
                TransferStatus::WaitingCounterparty,
                Some(expired),
                None,
                &server.url(),
                None,
            );
            wallet.transfer_artifacts.insert(
                txid,
                TransferArtifacts {
                    signed_psbt: Some(psbt),
                    ..artifacts(BTreeMap::new())
                },
            );

            let res = block_on(wallet.fail_transfers(online, None, false, true));

            assert_eq!(res, Ok(false));
            assert_eq!(status(&wallet, idx), TransferStatus::WaitingCounterparty);
            assert_eq!(batch_transfer_row(&wallet, idx).ack, Some(true));
        }
    }

    // the refresh fails a send that lost its signed TX, and the fail must not be rolled back
    #[test]
    fn fail_transfers_fails_a_send_missing_its_signed_tx() {
        let (_, txid) = dummy_psbt();
        let (mut wallet, online) = setup_at(&fake_esplora(esplora_route(None)));
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(&mut server, "ack.get", &ack_get_body(RECIPIENT_SEED, true));
        let expiration = Some(now().unix_timestamp() + 1000);
        let status_before = TransferStatus::WaitingCounterparty;
        let idx = seed_outgoing(
            &wallet,
            &txid,
            status_before,
            expiration,
            None,
            &server.url(),
            None,
        );

        let res = block_on(wallet.fail_transfers(online.clone(), Some(idx), false, true));

        assert!(matches!(res, Err(Error::MissingTransferArtifacts { .. })));
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);

        // a refresh keeps this fail too
        let idx = seed_outgoing(
            &wallet,
            &"22".repeat(32),
            status_before,
            expiration,
            None,
            &server.url(),
            None,
        );

        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();

        assert_eq!(res[&idx].updated_status, Some(TransferStatus::Failed));
        assert!(matches!(
            res[&idx].failure,
            Some(Error::MissingTransferArtifacts { .. })
        ));
        assert_eq!(status(&wallet, idx), TransferStatus::Failed);
    }

    // a witness receive whose consignment is refused releases the TXO that paid it
    #[test]
    fn refusing_a_consignment_clears_pending_witness_txo() {
        let (mut wallet, online) = setup();
        let mut server = mockito::Server::new();
        let _ack = rpc_mock(
            &mut server,
            "ack.post",
            r#"{"jsonrpc":"2.0","id":null,"result":{"result":true}}"#,
        );
        let idx = seed_receive_with_consignment(
            &mut wallet,
            &server.url(),
            now().unix_timestamp() + 3600,
        );
        let script = new_script(&mut wallet);
        let rid = recipient_id_from_script_buf(script.clone(), BitcoinNetwork::Regtest);
        let mut transfer: DbTransferActMod = batch_transfer_row(&wallet, idx).into();
        transfer.recipient_type = ActiveValue::Set(Some(RecipientTypeFull::Witness { vout: None }));
        transfer.recipient_id = ActiveValue::Set(Some(rid.clone()));
        wallet.database.update_transfer(&mut transfer).unwrap();
        let consignment = wallet.received_consignments.remove("recipient").unwrap();
        wallet
            .received_consignments
            .insert(rid.clone(), consignment);
        let meta = wallet
            .received_consignment_meta
            .remove("recipient")
            .unwrap();
        wallet.received_consignment_meta.insert(rid, meta);
        let txid = apply_tx_paying(&mut wallet, script);
        let paid_txo = seed_pending_witness_txo(&wallet, &txid, 0);
        let unrelated_txo = seed_pending_witness_txo(&wallet, &"22".repeat(32), 0);

        let res = block_on(wallet.refresh(online, None, vec![], true)).unwrap();

        assert_eq!(res[&idx].updated_status, Some(TransferStatus::Failed));
        assert!(!pending_witness(&wallet, paid_txo));
        assert!(pending_witness(&wallet, unrelated_txo));
    }

    #[test]
    fn fail_clears_pending_witness_txo() {
        let (mut wallet, _) = setup();
        let status_before = TransferStatus::WaitingCounterparty;

        // the TXO an accepted consignment registered
        let txid = "11".repeat(32);
        let script = new_script(&mut wallet);
        let by_outpoint = seed_witness_receive(&wallet, &txid, status_before, Some(0), &script);
        let outpoint_txo = seed_pending_witness_txo(&wallet, &txid, 0);

        // the TXO a sync flagged before any consignment
        let script = new_script(&mut wallet);
        let by_script = seed_witness_receive(&wallet, "", status_before, None, &script);
        let txid = apply_tx_paying(&mut wallet, script);
        let script_txo = seed_pending_witness_txo(&wallet, &txid, 0);

        let unrelated_txo = seed_pending_witness_txo(&wallet, &"22".repeat(32), 0);

        for idx in [by_outpoint, by_script] {
            let batch_transfer = wallet
                .database
                .iter_batch_transfers()
                .unwrap()
                .into_iter()
                .find(|bt| bt.idx == idx)
                .unwrap();
            wallet._fail_batch_transfer(&batch_transfer).unwrap();
        }

        assert!(!pending_witness(&wallet, outpoint_txo));
        assert!(!pending_witness(&wallet, script_txo));
        assert!(pending_witness(&wallet, unrelated_txo));
    }

    #[test]
    fn reconcile_follows_the_witness_receive_status() {
        let (mut wallet, _) = setup();
        let mut scripts = vec![];
        let mut txos = vec![];
        for (i, status) in [
            TransferStatus::WaitingConfirmations,
            TransferStatus::Failed,
            TransferStatus::Settled,
        ]
        .into_iter()
        .enumerate()
        {
            let txid = format!("{}{}", i + 1, i + 1).repeat(32);
            let script = new_script(&mut wallet);
            let idx = seed_witness_receive(&wallet, &txid, status, Some(0), &script);
            let txo_idx = seed_pending_witness_txo(&wallet, &txid, 0);
            wallet
                .database
                .set_coloring(DbColoringActMod {
                    txo_idx: ActiveValue::Set(txo_idx),
                    asset_transfer_idx: ActiveValue::Set(
                        batch_transfer_row(&wallet, idx).asset_transfer_idx,
                    ),
                    r#type: ActiveValue::Set(ColoringType::Receive),
                    assignment: ActiveValue::Set(Assignment::Fungible(1)),
                    ..Default::default()
                })
                .unwrap();
            scripts.push(script.to_hex_string());
            txos.push(txo_idx);
        }
        // a receive paid before its consignment arrives has a TXO with no coloring
        let script = new_script(&mut wallet);
        seed_witness_receive(
            &wallet,
            "",
            TransferStatus::WaitingCounterparty,
            None,
            &script,
        );
        let txid = apply_tx_paying(&mut wallet, script);
        let paid_txo = seed_pending_witness_txo(&wallet, &txid, 0);
        // the script of a deleted receive has no transfer
        let deleted = new_script(&mut wallet).to_hex_string();
        for script in scripts.iter().chain([&deleted]) {
            wallet
                .database
                .set_pending_witness_script(DbPendingWitnessScriptActMod {
                    script: ActiveValue::Set(script.clone()),
                    ..Default::default()
                })
                .unwrap();
        }

        let kept = wallet._reconcile_pending_witness().unwrap();

        assert_eq!(kept, HashSet::from([scripts[0].clone()]));
        let rows: Vec<String> = wallet
            .database
            .iter_pending_witness_scripts()
            .unwrap()
            .into_iter()
            .map(|s| s.script)
            .collect();
        assert_eq!(rows, vec![scripts[0].clone()]);
        assert!(pending_witness(&wallet, txos[0]));
        assert!(pending_witness(&wallet, paid_txo));
        assert!(!pending_witness(&wallet, txos[1]));
        assert!(!pending_witness(&wallet, txos[2]));
    }
}

#[cfg(test)]
mod bfa_tests {
    use super::*;

    #[test]
    fn mint_right_is_not_an_input_of_a_fungible_send() {
        let mint = Opout::new(OpId::from([1u8; 32]), OS_MINT, 0);
        let needed = AssignmentsCollection {
            fungible: 30,
            ..Default::default()
        };
        // the send moves it as an extra allocation
        assert!(!AssignmentsCollection::default().opout_contributes(
            &mint,
            &AllocatedState::Void,
            &needed
        ));
        assert_eq!(
            Assignment::from_opout_and_state(mint, &AllocatedState::Void),
            Assignment::MintRight
        );
    }

    #[test]
    fn issue_asset_bfa_takes_unicode_ticker() {
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;

        let mut wallet = make_test_wallet(false);
        wallet.wallet_data.supported_schemas = vec![AssetSchema::Nia, AssetSchema::Bfa];
        let issue = |ticker: String| {
            wallet.issue_asset_bfa(
                ticker,
                s!("Name"),
                0,
                1,
                31337,
                s!("0x5FbDB2315678afecb367f032d93F642f64180aa3"),
                None,
            )
        };
        assert!(matches!(
            issue(s!("")),
            Err(Error::InvalidTicker { details: m }) if m == "ticker must contain at least one character."
        ));
        assert!(matches!(
            issue("₿TC".repeat(3)),
            Err(Error::InvalidTicker { details: m }) if m == "ticker has 9 characters, while at most 8 are allowed."
        ));
        // a valid unicode ticker passes the check: the wallet has no UTXO, so the issuance fails later
        assert!(!matches!(
            issue(s!("₿TC")),
            Err(Error::InvalidTicker { .. })
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn evm_reject_list_rejects_descendants_of_rejected_opout() {
        use crate::api::evm::tests::*;
        use crate::api::evm::{REJECT_LIST_ENTRIES_SELECTOR, REJECT_LIST_LENGTH_SELECTOR};
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;

        // entry 1 rejects, entry 2 allows, both finalized
        let mut server = mockito::Server::new();
        let length = format!("0x{:064x}", 2);
        let zero = EvmAddress::default().to_string();
        let _mocks = [
            mock_rpc(
                &mut server,
                r#"{"method":"eth_blockNumber"}"#,
                r#""result":"0x10""#,
            ),
            mock_finalized_block(&mut server, 8),
            mock_eth_call(&mut server, &zero, 8, REJECT_LIST_LENGTH_SELECTOR, &length),
            mock_eth_call(
                &mut server,
                &zero,
                0x10,
                REJECT_LIST_LENGTH_SELECTOR,
                &length,
            ),
            mock_eth_call(
                &mut server,
                &zero,
                0x10,
                &format!("{REJECT_LIST_ENTRIES_SELECTOR}{:064x}{:064x}", 0, 2),
                &encode_entries(&[(1, true), (2, false)]),
            ),
        ];
        let mut wallet = make_test_wallet(false);
        let mut online_data = OnlineData::for_test(&server.url());
        online_data.evm_chains = HashMap::from([(
            31337,
            EvmChain {
                rpc_url: server.url(),
                min_mint_lock_blocks: 2,
                max_mint_lock_blocks: 10,
            },
        )]);
        wallet.online_data = Some(online_data);
        let source = RejectListSource::Evm(EvmContract {
            chain_id: 31337,
            address: Default::default(),
        });
        let (reject, allow) = block_on(wallet._get_reject_list_async(&source)).unwrap();
        assert_eq!(reject, HashSet::from([test_opout(1)]));
        assert_eq!(allow, HashSet::from([test_opout(2)]));

        // 1 -> 3 -> 5 and 1 -> 2 -> 4: the allowed opout 2 shields 4 but not 3 and 5
        let mut dag = rgbstd::validation::OpsDag::new();
        for b in 1..=5 {
            dag.register_op(test_opout(b).op);
        }
        for (parent, child) in [(1, 3), (1, 2), (2, 4), (3, 5)] {
            dag.connect_input(test_opout(parent), test_opout(child).op);
        }
        let check = |opouts: &[u8]| {
            wallet
                ._check_dag(
                    &dag,
                    &reject,
                    &allow,
                    &opouts.iter().map(|b| test_opout(*b)).collect(),
                )
                .unwrap()
        };
        assert_eq!(check(&[3, 4]), HashSet::from([test_opout(3)]));
        assert_eq!(check(&[5]), HashSet::from([test_opout(5)]));
        assert_eq!(check(&[1]), HashSet::from([test_opout(1)]));
        assert_eq!(check(&[2]), HashSet::new());

        // a chain with no URL gives an error, not an empty list
        wallet.online_data.as_mut().unwrap().evm_chains.clear();
        assert!(matches!(
            block_on(wallet._get_reject_list_async(&source)),
            Err(Error::NoEvmRpcUrl { chain_id: 31337 })
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn url_reject_list_parse() {
        use crate::api::evm::tests::{block_on, test_opout};
        use crate::wallet::offline::address_reuse_tests::make_test_wallet;

        // lines are trimmed, empty lines are skipped and invalid lines are ignored
        let mut server = mockito::Server::new();
        let body = format!(
            "  {}\r\n\n\t!{} \nnot an opout\n# comment\n",
            test_opout(1),
            test_opout(2)
        );
        let _mock = server.mock("GET", "/").with_body(body).create();
        let mut wallet = make_test_wallet(false);
        wallet.online_data = Some(OnlineData::for_test(&server.url()));
        let source = RejectListSource::Url(server.url());
        let (reject, allow) = block_on(wallet._get_reject_list_async(&source)).unwrap();
        assert_eq!(reject, HashSet::from([test_opout(1)]));
        assert_eq!(allow, HashSet::from([test_opout(2)]));
    }

    #[test]
    fn many_rights_on_utxo() {
        // a UTXO can receive more mint rights than an issuance can create (u8::MAX)
        let num_rights = u8::MAX as u64 + 2;
        let opid = OpId::from([0u8; 32]);
        let mut from_opouts = AssignmentsCollection::default();
        let mut from_allocations = AssignmentsCollection::default();
        for no in 0..num_rights {
            let opout = Opout::new(opid, OS_MINT, no as u16);
            from_opouts.add_opout_state(&opout, &AllocatedState::Void);
            Assignment::MintRight.add_to_assignments(&mut from_allocations);
        }
        assert_eq!(from_opouts.mint, num_rights);
        assert_eq!(from_allocations.mint, num_rights);

        let needed = AssignmentsCollection {
            mint: 1,
            ..Default::default()
        };
        assert!(from_opouts.enough(&needed));
        assert_eq!(from_opouts.change(&needed).mint, num_rights - 1);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn burn_refuses_invalid_input() {
        use crate::api::evm::tests::block_on;
        use crate::database::memory_db::{ActiveValue, DbAssetActMod};

        let mut wallet = crate::wallet::offline::address_reuse_tests::make_test_wallet(false);
        wallet.wallet_data.supported_schemas = vec![AssetSchema::Nia, AssetSchema::Ifa];
        let url = "http://127.0.0.1:1";
        wallet.online_data = Some(OnlineData::for_test(url));
        let online = Online {
            id: 1,
            indexer_url: url.to_string(),
        };
        for (id, schema) in [("rgb:ifa", AssetSchema::Ifa), ("rgb:nia", AssetSchema::Nia)] {
            wallet
                .database
                .set_asset(DbAssetActMod {
                    id: ActiveValue::Set(id.to_string()),
                    schema: ActiveValue::Set(schema),
                    ..Default::default()
                })
                .unwrap();
        }
        let cases: [(&str, u64, Option<String>, fn(&Error) -> bool); 4] = [
            ("rgb:ifa", 0, None, |e| *e == Error::NoBurnAmount),
            ("rgb:ifa", 10, Some(s!("zz")), |e| {
                matches!(e, Error::InvalidBurnReason { .. })
            }),
            ("rgb:unknown", 10, None, |e| {
                matches!(e, Error::AssetNotFound { .. })
            }),
            ("rgb:nia", 10, None, |e| {
                *e == Error::UnsupportedBurn {
                    asset_schema: AssetSchema::Nia,
                }
            }),
        ];
        for (id, amount, reason, expected) in cases {
            let err = block_on(wallet.burn(online.clone(), id.to_string(), amount, reason, 1, 1))
                .unwrap_err();
            assert!(expected(&err), "{err:?}");
        }

        wallet.watch_only = true;
        assert!(matches!(
            block_on(wallet.burn(online, s!("rgb:ifa"), 10, None, 1, 1)),
            Err(Error::WatchOnly)
        ));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn mint_begin_refuses_invalid_input() {
        use crate::api::evm::tests::block_on;
        use crate::database::memory_db::{ActiveValue, DbAssetActMod};

        let mut wallet = crate::wallet::offline::address_reuse_tests::make_test_wallet(false);
        wallet.wallet_data.supported_schemas = vec![AssetSchema::Nia, AssetSchema::Bfa];
        let url = "http://127.0.0.1:1";
        wallet.online_data = Some(OnlineData::for_test(url));
        let online = Online {
            id: 1,
            indexer_url: url.to_string(),
        };
        for (id, schema) in [("rgb:bfa", AssetSchema::Bfa), ("rgb:nia", AssetSchema::Nia)] {
            wallet
                .database
                .set_asset(DbAssetActMod {
                    id: ActiveValue::Set(id.to_string()),
                    schema: ActiveValue::Set(schema),
                    ..Default::default()
                })
                .unwrap();
        }
        let endpoint = "rpc://127.0.0.1:3000/json-rpc";
        let cases: [(&str, Assignment, &str, fn(&Error) -> bool); 4] = [
            ("rgb:nia", Assignment::Fungible(10), endpoint, |e| {
                *e == Error::UnsupportedMint {
                    asset_schema: AssetSchema::Nia,
                }
            }),
            ("rgb:bfa", Assignment::Fungible(0), endpoint, |e| {
                *e == Error::InvalidAmountZero
            }),
            ("rgb:bfa", Assignment::InflationRight(10), endpoint, |e| {
                *e == Error::InvalidAssignment
            }),
            (
                "rgb:bfa",
                Assignment::Fungible(10),
                "not an endpoint",
                |e| matches!(e, Error::InvalidTransportEndpoint { .. }),
            ),
        ];
        for (id, assignment, sender_endpoint, expected) in cases {
            let recipient = Recipient {
                recipient_id: s!("recipient"),
                witness_data: None,
                assignment,
                transport_endpoints: vec![endpoint.to_string()],
                public_key: None,
                encryption: Encryption::Required,
                expiration_timestamp: None,
            };
            let err = block_on(wallet.mint_begin(
                online.clone(),
                id.to_string(),
                recipient,
                1000,
                1,
                1,
                sender_endpoint.to_string(),
            ))
            .unwrap_err();
            assert!(expected(&err), "{err:?}");
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn go_online_refuses_node_without_mint_support() {
        use crate::api::evm::tests::{block_on, mock_finalized_block, mock_rpc};

        // the node serves the chain and the finalized block, but not the state at it
        let mut server = mockito::Server::new();
        let _mocks = [
            mock_rpc(
                &mut server,
                r#"{"method":"eth_chainId"}"#,
                r#""result":"0x7a69""#,
            ),
            mock_finalized_block(&mut server, 0x10),
            mock_rpc(
                &mut server,
                r#"{"method":"eth_call","params":[{"data":"0x"},"0x10"]}"#,
                r#""error":{"code":-32000,"message":"missing trie node"}"#,
            ),
        ];
        let mut wallet = crate::wallet::offline::address_reuse_tests::make_test_wallet(false);
        wallet.wallet_data.supported_schemas = vec![AssetSchema::Nia, AssetSchema::Bfa];
        let result = block_on(wallet.go_online(OnlineOptions {
            indexer_config: IndexerConfig::Esplora {
                url: s!("http://127.0.0.1:1"),
            },
            skip_consistency_check: false,
            proxy_auth_keys: HashMap::new(),
            evm_chains: HashMap::from([(
                31337,
                EvmChain {
                    rpc_url: server.url(),
                    min_mint_lock_blocks: 2,
                    max_mint_lock_blocks: 10,
                },
            )]),
        }));
        assert!(
            matches!(result, Err(Error::InvalidEvmRpcUrl { ref details })
                if details.starts_with("cannot read the state")
                    && details.ends_with("missing trie node")),
            "{result:?}"
        );
        assert!(wallet.online_data.is_none());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn check_burn_reason_wants_32_hex_bytes() {
        let wallet = crate::wallet::offline::address_reuse_tests::make_test_wallet(false);
        let reason = "ab".repeat(32);
        assert_eq!(
            wallet
                .check_burn_reason(reason.clone())
                .unwrap()
                .to_string(),
            reason
        );
        assert!(matches!(
            wallet.check_burn_reason(s!("zz")),
            Err(Error::InvalidBurnReason { details }) if details.starts_with("not a hex string")
        ));
        for len in [31, 33] {
            assert!(matches!(
                wallet.check_burn_reason("ab".repeat(len)),
                Err(Error::InvalidBurnReason { details })
                    if details == format!("expected 32 bytes, got {len}")
            ));
        }
    }

    #[test]
    fn online_options_read_evm_chains() {
        let options = |urls: &str| {
            serde_json::from_str::<OnlineOptions>(&format!(
                r#"{{"indexer_config":{{"Esplora":{{"url":"u"}}}},"skip_consistency_check":true,
                "proxy_auth_keys":{{}}{urls}}}"#
            ))
        };
        let chain =
            r#"{"rpc_url":"http://evm","min_mint_lock_blocks":2,"max_mint_lock_blocks":10}"#;
        assert_eq!(
            options(&format!(r#","evm_chains":{{"31337":{chain}}}"#))
                .unwrap()
                .evm_chains,
            HashMap::from([(
                31337,
                EvmChain {
                    rpc_url: s!("http://evm"),
                    min_mint_lock_blocks: 2,
                    max_mint_lock_blocks: 10,
                }
            )])
        );
        assert!(options("").unwrap().evm_chains.is_empty());
        assert!(options(&format!(r#","evm_chains":{{"mainnet":{chain}}}"#)).is_err());
    }
}

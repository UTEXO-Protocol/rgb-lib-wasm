//! Inspection.
//!
//! This module defines the consignment and fascia inspections, which don't require a wallet,
//! and the objects they return.

use super::*;

/// An operation output, which RGB identifies as `<operation ID>/<assignment type>/<index>`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Opout {
    /// ID of the operation the output belongs to
    pub opid: String,
    /// Type of the assignment the output belongs to
    pub assignment_type: TypeOfAssignment,
    /// Index of the output among the ones of its assignment type
    pub index: u16,
}

impl TryFrom<RgbOpout> for Opout {
    /// The assignment type of the operation output, when it's not one of the supported schemas'
    type Error = AssignmentType;

    fn try_from(opout: RgbOpout) -> Result<Self, Self::Error> {
        Ok(Self {
            opid: opout.op.to_string(),
            assignment_type: TypeOfAssignment::from_assignment_type(opout.ty).ok_or(opout.ty)?,
            index: opout.no,
        })
    }
}

/// Information about an RGB input being spent.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbInputInfo {
    /// The operation output the input spends, which the bundle of the spending transition commits
    /// to in its input map
    pub opout: Opout,
    /// The index of the transaction input spending the allocation (`None` when the allocation
    /// can't be resolved, as inspecting a fascia without a wallet)
    pub vin: Option<u32>,
    /// The assignment in input, the state it carries included (`None` when the spent allocation
    /// isn't resolved, as inspecting a fascia without a wallet: the opout gives the type of the
    /// assignment, not the state the input spends)
    pub assignment: Option<Assignment>,
}

/// Information about an RGB output destination.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbOutputInfo {
    /// The output TXID (if the seal is revealed)
    pub txid: Option<String>,
    /// The output vout (if the seal is revealed)
    pub vout: Option<u32>,
    /// The blinding of the output seal (if the seal is revealed), telling apart allocations on the
    /// same vout
    pub blinding: Option<u64>,
    /// The assignment in output
    pub assignment: Assignment,
    /// Whether this output is allocated to a concealed seal
    pub is_concealed: bool,
    /// The recipient ID of the blinded receive the output is allocated to (if the seal is
    /// concealed)
    pub recipient_id: Option<String>,
    /// Whether this output belongs to the wallet (`None` when inspecting without a wallet)
    pub is_ours: Option<bool>,
    /// Whether this output belongs to a terminal operation of the consignment, one nothing in the
    /// consignment builds on, so to what the consignment proves rather than to its history (`None`
    /// when not inspecting a consignment)
    pub is_terminal: Option<bool>,
}

/// A global state entry committed in an RGB transition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransitionGlobal {
    /// Amount of newly issued supply, committed in inflate transitions
    IssuedSupply(u64),
    /// Amount of burned asset, committed in burn transitions
    BurnedAsset(u64),
    /// Amount of burned inflation rights, committed in burn transitions
    BurnedInflation(u64),
    /// Hex-encoded 32-byte burn reason, committed in burn transitions
    BurnReason(String),
    /// ID of the contract the asset is linked to, committed in link transitions
    LinkedToContract(String),
}

/// A metadata entry committed in an RGB transition, read by its validation but not part of the
/// contract state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransitionMetadata {
    /// Amount of inflation rights allowed in output, committed in IFA inflate transitions
    AllowedInflation(u64),
    /// External chain block before which the lock backing the mint has to be included, committed
    /// in BFA mint transitions
    BeforeBlock(u64),
}

/// Information about an RGB transition.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbTransitionInfo {
    /// The transition type
    pub r#type: TypeOfTransition,
    /// ID of the operation
    pub opid: String,
    /// Details of the transition inputs
    pub inputs: Vec<RgbInputInfo>,
    /// Details of the transition outputs
    pub outputs: Vec<RgbOutputInfo>,
    /// Global state entries committed in the transition
    pub globals: Vec<TransitionGlobal>,
    /// Metadata entries committed in the transition
    pub metadata: Vec<TransitionMetadata>,
}

/// Information about an RGB operation.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbOperationInfo {
    /// ID of the asset being operated on
    pub asset_id: String,
    /// Number of asset transitions committed by the witness transaction, which can be more than
    /// the inspected ones
    pub committed_transition_count: u32,
    /// Number of asset inputs spent by the transitions committed by the witness transaction
    pub committed_input_count: u32,
    /// Details of the inspected asset transitions committed by the witness transaction
    pub transitions: Vec<RgbTransitionInfo>,
}

impl RgbOperationInfo {
    /// Return whether the witness transaction commits asset transitions that are not part of the
    /// inspected ones, meaning the operation contains concealed transitions.
    pub fn has_concealed_transitions(&self) -> bool {
        self.committed_transition_count as usize != self.transitions.len()
    }
}

/// Result of inspecting the RGB operations committed by a witness transaction.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbInspection {
    /// ID of the witness transaction
    pub txid: String,
    /// The close method used for the operation
    pub close_method: CloseMethod,
    /// The RGB commitment in hex encoded format
    pub commitment_hex: String,
    /// Details of each operation
    pub operations: Vec<RgbOperationInfo>,
}

impl RgbInspection {
    /// Return whether any of the inspected operations contains concealed transitions.
    pub fn has_concealed_transitions(&self) -> bool {
        self.operations
            .iter()
            .any(RgbOperationInfo::has_concealed_transitions)
    }
}

/// Information about a media file attached to an RGB asset.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbMediaInfo {
    /// Hex-encoded digest of the media file
    pub digest: String,
    /// MIME type of the media file
    pub mime: String,
}

impl From<&Attachment> for RgbMediaInfo {
    fn from(attachment: &Attachment) -> Self {
        Self {
            digest: hex::encode(attachment.digest),
            mime: attachment.ty.to_string(),
        }
    }
}

/// A smart contract deployed on an EVM-compatible chain, as referenced by RGB contract state.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbEvmContractInfo {
    /// EIP-155 chain ID of the chain the contract is deployed on
    pub chain_id: u64,
    /// Address of the contract, hex-encoded with the `0x` prefix
    pub address: String,
}

impl From<&EvmContract> for RgbEvmContractInfo {
    fn from(contract: &EvmContract) -> Self {
        Self {
            chain_id: contract.chain_id,
            address: contract.address.to_string(),
        }
    }
}

/// Where the bridge contract backing an asset is deployed, committed in its genesis.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum RgbBridgeLocationInfo {
    /// A bridge contract deployed on an EVM-compatible chain, whose chain ID a claim resolver has
    /// to check against the chain it's connected to
    Evm(RgbEvmContractInfo),
}

impl From<BridgeLocation> for RgbBridgeLocationInfo {
    fn from(location: BridgeLocation) -> Self {
        match location {
            BridgeLocation::Evm(contract) => Self::Evm((&contract).into()),
        }
    }
}

/// Where the reject list of an asset is published, committed in its genesis.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum RgbRejectListInfo {
    /// A list of rejected opouts served at a URL
    Url(String),
    /// A list of rejected opouts kept by an `IRgbRejectList` contract deployed on an
    /// EVM-compatible chain
    Evm(RgbEvmContractInfo),
}

impl From<RejectListLocation> for RgbRejectListInfo {
    fn from(location: RejectListLocation) -> Self {
        match location {
            RejectListLocation::Url(url) => Self::Url(url.to_string()),
            RejectListLocation::Evm(contract) => Self::Evm((&contract).into()),
        }
    }
}

/// Information about the genesis of an RGB asset.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RgbGenesisInfo {
    /// Chain and network the asset is issued on, as in RGB invoices (e.g. `bc` for Bitcoin mainnet,
    /// `bcrt` for Bitcoin regtest)
    pub chain_net: String,
    /// Issuance timestamp
    pub timestamp: i64,
    /// Issuer identity
    pub issuer: String,
    /// Asset name
    pub name: String,
    /// Asset ticker
    pub ticker: Option<String>,
    /// Asset details
    pub details: Option<String>,
    /// Asset precision
    pub precision: u8,
    /// Supply issued by the genesis itself, committed by the schemas that allow an initial
    /// issuance (`None` for the BFA schema, whose supply all comes from mints). Later issuances
    /// commit their own amount, so this is the initial supply, not the current one (see
    /// [`TransitionGlobal::IssuedSupply`])
    pub issued_supply: Option<u64>,
    /// Maximum supply the asset can be inflated to, committed by the IFA schema (`None` for the
    /// other schemas: a BFA asset has no such cap, its supply being bounded by the tokens locked
    /// on the bridge contract)
    pub max_supply: Option<u64>,
    /// ID of the contract the asset is linked from, if any (only committed by the BFA and IFA
    /// schemas; see [`TransitionGlobal::LinkedToContract`] for the other end of the link)
    pub linked_from_contract: Option<String>,
    /// Where the bridge contract backing the asset is deployed, which the BFA schema requires
    /// (`None` for the other schemas)
    pub bridge_location: Option<RgbBridgeLocationInfo>,
    /// Where the reject list of the asset is published, if it declares one (only committed by the
    /// BFA and IFA schemas, the latter supporting a URL only)
    pub reject_list: Option<RgbRejectListInfo>,
    /// Media files of the asset: the contract terms media
    pub media: Vec<RgbMediaInfo>,
    /// Details of the issuance outputs
    pub outputs: Vec<RgbOutputInfo>,
}

/// Result of inspecting an RGB consignment.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct ConsignmentInspection {
    /// ID of the consignment
    pub consignment_id: String,
    /// Version of the consignment
    pub version: u8,
    /// Whether this is a transfer consignment (`false` for a contract consignment)
    pub is_transfer: bool,
    /// ID of the asset
    pub asset_id: String,
    /// Schema of the asset
    pub asset_schema: AssetSchema,
    /// Details of the asset genesis
    pub genesis: RgbGenesisInfo,
    /// Details of the inspected witness transactions (the ones of the terminal operations or all of
    /// them), in consignment order (none for a contract consignment)
    pub witnesses: Vec<RgbInspection>,
}

/// The bundles of a consignment to inspect.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum BundleFilter {
    /// All the consignment bundles
    All,
    /// The bundles holding a terminal operation of the consignment, one nothing in the consignment
    /// builds on (e.g. the last transfer or a burn)
    Terminals,
    /// The bundle committed by the witness transaction with the given TXID
    Witness(String),
}

/// Inspect an RGB consignment (transfer or contract) given as bytes.
///
/// Shows the asset and the transitions committed by the witness transactions of the bundles the
/// filter selects. Inputs come from the consignment history.
///
/// The witness transactions must commit to the inspected transitions. This does not validate the
/// consignment, and it does not mean the witness transactions are broadcast.
pub fn inspect_consignment(
    consignment_bytes: &[u8],
    bundle_filter: BundleFilter,
) -> Result<ConsignmentInspection, Error> {
    let witness_id = match &bundle_filter {
        BundleFilter::Witness(txid) => Some(Txid::from_str(txid).map_err(|_| Error::InvalidTxid)?),
        BundleFilter::All | BundleFilter::Terminals => None,
    };
    let selected = |txid: Txid, has_terminals: bool| match bundle_filter {
        BundleFilter::All => true,
        BundleFilter::Terminals => has_terminals,
        BundleFilter::Witness(_) => Some(txid) == witness_id,
    };

    let inspection = match UniversalFile::load(consignment_bytes) {
        Ok(UniversalFile::Transfer(consignment)) => consignment_inspection(&consignment, selected),
        Ok(UniversalFile::Contract(consignment)) => consignment_inspection(&consignment, selected),
        Ok(UniversalFile::SchemaDefinition(_)) => Err(Error::InvalidConsignment),
        // validated consignments have their own file type, but they're checked as any other: the
        // file doesn't prove it has been validated
        Err(LoadError::InvalidMagic) => {
            if let Some(consignment) = valid_consignment::<true>(consignment_bytes) {
                consignment_inspection(&consignment, selected)
            } else if let Some(consignment) = valid_consignment::<false>(consignment_bytes) {
                consignment_inspection(&consignment, selected)
            } else {
                Err(Error::InvalidConsignment)
            }
        }
        Err(_) => Err(Error::InvalidConsignment),
    }?;

    if let Some(witness_id) = witness_id
        && inspection.witnesses.is_empty()
    {
        return Err(Error::RgbInspection {
            details: format!("witness {witness_id} not found in the consignment"),
        });
    }
    Ok(inspection)
}

/// Read a validated consignment file (`ValidConsignment::load_file` of rgb-ops) from bytes.
fn valid_consignment<const TRANSFER: bool>(mut data: &[u8]) -> Option<Consignment<TRANSFER>> {
    let mut magic = [0u8; 7];
    data.read_exact(&mut magic).ok()?;
    let expected: &[u8; 3] = if TRANSFER { b"VTF" } else { b"VCO" };
    if &magic[..4] != b"RGB\0" || &magic[4..] != expected {
        return None;
    }
    let consignment =
        Consignment::<TRANSFER>::strict_decode(&mut StrictReader::with(StreamReader::new::<
            FILE_MAX_LEN,
        >(&mut data)))
        .ok()?;
    serde_json::from_slice::<ValidationStatus>(data).ok()?;
    Some(consignment)
}

/// Build the inspection details of a consignment, inspecting the bundles selected by the given
/// function (called with their witness TXID and whether they hold a terminal operation) and
/// checking the witness transactions commit to the inspected transitions.
fn consignment_inspection<const TRANSFER: bool>(
    consignment: &Consignment<TRANSFER>,
    selected: impl Fn(Txid, bool) -> bool,
) -> Result<ConsignmentInspection, Error> {
    let contract_id = consignment.contract_id();
    let asset_schema = AssetSchema::from_schema_id(consignment.schema_id())?;
    let genesis = genesis_info(contract_id, asset_schema, &consignment.genesis)?;

    // index the operations whose assignments transitions can spend
    // an operation with a wrong ID, or in two bundles, is refused
    let genesis_id = consignment.genesis.id();
    let mut transitions: HashMap<OpId, (&Transition, Txid)> = HashMap::new();
    for witness_bundle in consignment.bundles.iter() {
        let witness_id = witness_bundle.witness_id();
        for KnownTransition { opid, transition } in witness_bundle.bundle.known_transitions.iter() {
            if transition.id() != *opid {
                continue;
            }
            if let Some((_, other_witness_id)) = transitions.insert(*opid, (transition, witness_id))
                && other_witness_id != witness_id
            {
                return Err(Error::RgbInspection {
                    details: format!(
                        "transition {opid} in the bundles of witnesses {other_witness_id} and \
                         {witness_id}"
                    ),
                });
            }
        }
    }
    // get the spent outpoint and assignment of a transition input
    let resolve_input = |opout: RgbOpout| -> Option<(OutPoint, Assignment)> {
        if opout.op == genesis_id {
            let typed_assigns = consignment.genesis.assignments.get(&opout.ty)?;
            let seal = typed_assigns.revealed_seal_at(opout.no).ok()??;
            let assignment = Assignment::from_typed_assigns(opout.ty, typed_assigns, opout.no)?;
            Some((seal.to_outpoint(), assignment))
        } else {
            let (transition, witness_id) = transitions.get(&opout.op)?;
            let typed_assigns = transition.assignments.get(&opout.ty)?;
            let seal = typed_assigns.revealed_seal_at(opout.no).ok()??;
            let assignment = Assignment::from_typed_assigns(opout.ty, typed_assigns, opout.no)?;
            Some((seal.resolve(*witness_id).to_outpoint(), assignment))
        }
    };

    // inspect the selected bundles
    let terminal_ops = consignment.terminal_ops();
    let mut witnesses = vec![];
    for witness_bundle in consignment.bundles.iter() {
        let bundle_id = witness_bundle.bundle.bundle_id();
        let txid = witness_bundle.witness_id();
        let has_terminals = witness_bundle
            .bundle
            .known_transitions
            .iter()
            .any(|kt| terminal_ops.contains(&kt.opid));
        if !selected(txid, has_terminals) {
            continue;
        }
        let tx = &witness_bundle.tx;

        // get the commitment, verifying the witness transaction commits to it if requested
        let anchor = &witness_bundle.anchor;
        let commitment =
            anchor
                .convolve(contract_id, bundle_id)
                .map_err(|e| Error::RgbInspection {
                    details: format!("bundle {bundle_id} not committed by witness {txid}: {e}"),
                })?;
        verify_witness_commitment(&anchor.dbc_proof, &commitment, tx)?;

        let operation = bundle_info(
            contract_id,
            Some(asset_schema),
            consignment.genesis.chain_net,
            &witness_bundle.bundle,
            tx,
            true,
            resolve_input,
            |_| None,
            |opid| Some(terminal_ops.contains(opid)),
        )?;

        witnesses.push(RgbInspection {
            txid: txid.to_string(),
            close_method: anchor.dbc_proof.method(),
            commitment_hex: hex::encode(commitment.to_byte_array()),
            operations: vec![operation],
        });
    }

    Ok(ConsignmentInspection {
        consignment_id: consignment.consignment_id().to_string(),
        version: consignment.version as u8,
        is_transfer: consignment.transfer,
        asset_id: contract_id.to_string(),
        asset_schema,
        genesis,
        witnesses,
    })
}

/// Inspect a PSBT and the fascia of its RGB operation, given as bytes, without a wallet.
///
/// The entropy defaults to the one the fascia carries. This does not validate the operations.
pub fn inspect_fascia(
    psbt: String,
    fascia: &[u8],
    entropy: Option<u64>,
    bitcoin_network: BitcoinNetwork,
) -> Result<RgbInspection, Error> {
    let psbt = Psbt::from_str(&psbt)?;
    let fascia = parse_fascia(fascia)?;

    fascia_inspection(
        &psbt,
        &fascia,
        entropy,
        |contract_id, bundle, witness_tx| {
            bundle_info(
                contract_id,
                None,
                bitcoin_network.into(),
                bundle,
                witness_tx,
                false,
                |_| None,
                |_| None,
                |_| None,
            )
        },
    )
}

/// Read a strict encoded fascia.
pub(crate) fn parse_fascia(fascia: &[u8]) -> Result<Fascia, Error> {
    let bytes = Confined::try_from(fascia.to_vec()).map_err(|_| Error::InvalidFascia)?;
    Fascia::from_strict_serialized::<FILE_MAX_LEN>(bytes).map_err(|_| Error::InvalidFascia)
}

/// Inspect each bundle of a fascia with the given function, after checking that the PSBT commits
/// to these bundles only.
pub(crate) fn fascia_inspection(
    psbt: &Psbt,
    fascia: &Fascia,
    entropy: Option<u64>,
    mut bundle_operation: impl FnMut(
        ContractId,
        &TransitionBundle,
        &BdkTransaction,
    ) -> Result<RgbOperationInfo, Error>,
) -> Result<RgbInspection, Error> {
    // verify the fascia's witness ID matches the PSBT's transaction ID
    let fascia_witness_id = fascia.witness_id();
    let witness_id = psbt.unsigned_tx.compute_txid();
    if fascia_witness_id != witness_id {
        return Err(Error::RgbInspection {
            details: format!(
                "fascia witness ID {fascia_witness_id} does not match PSBT TXID {witness_id}"
            ),
        });
    }

    // inspect the fascia bundles and collect the operations and the commitment messages
    let mut messages = BTreeMap::new();
    let mut operations = Vec::new();
    for (contract_id, bundle) in fascia.bundles() {
        operations.push(bundle_operation(*contract_id, bundle, &psbt.unsigned_tx)?);
        messages.insert(
            ProtocolId::from(*contract_id),
            Message::from(bundle.bundle_id()),
        );
    }

    // the merkle block carries the entropy unless it is concealed
    let entropy = match (entropy, fascia.seal_witness().mpc_merkle_block.entropy()) {
        (Some(given), Some(carried)) if given != carried => {
            return Err(Error::RgbInspection {
                details: format!(
                    "the MPC commitment of the fascia was built with entropy {carried}, not the \
                     given {given}"
                ),
            });
        }
        (Some(entropy), _) | (None, Some(entropy)) => entropy,
        (None, None) => {
            return Err(Error::RgbInspection {
                details: "the MPC commitment of the fascia doesn't carry the entropy it was built \
                          with, which then has to be given"
                    .to_string(),
            });
        }
    };

    // the PSBT must commit to these bundles only
    let mut source = MultiSource::with_static_entropy(entropy);
    source.messages = MediumOrdMap::from_checked(messages);
    // a valid fascia always fits in a tree
    let merkle_tree = MerkleTree::try_commit(&source).map_err(|e| Error::RgbInspection {
        details: format!("cannot rebuild the MPC commitment of the fascia bundles: {e}"),
    })?;
    let commitment = merkle_tree.commit_id();
    let dbc_proof = &fascia.seal_witness().dbc_proof;
    verify_witness_commitment(dbc_proof, &commitment, &psbt.unsigned_tx)?;

    Ok(RgbInspection {
        txid: witness_id.to_string(),
        close_method: dbc_proof.method(),
        commitment_hex: hex::encode(commitment.to_byte_array()),
        operations,
    })
}

/// Verify the witness transaction commits to the given MPC commitment with the given DBC proof.
fn verify_witness_commitment(
    dbc_proof: &DbcProof,
    commitment: &Commitment,
    witness_tx: &BdkTransaction,
) -> Result<(), Error> {
    dbc_proof
        .verify(commitment, witness_tx)
        .map_err(|e| Error::RgbInspection {
            details: format!(
                "witness {} doesn't commit to {commitment}: {e}",
                witness_tx.compute_txid()
            ),
        })
}

/// Build the inspection details of the genesis of an asset with the given schema.
fn genesis_info(
    contract_id: ContractId,
    asset_schema: AssetSchema,
    genesis: &Genesis,
) -> Result<RgbGenesisInfo, Error> {
    // decode the first value of a global state type
    fn global<T: StrictDeserialize>(
        contract_id: ContractId,
        genesis: &Genesis,
        global_type: GlobalStateType,
    ) -> Result<Option<T>, Error> {
        genesis
            .globals
            .get(&global_type)
            .and_then(|values| values.first())
            .map(|value| {
                T::from_strict_serialized(value.to_inner()).map_err(|e| Error::RgbInspection {
                    details: format!(
                        "genesis of contract {contract_id}: invalid {global_type} global: {e}"
                    ),
                })
            })
            .transpose()
    }
    let missing = |global_type: GlobalStateType| Error::RgbInspection {
        details: format!("genesis of contract {contract_id}: missing {global_type} global"),
    };

    let (name, ticker, details, precision) = match asset_schema {
        AssetSchema::Nia | AssetSchema::Ifa => {
            let spec: AssetSpec =
                global(contract_id, genesis, GS_NOMINAL)?.ok_or(missing(GS_NOMINAL))?;
            let ticker = spec.ticker.to_string();
            (spec.name, Some(ticker), spec.details, spec.precision)
        }
        AssetSchema::Bfa => {
            let ticker: UnicodeTicker =
                global(contract_id, genesis, GS_TICKER)?.ok_or(missing(GS_TICKER))?;
            let name: Name = global(contract_id, genesis, GS_NAME)?.ok_or(missing(GS_NAME))?;
            let details: Option<Details> = global(contract_id, genesis, GS_DETAILS)?;
            let precision: Precision =
                global(contract_id, genesis, GS_PRECISION)?.ok_or(missing(GS_PRECISION))?;
            (name, Some(ticker.to_string()), details, precision)
        }
    };

    // the supply, link, bridge and reject list globals of the schemas defining them. The issued
    // supply and the max supply are mandatory where they're defined, the others are optional
    let issued_supply: Option<Amount> = match asset_schema {
        AssetSchema::Nia | AssetSchema::Ifa => {
            Some(global(contract_id, genesis, GS_ISSUED_SUPPLY)?.ok_or(missing(GS_ISSUED_SUPPLY))?)
        }
        AssetSchema::Bfa => None,
    };
    let max_supply: Option<Amount> = match asset_schema {
        AssetSchema::Ifa => {
            Some(global(contract_id, genesis, GS_MAX_SUPPLY)?.ok_or(missing(GS_MAX_SUPPLY))?)
        }
        AssetSchema::Nia | AssetSchema::Bfa => None,
    };
    let linked_from_contract: Option<ContractId> = match asset_schema {
        AssetSchema::Ifa | AssetSchema::Bfa => {
            global(contract_id, genesis, GS_LINKED_FROM_CONTRACT)?
        }
        AssetSchema::Nia => None,
    };
    let bridge_location = match asset_schema {
        AssetSchema::Bfa => Some(
            BridgedFungibleAsset::bridge_location(genesis)
                .map_err(|e| Error::RgbInspection {
                    details: format!(
                        "genesis of contract {contract_id}: invalid {GS_BRIDGE_LOCATION} global: \
                         {e}"
                    ),
                })?
                .into(),
        ),
        AssetSchema::Nia | AssetSchema::Ifa => None,
    };
    let invalid_reject_list = |e: DeserializeError| Error::RgbInspection {
        details: format!("genesis of contract {contract_id}: invalid reject list global: {e}"),
    };
    let reject_list_global = |global_type: GlobalStateType| {
        genesis
            .globals
            .get(&global_type)
            .and_then(|values| values.first())
            .map(|value| value.to_inner())
    };
    let reject_list = match asset_schema {
        AssetSchema::Bfa => reject_list_global(GS_REJECT_LIST)
            .map(RejectListLocation::from_strict_serialized)
            .transpose()
            .map_err(invalid_reject_list)?
            .map(RgbRejectListInfo::from),
        AssetSchema::Ifa => reject_list_global(GS_REJECT_LIST_URL)
            .map(RejectListUrl::from_strict_serialized)
            .transpose()
            .map_err(invalid_reject_list)?
            .map(|url| RgbRejectListInfo::Url(url.to_string())),
        AssetSchema::Nia => None,
    };

    // the media of the asset, read from the same fields the wallet extracts its attachments from
    let terms: ContractTerms = global(contract_id, genesis, GS_TERMS)?.ok_or(missing(GS_TERMS))?;
    let media: Vec<RgbMediaInfo> = terms.media.iter().map(RgbMediaInfo::from).collect();

    Ok(RgbGenesisInfo {
        chain_net: genesis.chain_net.prefix().to_string(),
        timestamp: genesis.timestamp,
        issuer: genesis.issuer.to_string(),
        name: name.to_string(),
        ticker,
        details: details.map(|d| d.to_string()),
        precision: precision.into(),
        issued_supply: issued_supply.map(u64::from),
        max_supply: max_supply.map(u64::from),
        linked_from_contract: linked_from_contract.map(|id| id.to_string()),
        bridge_location,
        reject_list,
        media,
        outputs: outputs_info(
            &format!("genesis of contract {contract_id}"),
            Some(asset_schema),
            &genesis.assignments,
            genesis.chain_net,
            None,
            |_| None,
            Some(false),
        )?,
    })
}

/// Build the inspection details of the operation of a bundle.
///
/// The bundle ID commits only to the input map. Transitions with a different ID, of another
/// contract, or listed twice are rejected. Every input must resolve, by the given function or by
/// another transition of the bundle. Without `resolve_inputs`, an input that does not spend the
/// bundle reports only its opout.
pub(crate) fn bundle_info(
    contract_id: ContractId,
    asset_schema: Option<AssetSchema>,
    chain_net: ChainNet,
    bundle: &TransitionBundle,
    witness_tx: &BdkTransaction,
    resolve_inputs: bool,
    mut resolve_input: impl FnMut(RgbOpout) -> Option<(OutPoint, Assignment)>,
    mut is_ours: impl FnMut(&BuilderSeal<GraphSeal>) -> Option<bool>,
    mut is_terminal: impl FnMut(&OpId) -> Option<bool>,
) -> Result<RgbOperationInfo, Error> {
    // looked up for each input, as one may be assigned by another transition of the bundle
    let mut bundle_transitions: HashMap<OpId, &Transition> = HashMap::new();
    for KnownTransition { opid, transition } in bundle.known_transitions.iter() {
        // the transitions are a list, so the same one can be listed twice: as the input map binds
        // each input to a single transition, it would be reported (and its state counted) twice
        if bundle_transitions.insert(*opid, transition).is_some() {
            return Err(Error::RgbInspection {
                details: format!(
                    "bundle for contract {contract_id}: transition {opid} is listed twice"
                ),
            });
        }
    }
    let mut transitions = vec![];
    for KnownTransition { opid, transition } in bundle.known_transitions.iter() {
        // nothing commits to the contract a bundle is filed under, so a transition of another
        // contract would be reported as one of this contract, while validating the operation,
        // which checks the contract of each transition, would reject it
        if transition.contract_id != contract_id {
            return Err(Error::RgbInspection {
                details: format!(
                    "bundle for contract {contract_id}: transition {opid} is of contract {}",
                    transition.contract_id
                ),
            });
        }
        let inputs = transition.inputs();
        let committed = transition.id() == *opid
            && !inputs.is_empty()
            && inputs
                .iter()
                .all(|opout| bundle.input_map.get(opout) == Some(opid));
        if !committed {
            return Err(Error::RgbInspection {
                details: format!(
                    "bundle for contract {contract_id}: transition {opid} is not committed"
                ),
            });
        }
        let mut inputs_info = vec![];
        for opout in inputs {
            // an input spending a transition of the bundle resolves within the bundle
            let in_bundle = bundle_transitions.contains_key(&opout.op);
            let resolved =
                bundle_output(&bundle_transitions, opout).or_else(|| resolve_input(opout));
            let (vin, assignment) = match resolved {
                Some((outpoint, assignment)) => {
                    let vin = witness_tx
                        .input
                        .iter()
                        .position(|i| i.previous_output == outpoint)
                        .ok_or_else(|| Error::RgbInspection {
                            details: format!(
                                "bundle for contract {contract_id}: input {opout} not spent by \
                                 witness {}",
                                witness_tx.compute_txid()
                            ),
                        })?;
                    (Some(vin as u32), Some(assignment))
                }
                None if in_bundle => {
                    return Err(Error::RgbInspection {
                        details: format!(
                            "bundle for contract {contract_id}: input {opout} isn't assigned by \
                             the bundle to an outpoint the witness spends"
                        ),
                    });
                }
                None if resolve_inputs => {
                    return Err(Error::RgbInspection {
                        details: format!(
                            "bundle for contract {contract_id}: unknown input {opout}"
                        ),
                    });
                }
                None => (None, None),
            };
            inputs_info.push(RgbInputInfo {
                opout: opout.try_into().map_err(|ty| Error::RgbInspection {
                    details: format!(
                        "bundle for contract {contract_id}: unknown assignment type: {ty}"
                    ),
                })?,
                vin,
                assignment,
            });
        }
        transitions.push(transition_info(
            contract_id,
            asset_schema,
            transition,
            chain_net,
            witness_tx.compute_txid(),
            inputs_info,
            &mut is_ours,
            is_terminal(opid),
        )?);
    }
    Ok(RgbOperationInfo {
        asset_id: contract_id.to_string(),
        committed_transition_count: bundle.input_map_opids().len() as u32,
        committed_input_count: bundle.input_map.len() as u32,
        transitions,
    })
}

/// The outpoint and assignment of `opout`, if it's assigned by one of the transitions of a bundle
/// (given by OpId, e.g. the split a burn spends) to an outpoint the witness transaction spends.
fn bundle_output(
    bundle_transitions: &HashMap<OpId, &Transition>,
    opout: RgbOpout,
) -> Option<(OutPoint, Assignment)> {
    let transition = bundle_transitions.get(&opout.op)?;
    let assigns = transition.assignments.get(&opout.ty)?;
    let seal = assigns.revealed_seal_at(opout.no).ok().flatten()?;
    let TxPtr::Txid(txid) = seal.txid else {
        return None;
    };
    let assignment = Assignment::from_typed_assigns(opout.ty, assigns, opout.no)?;
    Some((OutPoint::new(txid, seal.vout.into_u32()), assignment))
}

/// Build the inspection details of a transition.
fn transition_info(
    contract_id: ContractId,
    asset_schema: Option<AssetSchema>,
    transition: &Transition,
    chain_net: ChainNet,
    witness_txid: Txid,
    inputs: Vec<RgbInputInfo>,
    mut is_ours: impl FnMut(&BuilderSeal<GraphSeal>) -> Option<bool>,
    is_terminal: Option<bool>,
) -> Result<RgbTransitionInfo, Error> {
    let r#type = TypeOfTransition::try_from(transition.transition_type).map_err(|_| {
        Error::RgbInspection {
            details: format!(
                "bundle for contract {contract_id}: unknown transition type: {}",
                transition.transition_type
            ),
        }
    })?;

    let outputs = outputs_info(
        &format!("bundle for contract {contract_id}"),
        asset_schema,
        &transition.assignments,
        chain_net,
        Some(witness_txid),
        &mut is_ours,
        is_terminal,
    )?;

    // collect transition globals
    let mut globals = vec![];
    for (global_type, values) in transition.globals.iter() {
        for value in values.iter() {
            let decode_amount = |name: &str| {
                Amount::from_strict_serialized(value.to_inner())
                    .map(u64::from)
                    .map_err(|e| Error::RgbInspection {
                        details: format!(
                            "bundle for contract {contract_id}: invalid {name} global: {e}"
                        ),
                    })
            };
            let global = match *global_type {
                GS_ISSUED_SUPPLY => {
                    TransitionGlobal::IssuedSupply(decode_amount(RGB_GLOBAL_ISSUED_SUPPLY)?)
                }
                GS_BURNED_ASSET => {
                    TransitionGlobal::BurnedAsset(decode_amount(RGB_GLOBAL_BURNED_ASSET)?)
                }
                GS_BURNED_INFLATION => {
                    TransitionGlobal::BurnedInflation(decode_amount(RGB_GLOBAL_BURNED_INFLATION)?)
                }
                GS_BURN_REASON => TransitionGlobal::BurnReason(
                    BurnReason::from_strict_serialized(value.to_inner())
                        .map_err(|e| Error::RgbInspection {
                            details: format!(
                                "bundle for contract {contract_id}: invalid {RGB_GLOBAL_BURN_REASON} global: {e}"
                            ),
                        })?
                        .to_string(),
                ),
                GS_LINKED_TO_CONTRACT => TransitionGlobal::LinkedToContract(
                    ContractId::from_strict_serialized(value.to_inner())
                        .map_err(|e| Error::RgbInspection {
                            details: format!(
                                "bundle for contract {contract_id}: invalid {RGB_GLOBAL_LINKED_TO_CONTRACT} global: {e}"
                            ),
                        })?
                        .to_string(),
                ),
                _ => {
                    return Err(Error::RgbInspection {
                        details: format!(
                            "bundle for contract {contract_id}: unknown global state type: {global_type}"
                        ),
                    });
                }
            };
            globals.push(global);
        }
    }

    // collect transition metadata
    let mut metadata = vec![];
    for (meta_type, value) in transition.metadata.iter() {
        let invalid = |name: &str, e: DeserializeError| Error::RgbInspection {
            details: format!("bundle for contract {contract_id}: invalid {name} metadata: {e}"),
        };
        let meta = match *meta_type {
            MS_ALLOWED_INFLATION => TransitionMetadata::AllowedInflation(
                Amount::from_strict_serialized(value.to_inner())
                    .map_err(|e| invalid(RGB_METADATA_ALLOWED_INFLATION, e))?
                    .into(),
            ),
            MS_BEFORE_BLOCK => TransitionMetadata::BeforeBlock(
                BlockNumber::from_strict_serialized(value.to_inner())
                    .map_err(|e| invalid(RGB_METADATA_BEFORE_BLOCK, e))?
                    .into_inner(),
            ),
            _ => {
                return Err(Error::RgbInspection {
                    details: format!(
                        "bundle for contract {contract_id}: unknown metadata type: {meta_type}"
                    ),
                });
            }
        };
        metadata.push(meta);
    }

    Ok(RgbTransitionInfo {
        r#type,
        opid: transition.id().to_string(),
        inputs,
        outputs,
        globals,
        metadata,
    })
}

/// Build the inspection details of the outputs of an operation.
///
/// Reject assignment types that rgb-lib cannot represent. Without a schema, check only the type.
fn outputs_info<Id: SealTxid>(
    context: &str,
    asset_schema: Option<AssetSchema>,
    assignments: &Assignments<BlindSeal<Id>>,
    chain_net: ChainNet,
    witness_txid: Option<Txid>,
    mut is_ours: impl FnMut(&BuilderSeal<BlindSeal<Id>>) -> Option<bool>,
    is_terminal: Option<bool>,
) -> Result<Vec<RgbOutputInfo>, Error>
where
    BlindSeal<Id>: ExposedSeal,
{
    let mut outputs = vec![];
    for (ass_type, typed_assigns) in assignments.iter() {
        for no in 0..typed_assigns.len_u16() {
            let Some(assignment) = Assignment::from_typed_assigns(*ass_type, typed_assigns, no)
                .filter(|a| asset_schema.is_none_or(|schema| schema.supports_assignment(a)))
            else {
                let in_schema = asset_schema
                    .is_some_and(|schema| schema.schema().owned_types.contains_key(ass_type));
                let kind = if in_schema {
                    "currently unsupported"
                } else {
                    "unknown"
                };
                return Err(Error::RgbInspection {
                    details: format!("{context}: {kind} assignment type: {ass_type}"),
                });
            };
            let seal = typed_assigns.seal_at(no).expect("index in range");
            let (txid, vout, blinding, recipient_id) = match seal {
                BuilderSeal::Revealed(seal) => (
                    seal.txid().or(witness_txid).map(|t| t.to_string()),
                    Some(seal.vout().into_u32()),
                    Some(seal.blinding),
                    None,
                ),
                BuilderSeal::Concealed(seal) => {
                    let beneficiary = Beneficiary::BlindedSeal(*seal);
                    (
                        None,
                        None,
                        None,
                        Some(XChainNet::with(chain_net, beneficiary).to_string()),
                    )
                }
            };
            outputs.push(RgbOutputInfo {
                txid,
                vout,
                blinding,
                assignment,
                is_concealed: recipient_id.is_some(),
                recipient_id,
                is_ours: is_ours(seal),
                is_terminal,
            });
        }
    }
    Ok(outputs)
}

#[cfg(test)]
pub(crate) mod tests {
    //! Expected values come from rgb-lib inspect_consignment at 6236d7e9 and inspect_fascia at
    //! 4d87b372.

    use super::*;
    use bdk_wallet::bitcoin::absolute::LockTime;

    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/inspection/",
                $name
            ))
        };
    }

    const SEND: &[u8] = fixture!("send.rgbc");

    fn json(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).unwrap()
    }

    fn inspect_json(bytes: &[u8], filter: BundleFilter) -> serde_json::Value {
        serde_json::to_value(inspect_consignment(bytes, filter).unwrap()).unwrap()
    }

    fn matches_rgb_lib(bytes: &[u8], filter: BundleFilter, expected: &[u8]) {
        assert_eq!(inspect_json(bytes, filter), json(expected));
    }

    fn witness_txid(expected: &[u8]) -> String {
        json(expected)["witnesses"][0]["txid"]
            .as_str()
            .unwrap()
            .to_string()
    }

    // Write the send consignment in the validated transfer file format.
    fn validated_send(status: &[u8]) -> Vec<u8> {
        let mut bytes = SEND.to_vec();
        bytes[4..7].copy_from_slice(b"VTF");
        bytes.extend_from_slice(status);
        bytes
    }

    fn forged(witness_txid: &str, modifier: impl Fn(&mut WitnessBundle)) -> Vec<u8> {
        let mut consignment = RgbTransfer::load(SEND).unwrap();
        let witness_id = RgbTxid::from_str(witness_txid).unwrap();
        assert!(consignment.modify_bundle(witness_id, modifier));
        let mut bytes = vec![];
        consignment.save(&mut bytes).unwrap();
        bytes
    }

    fn bump_nonce(witness_bundle: &mut WitnessBundle) {
        let bundle = witness_bundle.bundle_mut();
        let mut known_transitions = bundle.known_transitions.to_unconfined();
        known_transitions[0].transition.nonce =
            known_transitions[0].transition.nonce.wrapping_add(1);
        bundle.known_transitions = Confined::from_iter_checked(known_transitions);
    }

    fn assert_inspection_error<T: fmt::Debug>(result: Result<T, Error>, text: &str) {
        assert!(
            matches!(&result, Err(Error::RgbInspection { details }) if details.contains(text)),
            "{result:?}"
        );
    }

    fn psbt(bytes: &[u8]) -> String {
        std::str::from_utf8(bytes).unwrap().trim().to_string()
    }

    fn entropy(fascia: &[u8]) -> u64 {
        parse_fascia(fascia)
            .unwrap()
            .seal_witness()
            .mpc_merkle_block
            .entropy()
            .unwrap()
    }

    fn fascia_json(
        psbt: &str,
        fascia: &[u8],
        entropy: Option<u64>,
        network: BitcoinNetwork,
    ) -> serde_json::Value {
        serde_json::to_value(inspect_fascia(psbt.to_string(), fascia, entropy, network).unwrap())
            .unwrap()
    }

    /// Edit the JSON of a fascia and encode it again.
    pub(crate) fn tampered_fascia(
        fascia: &[u8],
        edit: impl FnOnce(&mut serde_json::Value),
    ) -> Vec<u8> {
        let mut json = serde_json::to_value(parse_fascia(fascia).unwrap()).unwrap();
        edit(&mut json);
        serde_json::from_value::<Fascia>(json)
            .unwrap()
            .to_strict_serialized::<FILE_MAX_LEN>()
            .unwrap()
            .release()
    }

    /// Raise the nonce of the first known transition of the first bundle.
    pub(crate) fn fascia_with_bumped_nonce(fascia: &[u8]) -> Vec<u8> {
        tampered_fascia(fascia, |json| {
            let transition = json["bundles"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap()["knownTransitions"][0]["transition"]
                .as_object_mut()
                .unwrap();
            let nonce = transition["nonce"].as_u64().unwrap();
            transition.insert(s!("nonce"), nonce.wrapping_add(1).into());
        })
    }

    /// Conceal the seals of the transition that another transition of the same bundle spends.
    /// The bundle ID does not change.
    pub(crate) fn conceal_split(fascia: &[u8]) -> Vec<u8> {
        let parsed = parse_fascia(fascia).unwrap();
        let bundle = parsed.bundles().values().next().unwrap();
        let spent_opids: Vec<OpId> = bundle
            .known_transitions
            .iter()
            .flat_map(|kt| kt.transition.inputs().iter().map(|i| i.op))
            .collect();
        let split = bundle
            .known_transitions
            .iter()
            .find(|kt| spent_opids.contains(&kt.opid))
            .unwrap();
        tampered_fascia(fascia, |json| {
            let split_json = json["bundles"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap()["knownTransitions"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|kt| kt["opid"] == serde_json::to_value(split.opid).unwrap())
                .unwrap();
            for (ass_type, assignment_json) in split_json["transition"]["assignments"]
                .as_object_mut()
                .unwrap()
                .iter_mut()
            {
                let ass_type = AssignmentType::with(ass_type.parse().unwrap());
                let assignment = split.transition.assignments.get(&ass_type).unwrap();
                *assignment_json = serde_json::to_value(assignment.conceal()).unwrap();
            }
        })
    }

    #[test]
    fn inspect_consignment_invalid_bytes() {
        // not a consignment
        for bytes in [&b""[..], b"not a consignment"] {
            let result = inspect_consignment(bytes, BundleFilter::All);
            assert!(
                matches!(&result, Err(Error::InvalidConsignment)),
                "{result:?}"
            );
        }

        // invalid witness TXID
        let result = inspect_consignment(
            b"not a consignment",
            BundleFilter::Witness("invalid".to_string()),
        );
        assert!(matches!(&result, Err(Error::InvalidTxid)), "{result:?}");

        // a schema definition
        let mut bytes = vec![];
        NonInflatableAsset::schema_definition()
            .save(&mut bytes)
            .unwrap();
        let result = inspect_consignment(&bytes, BundleFilter::All);
        assert!(
            matches!(&result, Err(Error::InvalidConsignment)),
            "{result:?}"
        );
    }

    #[test]
    fn issuance_matches_rgb_lib() {
        let nia = fixture!("issuance_nia.rgbc");
        let ifa = fixture!("issuance_ifa.rgbc");
        let linked = fixture!("issuance_ifa_linked.rgbc");
        let bfa = fixture!("issuance_bfa.rgbc");
        matches_rgb_lib(nia, BundleFilter::All, fixture!("issuance_nia.all.json"));
        matches_rgb_lib(ifa, BundleFilter::All, fixture!("issuance_ifa.all.json"));
        matches_rgb_lib(
            linked,
            BundleFilter::All,
            fixture!("issuance_ifa_linked.all.json"),
        );
        matches_rgb_lib(bfa, BundleFilter::All, fixture!("issuance_bfa.all.json"));
    }

    #[test]
    fn send_matches_rgb_lib() {
        let t1 = witness_txid(fixture!("send.witness.json"));
        matches_rgb_lib(
            SEND,
            BundleFilter::Terminals,
            fixture!("send.terminals.json"),
        );
        matches_rgb_lib(SEND, BundleFilter::All, fixture!("send.all.json"));
        matches_rgb_lib(
            SEND,
            BundleFilter::Witness(t1),
            fixture!("send.witness.json"),
        );

        // the validated transfer file gives the same inspection
        let valid = validated_send(&serde_json::to_vec(&ValidationStatus::default()).unwrap());
        assert_eq!(
            inspect_json(&valid, BundleFilter::Terminals),
            json(fixture!("send.terminals.json"))
        );
        let result = inspect_consignment(&validated_send(b"{"), BundleFilter::Terminals);
        assert!(
            matches!(&result, Err(Error::InvalidConsignment)),
            "{result:?}"
        );

        // the contract consignment carries the genesis only
        let mut contract = vec![];
        RgbTransfer::load(SEND)
            .unwrap()
            .into_contract()
            .save(&mut contract)
            .unwrap();
        let contract = inspect_consignment(&contract, BundleFilter::Terminals).unwrap();
        assert!(!contract.is_transfer);
        assert!(contract.witnesses.is_empty());
        assert_eq!(
            serde_json::to_value(&contract.genesis).unwrap(),
            json(fixture!("send.terminals.json"))["genesis"]
        );
    }

    #[test]
    fn mint_matches_rgb_lib() {
        let mint = fixture!("mint.rgbc");
        matches_rgb_lib(mint, BundleFilter::Terminals, fixture!("mint.all.json"));
    }

    #[test]
    fn link_burn_inflate_match_rgb_lib() {
        matches_rgb_lib(
            fixture!("link.rgbc"),
            BundleFilter::Terminals,
            fixture!("link.terminals.json"),
        );
        matches_rgb_lib(
            fixture!("burn.rgbc"),
            BundleFilter::Terminals,
            fixture!("burn.terminals.json"),
        );
        // a send of inflated supply carries the inflation
        matches_rgb_lib(
            fixture!("inflate.rgbc"),
            BundleFilter::All,
            fixture!("inflate.all.json"),
        );
    }

    #[test]
    fn fascia_matches_rgb_lib() {
        let cases: [(&[u8], &[u8], &[u8]); 4] = [
            (
                fixture!("fascia_send.psbt"),
                fixture!("fascia_send.fascia"),
                fixture!("fascia_send.json"),
            ),
            (
                fixture!("fascia_burn.psbt"),
                fixture!("fascia_burn.fascia"),
                fixture!("fascia_burn.json"),
            ),
            (
                fixture!("fascia_inflation.psbt"),
                fixture!("fascia_inflation.fascia"),
                fixture!("fascia_inflation.json"),
            ),
            (
                fixture!("fascia_link.psbt"),
                fixture!("fascia_link.fascia"),
                fixture!("fascia_link.json"),
            ),
        ];
        for (psbt_bytes, fascia, expected) in cases {
            let psbt = psbt(psbt_bytes);
            let e = entropy(fascia);
            assert_eq!(
                fascia_json(&psbt, fascia, Some(e), BitcoinNetwork::Regtest),
                json(expected)
            );
            assert_eq!(
                fascia_json(&psbt, fascia, None, BitcoinNetwork::Regtest),
                json(expected)
            );
        }

        let fascia = fixture!("fascia_send.fascia");
        assert_eq!(
            fascia_json(
                &psbt(fixture!("fascia_send.psbt")),
                fascia,
                Some(entropy(fascia)),
                BitcoinNetwork::Testnet
            ),
            json(
                std::str::from_utf8(fixture!("fascia_send.json"))
                    .unwrap()
                    .replace("bcrt:utxob:", "tb3:utxob:")
                    .as_bytes()
            )
        );
    }

    #[test]
    fn inspect_fascia_invalid_input() {
        let psbt = psbt(fixture!("fascia_send.psbt"));
        let fascia = fixture!("fascia_send.fascia");
        let inspect = |psbt: &str, fascia: &[u8]| {
            inspect_fascia(psbt.to_string(), fascia, None, BitcoinNetwork::Regtest)
        };

        let result = inspect("not a PSBT", fascia);
        assert!(
            matches!(result, Err(Error::InvalidPsbt { .. })),
            "{result:?}"
        );

        let mut trailing = fascia.to_vec();
        trailing.push(0);
        for bytes in [&b""[..], b"not a fascia", &trailing] {
            let result = inspect(&psbt, bytes);
            assert!(matches!(result, Err(Error::InvalidFascia)), "{result:?}");
        }
    }

    #[test]
    fn forged_fascia_refused() {
        let burn = fixture!("fascia_burn.fascia");
        let burn_psbt = psbt(fixture!("fascia_burn.psbt"));
        let psbt = psbt(fixture!("fascia_send.psbt"));
        let fascia = fixture!("fascia_send.fascia");
        let e = entropy(fascia);
        let inspect = |fascia: &[u8], entropy: Option<u64>| {
            inspect_fascia(psbt.clone(), fascia, entropy, BitcoinNetwork::Regtest)
        };

        // an entropy that is not the one the fascia carries
        let result = inspect(fascia, Some(e.wrapping_add(1)));
        assert_inspection_error(result, &format!("built with entropy {e}, not the given"));

        // a transition not committed by its bundle
        let tampered = fascia_with_bumped_nonce(fascia);
        assert_inspection_error(inspect(&tampered, Some(e)), "is not committed");

        // a bundle filed under another contract
        let asset_id = json(fixture!("fascia_send.json"))["operations"][0]["asset_id"]
            .as_str()
            .unwrap()
            .to_string();
        let tampered = tampered_fascia(fascia, |json| {
            let bundle = json["bundles"]
                .as_object()
                .unwrap()
                .values()
                .next()
                .unwrap()
                .clone();
            json["bundles"] = serde_json::Value::Object(serde_json::Map::from_iter([(
                ContractId::from([1u8; 32]).to_string(),
                bundle,
            )]));
        });
        assert_inspection_error(
            inspect(&tampered, Some(e)),
            &format!("is of contract {asset_id}"),
        );

        // a bundle that lists the same transition twice
        let tampered = tampered_fascia(fascia, |json| {
            let known_transitions = json["bundles"]
                .as_object_mut()
                .unwrap()
                .values_mut()
                .next()
                .unwrap()["knownTransitions"]
                .as_array_mut()
                .unwrap();
            known_transitions.push(known_transitions[0].clone());
        });
        assert_inspection_error(inspect(&tampered, Some(e)), "is listed twice");

        // a commitment that does not carry its entropy
        let tampered = tampered_fascia(fascia, |json| {
            json["sealWitness"]["mpcMerkleBlock"]["entropy"] = serde_json::Value::Null;
        });
        assert_inspection_error(inspect(&tampered, None), "doesn't carry the entropy");
        assert_eq!(
            serde_json::to_value(inspect(&tampered, Some(e)).unwrap()).unwrap(),
            json(fixture!("fascia_send.json"))
        );
        assert_inspection_error(
            inspect(&tampered, Some(e.wrapping_add(1))),
            "doesn't commit to",
        );

        // a PSBT of another transaction
        let mut other_psbt = Psbt::from_str(&psbt).unwrap();
        let lock_time = other_psbt.unsigned_tx.lock_time.to_consensus_u32();
        other_psbt.unsigned_tx.lock_time = LockTime::from_consensus(lock_time.wrapping_add(1));
        let result = inspect_fascia(
            other_psbt.to_string(),
            fascia,
            Some(e),
            BitcoinNetwork::Regtest,
        );
        assert_inspection_error(result, "does not match PSBT TXID");

        // an input that spends a concealed seal of the bundle
        let result = inspect_fascia(
            burn_psbt,
            &conceal_split(burn),
            Some(entropy(burn)),
            BitcoinNetwork::Regtest,
        );
        assert_inspection_error(result, "isn't assigned by the bundle");
    }

    #[test]
    fn forged_consignment_refused() {
        let t1 = witness_txid(fixture!("send.witness.json"));
        let t2 = witness_txid(fixture!("send.terminals.json"));

        // a history transition with another ID does not resolve the inputs that spend it
        let bytes = forged(&t1, bump_nonce);
        let result = inspect_consignment(&bytes, BundleFilter::Terminals);
        assert_inspection_error(result, "unknown input");

        let result = inspect_consignment(SEND, BundleFilter::Witness("00".repeat(32)));
        assert_inspection_error(result, "not found");

        // a bundle not committed by its witness
        let bytes = forged(&t2, |witness_bundle| {
            for output in witness_bundle.tx.output.iter_mut() {
                output.script_pubkey = ScriptBuf::new();
            }
        });
        let result = inspect_consignment(&bytes, BundleFilter::Terminals);
        assert_inspection_error(result, "doesn't commit to");

        // a transition not committed by its bundle
        let bytes = forged(&t2, bump_nonce);
        let result = inspect_consignment(&bytes, BundleFilter::Terminals);
        assert_inspection_error(result, "is not committed");
    }

    #[test]
    fn has_concealed_transitions() {
        let operation = |committed_transition_count| RgbOperationInfo {
            asset_id: s!("asset"),
            committed_transition_count,
            committed_input_count: 1,
            transitions: vec![RgbTransitionInfo {
                r#type: TypeOfTransition::Transfer,
                opid: s!("opid"),
                inputs: vec![],
                outputs: vec![],
                globals: vec![],
                metadata: vec![],
            }],
        };
        assert!(operation(2).has_concealed_transitions());
        assert!(!operation(1).has_concealed_transitions());
        let inspection = |operations| RgbInspection {
            txid: s!("txid"),
            close_method: CloseMethod::OpretFirst,
            commitment_hex: s!("00"),
            operations,
        };
        assert!(inspection(vec![operation(1), operation(2)]).has_concealed_transitions());
        assert!(!inspection(vec![operation(1)]).has_concealed_transitions());
    }

    #[test]
    fn opout_assignment_types() {
        let opid = OpId::from([1; 32]);
        let opout = |ty| Opout::try_from(RgbOpout::new(opid, ty, 3));

        assert_eq!(
            opout(OS_ASSET).unwrap(),
            Opout {
                opid: opid.to_string(),
                assignment_type: TypeOfAssignment::Asset,
                index: 3,
            }
        );
        assert_eq!(
            opout(OS_INFLATION).unwrap().assignment_type,
            TypeOfAssignment::InflationRight
        );
        assert_eq!(
            opout(OS_LINK).unwrap().assignment_type,
            TypeOfAssignment::LinkRight
        );
        assert_eq!(
            opout(OS_MINT).unwrap().assignment_type,
            TypeOfAssignment::MintRight
        );

        // an assignment type no supported schema defines
        let unknown = AssignmentType::with(u16::MAX);
        assert_eq!(opout(unknown), Err(unknown));
    }

    #[test]
    fn outputs_info_unsupported_assignment_type() {
        // build assignments with a single declarative output of the given type
        let assignments = |ty: AssignmentType| {
            let assign = Assign::with(
                BuilderSeal::Revealed(GraphSeal::new_random_vout(0)),
                VoidState::default(),
            );
            let mut assignments = Assignments::<GraphSeal>::default();
            assignments
                .insert(
                    ty,
                    TypedAssigns::Declarative(AssignVec::with(NonEmptyVec::with(assign))),
                )
                .unwrap();
            assignments
        };
        let outputs = |asset_schema: Option<AssetSchema>, ty: AssignmentType| {
            outputs_info(
                "ctx",
                asset_schema,
                &assignments(ty),
                ChainNet::BitcoinRegtest,
                None,
                |_| None,
                None,
            )
        };

        // supported types
        let result = outputs(Some(AssetSchema::Ifa), OS_LINK).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].assignment, Assignment::LinkRight);
        let result = outputs(Some(AssetSchema::Bfa), OS_MINT).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].assignment, Assignment::MintRight);

        // type unknown to the schema
        let result = outputs(Some(AssetSchema::Nia), OS_LINK);
        assert!(
            matches!(
                &result,
                Err(Error::RgbInspection { details })
                    if *details == format!("ctx: unknown assignment type: {OS_LINK}")
            ),
            "{result:?}"
        );

        // without a schema, only the type is checked
        let result = outputs(None, OS_LINK).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].assignment, Assignment::LinkRight);
        let result = outputs(None, AssignmentType::with(u16::MAX));
        assert!(
            matches!(
                &result,
                Err(Error::RgbInspection { details })
                    if details == "ctx: unknown assignment type: 65535"
            ),
            "{result:?}"
        );
    }
}

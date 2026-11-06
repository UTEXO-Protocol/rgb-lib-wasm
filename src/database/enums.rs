use super::*;

/// The schema of an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum AssetSchema {
    /// NIA schema
    Nia = 1,
    /// IFA schema
    Ifa = 4,
    /// BFA schema
    Bfa = 5,
}

impl fmt::Display for AssetSchema {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl TryFrom<String> for AssetSchema {
    type Error = Error;

    fn try_from(schema_id: String) -> Result<Self, Self::Error> {
        Ok(match &schema_id[..] {
            SCHEMA_ID_NIA => AssetSchema::Nia,
            SCHEMA_ID_IFA => AssetSchema::Ifa,
            SCHEMA_ID_BFA => AssetSchema::Bfa,
            _ => return Err(Error::UnknownRgbSchema { schema_id }),
        })
    }
}

impl TryFrom<SchemaId> for AssetSchema {
    type Error = Error;

    fn try_from(schema_id: SchemaId) -> Result<Self, Self::Error> {
        schema_id.to_string().try_into()
    }
}

impl AssetSchema {
    pub(crate) const VALUES: [Self; NUM_KNOWN_SCHEMAS] = [Self::Nia, Self::Ifa, Self::Bfa];

    fn from_schema_id_str(schema_id: String) -> Result<Self, Error> {
        Ok(match &schema_id[..] {
            SCHEMA_ID_NIA => AssetSchema::Nia,
            SCHEMA_ID_IFA => AssetSchema::Ifa,
            SCHEMA_ID_BFA => AssetSchema::Bfa,
            _ => return Err(Error::UnknownRgbSchema { schema_id }),
        })
    }

    /// Get [`AssetSchema`] from [`SchemaId`].
    pub fn from_schema_id(schema_id: SchemaId) -> Result<Self, Error> {
        Self::from_schema_id_str(schema_id.to_string())
    }

    pub(crate) fn get_from_contract_id(
        contract_id: ContractId,
        runtime: &RgbRuntime,
    ) -> Result<Self, Error> {
        let schema_id = runtime.genesis(contract_id)?.schema_id;
        Self::from_schema_id(schema_id)
    }

    pub(crate) fn schema_rules(&self) -> SchemaRules {
        match self {
            Self::Nia => NonInflatableAsset::schema_rules(),
            Self::Ifa => InflatableFungibleAsset::schema_rules(),
            Self::Bfa => BridgedFungibleAsset::schema_rules(),
        }
    }

    pub(crate) fn schema_definition(&self) -> SchemaDefinition {
        match self {
            Self::Nia => NonInflatableAsset::schema_definition(),
            Self::Ifa => InflatableFungibleAsset::schema_definition(),
            Self::Bfa => BridgedFungibleAsset::schema_definition(),
        }
    }

    pub(crate) fn import_schema_definition(&self, runtime: &mut RgbRuntime) -> Result<(), Error> {
        runtime.import_schema_definition(self.schema_definition())?;
        Ok(())
    }
}

impl From<AssetSchema> for SchemaId {
    fn from(asset_schema: AssetSchema) -> Self {
        match asset_schema {
            AssetSchema::Ifa => SchemaId::from_str(SCHEMA_ID_IFA).unwrap(),
            AssetSchema::Nia => SchemaId::from_str(SCHEMA_ID_NIA).unwrap(),
            AssetSchema::Bfa => SchemaId::from_str(SCHEMA_ID_BFA).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ColoringType {
    Receive = 1,
    Issue = 2,
    Input = 3,
    Change = 4,
}

/// The type of an RGB recipient
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RecipientTypeFull {
    /// Receive via blinded UTXO
    Blind { unblinded_utxo: Outpoint },
    /// Receive via witness TX
    Witness { vout: Option<u32> },
}

/// The type of an RGB transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransportType {
    /// HTTP(s) JSON-RPC ([specification](https://github.com/RGB-Tools/rgb-http-json-rpc))
    JsonRpc = 1,
}

/// The role of a transport endpoint in a transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum TransportEndpointKind {
    /// The proxy of the recipient, where the sender posts the consignment
    RecipientProxy = 1,
    /// The proxy of the sender, where the recipient gets the signed TX
    SenderProxy = 2,
}

/// The consignment and media encryption preference of a recipient (mirrors
/// [`rgbinvoice::Encryption`]).
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum Encryption {
    /// Encryption is mandatory for consignment and media (the recipient refuses cleartext)
    Required = 1,
    /// Encryption must not be used for consignment and media (the recipient refuses encrypted)
    Unsupported = 2,
    /// Encryption may be used for consignment and media (the recipient accepts both)
    Optional = 3,
}

impl From<rgbinvoice::Encryption> for Encryption {
    fn from(e: rgbinvoice::Encryption) -> Self {
        match e {
            rgbinvoice::Encryption::Required => Encryption::Required,
            rgbinvoice::Encryption::Unsupported => Encryption::Unsupported,
            rgbinvoice::Encryption::Optional => Encryption::Optional,
        }
    }
}

impl From<Encryption> for rgbinvoice::Encryption {
    fn from(e: Encryption) -> Self {
        match e {
            Encryption::Required => rgbinvoice::Encryption::Required,
            Encryption::Unsupported => rgbinvoice::Encryption::Unsupported,
            Encryption::Optional => rgbinvoice::Encryption::Optional,
        }
    }
}

/// The status of a [`crate::wallet::Transfer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Ord, PartialOrd, Deserialize, Serialize)]
pub enum TransferStatus {
    /// Transfer has been initiated (PSBT prepared) but not yet finalized
    Initiated = 5,
    /// Waiting for the counterparty to take action
    WaitingCounterparty = 1,
    /// Waiting for the transfer transaction to be broadcasted
    WaitingBroadcast = 7,
    /// Waiting for the transfer transaction to reach the required number of confirmations
    WaitingConfirmations = 2,
    /// Settled transfer, this status is final
    Settled = 3,
    /// Failed transfer, this status is final
    Failed = 4,
}

impl TransferStatus {
    pub(crate) fn failed(&self) -> bool {
        self == &TransferStatus::Failed
    }

    pub(crate) fn initiated(&self) -> bool {
        self == &TransferStatus::Initiated
    }

    pub(crate) fn pending(&self) -> bool {
        [
            TransferStatus::Initiated,
            TransferStatus::WaitingCounterparty,
            TransferStatus::WaitingConfirmations,
            TransferStatus::WaitingBroadcast,
        ]
        .contains(self)
    }

    pub(crate) fn settled(&self) -> bool {
        self == &TransferStatus::Settled
    }

    pub(crate) fn waiting_confirmations(&self) -> bool {
        self == &TransferStatus::WaitingConfirmations
    }

    pub(crate) fn waiting_counterparty(&self) -> bool {
        self == &TransferStatus::WaitingCounterparty
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn waiting(&self) -> bool {
        [
            TransferStatus::WaitingCounterparty,
            TransferStatus::WaitingConfirmations,
            TransferStatus::WaitingBroadcast,
        ]
        .contains(self)
    }

    #[cfg_attr(not(feature = "esplora"), allow(dead_code))]
    pub(crate) fn is_fallible(&self) -> bool {
        [
            TransferStatus::Initiated,
            TransferStatus::WaitingCounterparty,
            TransferStatus::WaitingBroadcast,
        ]
        .contains(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum WalletTransactionType {
    CreateUtxos = 1,
    Drain = 2,
}

/// An RGB assignment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd, Deserialize, Serialize)]
pub enum Assignment {
    /// Fungible value in RGB units (not considering precision)
    Fungible(u64),
    /// Non-fungible value
    NonFungible,
    /// Inflation right
    InflationRight(u64),
    /// Mint right
    MintRight,
    /// Any assignment
    Any,
}

impl Assignment {
    #[cfg(feature = "esplora")]
    pub(crate) fn from_opout_and_state(opout: Opout, state: &AllocatedState) -> Self {
        match state {
            AllocatedState::Amount(amt) if opout.ty == OS_ASSET => Self::Fungible(amt.as_u64()),
            AllocatedState::Amount(amt) if opout.ty == OS_INFLATION => {
                Self::InflationRight(amt.as_u64())
            }
            AllocatedState::Data(_) => Self::NonFungible,
            AllocatedState::Void if opout.ty == OS_MINT => Self::MintRight,
            _ => unreachable!(),
        }
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn add_to_assignments(&self, assignments: &mut AssignmentsCollection) {
        match self {
            Self::Fungible(amt) => assignments.fungible += amt,
            Self::NonFungible => assignments.non_fungible = true,
            Self::InflationRight(amt) => assignments.inflation += amt,
            Self::MintRight => assignments.mint += 1,
            _ => unreachable!("when using this method we should know the assignment type"),
        }
    }

    pub(crate) fn main_amount(&self) -> u64 {
        if let Self::Fungible(amt) = self {
            *amt
        } else if let Self::NonFungible = self {
            1
        } else {
            0
        }
    }

    #[cfg(feature = "esplora")]
    pub(crate) fn inflation_amount(&self) -> u64 {
        if let Self::InflationRight(amt) = self {
            *amt
        } else {
            0
        }
    }
}

/// The main transition of an outgoing operation.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum TypeOfTransition {
    /// Inflation transition (issuing new tokens)
    Inflate = 2,
    /// Transfer transition (moving existing tokens)
    Transfer = 1,
    /// Burn transition (burning existing tokens)
    Burn = 3,
    /// Mint transition (minting assets backed by tokens locked on the external chain)
    Mint = 4,
}

impl TypeOfTransition {
    pub(crate) fn type_name(&self) -> &'static str {
        match self {
            Self::Inflate => "inflate",
            Self::Transfer => "transfer",
            Self::Burn => "burn",
            Self::Mint => "mint",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The SCHEMA_ID_* constants must match the schemas actually shipped by the
    // rgb-schemas crate: every receive path maps a consignment's schema ID back
    // to an AssetSchema through them, and a stale constant makes that mapping
    // fail with UnknownRgbSchema before the ack/refuse step (issue #126).
    #[test]
    fn schema_id_constants_match_rgb_schemas_crate() {
        for asset_schema in AssetSchema::VALUES {
            assert_eq!(
                SchemaId::from(asset_schema),
                asset_schema.schema_definition().schema.schema_id(),
                "SCHEMA_ID constant for {asset_schema} is stale"
            );
        }
    }

    #[test]
    fn test_transfer_status_methods() {
        assert!(TransferStatus::Initiated.pending());
        assert!(TransferStatus::WaitingCounterparty.pending());
        assert!(TransferStatus::WaitingConfirmations.pending());
        assert!(TransferStatus::WaitingBroadcast.pending());
        assert!(!TransferStatus::Settled.pending());
        assert!(!TransferStatus::Failed.pending());

        assert!(TransferStatus::WaitingCounterparty.waiting());
        assert!(TransferStatus::WaitingConfirmations.waiting());
        assert!(TransferStatus::WaitingBroadcast.waiting());
        assert!(!TransferStatus::Initiated.waiting());
        assert!(!TransferStatus::Settled.waiting());
        assert!(!TransferStatus::Failed.waiting());

        assert!(TransferStatus::Initiated.is_fallible());
        assert!(TransferStatus::WaitingCounterparty.is_fallible());
        assert!(TransferStatus::WaitingBroadcast.is_fallible());
        assert!(!TransferStatus::WaitingConfirmations.is_fallible());
        assert!(!TransferStatus::Settled.is_fallible());
        assert!(!TransferStatus::Failed.is_fallible());
    }

    #[test]
    fn test_encryption_conversions() {
        // every variant maps both ways between the DB enum and the rgb-invoicing enum, and keeps a
        // stable on-disk num_value (the DB persistence contract)
        for (encryption, num_value) in [
            (Encryption::Required, 1u8),
            (Encryption::Unsupported, 2),
            (Encryption::Optional, 3),
        ] {
            // DB -> invoicing maps to the matching concrete variant
            let to_invoicing: rgbinvoice::Encryption = encryption.into();
            let invoicing_ok = matches!(
                (encryption, &to_invoicing),
                (Encryption::Required, rgbinvoice::Encryption::Required)
                    | (Encryption::Unsupported, rgbinvoice::Encryption::Unsupported)
                    | (Encryption::Optional, rgbinvoice::Encryption::Optional)
            );
            assert!(
                invoicing_ok,
                "{encryption:?} mapped to the wrong invoicing variant"
            );
            // invoicing -> DB round-trips back to the original variant
            let back: Encryption = to_invoicing.into();
            assert_eq!(back, encryption);
            // the integer stored in the DB must not change across versions
            assert_eq!(encryption as u8, num_value);
        }
    }
}

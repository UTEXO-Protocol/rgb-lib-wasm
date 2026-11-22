//! Objects and helpers that need no wallet.

use super::*;

pub(crate) mod inspection;
#[cfg(feature = "esplora")]
pub(crate) mod spv;

pub use inspection::{
    BundleFilter, ConsignmentInspection, Opout, RgbBridgeLocationInfo, RgbEvmContractInfo,
    RgbGenesisInfo, RgbInputInfo, RgbInspection, RgbMediaInfo, RgbOperationInfo, RgbOutputInfo,
    RgbRejectListInfo, RgbTransitionInfo, TransitionGlobal, TransitionMetadata,
    inspect_consignment,
};
#[cfg(feature = "esplora")]
pub use spv::{SpvProofFill, WitnessWithoutProof, fill_spv_proofs};

/// Bitcoin transaction outpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Outpoint {
    /// ID of the transaction
    pub txid: String,
    /// Output index
    pub vout: u32,
}

impl fmt::Display for Outpoint {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{}:{}", self.txid, self.vout)
    }
}

impl From<OutPoint> for Outpoint {
    fn from(x: OutPoint) -> Outpoint {
        Outpoint {
            txid: x.txid.to_string(),
            vout: x.vout,
        }
    }
}

impl From<Outpoint> for OutPoint {
    fn from(x: Outpoint) -> OutPoint {
        OutPoint::from_str(&x.to_string()).expect("outpoint should be parsable")
    }
}

/// The type of an RGB recipient
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum RecipientType {
    /// Receive via blinded UTXO
    Blind,
    /// Receive via witness TX
    Witness,
}

impl fmt::Display for RecipientType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// RGB recipient information used to be paid
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct RecipientInfo {
    /// Recipient ID
    pub recipient_id: String,
    /// Recipient type
    pub recipient_type: RecipientType,
    /// Recipient network
    pub network: BitcoinNetwork,
}

impl RecipientInfo {
    /// Builds a new [`RecipientInfo`] from the provided string, checking that it is valid.
    pub fn new(recipient_id: String) -> Result<Self, Error> {
        let xchainnet_beneficiary = XChainNet::<Beneficiary>::from_str(&recipient_id)
            .map_err(|_| Error::InvalidRecipientID)?;
        let recipient_type = match xchainnet_beneficiary.into_inner() {
            Beneficiary::WitnessVout(_, _) => RecipientType::Witness,
            Beneficiary::BlindedSeal(_) => RecipientType::Blind,
        };
        Ok(Self {
            recipient_id,
            recipient_type,
            network: xchainnet_beneficiary.chain_network().try_into()?,
        })
    }
}

/// An RGB transport endpoint.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct TransportEndpoint {
    /// Endpoint address
    pub endpoint: String,
    /// Endpoint transport type
    pub transport_type: TransportType,
}

impl TransportEndpoint {
    /// Builds a new [`TransportEndpoint::endpoint`] from the provided string, checking that it is
    /// valid.
    pub fn new(transport_endpoint: String) -> Result<Self, Error> {
        let rgb_transport = RgbTransport::from_str(&transport_endpoint)?;
        TransportEndpoint::try_from(rgb_transport)
    }

    /// Return the transport type of this transport endpoint.
    pub fn transport_type(&self) -> TransportType {
        self.transport_type
    }
}

impl TryFrom<RgbTransport> for TransportEndpoint {
    type Error = Error;

    fn try_from(x: RgbTransport) -> Result<Self, Self::Error> {
        match x {
            RgbTransport::JsonRpc { tls, host } => Ok(TransportEndpoint {
                endpoint: format!("http{}://{host}", if tls { "s" } else { "" }),
                transport_type: TransportType::JsonRpc,
            }),
            _ => Err(Error::UnsupportedTransportType),
        }
    }
}

/// An RGB invoice.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct Invoice {
    /// The RGB invoice string
    invoice_string: String,
    /// The data of the RGB invoice
    pub(crate) invoice_data: InvoiceData,
}

impl Invoice {
    /// Parse the provided `invoice_string`.
    /// Throws an error if the provided string is not a valid RGB invoice.
    pub fn new(invoice_string: String) -> Result<Self, Error> {
        let decoded = RgbInvoice::from_str(&invoice_string).map_err(|e| Error::InvalidInvoice {
            details: e.to_string(),
        })?;
        let asset_id = decoded.contract.map(|cid| cid.to_string());
        let asset_schema = if let Some(schema_id) = decoded.schema {
            Some(<AssetSchema as std::convert::TryFrom<_>>::try_from(
                schema_id,
            )?)
        } else {
            None
        };
        let assignment_name = decoded.assignment_name.map(|a| a.to_string());
        let assignment = match asset_schema {
            None => match (decoded.assignment_state, assignment_name.as_deref()) {
                (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER)) => {
                    Assignment::Fungible(v.value())
                }
                (Some(InvoiceState::Amount(v)), Some(RGB_STATE_INFLATION_ALLOWANCE)) => {
                    Assignment::InflationRight(v.value())
                }
                (Some(InvoiceState::Amount(_)), _) => Assignment::Any,
                (Some(InvoiceState::Data(_)), Some(RGB_STATE_ASSET_OWNER) | None) => {
                    Assignment::NonFungible
                }
                (None, None) => Assignment::Any,
                (_, _) => {
                    return Err(Error::InvalidInvoice {
                        details: s!("unsupported assignment"),
                    });
                }
            },
            Some(AssetSchema::Nia) => {
                match (decoded.assignment_state, assignment_name.as_deref()) {
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER) | None) => {
                        Assignment::Fungible(v.value())
                    }
                    (None, Some(RGB_STATE_ASSET_OWNER) | None) => Assignment::Fungible(0),
                    (_, _) => {
                        return Err(Error::InvalidInvoice {
                            details: s!("invalid assignment"),
                        });
                    }
                }
            }
            Some(AssetSchema::Ifa) => {
                match (decoded.assignment_state, assignment_name.as_deref()) {
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER)) => {
                        Assignment::Fungible(v.value())
                    }
                    (None, Some(RGB_STATE_ASSET_OWNER)) => Assignment::Fungible(0),
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_INFLATION_ALLOWANCE)) => {
                        Assignment::InflationRight(v.value())
                    }
                    (None, Some(RGB_STATE_INFLATION_ALLOWANCE)) => Assignment::InflationRight(0),
                    (Some(InvoiceState::Void) | None, Some(RGB_STATE_LINK_RIGHT)) => {
                        Assignment::LinkRight
                    }
                    (Some(InvoiceState::Amount(_)), None) => Assignment::Any,
                    (None, None) => Assignment::Any,
                    (_, _) => {
                        return Err(Error::InvalidInvoice {
                            details: s!("invalid assignment"),
                        });
                    }
                }
            }
            Some(AssetSchema::Bfa) => {
                match (decoded.assignment_state, assignment_name.as_deref()) {
                    (Some(InvoiceState::Amount(v)), Some(RGB_STATE_ASSET_OWNER)) => {
                        Assignment::Fungible(v.value())
                    }
                    (None, Some(RGB_STATE_ASSET_OWNER)) => Assignment::Fungible(0),
                    (Some(InvoiceState::Void) | None, Some(RGB_STATE_LINK_RIGHT)) => {
                        Assignment::LinkRight
                    }
                    (Some(InvoiceState::Void) | None, Some(RGB_STATE_MINT_RIGHT)) => {
                        Assignment::MintRight
                    }
                    (Some(InvoiceState::Amount(_)), None) => Assignment::Any,
                    (None, None) => Assignment::Any,
                    (_, _) => {
                        return Err(Error::InvalidInvoice {
                            details: s!("invalid assignment"),
                        });
                    }
                }
            }
        };
        let recipient_id = decoded.beneficiary.to_string();
        let transport_endpoints: Vec<String> =
            decoded.transports.iter().map(|t| t.to_string()).collect();

        let layer_1 = decoded.beneficiary.layer1();
        let network = match layer_1 {
            Layer1::Bitcoin => decoded.beneficiary.chain_network().try_into().unwrap(),
            _ => {
                return Err(Error::UnsupportedLayer1 {
                    layer_1: layer_1.to_string(),
                });
            }
        };

        let invoice_data = InvoiceData {
            recipient_id,
            asset_schema,
            asset_id,
            assignment,
            assignment_name,
            expiration_timestamp: decoded.expiry.map(|t| t as u64),
            transport_endpoints,
            network,
            public_key: decoded.public_key,
            encryption: Encryption::from(decoded.encryption),
            unknown_query_params: decoded.unknown_query.into_iter().collect(),
        };

        Ok(Invoice {
            invoice_string,
            invoice_data,
        })
    }

    /// Return the data associated with this [`Invoice`].
    pub fn invoice_data(&self) -> InvoiceData {
        self.invoice_data.clone()
    }

    /// Return the string associated with this [`Invoice`].
    pub fn invoice_string(&self) -> String {
        self.invoice_string.clone()
    }
}

/// The data of an RGB invoice.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct InvoiceData {
    /// ID of the receive operation (blinded UTXO or Bitcoin script)
    pub recipient_id: String,
    /// RGB schema
    pub asset_schema: Option<AssetSchema>,
    /// RGB asset ID
    pub asset_id: Option<String>,
    /// RGB assignment
    pub assignment: Assignment,
    /// RGB assignment name
    pub assignment_name: Option<String>,
    /// Bitcoin network
    pub network: BitcoinNetwork,
    /// Invoice expiration
    pub expiration_timestamp: Option<u64>,
    /// Transport endpoints
    pub transport_endpoints: Vec<String>,
    /// Recipient's Ed25519 public key (hex-encoded), from the invoice `pk` field (if present)
    pub public_key: Option<String>,
    /// Consignment/media encryption preference, from the invoice `enc` field
    pub encryption: Encryption,
    /// Unknown query parameters carried by the invoice
    pub unknown_query_params: HashMap<String, String>,
}

/// Extract the witness script if recipient is a Witness one
pub fn script_buf_from_recipient_id(recipient_id: String) -> Result<Option<ScriptBuf>, Error> {
    let xchainnet_beneficiary =
        XChainNet::<Beneficiary>::from_str(&recipient_id).map_err(|_| Error::InvalidRecipientID)?;
    match xchainnet_beneficiary.into_inner() {
        Beneficiary::WitnessVout(pay_2_vout, _) => {
            let script_buf = pay_2_vout.to_script();
            Ok(Some(script_buf))
        }
        Beneficiary::BlindedSeal(_) => Ok(None),
    }
}

pub(crate) fn beneficiary_from_script_buf(script_buf: ScriptBuf) -> Beneficiary {
    let address_payload = AddressPayload::from_script(&script_buf).unwrap();
    Beneficiary::WitnessVout(Pay2Vout::new(address_payload), None)
}

/// Return the recipient ID for a specific script buf
pub fn recipient_id_from_script_buf(
    script_buf: ScriptBuf,
    bitcoin_network: BitcoinNetwork,
) -> String {
    let beneficiary = beneficiary_from_script_buf(script_buf);
    XChainNet::with(bitcoin_network.into(), beneficiary).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLINDED_RECIPIENT_ID: &str =
        "bcrt:utxob:tjVmHbI2-U0_umHn-bU4cmP6-l3VW00H-ewoi2uz-XZG6O3i-wUFBW";

    #[test]
    fn new_recipient_info() {
        let recipient_info = RecipientInfo::new(BLINDED_RECIPIENT_ID.to_string()).unwrap();
        assert_eq!(recipient_info.recipient_id, BLINDED_RECIPIENT_ID);
        assert_eq!(recipient_info.recipient_type, RecipientType::Blind);
        assert_eq!(recipient_info.network, BitcoinNetwork::Regtest);

        let result = RecipientInfo::new(s!("invalid"));
        assert!(matches!(result, Err(Error::InvalidRecipientID)));
    }

    fn mint_right_invoice(schema: AssetSchema) -> String {
        RgbInvoiceBuilder::new(XChainNet::<Beneficiary>::from_str(BLINDED_RECIPIENT_ID).unwrap())
            .set_schema(schema.into())
            .set_contract(ContractId::from([1u8; 32]))
            .set_void()
            .set_assignment_name(RGB_STATE_MINT_RIGHT)
            .finish()
            .to_string()
    }

    #[test]
    fn new_invoice() {
        let invoice = Invoice::new(format!(
            "rgb:~/~/ae/{BLINDED_RECIPIENT_ID}?assignment_name=assetOwner"
        ))
        .unwrap();
        let invoice_data = invoice.invoice_data();
        assert_eq!(invoice_data.recipient_id, BLINDED_RECIPIENT_ID);
        assert_eq!(invoice_data.assignment, Assignment::Fungible(1));
        assert_eq!(
            invoice_data.assignment_name.as_deref(),
            Some(RGB_STATE_ASSET_OWNER)
        );
        assert_eq!(invoice_data.network, BitcoinNetwork::Regtest);
        assert_eq!(invoice_data.encryption, Encryption::Required);

        let invoice = Invoice::new(mint_right_invoice(AssetSchema::Bfa)).unwrap();
        let invoice_data = invoice.invoice_data();
        assert_eq!(invoice_data.assignment, Assignment::MintRight);
        assert_eq!(
            invoice_data.assignment_name.as_deref(),
            Some(RGB_STATE_MINT_RIGHT)
        );

        let result = Invoice::new(s!("invalid"));
        assert!(matches!(result, Err(Error::InvalidInvoice { details: _ })));
    }

    #[test]
    fn new_transport_endpoint() {
        let transport_endpoint =
            TransportEndpoint::new(s!("rpc://127.0.0.1:3000/json-rpc")).unwrap();
        assert_eq!(
            transport_endpoint.endpoint,
            "http://127.0.0.1:3000/json-rpc"
        );
        assert_eq!(transport_endpoint.transport_type(), TransportType::JsonRpc);

        let result = TransportEndpoint::new(s!("ws://127.0.0.1:3000/json-rpc"));
        assert!(matches!(result, Err(Error::UnsupportedTransportType)));

        let result = TransportEndpoint::new(s!("127.0.0.1:3000/json-rpc"));
        assert!(matches!(
            result,
            Err(Error::InvalidTransportEndpoint { details: _ })
        ));
    }

    #[test]
    fn outpoint_conversions() {
        let outpoint = Outpoint {
            txid: s!("0000000000000000000000000000000000000000000000000000000000000001"),
            vout: 7,
        };
        assert_eq!(
            outpoint.to_string(),
            "0000000000000000000000000000000000000000000000000000000000000001:7"
        );
        assert_eq!(Outpoint::from(OutPoint::from(outpoint.clone())), outpoint);
    }

    #[test]
    fn recipient_type_display() {
        assert_eq!(RecipientType::Blind.to_string(), "Blind");
        assert_eq!(RecipientType::Witness.to_string(), "Witness");
    }
}

//! SPV proofs fill, which needs no wallet.

use super::*;

use crate::wallet::online::connect_indexer;

/// Report of [`fill_spv_proofs`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct SpvProofFill {
    /// Witness transactions that still have no SPV proof, sorted by TXID
    pub without_proof: Vec<WitnessWithoutProof>,
    /// Whether at least one proof was set
    pub rewritten: bool,
}

/// A witness transaction the indexer gave no SPV proof for.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[cfg_attr(feature = "camel_case", serde(rename_all = "camelCase"))]
pub struct WitnessWithoutProof {
    /// TXID of the witness transaction
    pub txid: String,
    /// Why the indexer gave no proof
    pub details: String,
}

/// Fill in the SPV proofs the witnesses of a transfer consignment miss, from an esplora indexer.
///
/// Only witnesses without a proof are looked up. Existing proofs are kept.
/// Returns the report and, only if a proof was set, the new consignment bytes.
pub async fn fill_spv_proofs(
    consignment_bytes: &[u8],
    indexer_url: &str,
) -> Result<(SpvProofFill, Option<Vec<u8>>), Error> {
    let Ok(UniversalFile::Transfer(mut consignment)) = UniversalFile::load(consignment_bytes)
    else {
        return Err(Error::InvalidConsignment);
    };
    let bitcoin_network = BitcoinNetwork::try_from(consignment.genesis().chain_net)?;
    #[cfg(target_arch = "wasm32")]
    let _hold = crate::utils::NoStoreHold::new(indexer_url);
    let indexer = connect_indexer(
        crate::utils::esplora_indexer(indexer_url),
        bitcoin_network,
        "not a valid esplora server",
    )
    .await?;

    let missing: BTreeSet<RgbTxid> = consignment
        .bundled_witnesses()
        .filter(|witness_bundle| witness_bundle.spv_proof.is_none())
        .map(|witness_bundle| witness_bundle.witness_id())
        .collect();

    let mut fill = SpvProofFill {
        without_proof: vec![],
        rewritten: false,
    };
    for witness_id in missing {
        let txid = Txid::from_str(&witness_id.to_string()).map_err(|_| Error::InvalidTxid)?;
        match indexer.get_merkle_proof(&txid).await? {
            None => fill.without_proof.push(WitnessWithoutProof {
                txid: witness_id.to_string(),
                details: s!("TX is unknown or not mined"),
            }),
            Some(merkle_proof) => {
                let proof = spv_proof_from_esplora(merkle_proof).ok_or_else(|| Error::Indexer {
                    details: WitnessResolverError::InvalidResolverData.to_string(),
                })?;
                consignment.modify_bundle(witness_id, |witness_bundle| {
                    if witness_bundle.spv_proof.is_none() {
                        witness_bundle.spv_proof = Some(proof.clone());
                    }
                });
                fill.rewritten = true;
            }
        }
    }
    // the lookup order is the internal byte order, so sort by TXID string
    fill.without_proof.sort_by(|a, b| a.txid.cmp(&b.txid));

    if !fill.rewritten {
        return Ok((fill, None));
    }
    let mut bytes = vec![];
    consignment.save(&mut bytes)?;
    Ok((fill, Some(bytes)))
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::wallet::online::fake_esplora;
    use rgbstd::bitcoin::absolute::LockTime;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    const SEND: &[u8] = include_bytes!("../../tests/fixtures/inspection/send.rgbc");
    const NODE: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const UNMINED: &str = "TX is unknown or not mined";

    fn merkle_ok() -> (&'static str, Vec<u8>) {
        (
            "200 OK",
            format!(r#"{{"block_height":1,"merkle":["{NODE}"],"pos":0}}"#).into_bytes(),
        )
    }

    fn not_found() -> (&'static str, Vec<u8>) {
        ("404 Not Found", vec![])
    }

    fn p_old() -> SpvProof {
        SpvProof {
            block_height: NonZeroU32::new(7).unwrap(),
            pos: 3,
            merkle: Confined::try_from(vec![]).unwrap(),
        }
    }

    fn p_new() -> SpvProof {
        SpvProof {
            block_height: NonZeroU32::new(1).unwrap(),
            pos: 0,
            merkle: Confined::try_from(vec![TxMerkleNode::from_byte_array([0x11; 32])]).unwrap(),
        }
    }

    fn with_bundles(f: impl FnOnce(WitnessBundle) -> Vec<WitnessBundle>) -> Vec<u8> {
        // keep the first bundle only
        let mut transfer = RgbTransfer::load(SEND).unwrap();
        let bundle = transfer.bundles[0].clone();
        transfer.bundles = Confined::try_from(f(bundle)).unwrap();
        let mut bytes = vec![];
        transfer.save(&mut bytes).unwrap();
        bytes
    }

    fn with_proofs(proof: Option<SpvProof>) -> Vec<u8> {
        with_bundles(|mut bundle| {
            bundle.spv_proof = proof;
            vec![bundle]
        })
    }

    fn variant(bundle: &WitnessBundle, n: u32) -> WitnessBundle {
        let mut bundle = bundle.clone();
        bundle.tx.lock_time = LockTime::from_consensus(n);
        bundle.spv_proof = None;
        bundle
    }

    fn esplora(
        merkle: impl Fn(&str) -> (&'static str, Vec<u8>) + Send + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let lookups = Arc::new(AtomicUsize::new(0));
        let counter = lookups.clone();
        let url = fake_esplora(move |path| {
            if path.ends_with("/block-height/0") {
                (
                    "200 OK",
                    b"0f9188f13cb7b2c71f2a335e3a4fc328bf5beb436012afca590b1a11466e2206".to_vec(),
                )
            } else if path.contains("/merkle-proof") {
                counter.fetch_add(1, Ordering::SeqCst);
                merkle(path)
            } else {
                not_found()
            }
        });
        (url, lookups)
    }

    fn fill(bytes: &[u8], url: &str) -> Result<(SpvProofFill, Option<Vec<u8>>), Error> {
        crate::block_on(fill_spv_proofs(bytes, url))
    }

    fn proofs(bytes: &[u8]) -> Vec<Option<SpvProof>> {
        let transfer = RgbTransfer::load(bytes).unwrap();
        transfer
            .bundles
            .iter()
            .map(|b| b.spv_proof.clone())
            .collect()
    }

    fn unmined(txid: String) -> WitnessWithoutProof {
        WitnessWithoutProof {
            txid,
            details: UNMINED.to_string(),
        }
    }

    #[test]
    fn fill_spv_proofs_fail() {
        let unreachable = "http://127.0.0.1:1";

        // not a consignment
        assert_eq!(
            fill(b"not a consignment", unreachable),
            Err(Error::InvalidConsignment)
        );

        // a contract consignment has no witness
        let mut contract = vec![];
        RgbTransfer::load(SEND)
            .unwrap()
            .into_contract()
            .save(&mut contract)
            .unwrap();
        assert_eq!(fill(&contract, unreachable), Err(Error::InvalidConsignment));

        // an indexer for another network
        let mainnet = fake_esplora(|_| {
            (
                "200 OK",
                b"000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f".to_vec(),
            )
        });
        assert_eq!(
            fill(&with_proofs(None), &mainnet),
            Err(Error::InvalidIndexer {
                details: s!("resolver is for another chain-network pair")
            })
        );
    }

    #[test]
    fn fill_spv_proofs_lookups() {
        let bytes = with_proofs(None);

        // a proof that fails conversion fails the call
        let (url, _) = esplora(|_| {
            (
                "200 OK",
                br#"{"block_height":1,"merkle":[],"pos":4294967296}"#.to_vec(),
            )
        });
        assert_eq!(
            fill(&bytes, &url),
            Err(Error::Indexer {
                details: WitnessResolverError::InvalidResolverData.to_string()
            })
        );

        // a client error fails the call
        let (url, _) = esplora(|_| ("400 Bad Request", vec![]));
        assert!(matches!(fill(&bytes, &url), Err(Error::Indexer { .. })));
    }

    #[test]
    fn fill_spv_proofs_bundles() {
        // a shared witness is looked up once and only bundles without a proof get one
        let bytes = with_bundles(|bundle| {
            vec![
                WitnessBundle {
                    spv_proof: Some(p_old()),
                    ..bundle.clone()
                },
                WitnessBundle {
                    spv_proof: None,
                    ..bundle.clone()
                },
                WitnessBundle {
                    spv_proof: None,
                    ..bundle
                },
            ]
        });
        let (url, lookups) = esplora(|_| merkle_ok());
        let (report, filled) = fill(&bytes, &url).unwrap();
        assert_eq!(
            report,
            SpvProofFill {
                without_proof: vec![],
                rewritten: true,
            }
        );
        assert_eq!(
            proofs(&filled.unwrap()),
            vec![Some(p_old()), Some(p_new()), Some(p_new())]
        );
        assert_eq!(lookups.load(Ordering::SeqCst), 1);

        // two witnesses whose byte order and string order differ
        let base = RgbTransfer::load(SEND).unwrap().bundles[0].clone();
        let id1 = variant(&base, 1).witness_id();
        let id2 = variant(&base, 2).witness_id();
        let bytes = with_bundles(|bundle| vec![variant(&bundle, 1), variant(&bundle, 2)]);

        // the report is sorted by TXID string
        let mut expected = vec![unmined(id1.to_string()), unmined(id2.to_string())];
        expected.sort_by(|a, b| a.txid.cmp(&b.txid));
        let (url, lookups) = esplora(|_| not_found());
        assert_eq!(
            fill(&bytes, &url),
            Ok((
                SpvProofFill {
                    without_proof: expected,
                    rewritten: false,
                },
                None
            ))
        );
        assert_eq!(lookups.load(Ordering::SeqCst), 2);

        // a mixed fill sets one proof and reports the other witness
        let proven = id1.to_string();
        let (url, _) = esplora(move |path| {
            if path.contains(&proven) {
                merkle_ok()
            } else {
                not_found()
            }
        });
        let (report, filled) = fill(&bytes, &url).unwrap();
        assert_eq!(
            report,
            SpvProofFill {
                without_proof: vec![unmined(id2.to_string())],
                rewritten: true,
            }
        );
        assert_eq!(proofs(&filled.unwrap()), vec![Some(p_new()), None]);
    }
}

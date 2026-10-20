//! In-memory RGB store.
//!
//! Implements `RgbStore` with one map per table of the rgb-ops SQLite schema.
//! The tables are saved as a single strict-encoded blob.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::io;
use std::num::NonZeroU32;

use rgbstd::{
    AssignmentType, BundleId, ContractId, Genesis, GlobalStateType, GraphSeal, OpId,
    Operation as _, Opout, RevealedData, Schema, SchemaId, SecretSeal, TransitionBundle,
    TransitionType, Vout,
    containers::{SealWitness, SpvProof},
    persistence::{
        AllocKind, AllocSeal, AllocationFilter, AllocationRow, AllocationWrite, GlobalStateRow,
        GlobalStateWrite, Lib, LibId, RgbStore, TxBegin, TxMode, Visibility, decode, encode,
    },
    rgbcore::{
        bitcoin::{OutPoint, Transaction, Txid},
        commit_verify::Conceal,
        vm::WitnessOrd,
    },
};
use strict_encoding::{StrictDecode, StrictEncode};
use strict_types::{TypeLib, TypeLibId};

const FORMAT_VERSION: u8 = 1;

type AllocKey = (ContractId, OpId, AssignmentType, u16);
type GlobalKey = (ContractId, GlobalStateType, OpId, u16);

#[derive(Clone, Debug, Default)]
struct Tables {
    schemata: BTreeMap<SchemaId, Schema>,
    type_libs: BTreeMap<TypeLibId, TypeLib>,
    schema_type_libs: BTreeSet<(SchemaId, TypeLibId)>,
    aluvm_libs: BTreeMap<LibId, Lib>,
    geneses: BTreeMap<ContractId, Genesis>,
    bundles: BTreeMap<BundleId, TransitionBundle>,
    witnesses: BTreeMap<Txid, SealWitness>,
    secret_seals: BTreeMap<SecretSeal, GraphSeal>,
    globals: BTreeMap<GlobalKey, GlobalStateRow>,
    allocations: BTreeMap<AllocKey, AllocationRow>,
    witness_ords: BTreeMap<Txid, WitnessOrd>,
    invalid_ops: BTreeSet<OpId>,
    bundle_contracts: BTreeMap<BundleId, ContractId>,
    bundle_witnesses: BTreeSet<(BundleId, Txid)>,
    op_bundles: BTreeMap<OpId, BundleId>,
    op_inputs: BTreeMap<(Opout, OpId), BundleId>,
    outpoint_opouts: BTreeSet<(ContractId, OutPoint, Opout)>,
    secret_seal_opouts: BTreeSet<(ContractId, SecretSeal, Opout)>,
}

impl Tables {
    fn ord_is_visible(&self, txid: &Txid) -> bool {
        self.witness_ords
            .get(txid)
            .is_some_and(|ord| !matches!(ord, WitnessOrd::Archived))
    }

    fn bundle_is_visible(&self, bundle_id: &BundleId) -> bool {
        self.bundle_witnesses
            .range((*bundle_id, min_txid())..)
            .take_while(|(b, _)| b == bundle_id)
            .any(|(_, txid)| self.ord_is_visible(txid))
    }

    fn row_is_visible(&self, op: &OpId, bundle_id: &Option<BundleId>) -> bool {
        bundle_id.is_none_or(|b| self.bundle_is_visible(&b)) && !self.invalid_ops.contains(op)
    }

    fn witnesses_of(&self, bundle_id: &BundleId) -> Vec<Txid> {
        self.bundle_witnesses
            .range((*bundle_id, min_txid())..)
            .take_while(|(b, _)| b == bundle_id)
            .map(|(_, txid)| *txid)
            .collect()
    }

    fn allocations_at(
        &self,
        contract: Option<ContractId>,
        outpoints: &BTreeSet<OutPoint>,
        visibility: Visibility,
        keep: impl Fn(&AllocationRow) -> bool,
    ) -> Vec<(ContractId, AllocationRow)> {
        let mut out = Vec::new();
        for ((cid, ..), row) in &self.allocations {
            if contract.is_some_and(|c| c != *cid) || !keep(row) {
                continue;
            }
            let valid = |witness: Option<&Txid>| match visibility {
                Visibility::All => true,
                Visibility::Valid => {
                    !self.invalid_ops.contains(&row.opout.op)
                        && match witness {
                            Some(txid) => self.ord_is_visible(txid),
                            None => row.bundle_id.is_none_or(|b| self.bundle_is_visible(&b)),
                        }
                }
            };
            if let Some(txid) = row.seal.txid {
                if outpoints.contains(&OutPoint::new(txid, row.seal.vout.into_u32())) && valid(None)
                {
                    out.push((*cid, row.clone()));
                }
            } else if let Some(bundle_id) = row.bundle_id {
                for txid in self.witnesses_of(&bundle_id) {
                    if outpoints.contains(&OutPoint::new(txid, row.seal.vout.into_u32()))
                        && valid(Some(&txid))
                    {
                        let mut row = row.clone();
                        row.seal.txid = Some(txid);
                        out.push((*cid, row));
                    }
                }
            }
        }
        out
    }
}

fn min_txid() -> Txid {
    Txid::from_raw_hash(rgbstd::rgbcore::bitcoin::hashes::Hash::all_zeros())
}

/// In-memory implementation of `RgbStore`.
#[derive(Debug, Default)]
pub(crate) struct MemRgbStore {
    tables: RefCell<Tables>,
    /// `None`: no transaction. `Some(None)`: read transaction.
    /// `Some(Some(_))`: write transaction with the tables to restore on rollback.
    tx: RefCell<Option<Option<Tables>>>,
}

impl RgbStore for MemRgbStore {
    type Error = Infallible;

    fn begin(&self, mode: TxMode) -> Result<TxBegin, Self::Error> {
        let mut tx = self.tx.borrow_mut();
        if tx.is_some() {
            return Ok(TxBegin::AlreadyOpen);
        }
        *tx = Some(match mode {
            TxMode::Read => None,
            TxMode::Write => Some(self.tables.borrow().clone()),
        });
        Ok(TxBegin::Opened)
    }

    fn commit(&self) -> Result<(), Self::Error> {
        self.tx.borrow_mut().take();
        Ok(())
    }

    fn rollback(&self) {
        if let Some(Some(saved)) = self.tx.borrow_mut().take() {
            *self.tables.borrow_mut() = saved;
        }
    }

    fn schema(&self, schema_id: SchemaId) -> Result<Option<Schema>, Self::Error> {
        Ok(self.tables.borrow().schemata.get(&schema_id).cloned())
    }

    fn put_schema(&mut self, schema: &Schema) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .schemata
            .entry(schema.schema_id())
            .or_insert_with(|| schema.clone());
        Ok(())
    }

    fn schemata(&self) -> impl Iterator<Item = Result<Schema, Self::Error>> + '_ {
        let all: Vec<_> = self.tables.borrow().schemata.values().cloned().collect();
        all.into_iter().map(Ok)
    }

    fn type_libs(&self, schema_id: SchemaId) -> Result<BTreeMap<TypeLibId, TypeLib>, Self::Error> {
        let t = self.tables.borrow();
        Ok(t.schema_type_libs
            .iter()
            .filter(|(s, _)| *s == schema_id)
            .filter_map(|(_, id)| t.type_libs.get(id).map(|lib| (*id, lib.clone())))
            .collect())
    }

    fn put_type_lib(&mut self, schema_id: SchemaId, type_lib: &TypeLib) -> Result<(), Self::Error> {
        let t = self.tables.get_mut();
        let id = type_lib.id();
        t.type_libs.entry(id).or_insert_with(|| type_lib.clone());
        t.schema_type_libs.insert((schema_id, id));
        Ok(())
    }

    fn aluvm_lib(&self, aluvm_lib_id: LibId) -> Result<Option<Lib>, Self::Error> {
        Ok(self.tables.borrow().aluvm_libs.get(&aluvm_lib_id).cloned())
    }

    fn put_aluvm_lib(&mut self, aluvm_lib: &Lib) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .aluvm_libs
            .entry(aluvm_lib.id())
            .or_insert_with(|| aluvm_lib.clone());
        Ok(())
    }

    fn genesis(&self, contract_id: ContractId) -> Result<Option<Genesis>, Self::Error> {
        Ok(self.tables.borrow().geneses.get(&contract_id).cloned())
    }

    fn put_genesis(&mut self, genesis: &Genesis) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .geneses
            .insert(genesis.contract_id(), genesis.clone());
        Ok(())
    }

    fn geneses(&self) -> impl Iterator<Item = Result<Genesis, Self::Error>> + '_ {
        let all: Vec<_> = self.tables.borrow().geneses.values().cloned().collect();
        all.into_iter().map(Ok)
    }

    fn contract_schema(&self, contract_id: ContractId) -> Result<Option<Schema>, Self::Error> {
        let t = self.tables.borrow();
        Ok(t.geneses
            .get(&contract_id)
            .and_then(|g| t.schemata.get(&g.schema_id))
            .cloned())
    }

    fn bundle(&self, bundle_id: BundleId) -> Result<Option<TransitionBundle>, Self::Error> {
        Ok(self.tables.borrow().bundles.get(&bundle_id).cloned())
    }

    fn put_bundle(&mut self, bundle: &TransitionBundle) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .bundles
            .insert(bundle.bundle_id(), bundle.clone());
        Ok(())
    }

    fn witness(&self, txid: Txid) -> Result<Option<SealWitness>, Self::Error> {
        Ok(self.tables.borrow().witnesses.get(&txid).cloned())
    }

    fn put_witness(&mut self, witness: &SealWitness) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .witnesses
            .insert(witness.witness_id(), witness.clone());
        Ok(())
    }

    fn witness_tx(&self, txid: Txid) -> Result<Option<Transaction>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .witnesses
            .get(&txid)
            .map(|w| w.tx.clone()))
    }

    fn witness_spv_proof(&self, txid: Txid) -> Result<Option<SpvProof>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .witnesses
            .get(&txid)
            .and_then(|w| w.spv_proof.clone()))
    }

    fn set_witness_spv_proof(
        &mut self,
        txid: Txid,
        spv_proof: Option<&SpvProof>,
    ) -> Result<bool, Self::Error> {
        let Some(witness) = self.tables.get_mut().witnesses.get_mut(&txid) else {
            return Ok(false);
        };
        if witness.spv_proof.as_ref() == spv_proof {
            return Ok(false);
        }
        witness.spv_proof = spv_proof.cloned();
        Ok(true)
    }

    fn seal_of_secret(&self, secret_seal: SecretSeal) -> Result<Option<GraphSeal>, Self::Error> {
        Ok(self.tables.borrow().secret_seals.get(&secret_seal).copied())
    }

    fn put_secret_seal(
        &mut self,
        graph_seal: &GraphSeal,
        secret_seal: SecretSeal,
    ) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .secret_seals
            .entry(secret_seal)
            .or_insert(*graph_seal);
        Ok(())
    }

    fn globals(
        &self,
        contract_id: ContractId,
        type_id: GlobalStateType,
        visibility: Visibility,
    ) -> Result<Vec<GlobalStateRow>, Self::Error> {
        let t = self.tables.borrow();
        Ok(t.globals
            .iter()
            .filter(|((cid, ty, ..), _)| *cid == contract_id && *ty == type_id)
            .filter(|(_, row)| {
                visibility == Visibility::All
                    || row.bundle_id.is_none_or(|b| t.bundle_is_visible(&b))
            })
            .map(|(_, row)| row.clone())
            .collect())
    }

    fn put_global(&mut self, global_state: GlobalStateWrite<'_>) -> Result<(), Self::Error> {
        let key = (
            global_state.contract_id,
            global_state.type_id,
            global_state.opid,
            global_state.index,
        );
        self.tables
            .get_mut()
            .globals
            .entry(key)
            .or_insert_with(|| GlobalStateRow {
                opid: global_state.opid,
                index: global_state.index,
                nonce: global_state.nonce,
                bundle_id: global_state.bundle_id,
                transition_type: global_state.transition_type,
                value: global_state.value.clone(),
            });
        Ok(())
    }

    fn allocations(
        &self,
        contract_id: ContractId,
        filter: AllocationFilter<'_>,
    ) -> Result<Vec<AllocationRow>, Self::Error> {
        let t = self.tables.borrow();
        let keep = |row: &AllocationRow| {
            filter.kind.is_none_or(|k| k == row.kind)
                && filter.type_id.is_none_or(|ty| ty == row.opout.ty)
        };
        if let Some(outpoints) = filter.outpoints {
            return Ok(t
                .allocations_at(Some(contract_id), outpoints, filter.visibility, keep)
                .into_iter()
                .map(|(_, row)| row)
                .collect());
        }
        Ok(t.allocations
            .iter()
            .filter(|((cid, ..), row)| {
                *cid == contract_id
                    && keep(row)
                    && (filter.visibility == Visibility::All
                        || t.row_is_visible(&row.opout.op, &row.bundle_id))
            })
            .map(|(_, row)| row.clone())
            .collect())
    }

    fn all_allocations_at_outpoints(
        &self,
        outpoints: &BTreeSet<OutPoint>,
        visibility: Visibility,
    ) -> Result<Vec<(ContractId, AllocationRow)>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .allocations_at(None, outpoints, visibility, |_| true))
    }

    fn put_allocation(&mut self, allocation: AllocationWrite<'_>) -> Result<(), Self::Error> {
        let key = (
            allocation.contract_id,
            allocation.opout.op,
            allocation.opout.ty,
            allocation.opout.no,
        );
        self.tables
            .get_mut()
            .allocations
            .entry(key)
            .or_insert_with(|| AllocationRow {
                kind: allocation.kind,
                opout: allocation.opout,
                seal: allocation.seal,
                bundle_id: allocation.bundle_id,
                value: allocation.value.to_vec(),
            });
        Ok(())
    }

    fn witness_ord(&self, txid: Txid) -> Result<Option<WitnessOrd>, Self::Error> {
        Ok(self.tables.borrow().witness_ords.get(&txid).copied())
    }

    fn witness_ords(
        &self,
        txids: &BTreeSet<Txid>,
    ) -> Result<BTreeMap<Txid, WitnessOrd>, Self::Error> {
        let t = self.tables.borrow();
        Ok(txids
            .iter()
            .filter_map(|txid| t.witness_ords.get(txid).map(|ord| (*txid, *ord)))
            .collect())
    }

    fn witness_ords_to_refresh(
        &self,
        min_height: NonZeroU32,
    ) -> Result<BTreeSet<Txid>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .witness_ords
            .iter()
            .filter(|(_, ord)| match ord {
                WitnessOrd::Mined(pos) => pos.height() >= min_height,
                WitnessOrd::Tentative | WitnessOrd::Archived => true,
                WitnessOrd::Ignored => false,
            })
            .map(|(txid, _)| *txid)
            .collect())
    }

    fn all_witness_ords(&self) -> Result<BTreeMap<Txid, WitnessOrd>, Self::Error> {
        Ok(self.tables.borrow().witness_ords.clone())
    }

    fn put_witness_ord(&mut self, txid: Txid, ord: WitnessOrd) -> Result<(), Self::Error> {
        self.tables.get_mut().witness_ords.insert(txid, ord);
        Ok(())
    }

    fn all_invalid_ops(&self) -> Result<BTreeSet<OpId>, Self::Error> {
        Ok(self.tables.borrow().invalid_ops.clone())
    }

    fn any_op_invalid(&self, opids: &BTreeSet<OpId>) -> Result<bool, Self::Error> {
        let t = self.tables.borrow();
        Ok(opids.iter().any(|op| t.invalid_ops.contains(op)))
    }

    fn set_op_validity(&mut self, opid: OpId, valid: bool) -> Result<(), Self::Error> {
        let invalid = &mut self.tables.get_mut().invalid_ops;
        if valid {
            invalid.remove(&opid);
        } else {
            invalid.insert(opid);
        }
        Ok(())
    }

    fn contract_registered(&self, contract_id: ContractId) -> Result<bool, Self::Error> {
        Ok(self.tables.borrow().geneses.contains_key(&contract_id))
    }

    fn bundle_contract(&self, bundle_id: BundleId) -> Result<Option<ContractId>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .bundle_contracts
            .get(&bundle_id)
            .copied())
    }

    fn put_bundle_contract(
        &mut self,
        bundle_id: BundleId,
        contract_id: ContractId,
    ) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .bundle_contracts
            .insert(bundle_id, contract_id);
        Ok(())
    }

    fn bundle_witnesses(&self, bundle_id: BundleId) -> Result<BTreeSet<Txid>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .witnesses_of(&bundle_id)
            .into_iter()
            .collect())
    }

    fn witnesses_of_bundles(
        &self,
        bundle_ids: &BTreeSet<BundleId>,
    ) -> Result<BTreeMap<BundleId, BTreeSet<Txid>>, Self::Error> {
        let t = self.tables.borrow();
        let mut out: BTreeMap<BundleId, BTreeSet<Txid>> = BTreeMap::new();
        for bundle_id in bundle_ids {
            let txids: BTreeSet<Txid> = t.witnesses_of(bundle_id).into_iter().collect();
            if !txids.is_empty() {
                out.insert(*bundle_id, txids);
            }
        }
        Ok(out)
    }

    fn bundles_of_witness(&self, txid: Txid) -> Result<BTreeSet<BundleId>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .bundle_witnesses
            .iter()
            .filter(|(_, t)| *t == txid)
            .map(|(b, _)| *b)
            .collect())
    }

    fn put_bundle_witness(&mut self, bundle_id: BundleId, txid: Txid) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .bundle_witnesses
            .insert((bundle_id, txid));
        Ok(())
    }

    fn bundle_of_op(&self, opid: OpId) -> Result<Option<BundleId>, Self::Error> {
        Ok(self.tables.borrow().op_bundles.get(&opid).copied())
    }

    fn ops_in_bundle(&self, bundle_id: BundleId) -> Result<BTreeSet<OpId>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .op_bundles
            .iter()
            .filter(|(_, b)| **b == bundle_id)
            .map(|(op, _)| *op)
            .collect())
    }

    fn put_op_bundle(&mut self, opid: OpId, bundle_id: BundleId) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .op_bundles
            .entry(opid)
            .or_insert(bundle_id);
        Ok(())
    }

    fn child_bundles_of_op(&self, opid: OpId) -> Result<BTreeSet<BundleId>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .op_inputs
            .iter()
            .filter(|((opout, _), _)| opout.op == opid)
            .map(|(_, b)| *b)
            .collect())
    }

    fn child_ops_of_op(&self, opid: OpId) -> Result<BTreeSet<(OpId, BundleId)>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .op_inputs
            .iter()
            .filter(|((opout, _), _)| opout.op == opid)
            .map(|((_, child), b)| (*child, *b))
            .collect())
    }

    fn input_opouts_for_op(&self, opid: OpId) -> Result<BTreeSet<Opout>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .op_inputs
            .keys()
            .filter(|(_, child)| *child == opid)
            .map(|(opout, _)| *opout)
            .collect())
    }

    fn put_op_input(
        &mut self,
        spent_opout: Opout,
        child_opid: OpId,
        child_bundle_id: BundleId,
    ) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .op_inputs
            .entry((spent_opout, child_opid))
            .or_insert(child_bundle_id);
        Ok(())
    }

    fn opouts_at(
        &self,
        contract_id: ContractId,
        outpoints: &BTreeSet<OutPoint>,
    ) -> Result<BTreeMap<OutPoint, BTreeSet<Opout>>, Self::Error> {
        let mut out: BTreeMap<OutPoint, BTreeSet<Opout>> = BTreeMap::new();
        for (cid, outpoint, opout) in &self.tables.borrow().outpoint_opouts {
            if *cid == contract_id && outpoints.contains(outpoint) {
                out.entry(*outpoint).or_default().insert(*opout);
            }
        }
        Ok(out)
    }

    fn contracts_assigning(
        &self,
        outpoints: &BTreeSet<OutPoint>,
    ) -> Result<BTreeSet<ContractId>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .outpoint_opouts
            .iter()
            .filter(|(_, outpoint, _)| outpoints.contains(outpoint))
            .map(|(cid, ..)| *cid)
            .collect())
    }

    fn put_outpoint_opout(
        &mut self,
        contract_id: ContractId,
        outpoint: OutPoint,
        opout: Opout,
    ) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .outpoint_opouts
            .insert((contract_id, outpoint, opout));
        Ok(())
    }

    fn opouts_by_secrets(
        &self,
        contract_id: ContractId,
        secret_seals: &BTreeSet<SecretSeal>,
    ) -> Result<BTreeSet<Opout>, Self::Error> {
        Ok(self
            .tables
            .borrow()
            .secret_seal_opouts
            .iter()
            .filter(|(cid, seal, _)| *cid == contract_id && secret_seals.contains(seal))
            .map(|(.., opout)| *opout)
            .collect())
    }

    fn put_secret_opout(
        &mut self,
        contract_id: ContractId,
        secret_seal: SecretSeal,
        opout: Opout,
    ) -> Result<(), Self::Error> {
        self.tables
            .get_mut()
            .secret_seal_opouts
            .insert((contract_id, secret_seal, opout));
        Ok(())
    }
}

fn put<T: StrictEncode>(out: &mut Vec<u8>, val: &T) -> io::Result<()> {
    let blob = encode(val)?;
    out.extend((blob.len() as u32).to_le_bytes());
    out.extend(blob);
    Ok(())
}

fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend((len as u32).to_le_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        if self.0.len() < n {
            return Err(s!("truncated RGB store"));
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn len(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    fn get<T: StrictDecode>(&mut self) -> Result<T, String> {
        let n = self.len()?;
        decode(self.take(n)?).map_err(|e| e.to_string())
    }
}

use amplify::s;

impl MemRgbStore {
    /// Encodes all tables. Call it outside of a transaction.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>, String> {
        self.dump().map_err(|e| e.to_string())
    }

    fn dump(&self) -> io::Result<Vec<u8>> {
        let t = self.tables.borrow();
        let mut o = vec![FORMAT_VERSION];

        put_len(&mut o, t.schemata.len());
        for v in t.schemata.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.type_libs.len());
        for v in t.type_libs.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.schema_type_libs.len());
        for (a, b) in &t.schema_type_libs {
            put(&mut o, a)?;
            put(&mut o, b)?;
        }
        put_len(&mut o, t.aluvm_libs.len());
        for v in t.aluvm_libs.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.geneses.len());
        for v in t.geneses.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.bundles.len());
        for v in t.bundles.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.witnesses.len());
        for w in t.witnesses.values() {
            put(&mut o, w)?;
        }
        put_len(&mut o, t.secret_seals.len());
        for v in t.secret_seals.values() {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.globals.len());
        for ((cid, ty, ..), r) in &t.globals {
            put(&mut o, cid)?;
            put(&mut o, ty)?;
            put(&mut o, &r.opid)?;
            o.extend(r.index.to_le_bytes());
            o.extend(r.nonce.to_le_bytes());
            put(&mut o, &r.bundle_id)?;
            put(&mut o, &r.transition_type)?;
            put(&mut o, &r.value)?;
        }
        put_len(&mut o, t.allocations.len());
        for ((cid, ..), r) in &t.allocations {
            put(&mut o, cid)?;
            o.push(r.kind as u8);
            put(&mut o, &r.opout)?;
            put(&mut o, &r.seal.txid)?;
            o.extend(r.seal.vout.into_u32().to_le_bytes());
            put(&mut o, &r.bundle_id)?;
            put_len(&mut o, r.value.len());
            o.extend(&r.value);
        }
        put_len(&mut o, t.witness_ords.len());
        for (txid, ord) in &t.witness_ords {
            put(&mut o, txid)?;
            put(&mut o, ord)?;
        }
        put_len(&mut o, t.invalid_ops.len());
        for v in &t.invalid_ops {
            put(&mut o, v)?;
        }
        put_len(&mut o, t.bundle_contracts.len());
        for (a, b) in &t.bundle_contracts {
            put(&mut o, a)?;
            put(&mut o, b)?;
        }
        put_len(&mut o, t.bundle_witnesses.len());
        for (a, b) in &t.bundle_witnesses {
            put(&mut o, a)?;
            put(&mut o, b)?;
        }
        put_len(&mut o, t.op_bundles.len());
        for (a, b) in &t.op_bundles {
            put(&mut o, a)?;
            put(&mut o, b)?;
        }
        put_len(&mut o, t.op_inputs.len());
        for ((opout, child), bundle) in &t.op_inputs {
            put(&mut o, opout)?;
            put(&mut o, child)?;
            put(&mut o, bundle)?;
        }
        put_len(&mut o, t.outpoint_opouts.len());
        for (cid, outpoint, opout) in &t.outpoint_opouts {
            put(&mut o, cid)?;
            put(&mut o, &outpoint.txid)?;
            o.extend(outpoint.vout.to_le_bytes());
            put(&mut o, opout)?;
        }
        put_len(&mut o, t.secret_seal_opouts.len());
        for (cid, seal, opout) in &t.secret_seal_opouts {
            put(&mut o, cid)?;
            put(&mut o, seal)?;
            put(&mut o, opout)?;
        }
        Ok(o)
    }

    /// Rebuilds the store from the bytes of `to_bytes`.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut r = Reader(bytes);
        if r.take(1)?[0] != FORMAT_VERSION {
            return Err(s!("unsupported RGB store version"));
        }
        let mut t = Tables::default();

        for _ in 0..r.len()? {
            let v: Schema = r.get()?;
            t.schemata.insert(v.schema_id(), v);
        }
        for _ in 0..r.len()? {
            let v: TypeLib = r.get()?;
            t.type_libs.insert(v.id(), v);
        }
        for _ in 0..r.len()? {
            t.schema_type_libs.insert((r.get()?, r.get()?));
        }
        for _ in 0..r.len()? {
            let v: Lib = r.get()?;
            t.aluvm_libs.insert(v.id(), v);
        }
        for _ in 0..r.len()? {
            let v: Genesis = r.get()?;
            t.geneses.insert(v.contract_id(), v);
        }
        for _ in 0..r.len()? {
            let v: TransitionBundle = r.get()?;
            t.bundles.insert(v.bundle_id(), v);
        }
        for _ in 0..r.len()? {
            let w: SealWitness = r.get()?;
            t.witnesses.insert(w.witness_id(), w);
        }
        for _ in 0..r.len()? {
            let v: GraphSeal = r.get()?;
            t.secret_seals.insert(v.conceal(), v);
        }
        for _ in 0..r.len()? {
            let cid: ContractId = r.get()?;
            let ty: GlobalStateType = r.get()?;
            let opid: OpId = r.get()?;
            let index = u16::from_le_bytes(r.take(2)?.try_into().unwrap());
            let nonce = u64::from_le_bytes(r.take(8)?.try_into().unwrap());
            let row = GlobalStateRow {
                opid,
                index,
                nonce,
                bundle_id: r.get()?,
                transition_type: r.get()?,
                value: r.get::<RevealedData>()?,
            };
            t.globals.insert((cid, ty, opid, index), row);
        }
        for _ in 0..r.len()? {
            let cid: ContractId = r.get()?;
            let kind = match r.take(1)?[0] {
                0 => AllocKind::Fungible,
                1 => AllocKind::Structured,
                2 => AllocKind::Declarative,
                _ => return Err(s!("unknown allocation kind")),
            };
            let opout: Opout = r.get()?;
            let txid = r.get()?;
            let vout = Vout::from_u32(u32::from_le_bytes(r.take(4)?.try_into().unwrap()));
            let bundle_id = r.get()?;
            let n = r.len()?;
            let value = r.take(n)?.to_vec();
            let row = AllocationRow {
                kind,
                opout,
                seal: AllocSeal { txid, vout },
                bundle_id,
                value,
            };
            t.allocations
                .insert((cid, opout.op, opout.ty, opout.no), row);
        }
        for _ in 0..r.len()? {
            t.witness_ords.insert(r.get()?, r.get()?);
        }
        for _ in 0..r.len()? {
            t.invalid_ops.insert(r.get()?);
        }
        for _ in 0..r.len()? {
            t.bundle_contracts.insert(r.get()?, r.get()?);
        }
        for _ in 0..r.len()? {
            t.bundle_witnesses.insert((r.get()?, r.get()?));
        }
        for _ in 0..r.len()? {
            t.op_bundles.insert(r.get()?, r.get()?);
        }
        for _ in 0..r.len()? {
            t.op_inputs.insert((r.get()?, r.get()?), r.get()?);
        }
        for _ in 0..r.len()? {
            let cid = r.get()?;
            let txid: Txid = r.get()?;
            let vout = u32::from_le_bytes(r.take(4)?.try_into().unwrap());
            t.outpoint_opouts
                .insert((cid, OutPoint::new(txid, vout), r.get()?));
        }
        for _ in 0..r.len()? {
            t.secret_seal_opouts.insert((r.get()?, r.get()?, r.get()?));
        }
        if !r.0.is_empty() {
            return Err(s!("trailing bytes in RGB store"));
        }
        Ok(Self {
            tables: RefCell::new(t),
            tx: RefCell::new(None),
        })
    }
}

#[cfg(test)]
mod tests {
    use rgbstd::rgbcore::{bitcoin::hashes::Hash, vm::WitnessPos};
    use rgbstd::{Layer1, OutputSeal, txout::BlindSeal};

    use super::*;

    fn op(n: u8) -> OpId {
        OpId::copy_from_slice([n; 32]).unwrap()
    }

    fn bundle(n: u8) -> BundleId {
        BundleId::from([n; 32])
    }

    fn contract() -> ContractId {
        ContractId::copy_from_slice([9; 32]).unwrap()
    }

    fn txid(n: u8) -> Txid {
        Txid::from_byte_array([n; 32])
    }

    fn opout(n: u8) -> Opout {
        Opout::new(op(n), AssignmentType::with(4000), 0)
    }

    fn alloc(n: u8, seal: AllocSeal, bundle_id: Option<BundleId>) -> AllocationWrite<'static> {
        AllocationWrite {
            contract_id: contract(),
            kind: AllocKind::Fungible,
            opout: opout(n),
            seal,
            bundle_id,
            value: &[1, 2, 3],
        }
    }

    fn at(outpoint: OutPoint) -> BTreeSet<OutPoint> {
        BTreeSet::from([outpoint])
    }

    #[test]
    fn rollback_restores_tables_and_nested_begin_is_refused() {
        let mut store = MemRgbStore::default();
        store
            .put_witness_ord(txid(1), WitnessOrd::Tentative)
            .unwrap();

        assert_eq!(store.begin(TxMode::Write), Ok(TxBegin::Opened));
        assert_eq!(store.begin(TxMode::Write), Ok(TxBegin::AlreadyOpen));
        store
            .put_witness_ord(txid(2), WitnessOrd::Tentative)
            .unwrap();
        store.rollback();
        assert_eq!(store.all_witness_ords().unwrap().len(), 1);

        assert_eq!(store.begin(TxMode::Write), Ok(TxBegin::Opened));
        store
            .put_witness_ord(txid(2), WitnessOrd::Tentative)
            .unwrap();
        store.commit().unwrap();
        store.rollback();
        assert_eq!(store.all_witness_ords().unwrap().len(), 2);
    }

    #[test]
    fn witness_seal_resolves_to_the_queried_outpoint_while_visible() {
        let mut store = MemRgbStore::default();
        let witness = txid(7);
        store
            .put_allocation(alloc(1, AllocSeal::witness_vout(1u32), Some(bundle(1))))
            .unwrap();
        store.put_bundle_witness(bundle(1), witness).unwrap();
        let outpoint = OutPoint::new(witness, 1);
        let filter = AllocationFilter::all(Visibility::Valid);
        let outpoints = at(outpoint);

        // no witness ord yet: not visible
        let rows = store
            .allocations(contract(), filter.at(&outpoints))
            .unwrap();
        assert!(rows.is_empty());

        store
            .put_witness_ord(witness, WitnessOrd::Tentative)
            .unwrap();
        let rows = store
            .allocations(contract(), filter.at(&outpoints))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].seal,
            AllocSeal::explicit(OutputSeal::with(witness, 1u32))
        );
        let all = store
            .all_allocations_at_outpoints(&outpoints, Visibility::Valid)
            .unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].0, contract());
        assert_eq!(store.allocations(contract(), filter).unwrap().len(), 1);

        // another vout of the witness holds nothing
        let other = at(OutPoint::new(witness, 0));
        assert!(
            store
                .allocations(contract(), filter.at(&other))
                .unwrap()
                .is_empty()
        );

        // an invalid op and an archived witness hide the row, with and without outpoints
        let hidden = AllocationFilter::all(Visibility::All);
        store.set_op_validity(op(1), false).unwrap();
        assert!(
            store
                .allocations(contract(), filter.at(&outpoints))
                .unwrap()
                .is_empty()
        );
        assert!(store.allocations(contract(), filter).unwrap().is_empty());
        assert_eq!(store.allocations(contract(), hidden).unwrap().len(), 1);
        store.set_op_validity(op(1), true).unwrap();
        store
            .put_witness_ord(witness, WitnessOrd::Archived)
            .unwrap();
        assert!(
            store
                .allocations(contract(), filter.at(&outpoints))
                .unwrap()
                .is_empty()
        );
        assert!(store.allocations(contract(), filter).unwrap().is_empty());
        assert_eq!(store.allocations(contract(), hidden).unwrap().len(), 1);
        assert_eq!(
            store
                .allocations(contract(), hidden.at(&outpoints))
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn rows_survive_encoding() {
        let mut store = MemRgbStore::default();
        let explicit = OutPoint::new(txid(3), 2);
        store
            .put_allocation(alloc(
                1,
                AllocSeal::explicit(OutputSeal::with(txid(3), 2u32)),
                None,
            ))
            .unwrap();
        store
            .put_allocation(alloc(2, AllocSeal::witness_vout(0u32), Some(bundle(1))))
            .unwrap();
        store.put_bundle_witness(bundle(1), txid(4)).unwrap();
        store.put_bundle_contract(bundle(1), contract()).unwrap();
        store.put_op_bundle(op(2), bundle(1)).unwrap();
        store.put_op_input(opout(1), op(2), bundle(1)).unwrap();
        store
            .put_outpoint_opout(contract(), explicit, opout(1))
            .unwrap();
        let pos = WitnessPos::with(Layer1::Bitcoin, NonZeroU32::new(10).unwrap(), 1_700_000_000);
        store
            .put_witness_ord(txid(4), WitnessOrd::Mined(pos.unwrap()))
            .unwrap();
        store.put_witness_ord(txid(5), WitnessOrd::Ignored).unwrap();
        store.set_op_validity(op(9), false).unwrap();
        let graph_seal = GraphSeal::from(BlindSeal::with_blinding(
            rgbstd::txout::TxPtr::WitnessTx,
            0u32,
            42,
        ));
        store
            .put_secret_seal(&graph_seal, graph_seal.conceal())
            .unwrap();
        store
            .put_secret_opout(contract(), graph_seal.conceal(), opout(2))
            .unwrap();

        let bytes = store.to_bytes().unwrap();
        let restored = MemRgbStore::from_bytes(&bytes).unwrap();
        assert_eq!(restored.to_bytes().unwrap(), bytes);

        assert_eq!(
            restored.all_witness_ords().unwrap(),
            store.all_witness_ords().unwrap()
        );
        assert_eq!(restored.all_invalid_ops().unwrap(), BTreeSet::from([op(9)]));
        assert_eq!(
            restored.bundle_witnesses(bundle(1)).unwrap(),
            BTreeSet::from([txid(4)])
        );
        assert_eq!(
            restored.bundle_contract(bundle(1)).unwrap(),
            Some(contract())
        );
        assert_eq!(restored.bundle_of_op(op(2)).unwrap(), Some(bundle(1)));
        assert_eq!(
            restored.child_ops_of_op(op(1)).unwrap(),
            BTreeSet::from([(op(2), bundle(1))])
        );
        assert_eq!(
            restored.opouts_at(contract(), &at(explicit)).unwrap()[&explicit],
            BTreeSet::from([opout(1)])
        );
        assert_eq!(
            restored.seal_of_secret(graph_seal.conceal()).unwrap(),
            Some(graph_seal)
        );
        assert_eq!(
            restored
                .opouts_by_secrets(contract(), &BTreeSet::from([graph_seal.conceal()]))
                .unwrap(),
            BTreeSet::from([opout(2)])
        );
        let filter = AllocationFilter::all(Visibility::All);
        let mut rows = restored.allocations(contract(), filter).unwrap();
        rows.sort_by_key(|r| r.opout);
        assert_eq!(rows, {
            let mut rows = store.allocations(contract(), filter).unwrap();
            rows.sort_by_key(|r| r.opout);
            rows
        });
        assert_eq!(rows.len(), 2);
    }
}

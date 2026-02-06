use wasm_bindgen::prelude::*;
use rgb_lib::BitcoinNetwork;

#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Bitcoin,
    Testnet,
    Signet,
    Regtest,
}

impl From<Network> for BitcoinNetwork {
    fn from(network: Network) -> Self {
        match network {
            Network::Bitcoin => BitcoinNetwork::Mainnet,
            Network::Testnet => BitcoinNetwork::Testnet,
            Network::Signet => BitcoinNetwork::Signet,
            Network::Regtest => BitcoinNetwork::Regtest,
        }
    }
}

impl From<BitcoinNetwork> for Network {
    fn from(network: BitcoinNetwork) -> Self {
        match network {
            BitcoinNetwork::Mainnet => Network::Bitcoin,
            BitcoinNetwork::Testnet | BitcoinNetwork::Testnet4 => Network::Testnet,
            BitcoinNetwork::Signet => Network::Signet,
            BitcoinNetwork::Regtest => Network::Regtest,
        }
    }
}

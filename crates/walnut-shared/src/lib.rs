use starknet_api::core::ChainId;
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use url::Url;

const GOERLI_CHAIN_ID: &str = "0x534e5f474f45524c49";
const MAIN_CHAIN_ID: &str = "0x534e5f4d41494e";

pub fn create_rpc_client(chain_id: &ChainId) -> JsonRpcClient<HttpTransport> {
    JsonRpcClient::new(HttpTransport::new(Url::parse(rpc_url(chain_id)).unwrap()))
}

pub fn rpc_url(chain_id: &ChainId) -> &str {
    match chain_id.0.as_str() {
        GOERLI_CHAIN_ID => {
            "https://starknet-goerli.g.alchemy.com/v2/D2pgqj4yeZmmZyBY7tw-CMnO2nUL8n94"
        }
        MAIN_CHAIN_ID => {
            "https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        _ => panic!("Invalid chain id"),
    }
}

pub fn extract_chain_id(chain_id: &str) -> ChainId {
    let main = ChainId(MAIN_CHAIN_ID.to_string());
    let goerli = ChainId(GOERLI_CHAIN_ID.to_string());
    match chain_id {
        "0x534e5f474f45524c49" => goerli,
        "SN_GOERLI" => goerli,
        "sn_goerli" => goerli,
        "0x534e5f4d41494e" => main,
        "SN_MAIN" => main,
        "sn_main" => main,
        _ => panic!("Invalid chain id"),
    }
}

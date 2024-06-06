use cairo_felt::Felt252;
use num_bigint::BigUint;
use starknet_api::core::ChainId;
use starknet_providers::jsonrpc::{HttpTransport, JsonRpcClient};
use url::Url;

#[derive(Debug, Clone)]
pub struct Datas {
    pub names: String,
    pub types: String,
}

#[derive(Debug, Clone)]
pub struct StructItems {
    pub name: String,
    pub members: Vec<Datas>,
}
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

pub fn voyager_api_url(chain_id: &ChainId) -> &str {
    match chain_id.0.as_str() {
        GOERLI_CHAIN_ID => "https://goerli-api.voyager.online/beta/",
        MAIN_CHAIN_ID => "https://api.voyager.online/beta/",
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

pub fn bytes_to_text(bytes: [u8; 32]) -> Result<String, std::str::Utf8Error> {
    let mut text = std::str::from_utf8(&bytes)?.to_string();
    text.retain(|c| c != '\0');
    Ok(text)
}

pub fn felt252_to_hex(felt_array: Vec<Felt252>) -> Result<Vec<String>, std::str::Utf8Error> {
    let hex_representation = felt_array
        .iter()
        .map(|felt| format!("0x{}", felt.to_str_radix(16)))
        .collect::<Vec<String>>();

    Ok(hex_representation)
}

pub fn decode_felt252(felt_array: Vec<Felt252>) -> Result<String, std::str::Utf8Error> {
    //convert do decimal string representation
    let decimal_arrays = felt_array
        .iter()
        .map(|felt| felt.to_string())
        .collect::<Vec<String>>();
    let decimal_string = decimal_arrays.join(", ");
    //convert to hex representation
    let hex_representation = BigUint::parse_bytes(decimal_string.as_bytes(), 10)
        .expect("Failed to parse BigUint")
        .to_str_radix(16);
    //conver it to bytes
    let bytes: Vec<u8> = hex_representation
        .as_bytes()
        .chunks(2)
        .map(|chunk| u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap())
        .collect();
    //get human readable text
    let text = String::from_utf8_lossy(&bytes);
    Ok(text.to_string())
}

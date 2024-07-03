pub fn rpc_url(chain_id: &str) -> &str {
    match chain_id {
        "0x534e5f4d41494e" => {
            "https://starknet-mainnet.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        "0x534e5f5345504f4c4941" => {
            "https://starknet-sepolia.g.alchemy.com/v2/9J1ION8Owu9eHgZeyWlE9-N0yEepGA58"
        }
        _ => panic!("Invalid chain id"),
    }
}

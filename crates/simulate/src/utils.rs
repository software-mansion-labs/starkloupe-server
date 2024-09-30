use blockifier::transaction::transaction_types::TransactionType;
use num_bigint::BigUint;
use num_traits::Num;

pub fn convert_to_hex(num_str: &str) -> String {
    let num = BigUint::from_str_radix(num_str, 10).unwrap();
    format!("{:x}", num)
}

//NOTE Move implementation here -> https://github.com/walnuthq/blockifier/blob/a6200402ab635d8a8e175f7f135be5914c960007/crates/blockifier/src/transaction/transaction_types.rs#L9
pub fn transaction_type_to_string(tx_type: TransactionType) -> String {
    match tx_type {
        TransactionType::Declare => "DECLARE".to_string(),
        TransactionType::DeployAccount => "DEPLOY".to_string(),
        TransactionType::InvokeFunction => "INVOKE".to_string(),
        TransactionType::L1Handler => "L1Handler".to_string(),
    }
}

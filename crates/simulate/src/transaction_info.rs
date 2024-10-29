use starknet::core::types::Felt;
use starknet_old::core::types as starknet_old_types;
use walnut_shared::{field_element_to_felt, vec_field_element_to_vec_felt};

pub trait TransactionInformation {
    fn sender_address(&self) -> Option<Felt>;
    fn nonce(&self) -> Option<Felt>;
    fn calldata(&self) -> Option<Vec<Felt>>;
    fn version(&self) -> Felt;
}

impl TransactionInformation for starknet_old_types::Transaction {
    fn sender_address(&self) -> Option<Felt> {
        match self {
            starknet_old_types::Transaction::Invoke(tx) => match tx {
                starknet_old_types::InvokeTransaction::V0(tx_v0) => {
                    Some(field_element_to_felt(tx_v0.contract_address))
                }
                starknet_old_types::InvokeTransaction::V1(tx_v1) => {
                    Some(field_element_to_felt(tx_v1.sender_address))
                }
                starknet_old_types::InvokeTransaction::V3(tx_v3) => {
                    Some(field_element_to_felt(tx_v3.sender_address))
                }
            },
            starknet_old_types::Transaction::Declare(tx) => match tx {
                starknet_old_types::DeclareTransaction::V0(tx_v0) => {
                    Some(field_element_to_felt(tx_v0.sender_address))
                }
                starknet_old_types::DeclareTransaction::V1(tx_v1) => {
                    Some(field_element_to_felt(tx_v1.sender_address))
                }
                starknet_old_types::DeclareTransaction::V2(tx_v2) => {
                    Some(field_element_to_felt(tx_v2.sender_address))
                }
                starknet_old_types::DeclareTransaction::V3(tx_v3) => {
                    Some(field_element_to_felt(tx_v3.sender_address))
                }
            },
            starknet_old_types::Transaction::L1Handler(tx) => {
                Some(field_element_to_felt(tx.contract_address))
            }
            _ => None,
        }
    }

    fn nonce(&self) -> Option<Felt> {
        match self {
            starknet_old_types::Transaction::Invoke(tx) => match tx {
                starknet_old_types::InvokeTransaction::V1(tx_v1) => {
                    Some(field_element_to_felt(tx_v1.nonce))
                }
                starknet_old_types::InvokeTransaction::V3(tx_v3) => {
                    Some(field_element_to_felt(tx_v3.nonce))
                }
                _ => None,
            },
            starknet_old_types::Transaction::Declare(tx) => match tx {
                starknet_old_types::DeclareTransaction::V1(tx_v1) => {
                    Some(field_element_to_felt(tx_v1.nonce))
                }
                starknet_old_types::DeclareTransaction::V2(tx_v2) => {
                    Some(field_element_to_felt(tx_v2.nonce))
                }
                starknet_old_types::DeclareTransaction::V3(tx_v3) => {
                    Some(field_element_to_felt(tx_v3.nonce))
                }
                _ => None,
            },
            starknet_old_types::Transaction::L1Handler(tx) => Some(Felt::from(tx.nonce)),
            starknet_old_types::Transaction::DeployAccount(tx) => match tx {
                starknet_old_types::DeployAccountTransaction::V1(tx_v1) => {
                    Some(field_element_to_felt(tx_v1.nonce))
                }
                starknet_old_types::DeployAccountTransaction::V3(tx_v3) => {
                    Some(field_element_to_felt(tx_v3.nonce))
                }
            },
            _ => None,
        }
    }

    fn calldata(&self) -> Option<Vec<Felt>> {
        match self {
            starknet_old_types::Transaction::Invoke(tx) => match tx {
                starknet_old_types::InvokeTransaction::V0(tx_v0) => {
                    Some(vec_field_element_to_vec_felt(tx_v0.calldata.clone()))
                }
                starknet_old_types::InvokeTransaction::V1(tx_v1) => {
                    Some(vec_field_element_to_vec_felt(tx_v1.calldata.clone()))
                }
                starknet_old_types::InvokeTransaction::V3(tx_v3) => {
                    Some(vec_field_element_to_vec_felt(tx_v3.calldata.clone()))
                }
            },
            starknet_old_types::Transaction::L1Handler(tx) => {
                Some(vec_field_element_to_vec_felt(tx.calldata.clone()))
            }
            starknet_old_types::Transaction::Deploy(tx) => Some(vec_field_element_to_vec_felt(
                tx.constructor_calldata.clone(),
            )),
            starknet_old_types::Transaction::DeployAccount(tx) => match tx {
                starknet_old_types::DeployAccountTransaction::V1(tx_v1) => Some(
                    vec_field_element_to_vec_felt(tx_v1.constructor_calldata.clone()),
                ),
                starknet_old_types::DeployAccountTransaction::V3(tx_v3) => Some(
                    vec_field_element_to_vec_felt(tx_v3.constructor_calldata.clone()),
                ),
            },
            _ => None,
        }
    }

    fn version(&self) -> Felt {
        match self {
            starknet_old_types::Transaction::Invoke(tx) => match tx {
                starknet_old_types::InvokeTransaction::V0(_) => Felt::ZERO,
                starknet_old_types::InvokeTransaction::V1(_) => Felt::ONE,
                starknet_old_types::InvokeTransaction::V3(_) => Felt::THREE,
            },
            starknet_old_types::Transaction::Declare(tx) => match tx {
                starknet_old_types::DeclareTransaction::V0(_) => Felt::ZERO,
                starknet_old_types::DeclareTransaction::V1(_) => Felt::ONE,
                starknet_old_types::DeclareTransaction::V2(_) => Felt::TWO,
                starknet_old_types::DeclareTransaction::V3(_) => Felt::THREE,
            },
            starknet_old_types::Transaction::L1Handler(tx) => field_element_to_felt(tx.version),
            starknet_old_types::Transaction::Deploy(tx) => field_element_to_felt(tx.version),
            _ => Felt::ONE,
        }
    }
}

use starknet_rust::core::types::{
    DeclareTransaction, DeployAccountTransaction, Felt, InvokeTransaction, Transaction,
};

pub trait TransactionInformation {
    fn sender_address(&self) -> Option<Felt>;
    fn nonce(&self) -> Option<Felt>;
    fn calldata(&self) -> Option<Vec<Felt>>;
    fn version(&self) -> Felt;
}

impl TransactionInformation for Transaction {
    fn sender_address(&self) -> Option<Felt> {
        match self {
            Transaction::Invoke(tx) => match tx {
                InvokeTransaction::V0(tx_v0) => Some(tx_v0.contract_address),
                InvokeTransaction::V1(tx_v1) => Some(tx_v1.sender_address),
                InvokeTransaction::V3(tx_v3) => Some(tx_v3.sender_address),
            },
            Transaction::Declare(tx) => match tx {
                DeclareTransaction::V0(tx_v0) => Some(tx_v0.sender_address),
                DeclareTransaction::V1(tx_v1) => Some(tx_v1.sender_address),
                DeclareTransaction::V2(tx_v2) => Some(tx_v2.sender_address),
                DeclareTransaction::V3(tx_v3) => Some(tx_v3.sender_address),
            },
            Transaction::L1Handler(tx) => Some(tx.contract_address),
            _ => None,
        }
    }

    fn nonce(&self) -> Option<Felt> {
        match self {
            Transaction::Invoke(tx) => match tx {
                InvokeTransaction::V1(tx_v1) => Some(tx_v1.nonce),
                InvokeTransaction::V3(tx_v3) => Some(tx_v3.nonce),
                _ => None,
            },
            Transaction::Declare(tx) => match tx {
                DeclareTransaction::V1(tx_v1) => Some(tx_v1.nonce),
                DeclareTransaction::V2(tx_v2) => Some(tx_v2.nonce),
                DeclareTransaction::V3(tx_v3) => Some(tx_v3.nonce),
                _ => None,
            },
            Transaction::L1Handler(tx) => Some(Felt::from(tx.nonce)),
            Transaction::DeployAccount(tx) => match tx {
                DeployAccountTransaction::V1(tx_v1) => Some(tx_v1.nonce),
                DeployAccountTransaction::V3(tx_v3) => Some(tx_v3.nonce),
            },
            _ => None,
        }
    }

    fn calldata(&self) -> Option<Vec<Felt>> {
        match self {
            Transaction::Invoke(tx) => match tx {
                InvokeTransaction::V0(tx_v0) => Some(tx_v0.calldata.clone()),
                InvokeTransaction::V1(tx_v1) => Some(tx_v1.calldata.clone()),
                InvokeTransaction::V3(tx_v3) => Some(tx_v3.calldata.clone()),
            },
            Transaction::L1Handler(tx) => Some(tx.calldata.clone()),
            Transaction::Deploy(tx) => Some(tx.constructor_calldata.clone()),
            Transaction::DeployAccount(tx) => match tx {
                DeployAccountTransaction::V1(tx_v1) => Some(tx_v1.constructor_calldata.clone()),
                DeployAccountTransaction::V3(tx_v3) => Some(tx_v3.constructor_calldata.clone()),
            },
            _ => None,
        }
    }

    fn version(&self) -> Felt {
        match self {
            Transaction::Invoke(tx) => match tx {
                InvokeTransaction::V0(_) => Felt::ZERO,
                InvokeTransaction::V1(_) => Felt::ONE,
                InvokeTransaction::V3(_) => Felt::THREE,
            },
            Transaction::Declare(tx) => match tx {
                DeclareTransaction::V0(_) => Felt::ZERO,
                DeclareTransaction::V1(_) => Felt::ONE,
                DeclareTransaction::V2(_) => Felt::TWO,
                DeclareTransaction::V3(_) => Felt::THREE,
            },
            Transaction::L1Handler(tx) => tx.version,
            Transaction::Deploy(tx) => tx.version,
            _ => Felt::ONE,
        }
    }
}

use crate::app_state::AppState;
use crate::handlers::simulations::SimulationRes;
use crate::utils::simulate::convert_to_hex;
use crate::utils::simulate::create_fork_cached_state_at;
use crate::utils::simulate::get_block_context;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use blockifier::state::cached_state::CachedState;
use blockifier::transaction::objects::TransactionExecutionInfo;
use blockifier::transaction::transaction_execution::Transaction;
use blockifier::transaction::transactions::ExecutableTransaction;
use serde::Serialize;
use sqlx::types::Uuid;
use starknet::core::types::{
    BlockId, CallType, EventContent, ExecuteInvocation, FeeEstimate, FieldElement,
    FunctionInvocation, InvokeTransactionTrace, SimulatedTransaction, TransactionTrace,
};
use starknet_api::block::BlockNumber;
use starknet_api::core::{ChainId, ContractAddress, Nonce, PatriciaKey};
use starknet_api::hash::{StarkFelt, StarkHash};
use starknet_api::transaction::{
    Calldata, Fee, InvokeTransaction as SAInvokeTransaction, InvokeTransactionV1,
    Transaction as StarknetApiTransaction, TransactionHash, TransactionSignature,
};
use starknet_api::{contract_address, patricia_key, stark_felt};

use std::{str::FromStr, sync::Arc};

#[derive(Serialize)]
pub struct SimulateTraceResponse {
    simulated_transaction: SimulatedTransaction,
    simulation: SimulationRes,
}

pub async fn simulate_trace(
    State(state): State<Arc<AppState>>,
    id: Path<String>,
) -> Result<Json<SimulateTraceResponse>, StatusCode> {
    // Implement your business logic here
    let row = sqlx::query!(
        "SELECT * FROM simulations WHERE id = $1",
        Uuid::from_str(&id.0).unwrap()
    )
    .fetch_one(&state.db_pool)
    .await
    .unwrap();

    let sim = SimulationRes {
        id: row.id.map_or(String::new(), |id| id.to_string()),
        team_id: row.team_id,
        chain_id: row.chain_id,
        block_at: row.block_at,
        transaction_version: row.transaction_version,
        nonce: row.nonce,
        max_fee: row.max_fee,
        cairo_version: row.cairo_version,
        wallet_address: row.wallet_address,
        calldata: row.calldata.map_or(Vec::new(), |calldata| calldata),
        created_at: row.created_at.assume_utc().unix_timestamp(),
        updated_at: row.updated_at.assume_utc().unix_timestamp(),
        status: row.status,
    };

    let calldata_raw: Vec<StarkFelt> = sim
        .calldata
        .clone()
        .iter()
        .map(|x| stark_felt!(convert_to_hex(x).as_str()))
        .collect();

    let tx_raw = InvokeTransactionV1 {
        sender_address: contract_address!(sim.wallet_address.as_str()),
        nonce: Nonce(StarkFelt::from(sim.nonce as u64)),
        calldata: Calldata(calldata_raw.into()),
        max_fee: Fee::default(),
        signature: TransactionSignature(vec![]),
    };

    let tx_hash = TransactionHash(StarkHash::default());
    let tx = Transaction::from_api(
        StarknetApiTransaction::Invoke(SAInvokeTransaction::V1(tx_raw)),
        tx_hash,
        None,
        None,
        None,
    )
    .unwrap();

    let chain_id = ChainId(sim.chain_id.clone());
    let block_context = get_block_context(chain_id.clone(), BlockNumber(sim.block_at as u64));

    // TODO: Don't use File cache
    let mut cached_fork_state = create_fork_cached_state_at(
        chain_id,
        BlockId::Number(sim.block_at as u64),
        "/tmp/sn-debugger/cache",
    );

    let mut tx_state = CachedState::<_>::create_transactional(&mut cached_fork_state);

    let tx_info = tx.execute(&mut tx_state, &block_context, true, false);
    // dbg!(tx_info);

    match tx_info {
        Ok(tx_info) => Ok(Json(SimulateTraceResponse {
            simulated_transaction: to_simulated_transaction(tx_info),
            simulation: sim,
        })),
        Err(_) => Err(StatusCode::EXPECTATION_FAILED),
    }
}

fn to_simulated_transaction(tx: TransactionExecutionInfo) -> SimulatedTransaction {
    let eci = tx.execute_call_info.unwrap();

    SimulatedTransaction {
        transaction_trace: TransactionTrace::Invoke(InvokeTransactionTrace {
            execute_invocation: ExecuteInvocation::Success(to_function_invocation(&eci)),
            // TODO: Implement this
            validate_invocation: None,
            fee_transfer_invocation: None,
        }),
        // TODO: Implement this
        fee_estimation: FeeEstimate {
            gas_consumed: 0,
            gas_price: 0,
            overall_fee: 0,
        },
    }
}

fn to_function_invocation(eci: &blockifier::execution::call_info::CallInfo) -> FunctionInvocation {
    let calldata = eci
        .call
        .calldata
        .0
        .iter()
        .map(|x| FieldElement::from_byte_slice_be(x.bytes()).unwrap())
        .collect();

    let events: Vec<EventContent> = eci
        .execution
        .events
        .iter()
        .map(|x| EventContent {
            data: x
                .event
                .data
                .0
                .iter()
                .map(|x| FieldElement::from(*x))
                .collect(),
            keys: x
                .event
                .keys
                .iter()
                .map(|x| FieldElement::from(x.0))
                .collect(),
        })
        .collect();

    let result = eci
        .execution
        .retdata
        .0
        .iter()
        .map(|x| FieldElement::from(*x))
        .collect();

    let internal_calls: Vec<FunctionInvocation> = eci
        .inner_calls
        .iter()
        .map(|x| to_function_invocation(x))
        .collect();

    FunctionInvocation {
        calldata: calldata,
        contract_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.storage_address.0.key(),
        )),
        call_type: match eci.call.call_type {
            blockifier::execution::entry_point::CallType::Call => CallType::Call,
            blockifier::execution::entry_point::CallType::Delegate => CallType::LibraryCall,
        },
        caller_address: FieldElement::from(*Into::<&StarkFelt>::into(
            eci.call.caller_address.0.key(),
        )),
        class_hash: FieldElement::from(eci.call.class_hash.unwrap().0),
        entry_point_type: match eci.call.entry_point_type {
            starknet_api::deprecated_contract_class::EntryPointType::Constructor => {
                starknet::core::types::EntryPointType::Constructor
            }
            starknet_api::deprecated_contract_class::EntryPointType::External => {
                starknet::core::types::EntryPointType::External
            }
            starknet_api::deprecated_contract_class::EntryPointType::L1Handler => {
                starknet::core::types::EntryPointType::L1Handler
            }
        },
        events: events,
        // TODO: Implement this
        messages: vec![],
        result: result,
        entry_point_selector: FieldElement::from(eci.call.entry_point_selector.0),
        calls: internal_calls,
    }
}

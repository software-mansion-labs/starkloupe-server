use db::Simulation;
use dotenv::dotenv;
use sqlx::postgres::PgPoolOptions;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use simulate::{get_error_from_call, simulate, to_simulated_transaction, SimulationArgs};
use starknet_rust::core::types::{ExecuteInvocation, TransactionTrace};

#[tokio::main]
async fn main() {
    dotenv().ok();

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("info"))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let db_addr = std::env::var("DATABASE_URL").unwrap_or("postgres://".to_string());
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&db_addr)
        .await
        .unwrap();

    let simulations = sqlx::query_as!(
        Simulation,
        "SELECT * FROM simulations WHERE project_id = 2 AND status = 'failure' AND created_at >= NOW() - INTERVAL '7 days' ORDER BY created_at DESC;"
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    for sim in simulations.iter() {
        let tx_info = simulate(SimulationArgs {
            chain_id: sim.chain_id.clone(),
            block_at: (sim.block_at as u64).clone(),
            nonce: (sim.nonce as u64).clone(),
            wallet_address: sim.wallet_address.clone(),
            calldata: sim.calldata.clone().unwrap_or_default(),
        });

        let mut error_message: Option<String> = None;
        let mut error_contract_address: Option<String> = None;

        let sim_status = match tx_info {
            Ok(tx_info) => {
                if tx_info.revert_error.is_some() {
                    let transaction_trace = to_simulated_transaction(tx_info).transaction_trace;
                    match transaction_trace {
                        TransactionTrace::Invoke(transaction_trace) => {
                            match transaction_trace.execute_invocation {
                                ExecuteInvocation::Success(function_invocation) => {
                                    (error_message, error_contract_address) =
                                        get_error_from_call(function_invocation);
                                }
                                _ => {}
                            }
                        }
                        _ => {}
                    }
                    "failure"
                } else {
                    "success"
                }
            }

            Err(err) => {
                error_message = Some(err.to_string());
                "failure"
            }
        };

        if sim_status != sim.status {
            println!("{}: {} -> {}", sim.id, sim.status, sim_status);
        }

        // sqlx::query!(
        //     "UPDATE simulations SET status = $1 WHERE id = $2",
        //     sim_status,
        //     id,
        // )
        // .execute(&state.db_pool)
        // .await
        // .unwrap();
    }
}

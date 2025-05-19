use blockifier::execution::call_info::CallInfo;
use starknet_api::execution_resources::GasAmount;

#[derive(Debug)]
pub struct GasCounter {
    pub spent_gas: GasAmount,
    pub remaining_gas: GasAmount,
}

impl GasCounter {
    pub(crate) fn new(initial_gas: GasAmount) -> Self {
        GasCounter {
            spent_gas: GasAmount(0),
            remaining_gas: initial_gas,
        }
    }

    fn spend(&mut self, amount: GasAmount) {
        self.spent_gas = self.spent_gas.checked_add(amount).expect("Gas overflow");
        self.remaining_gas = self
            .remaining_gas
            .checked_sub(amount)
            .expect("Overuse of gas; should have been caught earlier");
    }

    /// Limits the amount of gas that can be used (in validate\execute) by the given global limit.
    pub fn limit_usage(&self, amount: GasAmount) -> u64 {
        self.remaining_gas.min(amount).0
    }

    pub fn subtract_used_gas(&mut self, call_info: &CallInfo) {
        self.spend(GasAmount(call_info.execution.gas_consumed));
    }
}

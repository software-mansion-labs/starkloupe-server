use phf::phf_set;

pub static SKIP_BUILTIN_TYPES: phf::Set<&'static str> = phf_set! {
    "Const",
    "Step",
    "Hole",
    "GasBuiltin",
    "Bitwise",
    "BuiltinCosts",
    "EcOp",
    "RangeCheck",
    "SegmentArena",
    "Poseidon",
    "Pedersen",
    "RangeCheck96",
    "CircuitAdd",
    "CircuitMul",
    "Gas",
    "System",
    "()",
    "ContractState",
    "ComponentState",
};

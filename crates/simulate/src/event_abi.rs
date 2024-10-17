use serde::Serialize;

#[derive(Serialize, Debug, Clone)]
pub struct EventAbi {
    pub event_name: String,
    pub event_arguments_names: Vec<String>,
    pub event_arguments_types: Vec<String>,
}

#[derive(Serialize, Default, Debug, Clone)]
pub struct EventAbiStore {
    pub event_abis: Vec<EventAbi>,
}

impl EventAbiStore {
    pub fn add_event_abi(&mut self, event_abi: EventAbi) {
        self.event_abis.push(event_abi);
    }
}

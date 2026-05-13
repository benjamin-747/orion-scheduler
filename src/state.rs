use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::SharedConfig;
use crate::keep_alive::KeepAliveMachine;

/// Represents the current state of the VM
#[derive(Debug, Clone)]
pub struct VmInfo {
    pub id: String,
    pub ip: Option<String>,
    pub created_at: std::time::Instant,
    /// Path to the Orion log file
    pub log_file: Option<String>,
}

/// Global state for tracking VM lifecycle
pub struct AppState {
    pub vm: Arc<RwLock<Option<VmInfo>>>,
    pub machine: Arc<RwLock<Option<KeepAliveMachine>>>,
    pub config: SharedConfig,
}

impl AppState {
    /// Create a new AppState with empty VM and machine slots
    pub fn new(config: SharedConfig) -> Self {
        Self {
            vm: Arc::new(RwLock::new(None)),
            machine: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// Set VM info and machine reference together atomically
    pub async fn set_vm(&self, info: VmInfo, machine: KeepAliveMachine) {
        let mut vm = self.vm.write().await;
        *vm = Some(info);
        let mut m = self.machine.write().await;
        *m = Some(machine);
    }

    /// Clear both VM info and machine reference
    pub async fn clear_vm(&self) {
        let mut vm = self.vm.write().await;
        *vm = None;
        let mut m = self.machine.write().await;
        *m = None;
    }

    /// Get a clone of the current VM info if any
    pub async fn get_vm(&self) -> Option<VmInfo> {
        let vm = self.vm.read().await;
        vm.clone()
    }

    /// Get a clone of the current machine reference if any
    pub async fn get_machine(&self) -> Option<KeepAliveMachine> {
        let m = self.machine.read().await;
        m.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_state_set_clear() {
        let config = Arc::new(tokio::sync::RwLock::new(crate::config::Config::new(
            "/tmp".to_string(),
        )));
        let state = AppState::new(config);
        assert!(state.get_vm().await.is_none());
        assert!(state.get_machine().await.is_none());
    }
}
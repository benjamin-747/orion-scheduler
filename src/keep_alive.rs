use anyhow::Result;
use qlean::{Distro, MachineConfig, Machine};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Wrapper around Machine that keeps it alive after initial operations.
/// This allows multiple operations (deploy, start_orion, etc.) on the same VM.
pub struct KeepAliveMachine {
    machine: Arc<Mutex<Option<Machine>>>,
}

impl KeepAliveMachine {
    /// Create a new VM and keep it alive
    pub async fn new(vm_name: &str) -> Result<Self> {
        tracing::info!("[keep-alive] Creating VM: {}", vm_name);

        let config = MachineConfig::default();
        let image = qlean::create_image(Distro::Debian, "debian-13-generic-amd64").await?;

        let mut machine = Machine::new(&image, &config).await?;
        machine.init().await?;

        tracing::info!("[keep-alive] VM {} initialized and running", vm_name);

        Ok(Self {
            machine: Arc::new(Mutex::new(Some(machine))),
        })
    }

    /// Execute a command in the VM
    pub async fn exec(&self, cmd: &str) -> Result<std::process::Output> {
        let mut guard = self.machine.lock().await;
        if let Some(machine) = guard.as_mut() {
            tracing::info!("[keep-alive] Executing: {}", cmd);
            let output = machine.exec(cmd).await?;
            Ok(output)
        } else {
            anyhow::bail!("VM has been shut down")
        }
    }

    /// Upload a file to the VM
    pub async fn upload(
        &self,
        local: impl AsRef<std::path::Path>,
        remote: impl AsRef<std::path::Path>,
    ) -> Result<()> {
        let mut guard = self.machine.lock().await;
        if let Some(machine) = guard.as_mut() {
            let local_path = local.as_ref();
            let remote_path_str = remote.as_ref().to_string_lossy().into_owned();
            tracing::info!("[keep-alive] Uploading: {} -> {}", local_path.display(), remote_path_str);
            machine.upload(local, remote).await?;
            Ok(())
        } else {
            anyhow::bail!("VM has been shut down")
        }
    }

    /// Shutdown the VM
    pub async fn shutdown(self) -> Result<()> {
        tracing::info!("[keep-alive] Shutting down VM");
        let mut guard = self.machine.lock().await;
        if let Some(mut machine) = guard.take() {
            machine.shutdown().await?;
            tracing::info!("[keep-alive] VM shutdown complete");
        }
        Ok(())
    }

    /// Check if VM is still running
    pub async fn is_alive(&self) -> bool {
        self.machine.lock().await.is_some()
    }
}

impl Clone for KeepAliveMachine {
    fn clone(&self) -> Self {
        Self {
            machine: Arc::clone(&self.machine),
        }
    }
}

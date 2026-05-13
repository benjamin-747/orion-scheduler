use anyhow::Result;
use qlean::{Distro, MachineConfig, Machine, CustomImageConfig, ImageSource, ShaType};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Wrapper around Machine that keeps it alive after initial operations.
/// This allows multiple operations (deploy, start_orion, etc.) on the same VM.
pub struct KeepAliveMachine {
    machine: Arc<Mutex<Option<Machine>>>,
}

impl KeepAliveMachine {
    /// Create a new VM and keep it alive
    /// If custom_image_path is provided, use it instead of the default Debian image
    pub async fn new(vm_name: &str, custom_image_path: Option<String>, disk_gb: Option<u32>) -> Result<Self> {
        tracing::info!("[keep-alive] Creating VM: {}", vm_name);

        let config = MachineConfig {
            disk: disk_gb,
            ..Default::default()
        };
        let image = if let Some(path) = custom_image_path {
            tracing::info!("[keep-alive] Using custom image: {}", path);

            // Extract directory from path
            let image_dir = std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new("."));
            let kernel_path = image_dir.join("vmlinuz-6.12.85+deb13-amd64");
            let initrd_path = image_dir.join("initrd.img-6.12.85+deb13-amd64");

            // Compute SHA256 hash of the local image for validation (streaming, low memory)
            let image_hash = qlean::compute_sha256_streaming(std::path::Path::new(&path)).await
                .map_err(|e| anyhow::anyhow!("Failed to hash image: {}", e))?;
            tracing::info!("[keep-alive] Custom image hash: {}", &image_hash[..16]);

            // Compute kernel and initrd hashes (streaming)
            let kernel_hash: Option<String> = if kernel_path.exists() {
                qlean::compute_sha256_streaming(&kernel_path).await.ok()
            } else {
                None
            };
            let initrd_hash: Option<String> = if initrd_path.exists() {
                qlean::compute_sha256_streaming(&initrd_path).await.ok()
            } else {
                None
            };

            let image_config = CustomImageConfig {
                image_source: ImageSource::LocalPath(PathBuf::from(&path)),
                image_hash,
                image_hash_type: ShaType::Sha256,
                kernel_source: if kernel_path.exists() {
                    Some(ImageSource::LocalPath(kernel_path))
                } else {
                    None
                },
                kernel_hash,
                initrd_source: if initrd_path.exists() {
                    Some(ImageSource::LocalPath(initrd_path))
                } else {
                    None
                },
                initrd_hash,
            };
            qlean::create_custom_image(&format!("custom-{}", vm_name), image_config).await?
        } else {
            tracing::info!("[keep-alive] Using default Debian image");
            qlean::create_image(Distro::Debian, "debian-13-generic-amd64").await?
        };

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

    /// Get the VM's IP address by running hostname -I inside the VM
    pub async fn get_ip(&self) -> Result<Option<String>> {
        let output = self.exec("hostname -I | awk '{print $1}'").await?;
        let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if ip.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ip))
        }
    }
}

impl Clone for KeepAliveMachine {
    fn clone(&self) -> Self {
        Self {
            machine: Arc::clone(&self.machine),
        }
    }
}

# Qlean 问题分析

本文档记录了 `orion-scheduler` 开发过程中遇到的，需要修改 `qlean` crate 的问题。

## 1. KVM 检测警告

### 问题现象

```
WARN qlean::qemu: KVM is not available on this host. QEMU will run without hardware acceleration, which may result in significantly reduced performance.
```

### 根因分析

`qlean` 使用 `kvm-ioctls::Kvm::new()` 检测 KVM 可用性，并通过 `OnceLock` 缓存结果：

```rust
// qlean/src/lib.rs
static KVM_AVAILABLE: OnceLock<bool> = OnceLock::new();

pub async fn with_machine<'a, F, R>(image: &'a Image, config: &'a MachineConfig, f: F) -> Result<R>
{
    // ...
    KVM_AVAILABLE.get_or_init(|| Kvm::new().is_ok());
    // ...
}
```

检测结果被全局缓存。如果首次调用时 `Kvm::new()` 失败（即使是暂时性的），所有后续 VM 创建都会认为 KVM 不可用。

### 观察到的现象

1. **独立测试 KVM 成功**：在隔离环境中测试 `Kvm::new()` 时，它能正常工作并返回 API 版本 12。

2. **权限检查通过**：`/dev/kvm` 权限正确（`crw-rw---- root:kvm`），用户也在 `kvm` 组中。

3. **嵌套虚拟化已启用**：`/sys/module/kvm_intel/parameters/nested = Y`

4. **QEMU 可以使用 KVM**：当 QEMU 直接以 `-enable-kvm -cpu host` 启动时，能正常工作。

### 可能的修复方案

#### 方案 A：延迟重新检测

修改 `qlean`，在每次 VM 启动前重新检查 KVM 可用性，而不是全局缓存：

```rust
fn check_kvm_available() -> bool {
    Kvm::new().is_ok()
}
```

#### 方案 B：移除 OnceLock 缓存

每次调用 `Kvm::new().is_ok()` 时不再缓存，因为检查开销很小：

```rust
let kvm_available = Kvm::new().is_ok();
```

#### 方案 C：提供手动覆盖选项

添加环境变量或配置选项来覆盖 KVM 检测：

```rust
let kvm_available = match std::env::var("QLEAN_FORCE_KVM") {
    Ok(v) => v == "true",
    _ => Kvm::new().is_ok(),
};
```

## 2. 目录权限问题

### 问题现象

`qlean` 将数据存储在 `~/.local/share/qlean/`，当使用 `sudo` 启动时，该路径解析为 `/root/.local/share/qlean`。这导致之后以非 root 用户运行服务时出现权限拒绝错误。

### 根因分析

`qlean` 使用 `directories::ProjectDirs` 确定数据目录：

```rust
// qlean/src/utils.rs
impl QleanDirs {
    pub fn new() -> Result<Self> {
        let project_dir = ProjectDirs::from("", "", "qlean").expect("Couldn't get project dir");
        let data_dir = project_dir.data_dir().to_path_buf();
        // ...
    }
}
```

当使用 `sudo` 启动时，`HOME` 环境变量可能仍指向 `/root`，导致数据存储在 root 的目录中。

### 观察到的问题

1. `/home/ubuntu/.local/share/qlean/images/debian-13-generic-amd64/` 属于 `root:root`
2. `/var/log/orion-scheduler/` 属于 `root:root`

### 可能的修复方案

#### 方案 A：使用 XDG 基础目录规范

遵守 `XDG_DATA_HOME` 环境变量，并回退到用户主目录：

```rust
pub fn get_data_dir() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("qlean")
        })
}
```

#### 方案 B：要求显式配置

要求用户通过环境变量显式设置数据目录：

```rust
pub fn new() -> Result<Self> {
    let data_dir = std::env::var("QLEAN_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::data_dir()
                .unwrap_or_else(|| panic!("Cannot determine data directory"))
                .join("qlean")
        });
    // ...
}
```

#### 方案 C：创建时设置正确权限

创建目录时，确保以正确的所有者创建：

```rust
pub fn create_dir(purpose: &str, path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path).expect("Failed to create directory");
        // 设置权限以允许当前用户访问
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::Permissions::from_mode(0o755);
            std::fs::set_permissions(path, perms)?;
        }
    }
    Ok(())
}
```

## 3. Guest CID 冲突

### 问题现象

当 VM 未正确清理时，vsock guest CID 仍被占用：

```
ERROR qlean::qemu: qemu-system-x86_64: -device vhost-vsock-pci,id=vhost-vsock-pci0,guest-cid=10: vhost-vsock: unable to set guest cid: Address already in use
```

### 根因分析

QEMU 使用静态 guest CID（默认为 10）进行 vsock 连接。如果之前的 VM 进程未正确终止，CID 会保持预留状态。

### 当前临时解决方案

需要手动清理：

```bash
pkill -9 -f qemu-system-x86
```

### 可能的修复方案

1. **动态 CID 分配**：使用随机 CID 而非固定的 10
2. **启动时清理**：启动时检查并终止残留的 QEMU 进程
3. **更好的错误提示**：检测到 CID 冲突时提供更清晰的说明

## 4. KVM 检测时序问题

### 问题现象

`OnceLock` 模式意味着 KVM 检测发生在首次调用 `with_machine()` 或 `with_pool()` 时。如果进程在初始设置后需要降级权限，缓存的"不可用"结果将持续存在。

### 示例场景

1. 进程以 root 身份启动
2. `qlean` 被初始化（在某处依赖链中）
3. KVM 检测运行并失败（由于任何暂时性原因）
4. 进程降级权限到 ubuntu
5. 所有后续 VM 操作都报告 KVM 不可用

### 可能的修复方案

实现懒加载的、每个 VM 的 KVM 检查，而不是全局缓存：

```rust
pub async fn with_machine<'a, F, R>(image: &'a Image, config: &'a MachineConfig, f: F) -> Result<R>
{
    // 在启动 VM 前立即检查 KVM 可用性
    let kvm_available = Kvm::new().is_ok();
    // 仅为此 VM 实例使用 kvm_available
    // ...
}
```

## 修复建议汇总

| 问题 | 优先级 | 建议修复方案 |
|------|--------|--------------|
| KVM OnceLock 缓存 | 高 | 移除全局缓存，每次 VM 检查 |
| 目录权限 | 中 | 遵守 XDG_DATA_HOME，使用当前用户目录 |
| Guest CID 冲突 | 低 | 启动时清理或动态 CID |
| KVM 检测时序 | 中 | 在 VM 启动前懒加载检查 |
| SSH 超时硬编码 | 高 | 支持配置化超时，或减少无 KVM 时的 180 秒超时 |
| 优雅关闭信号处理 | 中 | 支持 SIGTERM/SIGQUIT，不只是 SIGINT |

## 6. VM 启动优化建议

### 当前瓶颈分析

VM 启动耗时分布（无 KVM 情况）：

| 阶段 | 默认超时 | 说明 |
|------|----------|------|
| SSH 连接等待 | **180 秒** | qlean 硬编码，`machine.rs:609-613` |
| Orion 文件上传 | ~10-30 秒 | 477MB 通过 vsock |
| Cloud-init 初始化 | ~10-30 秒 | VM 内部初始化 |

### 优化方案

#### 方案 A：减少无 KVM 时的 SSH 超时

修改 `qlean/src/machine.rs`：

```rust
// 当前：无 KVM 时 180 秒
let ssh_timeout = if kvm_available {
    Duration::from_secs(60)
} else {
    Duration::from_secs(180)  // 太长了！
};

// 优化：减少到 60 秒
let ssh_timeout = if kvm_available {
    Duration::from_secs(60)
} else {
    Duration::from_secs(60)  // 改为 60 秒
};
```

#### 方案 B：添加环境变量配置

```rust
let ssh_timeout = if kvm_available {
    Duration::from_secs(60)
} else {
    std::env::var("QLEAN_SSH_TIMEOUT")
        .unwrap_or_else(|_| "60".to_string())
        .parse()
        .unwrap_or(60)
};
```

#### 方案 C：优化 Orion 文件传输

考虑使用更快的文件传输方式（如并行传输、压缩）减少 Orion 上传时间。

## 5. 调试建议

如果需要进一步调试 qlean 的 KVM 检测问题，可以使用以下方法：

### 方法 1：独立测试程序

创建一个独立的测试程序来验证 KVM：

```rust
use kvm_ioctls::Kvm;

fn main() {
    match Kvm::new() {
        Ok(kvm) => {
            println!("KVM created successfully");
            println!("KVM API version: {}", kvm.get_api_version());
        }
        Err(e) => {
            println!("KVM new failed: {:?}", e);
        }
    }
}
```

### 方法 2：检查进程 capability

```bash
cat /proc/<pid>/status | grep -i cap
```

确保 `CapBnd` 包含 `CAP_SYS_ADMIN`。

### 方法 3：检查 AppArmor 状态

```bash
aa-status
cat /proc/self/attr/current
```

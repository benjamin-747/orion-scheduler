# orion-scheduler 设计文档

## 1. 概述

**目的**：orion-scheduler 是一个服务，接收 GitHub Actions 的 webhook 回调，使用 qlean/QEMU/KVM 管理 VM 生命周期，将 Orion 二进制文件和配置部署到 VM，并管理 Orion 服务。

**前提条件（AWS EC2 环境）**：

orion-scheduler 依赖 KVM 虚拟化，需在 AWS EC2 实例上启用嵌套虚拟化：

| 条件 | 说明 |
|------|------|
| 实例类型 | 支持嵌套虚拟化的类型：`C8i`、`M8i`、`R8i` |
| 嵌套虚拟化 | 需在实例上启用（新建实例时开启或对现有已停止实例修改 CPU 选项） |
| 操作系统 | 本服务运行在 EC2 实例的 Linux 系统中 |

启用方式：

**AWS 控制台**：
1. 停止目标实例
2. 选择实例 → Actions → Instance settings → Change CPU options
3. 在 "Nested virtualization" 选择 "Enable"
4. 保存后重新启动实例

**AWS CLI**：
```bash
# 新建实例时启用
aws ec2 run-instances --cpu-options "NestedVirtualization=enabled" ...

# 对现有已停止实例启用
aws ec2 stop-instances --instance-id i-xxxxx
aws ec2 modify-instance-cpu-options --instance-id i-xxxxx --nested-virtualization enabled
aws ec2 start-instances --instance-id i-xxxxx
```

**GCP 环境**：（待调查）

**架构**：
```
GitHub Actions  --webhook-->  orion-scheduler  --qlean-->  QEMU/KVM VM
                                              |
                                              +-- SSH/SFTP -->  orion 二进制 + 配置
```

## 2. 组件

| 组件 | 描述 |
|------|------|
| `main.rs` | 使用 axum 的 HTTP 服务器入口，支持优雅关闭 |
| `handlers.rs` | HTTP 请求处理器：/webhook, /health, /status, /logs/orion, /logs/orion/live, /logs/orion/stream, /scorpio/status, /shutdown |
| `state.rs` | 用于跟踪 VM 生命周期的 AppState |
| `vm_manager.rs` | VM 部署操作（上传文件、替换环境变量、启动服务） |
| `orion_deployer.rs` | Orion 部署编排，协调 VM 操作 |
| `config.rs` | 动态配置加载和管理，支持从 JSON 文件读取 target 环境配置 |
| `keep_alive.rs` | Keep-alive VM 包装器，支持持久化 VM 连接 |

## 3. API 端点

### GET /health

健康检查端点。

**响应**：
```json
{"status": "healthy", "service": "orion-scheduler"}
```

### GET /status

获取当前 VM 状态。

**响应**（VM 运行中）：
```json
{"status": "running", "vm_id": "orion-vm-1234567890", "uptime_secs": 60, "log_file": "/var/log/orion-scheduler/orion-vm-1234567890-1746766200.log"}
```

**响应**（无 VM）：
```json
{"status": "no_vm", "vm_id": null}
```

### GET /logs/orion

获取格式化后的 Orion 日志（适合终端查看）。

**响应**（成功）：返回 HTML 格式的带颜色日志，带框线和 emoji 标识

```
╔══════════════════════════════════════════════════════════════════════════════╗
║                        ORION STARTUP LOGS                                  ║
╚══════════════════════════════════════════════════════════════════════════════╝

  ✅ May 09 06:39:27 qlean-vm systemd[1]: Starting orion-runner.service...
  🔍 May 09 06:39:27 qlean-vm preflight.sh[801]: ==> [预检] 检查 /dev/fuse ...
  🧹 May 09 06:39:27 qlean-vm cleanup.sh[804]: ==> [清理] 停止旧进程...
  ...
```

**响应**（无 VM）：返回 JSON
```json
{"status": "no_vm", "error": "No VM is currently running"}
```

### GET /logs/orion/live

获取 VM 中 Orion 的实时日志（journalctl + orion.log），返回 JSON 格式。

**响应**（成功）：
```json
{
  "status": "ok",
  "logs": "May 09 06:39:27 qlean-vm systemd[1]: Starting orion-runner.service...\n..."
}
```

**响应**（无 VM）：
```json
{"status": "error", "error": "No VM is currently running"}
```

### GET /logs/orion/stream

SSE 流式端点，每 2 秒推送一次格式化日志。

**使用方式**：
```bash
# 实时查看日志（终端持续刷新）
curl -N http://localhost:8080/logs/orion/stream
```

**响应**：SSE 事件流，格式同 `/logs/orion`

### POST /shutdown

优雅关闭 VM 并退出服务。

**使用方式**：
```bash
curl -X POST http://localhost:8080/shutdown
```

**响应**：
```json
{"status": "ok", "message": "Shutdown initiated, VM will be stopped"}
```

### GET /logs/orion/live

获取 VM 中 Orion 的实时日志（journalctl + orion.log），返回 JSON 格式。

**响应**（成功）：
```json
{
  "status": "ok",
  "logs": "May 09 06:39:27 qlean-vm systemd[1]: Starting orion-runner.service...\norion-runner[797]: ==> [预检] 检查 /dev/fuse...\n..."
}
```

**响应**（无 VM）：
```json
{"status": "error", "error": "No VM is currently running"}
```

### GET /webhook

Webhook 端点健康检查。

**响应**：
```json
{"status": "ok", "vm_id": null, "error": null, "orion_log_file": null}
```

### POST /webhook

接收来自 GitHub Actions 的更新请求。

**请求体**（GitHub Actions 格式）：
```json
{
  "action": "requested",
  "workflow": "deploy.yml",
  "target": "aws-gitmega"
}
```

| 字段 | 类型 | 描述 |
|------|------|------|
| `action` | string | GitHub Actions 事件类型 |
| `workflow` | string | 工作流名称 |
| `target` | string | 目标环境：`aws-gitmega`、`aws-gitmono`、`gcp-buck2hub`（必填） |

**响应**：
```json
{
  "status": "ok",
  "vm_id": "orion-vm-1234567890",
  "error": null,
  "orion_log_file": "/var/log/orion-scheduler/orion-vm-1234567890-1746766200.log"
}
```

## 4. 核心逻辑

### 4.1 状态管理

```rust
use crate::config::SharedConfig;
use crate::keep_alive::KeepAliveMachine;

pub struct VmInfo {
    pub id: String,
    pub created_at: std::time::Instant,
    pub log_file: Option<String>,  // Orion 日志文件路径
}

pub struct AppState {
    vm: Arc<RwLock<Option<VmInfo>>>,
    machine: Arc<RwLock<Option<KeepAliveMachine>>>,  // 持久化的 VM 连接
    pub config: SharedConfig,  // 从 JSON 文件加载的 target 配置
}
```

**Keep-alive 模式**：VM 在部署后保持运行状态，可通过 `GET /logs/orion/live` 实时获取日志。

### 4.2 生命周期

```
[1] 接收 POST /webhook
         ↓
[2] 获取 target 配置（如 aws-gitmega）
         ↓
[3] 检查现有 VM 并优雅关闭（如果存在）
         ↓
[4] 创建新 VM（Debian 镜像，keep-alive 模式）
         ↓
[5] 部署 Orion 文件到 VM
         ↓
[6] 替换环境变量（基于 target 配置）
         ↓
[7] 启动 Orion 服务并获取日志
         ↓
[8] 保存初始日志到文件
         ↓
[9] 更新 VM 状态，VM 保持运行
         ↓
[10] 返回成功响应
```

**注意**：VM 在部署后保持运行状态，可通过 `GET /logs/orion/live` 实时获取日志。

### 4.3 详细步骤

| 阶段 | 步骤 | 操作 | 说明 |
|------|------|------|------|
| **接收请求** | 1 | 接收 webhook | 解析 `target` 参数（必填），从配置获取对应的 `TargetConfig` |
| **清理** | 2 | 清理旧 VM | 优雅关闭已有 VM（调用 `machine.shutdown()`） |
| **创建** | 3 | 创建 VM | 使用 `KeepAliveMachine::new()` 创建新 VM |
| **部署** | 4 | 创建目录 | 在 VM 内创建 `/home/orion/orion-runner/` 目录 |
| | 5 | 上传配置文件 | 通过 SFTP 上传 `run.sh`、`scorpio.toml`、`preflight.sh`、`cleanup.sh` |
| | 6 | 上传 .env 文件 | 上传 `.env.prod` 重命名为 `.env` |
| | 7 | 上传 systemd 服务 | 上传 `orion-runner.service` 到 `/etc/systemd/system/` |
| | 8 | 上传 Orion 二进制 | 通过 SFTP 上传 orion 二进制文件（~500MB） |
| | 9 | 设置权限 | `chmod +x` 对脚本和二进制，设置 `setcap cap_dac_read_search+ep` |
| | 10 | 重载 systemd | 执行 `systemctl daemon-reload` |
| **配置** | 11 | 替换环境变量 | 使用 `sed` 替换 `.env` 中的 `SERVER_WS` 和 `scorpio.toml` 中的 `base_url`、`lfs_url` |
| **启动** | 12 | 创建 Scorpio 目录 | 创建 `/data/scorpio/store`、`/data/scorpio/antares/{upper,cl,mnt}`、`/workspace/mount` |
| | 13 | 设置目录所有权 | `chown -R orion:orion /data/scorpio` 和 `/workspace/mount` |
| | 14 | 启动服务 | `systemctl start orion-runner` |
| | 15 | 验证状态 | 检查 `systemctl is-active orion-runner` 和进程状态 |
| **完成** | 16 | 保存日志 | 将初始日志写入 `log_dir` 目录 |
| | 17 | 保持运行 | VM 保持运行，`orion_log_file` 返回日志文件路径 |

## 5. 功能

### 5.1 环境变量替换

#### 背景

在 GitHub Actions 中，不同环境（`aws-gitmega`、`aws-gitmono`）对应不同的配置值。原工作流在文件上传后通过 SSH 执行 `sed -i` 进行替换。

orion-scheduler 将环境配置外部化为 JSON 配置文件，无需修改代码即可添加新环境。

#### 配置文件格式

通过 `CONFIG_PATH` 环境变量指定配置文件路径（默认为 `target_config.json`）：

```json
{
  "log_dir": "/var/log/orion-scheduler",
  "targets": {
    "aws-gitmega": {
      "server_ws": "wss://orion.gitmega.com/ws",
      "scorpio_base_url": "https://git.gitmega.com",
      "scorpio_lfs_url": "https://git.gitmega.com"
    },
    "aws-gitmono": {
      "server_ws": "wss://orion.gitmono.com/ws",
      "scorpio_base_url": "https://git.gitmono.com",
      "scorpio_lfs_url": "https://git.gitmono.com"
    },
    "gcp-buck2hub": {
      "server_ws": "wss://orion.buck2hub.com/ws",
      "scorpio_base_url": "https://git.buck2hub.com",
      "scorpio_lfs_url": "https://git.buck2hub.com"
    }
  }
}
```

#### target 对应表

| target | SERVER_WS | scorpio.toml base_url | scorpio.toml lfs_url |
|--------|-----------|------------------------|----------------------|
| `aws-gitmega` | `wss://orion.gitmega.com/ws` | `https://git.gitmega.com` | `https://git.gitmega.com` |
| `aws-gitmono` | `wss://orion.gitmono.com/ws` | `https://git.gitmono.com` | `https://git.gitmono.com` |
| `gcp-buck2hub` | `wss://orion.buck2hub.com/ws` | `https://git.buck2hub.com` | `https://git.buck2hub.com` |

#### 扩展新的 target

要添加新的 target，只需在 `target_config.json` 的 `targets` 对象中添加一个新条目：

1. 编辑 `target_config.json`：
```json
{
  "targets": {
    "aws-gitmega": { ... },
    "aws-gitmono": { ... },
    "gcp-buck2hub": { ... },
    "new-target-name": {
      "server_ws": "wss://orion.newdomain.com/ws",
      "scorpio_base_url": "https://git.newdomain.com",
      "scorpio_lfs_url": "https://git.newdomain.com"
    }
  }
}
```

2. 重启 orion-scheduler 服务使配置生效

3. 在 webhook 请求中指定新的 target：
```json
{
  "action": "requested",
  "workflow": "deploy.yml",
  "target": "new-target-name"
}
```

**注意**：如果请求的 target 不存在于配置中，服务会返回错误并列出所有可用的 target。

#### 实现方式

在 `vm_manager.rs` 中通过 SSH 在 VM 内执行 `sed` 命令：

```rust
pub async fn replace_env_vars(
    machine: &mut Machine,
    target_config: &TargetConfig,
    target_name: &str,
) -> Result<()> {
    let server_ws = &target_config.server_ws;
    let scorpio_base_url = &target_config.scorpio_base_url;
    let scorpio_lfs_url = &target_config.scorpio_lfs_url;

    // 替换 .env 中的 SERVER_WS
    let cmd = format!(
        r#"sed -i 's|^SERVER_WS=.*|SERVER_WS="{}"|' /home/orion/orion-runner/.env"#,
        server_ws
    );
    machine.exec(&cmd).await?;

    // 替换 scorpio.toml 中的 base_url（任意值替换为配置的值）
    let cmd = format!(
        r#"sed -i 's|base_url = ".*"|base_url = "{}"|' /home/orion/orion-runner/scorpio.toml"#,
        scorpio_base_url
    );
    machine.exec(&cmd).await?;

    // 替换 scorpio.toml 中的 lfs_url（任意值替换为配置的值）
    let cmd = format!(
        r#"sed -i 's|lfs_url = ".*"|lfs_url = "{}"|' /home/orion/orion-runner/scorpio.toml"#,
        scorpio_lfs_url
    );
    machine.exec(&cmd).await?;

    tracing::info!("[env] Replaced env vars for target: {}", target_name);
    Ok(())
}
```

### 5.2 日志输出

Orion 启动时，将以下信息输出到服务端日志：

| 阶段 | 日志内容 |
|------|----------|
| 目录创建 | `Creating directory: /data/scorpio/store` |
| 文件上传 | `Uploading file: orion (~500MB)` |
| 权限设置 | `Setting permissions on /home/orion/orion-runner/orion` |
| 服务启动 | `Starting Orion service: systemctl start orion-runner` |
| 启动结果 | `Orion service started successfully` 或 `Orion service failed: <error>` |
| 健康检查 | `Orion health check: systemctl is-active orion-runner` |

日志格式：

```rust
tracing::info!("[orion-deploy] Creating directory: {}", path);
tracing::info!("[orion-deploy] Uploading file: {} -> {}", local, remote);
tracing::info!("[orion-deploy] Setting permissions: {}", path);
tracing::info!("[orion-deploy] Starting Orion service");
tracing::info!("[orion-deploy] Orion started successfully");
tracing::error!("[orion-deploy] Orion start failed: {}", error);
```

## 6. 配置文件

| 来源 | 目标 | 作用描述 |
|------|------|----------|
| `scorpio.toml` | `/home/orion/orion-runner/scorpio.toml` | Scorpio FUSE 文件系统配置，定义 Mega 服务地址、store_path、workspace 挂载点、Dicfuse 和 Antares overlay 的行为参数 |
| `.env.prod` | `/home/orion/orion-runner/.env` | Orion 运行时的环境变量，包括 `SERVER_WS`（WebSocket 服务器地址）、`BUCK_PROJECT_ROOT`（Buck 项目路径）等 |
| `run.sh` | `/home/orion/orion-runner/run.sh` | Orion 启动脚本，加载 `.env` 环境变量，执行 `preflight.sh` 前置检查，然后启动 orion 进程 |
| `preflight.sh` | `/home/orion/orion-runner/preflight.sh` | 前置检查脚本，验证 FUSE 能力和设备访问权限，确保环境满足 Orion 运行要求 |
| `cleanup.sh` | `/home/orion/orion-runner/cleanup.sh` | 清理脚本，在 Orion 启动前杀死旧进程并卸载 FUSE 文件系统 |
| `orion-runner.service` | `/etc/systemd/system/orion-runner.service` | systemd 服务单元定义，负责配置 Orion 服务的启动参数、运行环境、权限和能力、停止超时等 |
| `orion` | `/home/orion/orion-runner/orion` | Orion 主程序二进制文件，Buck 构建任务的 WebSocket 客户端，接收并执行构建任务 |

## 7. 部署与运行

### 7.1 资源回收

#### 优雅关闭流程

当服务收到 SIGTERM 或 SIGINT 信号时：

```
1. 收到终止信号
2. 停止接收新请求
3. 检查是否有运行中的 VM
4. 如果有 VM：
   a. 调用 machine.shutdown() 关闭 Orion 服务
   b. 调用 machine.stop() 停止 QEMU 进程
   c. 等待 VM 进程完全退出（最多 30 秒）
   d. 如果超时，强制 kill QEMU 进程
5. 清理状态文件（runs 目录下的临时文件）
6. 退出进程
```

#### 实现机制

```rust
async fn graceful_shutdown(app_state: Arc<AppState>) {
    if let Some(mut vm_info) = app_state.vm.write().await.take() {
        if let Some(machine) = vm_info.machine.take() {
            machine.exec("systemctl stop orion-runner").await;
            machine.shutdown().await;
        }
    }
}
```

#### 异常情况处理

| 场景 | 处理方式 |
|------|----------|
| VM 关闭超时 | 强制 kill QEMU 进程（`kill -9`） |
| QEMU 僵死 | 使用 `fuser -k` 释放端口 |
| 残留进程 | 启动时检查并清理孤儿进程 |
| 文件锁 | 清理 `/var/lock/qemu/` 下的锁文件 |

#### 启动时检查

服务启动时执行以下清理：

```bash
# 清理残留 QEMU 进程
pkill -9 qemu-system-x86

# 清理端口占用
fuser -k 8080/tcp 2>/dev/null

# 清理残留的 runs 目录
rm -rf ~/.local/share/qlean/runs/*
```

### 7.2 运行服务

```bash
# 构建
cargo build --release

# 运行（需要 KVM 和 root 权限）
sudo env "PATH=$PATH" "RUSTUP_HOME=$RUSTUP_HOME" "CARGO_HOME=$CARGO_HOME" "HOME=$HOME" cargo run --release

# 指定配置文件运行
CONFIG_PATH=/path/to/target_config.json sudo env "PATH=$PATH" ... cargo run --release

# 查看日志
RUST_LOG=debug cargo run --release 2>&1 | grep -E '\[orion|webhook|vm'
```

## 8. 测试方法

### 8.1 本地调试

```bash
# 1. 构建调试版本
cargo build

# 2. 运行服务（调试模式）
sudo env "PATH=$PATH" "RUSTUP_HOME=$RUSTUP_HOME" "CARGO_HOME=$CARGO_HOME" "HOME=$HOME" cargo run

# 3. 新开终端，发送 webhook 请求
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{"action": "requested", "workflow": "deploy.yml", "target": "aws-gitmega"}'

# 4. 检查服务状态（VM 应保持运行状态）
curl http://localhost:8080/status

# 5. 获取格式化日志（HTML 格式，带颜色，适合终端直接查看）
curl http://localhost:8080/logs/orion

# 6. 获取实时日志（JSON 格式，journalctl + orion.log）
curl http://localhost:8080/logs/orion/live

# 7. 持续监控日志（SSE 流，每 2 秒刷新，适合终端监控）
curl -N http://localhost:8080/logs/orion/stream
```

### 8.2 API 测试

```bash
# 健康检查
curl http://localhost:8080/health
# 响应: {"status": "healthy", "service": "orion-scheduler"}

# Webhook GET（健康检查）
curl http://localhost:8080/webhook
# 响应: {"status": "ok", "vm_id": null, "error": null, "orion_log_file": null}

# Webhook POST（触发部署，keep-alive 模式）
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{"target": "gcp-buck2hub"}'
# 响应: {"status": "ok", "vm_id": "orion-vm-xxx", "error": null, "orion_log_file": null}
# 注意：orion_log_file 在响应中为 null，日志通过日志端点获取

# 获取 VM 状态（keep-alive 模式，VM 持续运行）
curl http://localhost:8080/status
# 响应: {"status": "running", "vm_id": "orion-vm-xxx", "uptime_secs": 60, "log_file": "/var/log/orion-scheduler/..."}

# 获取格式化日志（HTML，带颜色框线）
curl http://localhost:8080/logs/orion
# 响应: HTML 格式日志，适合直接 curl 查看

# 获取实时 JSON 日志
curl http://localhost:8080/logs/orion/live
# 响应: {"status": "ok", "logs": "May 09 03:15:45 orion-runner..."}

# SSE 持续监控
curl -N http://localhost:8080/logs/orion/stream
# 响应: SSE 事件流，每 2 秒推送格式化日志

# 检查 Scorpio 挂载状态
curl http://localhost:8080/scorpio/status
# 响应: {"status": "ok", "directories": {...}, "mounts": "...", "orion_process": "...", "scorpio_process": "..."}

# 优雅关闭（停止 VM 并退出）
curl -X POST http://localhost:8080/shutdown
# 响应: {"status": "ok", "message": "Shutdown initiated, VM will be stopped"}
```

### 8.3 日志端点对比

| 端点 | 响应格式 | 特点 | 使用场景 |
|------|----------|------|----------|
| `GET /logs/orion` | HTML | 带颜色、emoji、框线 | `curl` 直接查看，终端可视化 |
| `GET /logs/orion/live` | JSON | 实时查询 | 程序调用，获取日志文本 |
| `GET /logs/orion/stream` | SSE | 每 2 秒推送 | `curl -N` 持续监控 |

### 8.4 服务管理

#### 停止 orion-scheduler 服务

```bash
# 停止 orion-scheduler 服务进程
pkill -9 -f orion-scheduler

# 停止所有 QEMU 进程（如果有残留的 VM）
sudo pkill -9 -f qemu-system-x86

# 验证进程已停止
ps aux | grep -E "orion-scheduler|qemu-system" | grep -v grep

# 检查端口是否释放
fuser 8080/tcp 2>/dev/null || echo "Port 8080 is free"
```

#### 启动 orion-scheduler 服务

```bash
cd /home/ubuntu/orion-scheduler
cargo run
```

#### 检查服务状态

```bash
# 检查 HTTP API 状态
curl http://localhost:8080/status

# 检查 orion-scheduler 进程
ps aux | grep orion-scheduler | grep -v grep

# 检查 QEMU 进程
ps aux | grep qemu | grep -v grep
```

#### 优雅关闭服务（推荐方式）

```bash
# 方法一：使用 HTTP POST 请求关闭（会先停止 VM 再退出）
curl -X POST http://localhost:8080/shutdown

# 方法二：发送 SIGTERM 信号
kill -TERM <pid>

# 方法三：使用 systemctl (如果是 systemd 服务)
systemctl stop orion-scheduler
```

**注意**：`pkill -9 -f orion-scheduler` 会直接杀死进程，不会优雅关闭 VM！

### 8.5 查看日志

```bash
# 服务端日志
RUST_LOG=debug cargo run 2>&1 | grep -E '\[orion|webhook|vm'

# Orion 格式化日志（推荐 - 终端带颜色）
curl http://localhost:8080/logs/orion

# Orion 实时 SSE 流（持续刷新，Ctrl+C 退出）
curl -N http://localhost:8080/logs/orion/stream

# systemd 日志（如服务以 systemd 运行）
journalctl -u orion-scheduler -f
```

### 8.6 常见问题排查

| 问题 | 排查方法 |
|------|----------|
| KVM 权限错误 | 检查 `/dev/kvm` 权限，确保用户在 `kvm` 组 |
| QEMU 网络桥接失败 | 检查 `/etc/qemu/bridge.conf` 是否配置 `allow qlbr0` |
| VM 启动超时 | 检查 cloud-init 是否正常，SSH 是否可连接 |
| Orion 启动失败 | `curl http://localhost:8080/logs/orion` 查看格式化日志 |
| Scorpio 挂载问题 | `curl http://localhost:8080/scorpio/status` 检查挂载状态 |
| VM 已关闭但状态显示 running | 重启服务或检查 VM 是否异常退出 |

## 9. 限制和未来工作

- **状态持久化**：VM 状态持久化在内存中，服务重启后 VM 状态丢失
- **安全**：没有 webhook 签名验证
- **错误处理**：需要更健壮的错误恢复
- **并发请求**：不支持 - 一次只能有一个 VM
- **日志持久化**：初始日志持久化到文件，实时日志从 journalctl 读取
# 测试方法

本文档包含本地调试、API 测试、服务管理和常见问题排查的方法。

## 1. 本地调试

### 构建与运行

```bash
# 构建调试版本
cargo build

# 运行服务（调试模式）
sudo env "PATH=$PATH" "RUSTUP_HOME=$RUSTUP_HOME" "CARGO_HOME=$CARGO_HOME" "HOME=$HOME" cargo run
```

### 调试流程

```bash
# 1. 发送 webhook 请求（使用默认 Debian 镜像）
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{"target": "aws-gitmega"}'

# 1b. 发送 webhook 请求（指定本地自定义镜像）
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{
    "target": "aws-gitmega",
    "image_path": "/home/ubuntu/.local/share/qlean/images/debian-13-buck2/debian-13-buck2.qcow2",
    "image_digest": "sha256:677ef198bb2a8a30bb3a593b1b70efb9a14f6e06a1193df47d4e028bce0445d6",
    "image_disk_gb": 20,
    "image_cpus": 4,
    "image_memory_mb": 8192
  }'

# 1c. 发送 webhook 请求（指定远程镜像）
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{
    "target": "aws-gitmega",
    "image_url": "https://artifacts.company.com/buck2-custom.qcow2",
    "image_digest": "sha256:efgh5678..."
  }'

# 2. 检查服务状态（VM 应保持运行状态）
curl http://localhost:8080/status

# 3. 获取格式化日志（HTML 格式，带颜色，适合终端直接查看）
curl http://localhost:8080/logs/orion

# 4. 获取实时日志（JSON 格式，journalctl + orion.log）
curl http://localhost:8080/logs/orion/live

# 5. 持续监控日志（SSE 流，每 2 秒刷新，适合终端监控）
curl -N http://localhost:8080/logs/orion/stream
```

## 2. API 测试

### 健康检查

```bash
curl http://localhost:8080/health
# 响应: {"status": "healthy", "service": "orion-scheduler"}
```

### Webhook

```bash
# GET - 健康检查
curl http://localhost:8080/webhook
# 响应: {"status": "ok", "vm_id": null, "error": null, "orion_log_file": null}

# POST - 触发部署（keep-alive 模式，使用默认 Debian 镜像）
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{"target": "gcp-buck2hub"}'
# 响应: {"status": "ok", "vm_id": "orion-vm-xxx", "error": null, "orion_log_file": null}

# POST - 指定本地镜像
curl -X POST http://localhost:8080/webhook \
  -H "Content-Type: application/json" \
  -d '{
    "target": "gcp-buck2hub",
    "image_path": "/home/ubuntu/.local/share/qlean/images/debian-13-buck2/debian-13-buck2.qcow2",
    "image_digest": "sha256:abcd1234...",
    "image_disk_gb": 20,
    "image_cpus": 4,
    "image_memory_mb": 8192
  }'
```

### VM 状态

```bash
# 获取 VM 状态（keep-alive 模式，VM 持续运行）
curl http://localhost:8080/status
# 响应: {"status": "running", "vm_id": "orion-vm-xxx", "vm_ip": "192.168.221.87", "uptime_secs": 60, "log_file": "/var/log/orion-scheduler/..."}
```

### 日志端点


| 端点                       | 响应格式 | 特点           | 使用场景              |
| ------------------------ | ---- | ------------ | ----------------- |
| `GET /logs/orion`        | HTML | 带颜色、emoji、框线 | `curl` 直接查看，终端可视化 |
| `GET /logs/orion/live`   | JSON | 实时查询         | 程序调用，获取日志文本       |
| `GET /logs/orion/stream` | SSE  | 每 2 秒推送      | `curl -N` 持续监控    |


```bash
# 获取格式化日志（HTML，带颜色框线）
curl http://localhost:8080/logs/orion

# 获取实时 JSON 日志
curl http://localhost:8080/logs/orion/live

# SSE 持续监控
curl -N http://localhost:8080/logs/orion/stream
```

### Scorpio 状态

```bash
curl http://localhost:8080/scorpio/status
# 响应: {"status": "ok", "directories": {...}, "mounts": "...", "orion_process": "...", "scorpio_process": "..."}
```

### 关闭

```bash
# 优雅关闭（停止 VM 并退出）
curl -X POST http://localhost:8080/shutdown
# 响应: {"status": "ok", "message": "Shutdown initiated, VM will be stopped"}
```

## 3. 服务管理

### 启动服务

```bash
cd /home/ubuntu/orion-scheduler
cargo run
```

### 停止服务

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

### 检查服务状态

```bash
# 检查 HTTP API 状态
curl http://localhost:8080/status

# 检查 orion-scheduler 进程
ps aux | grep orion-scheduler | grep -v grep

# 检查 QEMU 进程
ps aux | grep qemu | grep -v grep
```

### 优雅关闭对比


| 操作                            | VM  | 服务器  | 说明                     |
| ----------------------------- | --- | ---- | ---------------------- |
| `Ctrl+C`                      | 停止  | 停止   | 关闭 VM 后退出服务            |
| SIGTERM                       | 停止  | 停止   | 关闭 VM 后退出服务            |
| SIGQUIT                       | 停止  | 停止   | 关闭 VM 后退出服务            |
| `POST /shutdown`              | 停止  | 继续运行 | 仅关闭 VM，服务保持运行          |
| `pkill -9 -f orion-scheduler` | -   | 停止   | **不优雅**：直接杀死进程，不会关闭 VM |


```bash
# 关闭 VM，服务继续运行（推荐）
curl -X POST http://localhost:8080/shutdown

# 发送 SIGTERM 信号（关闭 VM 并停止服务）
kill -TERM <pid>

# 强制杀死进程（不优雅）
pkill -9 -f orion-scheduler
```

## 4. 查看日志

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

## 5. 常见问题排查


| 问题                  | 排查方法                                                                                                                                  |
| ------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| KVM 权限错误            | 检查 `/dev/kvm` 权限，确保用户在 `kvm` 组                                                                                                        |
| QEMU 网络桥接失败         | 检查 `/etc/qemu/bridge.conf` 是否配置 `allow qlbr0`                                                                                         |
| VM 启动超时             | 检查 cloud-init 是否正常，SSH 是否可连接                                                                                                          |
| Orion 启动失败          | `curl http://localhost:8080/logs/orion` 查看格式化日志                                                                                       |
| Scorpio 挂载问题        | `curl http://localhost:8080/scorpio/status` 检查挂载状态                                                                                    |
| VM 已关闭但状态显示 running | 重启服务或检查 VM 是否异常退出                                                                                                                     |
| 需要 SSH 进入 VM 调试     | Orion-scheduler 会自动注入 `/home/ubuntu/.ssh/orion_vm_access.pub` 对应的私钥访问权限。使用 `ssh -i /home/ubuntu/.ssh/orion_vm_access root@<vm-ip>` 连接 |



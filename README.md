# Qlean VM 测试指南

## 测试命令

由于 QEMU/KVM 需要特权访问，请使用以下命令运行测试：

```bash
sudo env "PATH=$PATH" "RUSTUP_HOME=/home/ubuntu/.rustup" "CARGO_HOME=/home/ubuntu/.cargo" "HOME=/home/ubuntu" cargo test
```

### 运行特定测试

```bash
# 运行 test_with_vm
sudo env "PATH=$PATH" "RUSTUP_HOME=/home/ubuntu/.rustup" "CARGO_HOME=/home/ubuntu/.cargo" "HOME=/home/ubuntu" cargo test test_with_vm

# 运行 test_ubuntu_vm
sudo env "PATH=$PATH" "RUSTUP_HOME=/home/ubuntu/.rustup" "CARGO_HOME=/home/ubuntu/.cargo" "HOME=/home/ubuntu" cargo test test_ubuntu_vm

# 仅测试镜像创建（不需要启动 VM）
cargo test test_ubuntu_image_creation
```

## 故障排除

### Permission denied (os error 13)

错误信息：`Error: Permission denied (os error 13)`

原因：ubuntu 用户虽然属于 kvm 组，但当前 shell 会话可能未正确继承组权限。

解决方案：使用上面的 sudo 命令运行测试。

### 为什么需要 sudo

QEMU/KVM 虚拟化需要访问以下特权资源：
- `/dev/kvm` - KVM 设备节点
- `qemu-bridge-helper` - 需要 `CAP_NET_ADMIN` capability 进行桥接网络操作
- libvirt 网络操作

## 环境要求

1. **KVM 支持**：确保嵌套虚拟化已启用
   ```bash
   ls -la /dev/kvm
   ```

2. **qemu-bridge-helper capabilities**：
   ```bash
   getcap /usr/lib/qemu/qemu-bridge-helper
   # 应输出：/usr/lib/qemu/qemu-bridge-helper cap_net_admin=ep
   ```

3. **桥接网络配置**：
   ```bash
   cat /etc/qemu/bridge.conf
   # 应输出：allow qlbr0
   ```

4. **libvirt 网络**：
   ```bash
   sudo virsh net-list --all
   # 应显示 qlean 网络处于 active 状态
   ```

## 别名设置

将以下内容添加到 `~/.bashrc` 以简化命令：

```bash
echo 'alias qtest="sudo env \"PATH=\$PATH\" \"RUSTUP_HOME=/home/ubuntu/.rustup\" \"CARGO_HOME=/home/ubuntu/.cargo\" \"HOME=/home/ubuntu\" cargo test"' >> ~/.bashrc
source ~/.bashrc
```

之后只需运行：

```bash
qtest
```
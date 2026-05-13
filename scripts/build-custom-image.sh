#!/bin/bash
# Build a custom Debian image with buck2 pre-installed
# Uses QEMU + cloud-init to install buck2 on first boot, then exports the image
#
# Usage: sudo ./build-custom-image.sh
#
# Note: Must run as root because QEMU uses KVM

set -e

OUTPUT_DIR="/home/ubuntu/.local/share/qlean/images"
IMAGE_NAME="debian-13-buck2"
IMAGE_DIR="$OUTPUT_DIR/$IMAGE_NAME"

echo "[build-custom-image] Starting custom image build..."
echo "[build-custom-image] This will take several minutes..."

# Create output directory
mkdir -p "$IMAGE_DIR"

# Copy base image
BASE_IMAGE="$OUTPUT_DIR/debian-13-generic-amd64/debian-13-generic-amd64.qcow2"
CUSTOM_IMAGE="$IMAGE_DIR/$IMAGE_NAME.qcow2"

if [ ! -f "$BASE_IMAGE" ]; then
    echo "[build-custom-image] ERROR: Base image not found at $BASE_IMAGE"
    exit 1
fi

echo "[build-custom-image] Copying base image..."
cp "$BASE_IMAGE" "$CUSTOM_IMAGE"

# Clear cloud-init state to force it to run on first boot
# Cloud-init in the base image has "already ran" state that would skip our runcmd
echo "[build-custom-image] Clearing cloud-init state..."
sudo qemu-nbd --disconnect /dev/nbd0 2>/dev/null || true
sudo qemu-nbd -c /dev/nbd0 "$CUSTOM_IMAGE"
sleep 2
sudo mount /dev/nbd0p1 /mnt 2>/dev/null || (sudo mkfs.ext4 /dev/nbd0p1 && sudo mount /dev/nbd0p1 /mnt)
sudo rm -rf /mnt/var/lib/cloud/data/* 2>/dev/null || true
sudo rm -rf /mnt/var/lib/cloud/instance/* 2>/dev/null || true
sudo umount /mnt
sudo qemu-nbd --disconnect /dev/nbd0
echo "[build-custom-image] Cloud-init state cleared"

# Get kernel and initrd from base image
KERNEL="$OUTPUT_DIR/debian-13-generic-amd64/vmlinuz-6.12.85+deb13-amd64"
INITRD="$OUTPUT_DIR/debian-13-generic-amd64/initrd.img-6.12.85+deb13-amd64"

# Buck2 installation commands (matching the Orion Dockerfile approach)
# Uses facebook/buck2 releases, not buck2.github.io
BUCK2_VERSION="2026-05-01"
BUCK2_ARCH="x86_64-unknown-linux-musl"
BUCK2_URL="https://github.com/facebook/buck2/releases/download/${BUCK2_VERSION}/buck2-${BUCK2_ARCH}.zst"

# Create a seed.iso with cloud-init to install buck2 on first boot
SEED_DIR="/tmp/custom_image_seed_$$"
mkdir -p "$SEED_DIR"

cat > "$SEED_DIR/user-data" << 'EOF'
#cloud-config
users:
  - name: root
    ssh_authorized_keys:
      - ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIF9LTEGIaaad0XP4qUfBoVRgeOg+G36jIWIiqIWP/k4g

runcmd:
  - echo "=== Installing Rust toolchain ==="
  - apt-get update && apt-get install -y clang curl zstd
  - curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
  - |
    export HOME="/root"
    export PATH="/root/.cargo/bin:$PATH"
    # Add to system-wide PATH for non-interactive shells (SSH, scripts)
    echo 'PATH="/root/.cargo/bin:/usr/local/bin:/usr/bin:/bin"' >> /etc/environment
    # Symlink rustc to /usr/local/bin for easier access
    ln -sf /root/.cargo/bin/rustc /usr/local/bin/rustc
    ln -sf /root/.cargo/bin/cargo /usr/local/bin/cargo
    rustc --version
  - echo "=== Installing buck2 ==="
  - curl -fsSL -o /tmp/buck2.zst "https://github.com/facebook/buck2/releases/download/2026-05-01/buck2-x86_64-unknown-linux-musl.zst"
  - zstd -d /tmp/buck2.zst -o /usr/local/bin/buck2
  - chmod +x /usr/local/bin/buck2
  - /usr/local/bin/buck2 --version
  - echo "=== Installation complete ==="
  - apt-get clean && rm -rf /var/lib/apt/lists/*
  - shutdown -h now
EOF

cat > "$SEED_DIR/meta-data" << 'EOF'
instance-id: custom-buck2-install
local-hostname: buck2-vm
EOF

# Generate seed.iso using xorriso
echo "[build-custom-image] Creating seed.iso..."
xorriso -as mkisofs -output "$SEED_DIR/seed.iso" -volid cidata -joliet -rock "$SEED_DIR/user-data" "$SEED_DIR/meta-data"

# Start QEMU with the custom image to install buck2
echo "[build-custom-image] Starting temporary VM to install buck2..."
qemu-system-x86_64 \
    -machine hpet=off \
    -device vhost-vsock-pci,id=vhost-vsock-pci0,guest-cid=99 \
    -kernel "$KERNEL" \
    -append "rw root=/dev/vda1 console=ttyS0" \
    -initrd "$INITRD" \
    -drive file="$CUSTOM_IMAGE,if=virtio,cache=writeback" \
    -nographic \
    -netdev bridge,id=net0,br=qlbr0 \
    -device virtio-net-pci,netdev=net0,mac=52:54:00:AB:CD:EF \
    -m 4096 -smp 2 \
    -drive file="$SEED_DIR/seed.iso,if=virtio,media=cdrom" \
    -monitor unix:/tmp/qemu-custom-image-monitor.sock,server,nowait \
    > /tmp/qemu-custom-image.log 2>&1 &

QEMU_PID=$!
echo "[build-custom-image] VM started with PID $QEMU_PID"

# Wait for VM to install and shutdown (up to 20 minutes)
echo "[build-custom-image] Waiting for installation to complete (this may take 10-20 minutes)..."
for i in $(seq 1 240); do
    if ! kill -0 $QEMU_PID 2>/dev/null; then
        echo "[build-custom-image] VM has shut down"
        break
    fi
    # Check for shutdown signal in logs
    if grep -q "Power down" /tmp/qemu-custom-image.log 2>/dev/null; then
        echo "[build-custom-image] VM received shutdown signal"
        sleep 5
        break
    fi
    # Check if buck2 was installed successfully
    if grep -q "Buck2 installed successfully" /tmp/qemu-custom-image.log 2>/dev/null; then
        echo "[build-custom-image] Buck2 installation detected as complete"
        sleep 10
        break
    fi
    if [ $((i % 20)) -eq 0 ]; then
        echo "[build-custom-image] Still waiting... ($i/240 seconds)"
        grep -E "(buck2|Buck2|installed|downloading|Error|error)" /tmp/qemu-custom-image.log 2>/dev/null | tail -5
    fi
    sleep 5
done

# Force kill if still running
if kill -0 $QEMU_PID 2>/dev/null; then
    echo "[build-custom-image] VM still running after 20 minutes, forcing shutdown..."
    echo "system_reset" | socat - UNIX-CONNECT:/tmp/qemu-custom-image-monitor.sock 2>/dev/null || true
    sleep 5
    kill -9 $QEMU_PID 2>/dev/null || true
fi

# Copy kernel and initrd
echo "[build-custom-image] Copying kernel and initrd..."
cp "$KERNEL" "$IMAGE_DIR/"
cp "$INITRD" "$IMAGE_DIR/"

# Cleanup seed
rm -rf "$SEED_DIR"

# Calculate checksums
echo "[build-custom-image] Calculating checksums..."
cd "$IMAGE_DIR"
sha256sum "$IMAGE_NAME.qcow2" > checksums

echo ""
echo "[build-custom-image] ==============================================="
echo "[build-custom-image] Custom image build complete!"
echo "[build-custom-image] ==============================================="
echo ""
echo "Image: $IMAGE_DIR/$IMAGE_NAME.qcow2"
echo "Kernel: $IMAGE_DIR/vmlinuz-6.12.85+deb13-amd64"
echo "Initrd: $IMAGE_DIR/initrd.img-6.12.85+deb13-amd64"
echo ""
cat checksums
echo ""
echo "To use this image, add to target_config.json:"
echo '  "custom_images": {'
echo '    "buck2": {'
echo '      "path": "/home/ubuntu/.local/share/qlean/images/debian-13-buck2/debian-13-buck2.qcow2",'
echo '      "description": "Debian 13 with pre-installed buck2"'
echo '    }'
echo '  }'
echo '  "default_image": "buck2"'
echo '}'

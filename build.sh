#!/bin/bash
set -e

# 确保脚本在自身所在目录下运行
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"

API_LEVEL="${API_LEVEL:-21}"

# 1. 检查 Rust 环境
if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        source "$HOME/.cargo/env"
    fi
fi

if ! command -v cargo >/dev/null 2>&1; then
    echo "错误: 未找到 cargo 命令！" >&2
    echo "请先安装 Rust 工具链: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 1
fi

echo "使用 Cargo 版本: $(cargo --version)"

# 2. 自动检查并安装目标架构 (若使用 rustup)
TARGETS=(
    "aarch64-linux-android"
    "armv7-linux-androideabi"
    "x86_64-linux-android"
    "i686-linux-android"
)

if command -v rustup >/dev/null 2>&1; then
    INSTALLED_TARGETS="$(rustup target list --installed)"
    for t in "${TARGETS[@]}"; do
        if ! echo "$INSTALLED_TARGETS" | grep -q "^${t}\$"; then
            echo "正在添加目标架构: $t ..."
            rustup target add "$t"
        fi
    done
fi

# 3. 查找 Android NDK 路径
find_ndk() {
    # 命令行参数优先
    if [ -n "$1" ] && [ -d "$1/toolchains/llvm/prebuilt" ]; then
        echo "$1"
        return 0
    fi

    # 环境变量检测
    for env_var in "$ANDROID_NDK_HOME" "$NDK_HOME" "$ANDROID_NDK_ROOT" "$ANDROID_NDK" "$NDK_PATH"; do
        if [ -n "$env_var" ] && [ -d "$env_var/toolchains/llvm/prebuilt" ]; then
            echo "$env_var"
            return 0
        fi
    done

    # 常见 SDK / NDK 安装路径自动检测
    local candidates=(
        "$ANDROID_HOME/ndk"/*
        "$ANDROID_SDK_ROOT/ndk"/*
        "$HOME/android-sdk/ndk"/*
        "$HOME/Android/Sdk/ndk"/*
        "$HOME/Android/sdk/ndk"/*
        "/home/Android/Sdk/ndk"/*
        "/opt/android-sdk/ndk"/*
        "/opt/android-ndk"*
    )

    local valid_ndks=()
    for c in "${candidates[@]}"; do
        if [ -d "$c/toolchains/llvm/prebuilt" ]; then
            valid_ndks+=("$c")
        fi
    done

    if [ ${#valid_ndks[@]} -gt 0 ]; then
        # 按版本号排序，取最新版本
        printf "%s\n" "${valid_ndks[@]}" | sort -V | tail -n 1
        return 0
    fi

    return 1
}

NDK_PATH="$(find_ndk "$1")" || {
    echo "错误: 未找到有效的 Android NDK！" >&2
    echo "请设置 ANDROID_NDK_HOME 环境变量或作为参数传入，例如:" >&2
    echo "  export ANDROID_NDK_HOME=/path/to/ndk" >&2
    echo "  ./build.sh /path/to/ndk" >&2
    exit 1
}

echo "使用 NDK 路径: $NDK_PATH"

# 4. 自动检测 LLVM prebuilt 目录（适配当前宿主系统和架构）
PREBUILT_BASE="$NDK_PATH/toolchains/llvm/prebuilt"
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64|amd64) HOST_ARCH_NAME="x86_64" ;;
    aarch64|arm64) HOST_ARCH_NAME="arm64" ;;
    *) HOST_ARCH_NAME="$HOST_ARCH" ;;
esac

TOOLCHAIN=""
if [ -d "$PREBUILT_BASE/${HOST_OS}-${HOST_ARCH_NAME}/bin" ]; then
    TOOLCHAIN="$PREBUILT_BASE/${HOST_OS}-${HOST_ARCH_NAME}/bin"
elif [ -d "$PREBUILT_BASE/${HOST_OS}-x86_64/bin" ]; then
    TOOLCHAIN="$PREBUILT_BASE/${HOST_OS}-x86_64/bin"
else
    for d in "$PREBUILT_BASE"/*; do
        if [ -d "$d/bin" ]; then
            TOOLCHAIN="$d/bin"
            break
        fi
    done
fi

if [ -z "$TOOLCHAIN" ] || [ ! -d "$TOOLCHAIN" ]; then
    echo "错误: 未找到 NDK LLVM 工具链 bin 目录: $PREBUILT_BASE" >&2
    exit 1
fi

echo "使用工具链路径: $TOOLCHAIN"

mkdir -p bin

# 5. 跨架构 Cargo 编译函数
compile_rust_binary() {
    local rust_target="$1"
    local clang_wrapper="$2"
    local output_file="$3"
    local linker_env_var="$4"

    echo "----------------------------------------"
    echo "正在编译 [$rust_target] -> $output_file ..."

    local linker_path="$TOOLCHAIN/$clang_wrapper"
    if [ ! -x "$linker_path" ]; then
        linker_path="$TOOLCHAIN/clang"
    fi

    export "$linker_env_var"="$linker_path"

    cargo build --release --target "$rust_target"

    local built_bin="target/$rust_target/release/pid_wrap"
    if [ ! -f "$built_bin" ]; then
        echo "错误: 编译产物不存在: $built_bin" >&2
        return 1
    fi

    cp -f "$built_bin" "$output_file"

    if [ -x "$TOOLCHAIN/llvm-strip" ]; then
        "$TOOLCHAIN/llvm-strip" -s "$output_file" 2>/dev/null || true
    fi
    echo "完成: $output_file ($(ls -lh "$output_file" | awk '{print $5}'))"
}

compile_rust_binary "aarch64-linux-android" "aarch64-linux-android${API_LEVEL}-clang" "bin/pid_wrap_arm64" "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
compile_rust_binary "armv7-linux-androideabi" "armv7a-linux-androideabi${API_LEVEL}-clang" "bin/pid_wrap_armeabi" "CARGO_TARGET_ARMV7_LINUX_ANDROIDEABI_LINKER"
compile_rust_binary "x86_64-linux-android" "x86_64-linux-android${API_LEVEL}-clang" "bin/pid_wrap_x86_64" "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER"
compile_rust_binary "i686-linux-android" "i686-linux-android${API_LEVEL}-clang" "bin/pid_wrap_x86" "CARGO_TARGET_I686_LINUX_ANDROID_LINKER"

chmod +x bin/* *.sh 2>/dev/null || true

# 6. 打包 ZIP（优先 Python3，回退至 zip 命令）
ZIP_NAME="soft_restart_fix.zip"

echo "----------------------------------------"
echo "正在打包 $ZIP_NAME ..."

if command -v python3 >/dev/null 2>&1; then
    python3 -c '
import zipfile, os

zip_filename = "'"$ZIP_NAME"'"
files_to_pack = [
    "module.prop",
    "customize.sh",
    "emulated-soft-reboot.sh",
    "LICENSE",
    "bin/pid_wrap_arm64",
    "bin/pid_wrap_armeabi",
    "bin/pid_wrap_x86",
    "bin/pid_wrap_x86_64"
]

with zipfile.ZipFile(zip_filename, "w", zipfile.ZIP_DEFLATED) as zf:
    for item in files_to_pack:
        if os.path.exists(item):
            zf.write(item)
        else:
            print(f"警告: 待打包文件不存在: {item}")
print(f"Zip 包生成成功: {zip_filename}")
'
elif command -v zip >/dev/null 2>&1; then
    rm -f "$ZIP_NAME"
    zip -r -q "$ZIP_NAME" module.prop customize.sh emulated-soft-reboot.sh LICENSE bin/
    echo "Zip 包生成成功: $ZIP_NAME"
else
    echo "错误: 未找到 python3 或 zip 命令，无法打包" >&2
    exit 1
fi

ls -lh "$ZIP_NAME"

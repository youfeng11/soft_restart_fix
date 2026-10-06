#!/bin/bash
set -e

# 确保脚本在自身所在目录下运行
PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$PROJECT_DIR"

API_LEVEL="${API_LEVEL:-21}"

# 1. 查找 NDK 路径
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

# 2. 自动检测 LLVM prebuilt 目录（适配当前宿主系统和架构）
PREBUILT_BASE="$NDK_PATH/toolchains/llvm/prebuilt"
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"
case "$HOST_ARCH" in
    x86_64|amd64) HOST_ARCH_NAME="x86_64" ;;
    aarch64|arm64) HOST_ARCH_NAME="arm64" ;;
    *) HOST_ARCH_NAME="$HOST_ARCH" ;;
esac

TOOLCHAIN=""
# 优先查找当前宿主匹配的目录 (例如 linux-x86_64, linux-arm64, darwin-arm64 等)
if [ -d "$PREBUILT_BASE/${HOST_OS}-${HOST_ARCH_NAME}/bin" ]; then
    TOOLCHAIN="$PREBUILT_BASE/${HOST_OS}-${HOST_ARCH_NAME}/bin"
elif [ -d "$PREBUILT_BASE/${HOST_OS}-x86_64/bin" ]; then
    TOOLCHAIN="$PREBUILT_BASE/${HOST_OS}-x86_64/bin"
else
    # 回退到 prebuilt 下首个包含 bin 的目录
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

# 3. 跨架构编译函数 (兼容带 API 版本号的脚本包装与直接传 --target 的方式)
compile_binary() {
    local target="$1"
    local wrapper_name="$2"
    local output_file="$3"

    echo "正在编译 $output_file ..."
    if [ -x "$TOOLCHAIN/$wrapper_name" ]; then
        "$TOOLCHAIN/$wrapper_name" -O3 -s pid_wrap.c -o "$output_file"
    elif [ -x "$TOOLCHAIN/clang" ]; then
        "$TOOLCHAIN/clang" --target="$target" -O3 -s pid_wrap.c -o "$output_file"
    else
        echo "错误: 找不到适用于 $target 的编译工具！" >&2
        return 1
    fi
}

compile_binary "aarch64-linux-android${API_LEVEL}" "aarch64-linux-android${API_LEVEL}-clang" "bin/pid_wrap_arm64"
compile_binary "armv7a-linux-androideabi${API_LEVEL}" "armv7a-linux-androideabi${API_LEVEL}-clang" "bin/pid_wrap_armeabi"
compile_binary "x86_64-linux-android${API_LEVEL}" "x86_64-linux-android${API_LEVEL}-clang" "bin/pid_wrap_x86_64"
compile_binary "i686-linux-android${API_LEVEL}" "i686-linux-android${API_LEVEL}-clang" "bin/pid_wrap_x86"

chmod +x bin/* *.sh 2>/dev/null || true

# 4. 打包 ZIP（优先 Python3，回退至 zip 命令）
ZIP_NAME="soft_restart_fix.zip"

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

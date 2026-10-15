#!/system/bin/sh

if [ "$KSU" != "true" ]; then
    ui_print "****************************************"
    ui_print "! 错误：此模块仅支持 KernelSU！"
    ui_print "****************************************"
    rm -rf "$MODPATH" 2>/dev/null
    abort "! 此模块仅支持 KernelSU 环境"
    exit 1
fi

SKIPUNZIP=0

ui_print "- 正在安装软重启 PID 修复模块..."

# 1. 优先通过 Android 系统属性检测主 ABI
DEVICE_ABI=$(getprop ro.product.cpu.abi 2>/dev/null)
UNAME_M=$(uname -m 2>/dev/null)

TARGET_BIN=""

if [ -n "$DEVICE_ABI" ]; then
    case "$DEVICE_ABI" in
        arm64*|arm64-v8a*)
            TARGET_BIN="pid_wrap_arm64"
            ;;
        armeabi*|armeabi-v7a*)
            TARGET_BIN="pid_wrap_armeabi"
            ;;
        x86_64*)
            TARGET_BIN="pid_wrap_x86_64"
            ;;
        x86*)
            TARGET_BIN="pid_wrap_x86"
            ;;
        *)
            # 明确的不支持 ABI（如 riscv64, mips 等）
            TARGET_BIN=""
            ;;
    esac
else
    # 2. 无法获取 getprop 时的备用检测（通过 ARCH 与 uname -m 严格匹配）
    case "$ARCH" in
        arm64|aarch64)
            TARGET_BIN="pid_wrap_arm64"
            ;;
        arm)
            case "$UNAME_M" in
                arm*|armv7*|armv8l)
                    TARGET_BIN="pid_wrap_armeabi"
                    ;;
                *)
                    TARGET_BIN=""
                    ;;
            esac
            ;;
        x64|x86_64)
            TARGET_BIN="pid_wrap_x86_64"
            ;;
        x86|i686|i386)
            TARGET_BIN="pid_wrap_x86"
            ;;
        *)
            TARGET_BIN=""
            ;;
    esac
fi

if [ -z "$TARGET_BIN" ]; then
    ui_print "****************************************"
    ui_print "! 错误：不支持的设备架构，禁止安装！"
    ui_print "! 当前设备 ABI: ${DEVICE_ABI:-未知}, 架构: ${ARCH:-$UNAME_M}"
    ui_print "! 本模块仅支持: arm64, armeabi-v7a, x86_64, x86"
    ui_print "****************************************"
    rm -rf "$MODPATH" 2>/dev/null
    abort "! 不支持的架构，安装已终止"
    exit 1
fi

if [ -f "$MODPATH/bin/$TARGET_BIN" ]; then
    mv -f "$MODPATH/bin/$TARGET_BIN" "$MODPATH/bin/pid_wrap"
else
    ui_print "****************************************"
    ui_print "! 错误：未找到目标架构执行文件 bin/$TARGET_BIN"
    ui_print "****************************************"
    rm -rf "$MODPATH" 2>/dev/null
    abort "! 文件缺失，安装已终止"
    exit 1
fi

for file in "$MODPATH/bin"/*; do
    if [ "$file" != "$MODPATH/bin/pid_wrap" ]; then
        rm -rf "$file"
    fi
done

set_perm_recursive "$MODPATH/bin" 0 0 0755 0755

# 清除可能残留的历史 override.description，使管理器恢复显示 module.prop 的初始未执行提示
rm -rf "/data/adb/ksu/module_configs/soft_restart_fix" 2>/dev/null || true

export KSU_MODULE="soft_restart_fix"
for k in /data/adb/ksud /data/adb/ksu/bin/ksud /system/bin/ksud /system/xbin/ksud; do
    if [ -x "$k" ]; then
        "$k" module config delete override.description >/dev/null 2>&1 || true
        break
    fi
done
if command -v ksud >/dev/null 2>&1; then
    ksud module config delete override.description >/dev/null 2>&1 || true
fi

ui_print "- 安装完成"

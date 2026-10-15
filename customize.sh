#!/system/bin/sh

if [ "$KSU" != "true" ]; then
    ui_print "****************************************"
    ui_print "! 错误：此模块仅支持 KernelSU！"
    ui_print "****************************************"
    abort "! 此模块仅支持 KernelSU 环境"
fi

SKIPUNZIP=0
export MODULE_HOT_INSTALL_REQUEST=true

ui_print "- 正在安装软重启 PID 修复模块..."

case "$ARCH" in
    arm64)
        TARGET_BIN="pid_wrap_arm64"
        ;;
    arm)
        TARGET_BIN="pid_wrap_armeabi"
        ;;
    x64|x86_64)
        TARGET_BIN="pid_wrap_x86_64"
        ;;
    x86)
        TARGET_BIN="pid_wrap_x86"
        ;;
    *)
        ui_print "****************************************"
        ui_print "! 错误：不支持的设备架构: ${ARCH:-未知}"
        ui_print "! 本模块仅支持: arm64, arm, x86_64, x86"
        ui_print "****************************************"
        abort "! 不支持的架构，安装已终止"
        ;;
esac

if [ ! -f "$MODPATH/bin/$TARGET_BIN" ]; then
    ui_print "****************************************"
    ui_print "! 错误：未找到目标架构执行文件 bin/$TARGET_BIN"
    ui_print "****************************************"
    abort "! 文件缺失，安装已终止"
fi

mv -f "$MODPATH/bin/$TARGET_BIN" "$MODPATH/bin/pid_wrap"

for file in "$MODPATH/bin"/*; do
    if [ "$file" != "$MODPATH/bin/pid_wrap" ]; then
        rm -rf "$file"
    fi
done

set_perm_recursive "$MODPATH/bin" 0 0 0755 0755

ksud module config clear

ui_print "- 安装完成"

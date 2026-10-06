#!/system/bin/sh

if [ "$KSU" != "true" ]; then
    ui_print "! 错误：此模块仅支持 KernelSU！"
    abort
fi

SKIPUNZIP=0

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
        ui_print "! 错误：不支持的架构: $ARCH"
        abort
        ;;
esac

if [ -f "$MODPATH/bin/$TARGET_BIN" ]; then
    mv -f "$MODPATH/bin/$TARGET_BIN" "$MODPATH/bin/pid_wrap"
else
    ui_print "! 错误：未找到目标架构文件 bin/$TARGET_BIN"
    abort
fi

for file in "$MODPATH/bin"/*; do
    if [ "$file" != "$MODPATH/bin/pid_wrap" ]; then
        rm -rf "$file"
    fi
done

set_perm_recursive "$MODPATH/bin" 0 0 0755 0755

ui_print "- 安装完成"

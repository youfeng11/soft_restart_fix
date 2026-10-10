#!/system/bin/sh

MODDIR="${MODDIR:-${0%/*}}"

if [ -x "$MODDIR/bin/pid_wrap" ]; then
    "$MODDIR/bin/pid_wrap"
    exit $?
elif [ -x "$MODDIR/pid_wrap" ]; then
    "$MODDIR/pid_wrap"
    exit $?
fi

# 原生二进制不可用时，进入纯 Shell 循环回绕
last_pid=$$
count=0

while :; do
    : &
    current_pid=$!
    count=$((count + 1))
    [ $((count % 100)) -eq 0 ] && wait 2>/dev/null
    [ "$current_pid" -lt "$last_pid" ] && break
    last_pid=$current_pid
done
wait 2>/dev/null

# 更新模块可变简介（Shell 兜底）
tag="[✅正常 (Shell)]"
prop_file="$MODDIR/module.prop"

if [ -f "$prop_file" ]; then
    orig_desc=$(grep '^description=' "$prop_file" | head -n 1 | sed 's/^description=//' | sed 's/^\[[^]]*\][[:space:]]*//' | sed 's/^【[^】]*】[[:space:]]*//')
    new_desc="${tag} ${orig_desc}"
    escaped_desc=$(printf '%s\n' "$new_desc" | sed 's/\\/\\\\/g')
    sed -i "s|^description=.*|description=${escaped_desc}|" "$prop_file" 2>/dev/null || true

    # ksud override.description 在管理器中按原样渲染，需转换为真实换行符
    real_desc=$(printf '%b' "$new_desc")
    export KSU_MODULE="soft_restart_fix"
    if [ -x "/data/adb/ksu/bin/ksud" ]; then
        /data/adb/ksu/bin/ksud module config set override.description "$real_desc" >/dev/null 2>&1 || true
    elif command -v ksud >/dev/null 2>&1; then
        ksud module config set override.description "$real_desc" >/dev/null 2>&1 || true
    fi
fi

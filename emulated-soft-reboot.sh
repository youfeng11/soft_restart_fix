#!/system/bin/sh
MODDIR="${0%/*}"

if [ -x "$MODDIR/bin/pid_wrap" ]; then
    exec "$MODDIR/bin/pid_wrap"
elif [ -x "$MODDIR/pid_wrap" ]; then
    exec "$MODDIR/pid_wrap"
fi

tag="[❌错误，模块文件缺失]"
prop_file="$MODDIR/module.prop"

if [ -f "$prop_file" ]; then
    orig_desc=$(grep '^description=' "$prop_file" | head -n 1 | sed 's/^description=//' | sed 's/^\[[^]]*\][[:space:]]*//' | sed 's/^【[^】]*】[[:space:]]*//')
    new_desc="${tag} ${orig_desc}"
    escaped_desc=$(printf '%s\n' "$new_desc" | sed 's/\\/\\\\/g')
    sed -i "s|^description=.*|description=${escaped_desc}|" "$prop_file" 2>/dev/null || true

    # ksud override.description 在管理器中按原样渲染，需转换为真实换行符
    real_desc=$(printf '%b' "$new_desc")
    ksud module config set override.description "$real_desc" >/dev/null 2>&1 || true

fi

exit 1

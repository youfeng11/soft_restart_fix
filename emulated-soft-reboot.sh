#!/system/bin/sh

MODDIR="${MODDIR:-${0%/*}}"

if [ -x "$MODDIR/bin/pid_wrap" ]; then
    "$MODDIR/bin/pid_wrap"
    exit $?
elif [ -x "$MODDIR/pid_wrap" ]; then
    "$MODDIR/pid_wrap"
    exit $?
fi

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

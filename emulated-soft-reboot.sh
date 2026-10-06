#!/system/bin/sh

MODDIR="${MODDIR:-${0%/*}}"
TARGET_PID="${1:-1000}"

if [ -x "$MODDIR/bin/pid_wrap" ]; then
    "$MODDIR/bin/pid_wrap" "$TARGET_PID"
    exit $?
elif [ -x "$MODDIR/pid_wrap" ]; then
    "$MODDIR/pid_wrap" "$TARGET_PID"
    exit $?
fi

last_pid=$$
wrapped=0
count=0

while :; do
    : &
    current_pid=$!
    count=$((count + 1))
    [ $((count % 100)) -eq 0 ] && wait 2>/dev/null
    [ "$current_pid" -lt "$last_pid" ] && wrapped=1
    [ "$wrapped" -eq 1 ] && [ "$current_pid" -ge "$TARGET_PID" ] && break
    last_pid=$current_pid
done
wait 2>/dev/null

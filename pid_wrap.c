#define _GNU_SOURCE
#include <unistd.h>
#include <sys/wait.h>
#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <sched.h>
#include <fcntl.h>
#include <string.h>
#include <time.h>
#include <errno.h>

static double get_elapsed_ms(struct timespec *start, struct timespec *end) {
    return (end->tv_sec - start->tv_sec) * 1000.0 + (end->tv_nsec - start->tv_nsec) / 1000000.0;
}

// 尝试利用 /proc/sys/kernel/pid_max 触发内核级瞬间回绕（毫秒级极速完成）
static int try_fast_wrap(long target) {
    int fd = open("/proc/sys/kernel/pid_max", O_RDWR);
    if (fd < 0) {
        fprintf(stderr, "[pid_wrap] 打开 /proc/sys/kernel/pid_max 失败: %s (errno=%d)\n", strerror(errno), errno);
        fprintf(stderr, "[pid_wrap] 提示: 常见原因为 SELinux 拦截 (avc: denied) 或无 root 权限\n");
        return 0;
    }

    char orig_buf[64];
    ssize_t orig_len = read(fd, orig_buf, sizeof(orig_buf) - 1);
    if (orig_len <= 0) {
        fprintf(stderr, "[pid_wrap] 读取 /proc/sys/kernel/pid_max 失败: %s\n", strerror(errno));
        close(fd);
        return 0;
    }
    orig_buf[orig_len] = '\0';
    long orig_pid_max = atol(orig_buf);
    if (orig_pid_max <= 300) {
        fprintf(stderr, "[pid_wrap] 读取到异常的 pid_max 原始值: %ld\n", orig_pid_max);
        close(fd);
        return 0;
    }

    pid_t cur = getpid();
    long temp_max = cur + 64;

    printf("[pid_wrap] 当前进程 PID=%d, 系统原始 pid_max=%ld\n", cur, orig_pid_max);

    // 若当前 PID 加上余量已接近甚至超过原 pid_max，说明即将自然回绕，无需缩小 pid_max
    if (temp_max >= orig_pid_max) {
        printf("[pid_wrap] 当前 PID 接近上限 (%ld)，无需修改参数，将自然回绕\n", orig_pid_max);
        close(fd);
        return 0;
    }
    if (temp_max < 302) {
        temp_max = 302;
    }

    char new_buf[64];
    int new_len = snprintf(new_buf, sizeof(new_buf), "%ld\n", temp_max);

    // 阻塞信号，确保在微小的修改窗口期间不被中断，原值必定被还原
    sigset_t all_sigs, old_sigs;
    sigfillset(&all_sigs);
    sigprocmask(SIG_BLOCK, &all_sigs, &old_sigs);

    lseek(fd, 0, SEEK_SET);
    if (write(fd, new_buf, new_len) != new_len) {
        fprintf(stderr, "[pid_wrap] 写入临时 pid_max=%ld 失败: %s (errno=%d)\n", temp_max, strerror(errno), errno);
        sigprocmask(SIG_SETMASK, &old_sigs, NULL);
        close(fd);
        return 0;
    }

    printf("[pid_wrap] 极速通道: 临时设置 pid_max=%ld 成功，正在触发内核瞬间回绕...\n", temp_max);

    // 快速触发内核回绕（由于 pid_max 临时设在当前 PID 附近，数次 vfork 即触发内核重置）
    pid_t last = cur, p = cur;
    int wrapped = 0;
    int attempts = 0;
    int max_attempts = 1000;
    while (max_attempts-- > 0) {
        attempts++;
        p = vfork();
        if (p == 0) _exit(0);
        if (p < 0) {
            sched_yield();
            continue;
        }
        if (p < last) {
            wrapped = 1;
            break;
        }
        last = p;
    }

    // 第一时间立刻恢复系统原始 pid_max 并解除信号屏蔽
    lseek(fd, 0, SEEK_SET);
    write(fd, orig_buf, orig_len);
    close(fd);
    sigprocmask(SIG_SETMASK, &old_sigs, NULL);

    if (!wrapped) {
        fprintf(stderr, "[pid_wrap] 尝试 %d 次 fork 未检测到回绕，回退到常规模式\n", attempts);
        return 0;
    }

    printf("[pid_wrap] 内核极速回绕成功！历经 %d 次 fork，PID 已重置到低位: %d (已恢复原始 pid_max=%ld)\n",
           attempts, p, orig_pid_max);

    int walk_count = 0;
    // 回绕成功后，PID 已重置到 ~300 低位区间，快速递增至指定目标
    while (p < target) {
        walk_count++;
        p = vfork();
        if (p == 0) _exit(0);
        if (p < 0) {
            sched_yield();
            continue;
        }
    }

    if (walk_count > 0) {
        printf("[pid_wrap] 从低位 PID(%d) 递增至目标 PID(%ld)，历经 %d 次 fork\n", last, target, walk_count);
    }
    printf("[pid_wrap] 极速通道完成，当前最后 PID: %d\n", p);
    return 1;
}

int main(int argc, char **argv) {
    struct timespec ts_start, ts_end;
    clock_gettime(CLOCK_MONOTONIC, &ts_start);

    long target = argc > 1 ? atol(argv[1]) : 1000;
    pid_t init_pid = getpid();

    printf("[pid_wrap] 开始执行 PID 修复, 目标 PID=%ld, 当前进程 PID=%d\n", target, init_pid);

    nice(-20);
    signal(SIGCHLD, SIG_IGN);

    // 优先尝试基于 pid_max 的极速重置（毫秒级）
    if (try_fast_wrap(target)) {
        clock_gettime(CLOCK_MONOTONIC, &ts_end);
        printf("[pid_wrap] ✅ 极速通道成功！总耗时: %.2f ms\n", get_elapsed_ms(&ts_start, &ts_end));
        return 0;
    }

    // 兜底方案：常规循环回绕
    printf("[pid_wrap] ⚠️ 极速通道不可用，降级进入常规循环回绕 (逐个遍历消耗 PID，单核运行中)...\n");
    pid_t last = init_pid, cur;
    int wrapped = 0;
    unsigned long fork_count = 0;
    while (1) {
        fork_count++;
        cur = vfork();
        if (cur == 0) _exit(0);
        if (cur < 0) {
            sched_yield();
            continue;
        }
        if (cur < last) wrapped = 1;
        if (wrapped && cur >= target) break;
        last = cur;
    }

    clock_gettime(CLOCK_MONOTONIC, &ts_end);
    printf("[pid_wrap] 降级通道完成: 共遍历 %lu 次 fork, 最终 PID=%d, 总耗时: %.2f ms (%.2f s)\n",
           fork_count, cur, get_elapsed_ms(&ts_start, &ts_end), get_elapsed_ms(&ts_start, &ts_end) / 1000.0);
    return 0;
}

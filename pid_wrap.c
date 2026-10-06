#define _GNU_SOURCE
#include <unistd.h>
#include <sys/wait.h>
#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <sched.h>
#include <fcntl.h>
#include <string.h>

// 尝试利用 /proc/sys/kernel/pid_max 触发内核级瞬间回绕（毫秒级极速完成）
static int try_fast_wrap(long target) {
    int fd = open("/proc/sys/kernel/pid_max", O_RDWR);
    if (fd < 0) {
        return 0;
    }

    char orig_buf[64];
    ssize_t orig_len = read(fd, orig_buf, sizeof(orig_buf) - 1);
    if (orig_len <= 0) {
        close(fd);
        return 0;
    }
    orig_buf[orig_len] = '\0';
    long orig_pid_max = atol(orig_buf);
    if (orig_pid_max <= 300) {
        close(fd);
        return 0;
    }

    pid_t cur = getpid();
    long temp_max = cur + 64;

    // 若当前 PID 加上余量已接近甚至超过原 pid_max，说明即将自然回绕，无需缩小 pid_max
    if (temp_max >= orig_pid_max) {
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
        sigprocmask(SIG_SETMASK, &old_sigs, NULL);
        close(fd);
        return 0;
    }

    // 快速触发内核回绕（由于 pid_max 临时设在当前 PID 附近，数次 vfork 即触发内核重置）
    pid_t last = cur, p = cur;
    int wrapped = 0;
    int max_attempts = 1000; // 安全上限，防止异常
    while (max_attempts-- > 0) {
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
        return 0;
    }

    // 回绕成功后，PID 已重置到 ~300 低位区间，快速递增至指定目标
    while (p < target) {
        p = vfork();
        if (p == 0) _exit(0);
        if (p < 0) {
            sched_yield();
            continue;
        }
    }

    printf("done, last_pid=%d\n", p);
    return 1;
}

int main(int argc, char **argv) {
    pid_t last = getpid(), cur;
    int wrapped = 0;
    long target = argc > 1 ? atol(argv[1]) : 1000;

    nice(-20);
    signal(SIGCHLD, SIG_IGN);

    // 优先尝试基于 pid_max 的极速重置（毫秒级）
    if (try_fast_wrap(target)) {
        return 0;
    }

    // 兜底方案：常规循环回绕
    while (1) {
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
    printf("done, last_pid=%d\n", cur);
    return 0;
}

#define _GNU_SOURCE
#include <unistd.h>
#include <sys/wait.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdarg.h>
#include <signal.h>
#include <sched.h>
#include <fcntl.h>
#include <string.h>
#include <time.h>
#include <errno.h>

static FILE *g_log_fp = NULL;

static void init_logger(void) {
    char log_path[512] = "/data/adb/modules/soft_restart_fix/pid_wrap.log";

    // 尝试根据当前可执行文件路径推导模块根目录
    char exe_dir[512];
    ssize_t len = readlink("/proc/self/exe", exe_dir, sizeof(exe_dir) - 1);
    if (len > 0) {
        exe_dir[len] = '\0';
        char *last_slash = strrchr(exe_dir, '/');
        if (last_slash) {
            *last_slash = '\0'; // 得到可执行文件所在目录
            int dlen = strlen(exe_dir);
            // 如果所在目录是 bin，则往上一级定位到模块根目录
            if (dlen >= 4 && strcmp(exe_dir + dlen - 4, "/bin") == 0) {
                exe_dir[dlen - 4] = '\0';
            }
            snprintf(log_path, sizeof(log_path), "%s/pid_wrap.log", exe_dir);
        }
    }

    // 若日志文件超过 256KB 则自动重置
    FILE *check_fp = fopen(log_path, "r");
    if (check_fp) {
        fseek(check_fp, 0, SEEK_END);
        long sz = ftell(check_fp);
        fclose(check_fp);
        if (sz > 256 * 1024) {
            g_log_fp = fopen(log_path, "w");
        }
    }

    if (!g_log_fp) {
        g_log_fp = fopen(log_path, "a");
    }

    // 若默认路径不可写（如本地测试非 root 环境），回退至当前工作目录
    if (!g_log_fp) {
        g_log_fp = fopen("pid_wrap.log", "a");
    }

    if (g_log_fp) {
        time_t now = time(NULL);
        struct tm tm_buf;
        localtime_r(&now, &tm_buf);
        char time_str[64];
        strftime(time_str, sizeof(time_str), "%Y-%m-%d %H:%M:%S", &tm_buf);
        fprintf(g_log_fp, "\n==================== [%s] ====================\n", time_str);
        fflush(g_log_fp);
    }
}

static void close_logger(void) {
    if (g_log_fp) {
        fflush(g_log_fp);
        fsync(fileno(g_log_fp));
        fclose(g_log_fp);
        g_log_fp = NULL;
    }
}

static void log_info(const char *fmt, ...) {
    va_list args;

    va_start(args, fmt);
    vprintf(fmt, args);
    va_end(args);

    if (g_log_fp) {
        va_start(args, fmt);
        vfprintf(g_log_fp, fmt, args);
        va_end(args);
        fflush(g_log_fp);
        fsync(fileno(g_log_fp));
    }
}

static void log_err(const char *fmt, ...) {
    va_list args;

    va_start(args, fmt);
    vfprintf(stderr, fmt, args);
    va_end(args);

    if (g_log_fp) {
        va_start(args, fmt);
        vfprintf(g_log_fp, fmt, args);
        va_end(args);
        fflush(g_log_fp);
        fsync(fileno(g_log_fp));
    }
}

static double get_elapsed_ms(struct timespec *start, struct timespec *end) {
    return (end->tv_sec - start->tv_sec) * 1000.0 + (end->tv_nsec - start->tv_nsec) / 1000000.0;
}

// 尝试利用 /proc/sys/kernel/pid_max 触发内核级瞬间回绕（毫秒级极速完成）
static int try_fast_wrap(void) {
    int fd = open("/proc/sys/kernel/pid_max", O_RDWR);
    if (fd < 0) {
        log_err("[pid_wrap] 打开 /proc/sys/kernel/pid_max 失败: %s (errno=%d)\n", strerror(errno), errno);
        log_err("[pid_wrap] 提示: 常见原因为 SELinux 拦截 (avc: denied) 或无 root 权限\n");
        return 0;
    }

    char orig_buf[64];
    ssize_t orig_len = read(fd, orig_buf, sizeof(orig_buf) - 1);
    if (orig_len <= 0) {
        log_err("[pid_wrap] 读取 /proc/sys/kernel/pid_max 失败: %s\n", strerror(errno));
        close(fd);
        return 0;
    }
    orig_buf[orig_len] = '\0';
    long orig_pid_max = atol(orig_buf);
    if (orig_pid_max <= 300) {
        log_err("[pid_wrap] 读取到异常的 pid_max 原始值: %ld\n", orig_pid_max);
        close(fd);
        return 0;
    }

    pid_t cur = getpid();
    long temp_max = cur + 64;

    log_info("[pid_wrap] 当前进程 PID=%d, 系统原始 pid_max=%ld\n", cur, orig_pid_max);

    // 若当前 PID 加上余量已接近甚至超过原 pid_max，说明即将自然回绕，无需缩小 pid_max
    if (temp_max >= orig_pid_max) {
        log_info("[pid_wrap] 当前 PID 接近上限 (%ld)，无需修改参数，将自然回绕\n", orig_pid_max);
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
        log_err("[pid_wrap] 写入临时 pid_max=%ld 失败: %s (errno=%d)\n", temp_max, strerror(errno), errno);
        sigprocmask(SIG_SETMASK, &old_sigs, NULL);
        close(fd);
        return 0;
    }

    log_info("[pid_wrap] 极速通道: 临时设置 pid_max=%ld 成功，正在触发内核瞬间回绕...\n", temp_max);

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
        log_err("[pid_wrap] 尝试 %d 次 fork 未检测到回绕，回退到常规模式\n", attempts);
        return 0;
    }

    // 回绕成功后立刻完成，不再做多余的单步循环累加！
    log_info("[pid_wrap] 内核极速回绕成功！仅历经 %d 次 fork，PID 已重置到低位: %d (已恢复原始 pid_max=%ld)\n",
             attempts, p, orig_pid_max);
    return 1;
}

int main(void) {
    init_logger();

    struct timespec ts_start, ts_end;
    clock_gettime(CLOCK_MONOTONIC, &ts_start);

    pid_t init_pid = getpid();
    log_info("[pid_wrap] 开始执行 PID 修复, 当前进程 PID=%d\n", init_pid);

    nice(-20);
    signal(SIGCHLD, SIG_IGN);

    // 优先尝试基于 pid_max 的极速重置（毫秒级）
    if (try_fast_wrap()) {
        clock_gettime(CLOCK_MONOTONIC, &ts_end);
        log_info("[pid_wrap] ✅ 极速通道成功！总耗时: %.2f ms\n", get_elapsed_ms(&ts_start, &ts_end));
        close_logger();
        return 0;
    }

    // 兜底方案：常规循环回绕（回绕即停）
    log_info("[pid_wrap] ⚠️ 极速通道不可用，降级进入常规循环回绕 (逐个遍历消耗 PID，单核运行中)...\n");
    pid_t last = init_pid, cur;
    unsigned long fork_count = 0;
    while (1) {
        fork_count++;
        cur = vfork();
        if (cur == 0) _exit(0);
        if (cur < 0) {
            sched_yield();
            continue;
        }
        if (cur < last) {
            // 一旦回绕到低位，立刻退出！
            break;
        }
        last = cur;
    }

    clock_gettime(CLOCK_MONOTONIC, &ts_end);
    log_info("[pid_wrap] 降级通道完成: 共遍历 %lu 次 fork, 最终重置到低位 PID=%d, 总耗时: %.2f ms (%.2f s)\n",
             fork_count, cur, get_elapsed_ms(&ts_start, &ts_end), get_elapsed_ms(&ts_start, &ts_end) / 1000.0);

    close_logger();
    return 0;
}

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::Instant;

struct Logger {
    file: Option<File>,
}

impl Logger {
    fn init() -> Self {
        let mut log_path = PathBuf::from("/data/adb/modules/soft_restart_fix/pid_wrap.log");

        // 尝试根据当前可执行文件路径推导模块根目录
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let module_dir = if exe_dir.file_name().and_then(|s| s.to_str()) == Some("bin") {
                    exe_dir.parent().unwrap_or(exe_dir)
                } else {
                    exe_dir
                };
                log_path = module_dir.join("pid_wrap.log");
            }
        }

        let mut file_opt = None;

        // 若日志文件超过 256KB 则自动重置截断
        if let Ok(metadata) = std::fs::metadata(&log_path) {
            if metadata.len() > 256 * 1024 {
                file_opt = File::create(&log_path).ok();
            }
        }

        if file_opt.is_none() {
            file_opt = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .ok();
        }

        // 若默认路径不可写（如本地测试非 root 环境），回退至当前工作目录
        if file_opt.is_none() {
            file_opt = OpenOptions::new()
                .create(true)
                .append(true)
                .open("pid_wrap.log")
                .ok();
        }

        let mut logger = Logger { file: file_opt };

        let time_str = get_current_time_str();
        logger.write_raw(&format!(
            "\n==================== [{}] ====================\n",
            time_str
        ));

        logger
    }

    fn write_raw(&mut self, s: &str) {
        if let Some(file) = &mut self.file {
            let _ = file.write_all(s.as_bytes());
            let _ = file.flush();
            let _ = file.sync_all();
        }
    }

    fn log_info(&mut self, msg: &str) {
        print!("{}", msg);
        self.write_raw(msg);
    }

    fn log_err(&mut self, msg: &str) {
        eprint!("{}", msg);
        self.write_raw(msg);
    }
}

fn get_current_time_str() -> String {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm_buf = std::mem::MaybeUninit::uninit();
        if !libc::localtime_r(&now, tm_buf.as_mut_ptr()).is_null() {
            let tm = tm_buf.assume_init();
            let mut buf = [0u8; 64];
            let fmt = b"%Y-%m-%d %H:%M:%S\0";
            let len = libc::strftime(
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                fmt.as_ptr() as *const libc::c_char,
                &tm,
            );
            if len > 0 {
                if let Ok(s) = std::str::from_utf8(&buf[..len]) {
                    return s.to_string();
                }
            }
        }
    }
    "unknown time".to_string()
}

// RAII 保护守卫：确保在修改 pid_max 后无论发生何种情况，都能 100% 恢复原始 pid_max 并解除信号屏蔽
struct PidMaxGuard {
    file: Option<File>,
    orig_content: Vec<u8>,
    old_sigs: libc::sigset_t,
    restored: bool,
}

impl PidMaxGuard {
    fn new(file: File, orig_content: Vec<u8>, old_sigs: libc::sigset_t) -> Self {
        Self {
            file: Some(file),
            orig_content,
            old_sigs,
            restored: false,
        }
    }

    fn restore(&mut self) {
        if !self.restored {
            if let Some(mut file) = self.file.take() {
                let _ = file.seek(SeekFrom::Start(0));
                let _ = file.write_all(&self.orig_content);
                let _ = file.flush();
                let _ = file.sync_all();
            }
            unsafe {
                libc::sigprocmask(libc::SIG_SETMASK, &self.old_sigs, std::ptr::null_mut());
            }
            self.restored = true;
        }
    }
}

impl Drop for PidMaxGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[inline(never)]
#[allow(deprecated)]
unsafe fn vfork_exit() -> libc::pid_t {
    let pid = libc::vfork();
    if pid == 0 {
        libc::_exit(0);
    }
    pid
}

// 尝试利用 /proc/sys/kernel/pid_max 触发内核级瞬间回绕（毫秒级极速完成）
fn try_fast_wrap(logger: &mut Logger) -> bool {
    let mut file = match OpenOptions::new()
        .read(true)
        .write(true)
        .open("/proc/sys/kernel/pid_max")
    {
        Ok(f) => f,
        Err(e) => {
            logger.log_err(&format!(
                "[pid_wrap] 打开 /proc/sys/kernel/pid_max 失败: {} (raw_os_error={:?})\n",
                e,
                e.raw_os_error()
            ));
            logger.log_err("[pid_wrap] 提示: 常见原因为 SELinux 拦截 (avc: denied) 或无 root 权限\n");
            return false;
        }
    };

    let mut orig_buf = String::new();
    if let Err(e) = file.read_to_string(&mut orig_buf) {
        logger.log_err(&format!(
            "[pid_wrap] 读取 /proc/sys/kernel/pid_max 失败: {}\n",
            e
        ));
        return false;
    }

    let orig_pid_max: i64 = match orig_buf.trim().parse() {
        Ok(val) => val,
        Err(_) => {
            logger.log_err(&format!(
                "[pid_wrap] 解析 pid_max 失败: 原始内容: '{}'\n",
                orig_buf.trim()
            ));
            return false;
        }
    };

    if orig_pid_max <= 300 {
        logger.log_err(&format!(
            "[pid_wrap] 读取到异常的 pid_max 原始值: {}\n",
            orig_pid_max
        ));
        return false;
    }

    let cur = unsafe { libc::getpid() } as i64;
    let mut temp_max = cur + 64;

    logger.log_info(&format!(
        "[pid_wrap] 当前进程 PID={}, 系统原始 pid_max={}\n",
        cur, orig_pid_max
    ));

    // 若当前 PID 加上余量已接近甚至超过原 pid_max，说明即将自然回绕，无需缩小 pid_max
    if temp_max >= orig_pid_max {
        logger.log_info(&format!(
            "[pid_wrap] 当前 PID 接近上限 ({})，无需修改参数，将自然回绕\n",
            orig_pid_max
        ));
        return false;
    }

    if temp_max < 302 {
        temp_max = 302;
    }

    let new_content = format!("{}\n", temp_max);

    // 阻塞所有信号，确保在微小的修改窗口期间不被中断，原值必定被还原
    let mut all_sigs = std::mem::MaybeUninit::uninit();
    let mut old_sigs = std::mem::MaybeUninit::uninit();
    unsafe {
        libc::sigfillset(all_sigs.as_mut_ptr());
        libc::sigprocmask(libc::SIG_BLOCK, all_sigs.as_ptr(), old_sigs.as_mut_ptr());
    }
    let old_sigs = unsafe { old_sigs.assume_init() };

    // 创建 RAII 守卫：无论后续发生什么，离开作用域前必定还原原始 pid_max 和信号屏蔽
    let mut guard = PidMaxGuard::new(file, orig_buf.into_bytes(), old_sigs);

    if let Some(f) = &mut guard.file {
        if f.seek(SeekFrom::Start(0)).is_err()
            || f.write_all(new_content.as_bytes()).is_err()
            || f.flush().is_err()
        {
            logger.log_err(&format!(
                "[pid_wrap] 写入临时 pid_max={} 失败\n",
                temp_max
            ));
            guard.restore();
            return false;
        }
    }

    logger.log_info(&format!(
        "[pid_wrap] 极速通道: 临时设置 pid_max={} 成功，正在触发内核瞬间回绕...\n",
        temp_max
    ));

    // 快速触发内核回绕（由于 pid_max 临时设在当前 PID 附近，数次 vfork 即触发内核重置）
    let mut last = cur as libc::pid_t;
    let mut p = cur as libc::pid_t;
    let mut wrapped = false;
    let mut attempts = 0;
    let mut max_attempts = 1000;

    while max_attempts > 0 {
        max_attempts -= 1;
        attempts += 1;
        p = unsafe { vfork_exit() };
        if p < 0 {
            unsafe {
                libc::sched_yield();
            }
            continue;
        }
        if p < last {
            wrapped = true;
            break;
        }
        last = p;
    }

    // 第一时间立刻显式恢复系统原始 pid_max 并解除信号屏蔽
    guard.restore();

    if !wrapped {
        logger.log_err(&format!(
            "[pid_wrap] 尝试 {} 次 fork 未检测到回绕，回退到常规模式\n",
            attempts
        ));
        return false;
    }

    logger.log_info(&format!(
        "[pid_wrap] 内核极速回绕成功！仅历经 {} 次 fork，PID 已重置到低位: {} (已恢复原始 pid_max={})\n",
        attempts, p, orig_pid_max
    ));
    true
}

// 兜底方案：常规循环回绕（回绕即停）
fn fallback_wrap(init_pid: libc::pid_t, logger: &mut Logger, ts_start: Instant) {
    logger.log_info("[pid_wrap] ⚠️ 极速通道不可用，降级进入常规循环回绕 (逐个遍历消耗 PID，单核运行中)...\n");

    let mut last = init_pid;
    let mut fork_count: u64 = 0;
    let final_pid: libc::pid_t;

    loop {
        fork_count += 1;
        let p = unsafe { vfork_exit() };
        if p < 0 {
            unsafe {
                libc::sched_yield();
            }
            continue;
        }
        if p < last {
            // 一旦回绕到低位，立刻退出！
            final_pid = p;
            break;
        }
        last = p;
    }

    let elapsed = ts_start.elapsed();
    let ms = elapsed.as_secs_f64() * 1000.0;
    let s = ms / 1000.0;

    logger.log_info(&format!(
        "[pid_wrap] 降级通道完成: 共遍历 {} 次 fork, 最终重置到低位 PID={}, 总耗时: {:.2} ms ({:.2} s)\n",
        fork_count, final_pid, ms, s
    ));
}

fn main() {
    let mut logger = Logger::init();
    let ts_start = Instant::now();

    let init_pid = unsafe { libc::getpid() };
    logger.log_info(&format!(
        "[pid_wrap] 开始执行 PID 修复, 当前进程 PID={}\n",
        init_pid
    ));

    unsafe {
        libc::nice(-20);
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }

    // 优先尝试基于 pid_max 的极速重置（毫秒级）
    if try_fast_wrap(&mut logger) {
        let elapsed = ts_start.elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        logger.log_info(&format!(
            "[pid_wrap] ✅ 极速通道成功！总耗时: {:.2} ms\n",
            ms
        ));
        return;
    }

    // 兜底方案：常规循环回绕
    fallback_wrap(init_pid, &mut logger, ts_start);
}

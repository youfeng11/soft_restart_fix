use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::time::Instant;

/// 安全低位阈值：若当前 PID 已低于此值，说明刚开机或刚完成回绕，无需重复操作
const SAFE_LOW_PID_THRESHOLD: libc::pid_t = 1280;
/// 临时 pid_max 的最低安全下限（避开 300~1500 之间密集的 init/kworker 占用区，防止 EAGAIN）
const MIN_SAFE_TEMP_PID_MAX: i64 = 2048;

struct Logger {
    file: Option<File>,
}

fn get_module_dir() -> PathBuf {
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let module_dir = if exe_dir.file_name().and_then(|s| s.to_str()) == Some("bin") {
                exe_dir.parent().unwrap_or(exe_dir)
            } else {
                exe_dir
            };
            if module_dir.join("module.prop").exists() {
                return module_dir.to_path_buf();
            }
        }
    }

    let default_path = PathBuf::from("/data/adb/modules/soft_restart_fix");
    if default_path.join("module.prop").exists() {
        return default_path;
    }

    if std::path::Path::new("module.prop").exists() {
        return PathBuf::from(".");
    }

    default_path
}

impl Logger {
    fn init() -> Self {
        let module_dir = get_module_dir();
        let log_path = module_dir.join("pid_wrap.log");

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
        }
    }

    fn log_info(&mut self, msg: &str) {
        let _ = std::io::stdout().write_all(msg.as_bytes());
        self.write_raw(msg);
    }

    fn log_err(&mut self, msg: &str) {
        let _ = std::io::stderr().write_all(msg.as_bytes());
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

/// 使用 pwrite 系统调用从偏移量 0 原子写入 procfs 节点，避免 seek + write 分离引发的状态问题
#[cfg(unix)]
fn pwrite_all_at_zero(file: &File, buf: &[u8]) -> std::io::Result<()> {
    let fd = file.as_raw_fd();
    let ret = unsafe { libc::pwrite(fd, buf.as_ptr() as *const libc::c_void, buf.len(), 0) };
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else if (ret as usize) != buf.len() {
        Err(std::io::ErrorKind::WriteZero.into())
    } else {
        Ok(())
    }
}

// 正常退出时尝试恢复 pid_max；显式恢复失败后保留句柄，供 Drop 再尝试一次。
// SIGKILL、panic=abort 等异常终止不会运行 Drop。
struct PidMaxGuard {
    file: File,
    orig_content: Vec<u8>,
    old_sigs: libc::sigset_t,
    restored: bool,
}

impl PidMaxGuard {
    fn new(file: File, orig_content: Vec<u8>, old_sigs: libc::sigset_t) -> Self {
        Self {
            file,
            orig_content,
            old_sigs,
            restored: false,
        }
    }

    fn restore(&mut self) -> std::io::Result<()> {
        if !self.restored {
            pwrite_all_at_zero(&self.file, &self.orig_content)?;
            self.restored = true;
        }
        Ok(())
    }
}

impl Drop for PidMaxGuard {
    fn drop(&mut self) {
        // 析构期间不输出日志；在恢复尝试结束后再解除信号屏蔽。
        let _ = self.restore();
        unsafe {
            libc::sigprocmask(libc::SIG_SETMASK, &self.old_sigs, std::ptr::null_mut());
        }
    }
}

#[derive(Debug)]
enum FastWrapError {
    Setup(std::io::Error),
    Restore(std::io::Error),
}

#[derive(Debug)]
struct FastWrapStats {
    wrapped: bool,
    attempts: u32,
    pid: libc::pid_t,
    consecutive_failures: u32,
}

// 参数修改窗口内只执行系统调用和计数，不做日志、格式化或堆分配。
// next_pid 允许测试用临时文件验证失败路径，无需修改真实的系统参数。
fn wrap_with_pid_max(
    file: File,
    orig_content: Vec<u8>,
    temp_content: Option<&[u8]>,
    cur_pid: libc::pid_t,
    mut next_pid: impl FnMut() -> libc::pid_t,
) -> Result<FastWrapStats, FastWrapError> {
    let mut all_sigs = std::mem::MaybeUninit::uninit();
    let mut old_sigs = std::mem::MaybeUninit::uninit();
    unsafe {
        if libc::sigfillset(all_sigs.as_mut_ptr()) != 0
            || libc::sigprocmask(libc::SIG_BLOCK, all_sigs.as_ptr(), old_sigs.as_mut_ptr()) != 0
        {
            return Err(FastWrapError::Setup(std::io::Error::last_os_error()));
        }
    }
    let mut guard = PidMaxGuard::new(file, orig_content, unsafe { old_sigs.assume_init() });
    // 未收缩参数时不需要写回原值。
    guard.restored = temp_content.is_none();

    if let Some(content) = temp_content {
        if let Err(error) = pwrite_all_at_zero(&guard.file, content) {
            guard.restore().map_err(FastWrapError::Restore)?;
            return Err(FastWrapError::Setup(error));
        }
    }

    let mut stats = FastWrapStats {
        wrapped: false,
        attempts: 0,
        pid: cur_pid,
        consecutive_failures: 0,
    };
    let mut last = cur_pid;
    while stats.attempts < 1000 {
        stats.attempts += 1;
        let pid = next_pid();
        if pid < 0 {
            stats.consecutive_failures += 1;
            if stats.consecutive_failures >= 10 {
                break;
            }
            std::thread::yield_now();
            continue;
        }
        stats.pid = pid;
        stats.consecutive_failures = 0;
        if pid < last {
            stats.wrapped = true;
            break;
        }
        last = pid;
    }

    guard.restore().map_err(FastWrapError::Restore)?;
    Ok(stats)
}

/// 零栈触碰的极速 vfork + exit 实现：
/// 避免在 Rust 函数中直接调用 libc::vfork() 导致父子进程共享栈帧引发未定义行为 (UB)。
#[inline(never)]
unsafe fn fast_vfork_exit() -> libc::pid_t {
    #[cfg(target_arch = "aarch64")]
    {
        let ret: isize;
        // CLONE_VM (0x0100) | CLONE_VFORK (0x4000) | SIGCHLD (17) = 0x4111
        core::arch::asm!(
            "mov x0, #0x4111",
            "mov x1, xzr",
            "mov x2, xzr",
            "mov x3, xzr",
            "mov x4, xzr",
            "mov x8, #220", // __NR_clone on aarch64
            "svc #0",
            "cbnz x0, 2f",  // 若 x0 != 0 (父进程或出错)，跳转至标签 2 返回
            "mov x0, xzr",  // 子进程：全程不触碰任何栈内存，直接调 SYS_exit(0)
            "mov x8, #93",  // __NR_exit on aarch64
            "svc #0",
            "2:",
            out("x0") ret,
            out("x1") _,
            out("x2") _,
            out("x3") _,
            out("x4") _,
            out("x8") _,
            options(nostack)
        );
        if ret < 0 {
            -1
        } else {
            ret as libc::pid_t
        }
    }

    #[cfg(target_arch = "x86_64")]
    {
        let ret: isize;
        // CLONE_VM (0x0100) | CLONE_VFORK (0x4000) | SIGCHLD (17) = 0x4111
        core::arch::asm!(
            "mov edi, 0x4111",
            "xor esi, esi",
            "xor edx, edx",
            "xor r10d, r10d",
            "xor r8d, r8d",
            "mov eax, 56",  // __NR_clone on x86_64
            "syscall",
            "test rax, rax",
            "jnz 2f",       // 父进程或出错则跳转
            "xor edi, edi", // 子进程：直接调 SYS_exit(0)
            "mov eax, 60",  // __NR_exit on x86_64
            "syscall",
            "2:",
            out("rax") ret,
            out("rdi") _,
            out("rsi") _,
            out("rdx") _,
            out("r10") _,
            out("r8") _,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
        if ret < 0 {
            -1
        } else {
            ret as libc::pid_t
        }
    }

    #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
    {
        // 其他架构兜底：直接通过 syscall 调用轻量级 clone(SIGCHLD)，绕过 libc pthread_atfork 开销且不共享栈
        let pid = libc::syscall(libc::SYS_clone, libc::SIGCHLD as libc::c_long, 0, 0, 0, 0)
            as libc::pid_t;
        if pid == 0 {
            libc::_exit(0);
        }
        pid
    }
}

/// 将当前进程绑定至编号最大的 CPU 核心（在 Android big.LITTLE 架构下通常为 Prime 超大核）
fn bind_to_prime_core(logger: &mut Logger) {
    #[cfg(unix)]
    unsafe {
        let num_cpus = libc::sysconf(libc::_SC_NPROCESSORS_ONLN);
        if num_cpus > 1 {
            let prime_cpu = (num_cpus - 1) as usize;
            let mut set: libc::cpu_set_t = std::mem::zeroed();
            libc::CPU_SET(prime_cpu, &mut set);
            if libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) == 0 {
                logger.log_info(&format!(
                    "[pid_wrap] 已将进程绑定至大核 CPU{} 以加速遍历\n",
                    prime_cpu
                ));
            }
        }
    }
}

fn unescape_description_for_override(desc: &str) -> String {
    desc.replace("\\n", "\n")
}

fn format_updated_description(content: &str, tag: &str) -> (String, String) {
    let mut new_lines = Vec::new();
    let mut updated_desc = String::new();

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("description=") {
            let mut val = rest.trim();

            // 去除已有的 [...] 或 【...】 状态前缀，保证幂等性
            if val.starts_with('[') {
                if let Some(idx) = val.find(']') {
                    val = val[idx + 1..].trim();
                }
            } else if val.starts_with('【') {
                if let Some(idx) = val.find('】') {
                    val = val[idx + '】'.len_utf8()..].trim();
                }
            }

            // 去除前缀后的 LF 换行符（仅支持 \n 与 \\n）
            while val.starts_with("\\n") || val.starts_with('\n') {
                if val.starts_with("\\n") {
                    val = val[2..].trim();
                } else if val.starts_with('\n') {
                    val = val[1..].trim();
                }
            }

            let full_desc = format!("{} {}", tag, val);
            new_lines.push(format!("description={}", full_desc));
            updated_desc = full_desc;
        } else {
            new_lines.push(line.to_string());
        }
    }

    let new_content = new_lines.join("\n") + "\n";
    (new_content, updated_desc)
}

fn update_ksud_override_description(full_desc: &str, logger: &mut Logger) {
    let unescaped_desc = unescape_description_for_override(full_desc);

    let res = std::process::Command::new("ksud")
        .env("KSU_MODULE", "soft_restart_fix")
        .args([
            "module",
            "config",
            "set",
            "override.description",
            &unescaped_desc,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    match res {
        Ok(status) if status.success() => {
            logger.log_info("[pid_wrap] 成功通过 ksud 更新 override.description\n");
        }
        Ok(status) => {
            logger.log_err(&format!(
                "[pid_wrap] ksud 更新 override.description 异常退出: {:?}\n",
                status.code()
            ));
        }
        Err(e) => {
            logger.log_err(&format!("[pid_wrap] 调用 ksud 失败: {}\n", e));
        }
    }
}

fn update_module_description(tag: &str, logger: &mut Logger) {
    let module_dir = get_module_dir();
    let prop_path = module_dir.join("module.prop");

    let (content, target_path) = if let Ok(c) = std::fs::read_to_string(&prop_path) {
        (c, prop_path)
    } else if let Ok(c) = std::fs::read_to_string("module.prop") {
        (c, PathBuf::from("module.prop"))
    } else {
        logger.log_info("[pid_wrap] 未找到 module.prop，跳过简介更新\n");
        return;
    };

    let (new_content, updated_desc) = format_updated_description(&content, tag);
    let temp_target = target_path.with_extension("tmp");

    let mut write_success = false;
    if let Ok(mut f) = File::create(&temp_target) {
        if f.write_all(new_content.as_bytes()).is_ok() && f.flush().is_ok() && f.sync_all().is_ok()
        {
            write_success = true;
        }
    }

    if write_success {
        if std::fs::rename(&temp_target, &target_path).is_ok() {
            logger.log_info(&format!("[pid_wrap] 成功更新模块可变简介: {}\n", tag));
        } else {
            let _ = std::fs::remove_file(&temp_target);
            logger.log_err("[pid_wrap] 重命名 module.tmp 失败\n");
        }
    } else {
        let _ = std::fs::remove_file(&temp_target);
        logger.log_err("[pid_wrap] 写入 module.tmp 失败\n");
    }

    if !updated_desc.is_empty() {
        update_ksud_override_description(&updated_desc, logger);
    }
}

// 尝试利用 /proc/sys/kernel/pid_max 触发内核级瞬间回绕（毫秒级极速完成）
fn try_fast_wrap(cur_pid: libc::pid_t, logger: &mut Logger) -> std::io::Result<bool> {
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
            logger
                .log_err("[pid_wrap] 提示: 常见原因为 SELinux 拦截 (avc: denied) 或无 root 权限\n");
            return Ok(false);
        }
    };

    let mut orig_buf = String::new();
    if let Err(e) = file.read_to_string(&mut orig_buf) {
        logger.log_err(&format!(
            "[pid_wrap] 读取 /proc/sys/kernel/pid_max 失败: {}\n",
            e
        ));
        return Ok(false);
    }

    let orig_pid_max: i64 = match orig_buf.trim().parse() {
        Ok(val) => val,
        Err(_) => {
            logger.log_err(&format!(
                "[pid_wrap] 解析 pid_max 失败: 原始内容: '{}'\n",
                orig_buf.trim()
            ));
            return Ok(false);
        }
    };

    if orig_pid_max <= 300 {
        logger.log_err(&format!(
            "[pid_wrap] 读取到异常的 pid_max 原始值: {}\n",
            orig_pid_max
        ));
        return Ok(false);
    }

    let cur = cur_pid as i64;
    let temp_max = (cur + 64).max(MIN_SAFE_TEMP_PID_MAX);

    logger.log_info(&format!(
        "[pid_wrap] 当前进程 PID={}, 系统原始 pid_max={}\n",
        cur, orig_pid_max
    ));

    // 在参数修改前准备好写入内容，所有结果日志均在守卫退出后输出。
    let temp_content = (temp_max < orig_pid_max).then(|| format!("{}\n", temp_max));
    if temp_content.is_none() {
        logger.log_info(&format!(
            "[pid_wrap] 当前 PID 已接近系统上限 ({})，无需收缩参数，直接执行极速自然回绕...\n",
            orig_pid_max
        ));
    }
    let stats = match wrap_with_pid_max(
        file,
        orig_buf.into_bytes(),
        temp_content.as_ref().map(|content| content.as_bytes()),
        cur_pid,
        || unsafe { fast_vfork_exit() },
    ) {
        Ok(stats) => stats,
        Err(FastWrapError::Setup(error)) => {
            logger.log_err(&format!("[pid_wrap] 极速通道准备失败: {}\n", error));
            return Ok(false);
        }
        Err(FastWrapError::Restore(error)) => return Err(error),
    };

    if stats.consecutive_failures >= 10 {
        logger.log_err("[pid_wrap] 极速通道连续 clone 失败，已尝试恢复原始 pid_max\n");
    }
    if !stats.wrapped {
        logger.log_err(&format!(
            "[pid_wrap] 尝试 {} 次 clone 未检测到回绕，回退到常规模式\n",
            stats.attempts
        ));
        return Ok(false);
    }

    logger.log_info(&format!(
        "[pid_wrap] 内核极速回绕成功！仅历经 {} 次 clone，PID 已重置到低位: {} (已确保 pid_max={})\n",
        stats.attempts, stats.pid, orig_pid_max
    ));
    Ok(true)
}

// 兜底方案：常规循环回绕（回绕即停）
fn fallback_wrap(init_pid: libc::pid_t, logger: &mut Logger, ts_start: Instant) -> bool {
    logger.log_info("[pid_wrap] ⚠️ 极速通道不可用，降级进入常规循环回绕 (逐个遍历消耗 PID)...\n");

    // 绑定至超大核，减少跨核缓存失效
    bind_to_prime_core(logger);

    let mut last = init_pid;
    let mut fork_count: u64 = 0;
    let mut consecutive_failures: u32 = 0;
    let final_pid: libc::pid_t;

    loop {
        fork_count += 1;
        let p = unsafe { fast_vfork_exit() };
        if p < 0 {
            consecutive_failures += 1;
            if consecutive_failures >= 100 {
                logger
                    .log_err("[pid_wrap] 连续 clone 失败达到 100 次，系统资源受限，中止降级回绕\n");
                return false;
            }
            std::thread::yield_now();
            continue;
        }
        consecutive_failures = 0;
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
        "[pid_wrap] 降级通道完成: 共遍历 {} 次 clone, 最终重置到低位 PID={}, 总耗时: {:.2} ms ({:.2} s)\n",
        fork_count, final_pid, ms, s
    ));
    true
}

fn main() {
    let mut logger = Logger::init();
    let ts_start = Instant::now();

    let init_pid = std::process::id() as libc::pid_t;
    logger.log_info(&format!(
        "[pid_wrap] 开始执行 PID 修复, 当前进程 PID={}\n",
        init_pid
    ));

    // 若当前 PID 本身已经处于安全低位，无需折腾内核，直接标记成功
    if init_pid < SAFE_LOW_PID_THRESHOLD {
        logger.log_info(&format!(
            "[pid_wrap] ✅ 当前 PID ({}) 已处于安全低位 (< {})，无需回绕\n",
            init_pid, SAFE_LOW_PID_THRESHOLD
        ));
        update_module_description("[✅已生效]", &mut logger);
        return;
    }

    unsafe {
        libc::nice(-20);
        // PID 遍历期间忽略 SIGCHLD，由内核直接自动回收子进程，防止产生僵尸进程
        libc::signal(libc::SIGCHLD, libc::SIG_IGN);
    }

    // 优先尝试基于 pid_max 的极速重置（毫秒级）
    let fast_ok = match try_fast_wrap(init_pid, &mut logger) {
        Ok(wrapped) => wrapped,
        Err(error) => {
            unsafe {
                libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            }
            logger.log_err(&format!(
                "[pid_wrap] ❌ 恢复 pid_max 失败: {}，中止 PID 回绕\n",
                error
            ));
            update_module_description("[❌错误]", &mut logger);
            std::process::exit(1);
        }
    };
    if fast_ok {
        // 回绕完成，恢复 SIGCHLD 默认行为，确保后续 Command::status() 可正常 waitpid
        unsafe {
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
        }

        let elapsed = ts_start.elapsed();
        let ms = elapsed.as_secs_f64() * 1000.0;
        logger.log_info(&format!(
            "[pid_wrap] ✅ 极速通道成功！总耗时: {:.2} ms\n",
            ms
        ));
        update_module_description("[✅已生效]", &mut logger);
        return;
    }

    // 兜底方案：常规循环回绕
    let fallback_ok = fallback_wrap(init_pid, &mut logger, ts_start);

    // 回绕完成，恢复 SIGCHLD 默认行为
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
    }

    if fallback_ok {
        update_module_description("[✅已生效 (备用方案)]", &mut logger);
    } else {
        update_module_description("[❌错误]", &mut logger);
        logger.log_err("[pid_wrap] ❌ PID 回绕失败，已更新模块简介状态为错误\n");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    const ORIGINAL: &[u8] = b"65536\n";
    const TEMPORARY: &[u8] = b"16384\n";

    struct TestFile {
        path: PathBuf,
        file: File,
    }

    impl TestFile {
        fn new() -> Self {
            static NEXT_ID: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "pid-wrap-test-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            file.write_all(ORIGINAL).unwrap();
            Self { path, file }
        }

        fn read(&self) -> Vec<u8> {
            std::fs::read(&self.path).unwrap()
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn current_signal_mask() -> libc::sigset_t {
        let mut mask = std::mem::MaybeUninit::uninit();
        assert_eq!(
            unsafe { libc::sigprocmask(libc::SIG_BLOCK, std::ptr::null(), mask.as_mut_ptr()) },
            0
        );
        unsafe { mask.assume_init() }
    }

    #[test]
    fn failed_restore_retains_file_and_allows_retry() {
        let fixture = TestFile::new();
        pwrite_all_at_zero(&fixture.file, TEMPORARY).unwrap();
        let mut guard = PidMaxGuard::new(
            File::open(&fixture.path).unwrap(),
            ORIGINAL.to_vec(),
            current_signal_mask(),
        );
        let fd = guard.file.as_raw_fd();
        assert!(guard.restore().is_err());
        assert!(!guard.restored);
        assert_eq!(guard.file.as_raw_fd(), fd);
        assert_eq!(fixture.read(), TEMPORARY);

        guard.file = fixture.file.try_clone().unwrap();
        guard.restore().unwrap();
        assert!(guard.restored);
        assert_eq!(fixture.read(), ORIGINAL);
    }

    #[test]
    fn drop_restores_without_an_explicit_restore() {
        let fixture = TestFile::new();
        {
            let guard = PidMaxGuard::new(
                fixture.file.try_clone().unwrap(),
                ORIGINAL.to_vec(),
                current_signal_mask(),
            );
            pwrite_all_at_zero(&guard.file, TEMPORARY).unwrap();
        }
        assert_eq!(fixture.read(), ORIGINAL);
    }

    #[test]
    fn successful_wrap_restores_original_value() {
        let fixture = TestFile::new();
        let mut pids = [20001, 300].into_iter();
        let stats = wrap_with_pid_max(
            fixture.file.try_clone().unwrap(),
            ORIGINAL.to_vec(),
            Some(TEMPORARY),
            20000,
            || pids.next().unwrap(),
        )
        .unwrap();
        assert!(stats.wrapped);
        assert_eq!(stats.attempts, 2);
        assert_eq!(stats.pid, 300);
        assert_eq!(fixture.read(), ORIGINAL);
    }

    #[test]
    fn allocation_failures_restore_before_returning() {
        let fixture = TestFile::new();
        let stats = wrap_with_pid_max(
            fixture.file.try_clone().unwrap(),
            ORIGINAL.to_vec(),
            Some(TEMPORARY),
            20000,
            || -1,
        )
        .unwrap();
        assert!(!stats.wrapped);
        assert_eq!(stats.attempts, 10);
        assert_eq!(fixture.read(), ORIGINAL);
    }

    #[test]
    fn restore_failure_after_wrap_is_a_fatal_error() {
        let fixture = TestFile::new();
        let file = fixture.file.try_clone().unwrap();
        let fd = file.as_raw_fd();
        let readonly = File::open(&fixture.path).unwrap();
        let before = current_signal_mask();
        let result = wrap_with_pid_max(file, ORIGINAL.to_vec(), Some(TEMPORARY), 20000, || {
            // 写入临时值后，将守卫的句柄替换为只读句柄，注入恢复失败。
            assert_eq!(unsafe { libc::dup2(readonly.as_raw_fd(), fd) }, fd);
            300
        });
        assert!(matches!(result, Err(FastWrapError::Restore(_))));
        assert_eq!(fixture.read(), TEMPORARY);
        let after = current_signal_mask();
        for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGCHLD] {
            assert_eq!(unsafe { libc::sigismember(&before, signal) }, unsafe {
                libc::sigismember(&after, signal)
            });
        }
    }

    #[test]
    fn unchanged_pid_max_needs_no_write_permission() {
        let fixture = TestFile::new();
        let stats = wrap_with_pid_max(
            File::open(&fixture.path).unwrap(),
            ORIGINAL.to_vec(),
            None,
            20000,
            || 300,
        )
        .unwrap();
        assert!(stats.wrapped);
        assert_eq!(fixture.read(), ORIGINAL);
    }
}

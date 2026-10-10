use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

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
            return module_dir.to_path_buf();
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

fn unescape_description_for_override(desc: &str) -> String {
    desc.replace("\\r\\n", "\n").replace("\\n", "\n")
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

            // 去除前缀后的 \\n 或物理换行符
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

fn find_ksud_bin() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/data/adb/ksud",
        "/data/adb/ksu/bin/ksud",
        "/system/bin/ksud",
        "/system/xbin/ksud",
    ];

    for &path in CANDIDATES {
        let p = Path::new(path);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }

    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(':') {
            if dir.is_empty() {
                continue;
            }
            let candidate = Path::new(dir).join("ksud");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }

    None
}

fn update_ksud_override_description(full_desc: &str, logger: &mut Logger) {
    // KernelSU 的 override.description 是原始字符串，不支持 Java properties 的 \n 转义，
    // 需将其替换为真实的换行符（0x0A），以确保在 KernelSU Manager 中正常换行渲染
    let unescaped_desc = unescape_description_for_override(full_desc);

    if let Some(ksud_path) = find_ksud_bin() {
        let res = std::process::Command::new(&ksud_path)
            .env("KSU_MODULE", "soft_restart_fix")
            .args(["module", "config", "set", "override.description", &unescaped_desc])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();

        match res {
            Ok(status) if status.success() => {
                logger.log_info(&format!(
                    "[pid_wrap] 成功通过 {} 更新 override.description\n",
                    ksud_path.display()
                ));
            }
            Ok(status) => {
                logger.log_err(&format!(
                    "[pid_wrap] {} 更新 override.description 异常退出: {:?}\n",
                    ksud_path.display(),
                    status.code()
                ));
            }
            Err(e) => {
                logger.log_err(&format!(
                    "[pid_wrap] 调用 {} 失败: {}\n",
                    ksud_path.display(),
                    e
                ));
            }
        }
    } else {
        logger.log_info("[pid_wrap] 未找到 ksud 命令，仅更新 module.prop\n");
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

    if let Ok(mut f) = File::create(&temp_target) {
        if f.write_all(new_content.as_bytes()).is_ok() && f.sync_all().is_ok() {
            drop(f);
            if std::fs::rename(&temp_target, &target_path).is_ok() {
                logger.log_info(&format!("[pid_wrap] 成功更新模块可变简介: {}\n", tag));
            } else {
                let _ = std::fs::remove_file(&temp_target);
            }
        }
    }

    if !updated_desc.is_empty() {
        update_ksud_override_description(&updated_desc, logger);
    }
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
        update_module_description("[✅正常]", &mut logger);
        return;
    }

    // 兜底方案：常规循环回绕
    fallback_wrap(init_pid, &mut logger, ts_start);
    update_module_description("[✅正常 (vfork)]", &mut logger);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unescape_description_for_override() {
        let raw = "[✅正常] 第一行\\n第二行\\r\\n第三行";
        let unescaped = unescape_description_for_override(raw);
        assert_eq!(unescaped, "[✅正常] 第一行\n第二行\n第三行");
        assert!(!unescaped.contains("\\n"));
        assert!(!unescaped.contains("\\r\\n"));
    }

    #[test]
    fn test_format_updated_description_initial() {
        let prop = "id=soft_restart_fix\nversion=v1.2.0\ndescription=[⚠️未执行，请执行软重启] 在软重启前自动重置内核PID计数器。\\n核心采用 Rust 实现。\\n提高效率！\nupdateJson=https://example.com/update.json\n";
        let (new_prop, updated_desc) = format_updated_description(prop, "[✅正常]");
        assert!(new_prop.contains("description=[✅正常] 在软重启前自动重置内核PID计数器。\\n核心采用 Rust 实现。\\n提高效率！\n"));
        assert!(!new_prop.contains("[⚠️未执行"));
        assert_eq!(
            updated_desc,
            "[✅正常] 在软重启前自动重置内核PID计数器。\\n核心采用 Rust 实现。\\n提高效率！"
        );

        let override_text = unescape_description_for_override(&updated_desc);
        assert_eq!(
            override_text,
            "[✅正常] 在软重启前自动重置内核PID计数器。\n核心采用 Rust 实现。\n提高效率！"
        );
    }

    #[test]
    fn test_format_updated_description_idempotent() {
        let prop = "id=soft_restart_fix\ndescription=[✅正常] 在软重启前自动重置。\\n第二行\n";
        let (new_prop, updated_desc) = format_updated_description(prop, "[✅正常 (vfork)]");
        assert!(new_prop.contains("description=[✅正常 (vfork)] 在软重启前自动重置。\\n第二行\n"));
        assert!(!new_prop.contains("[✅正常] 在软重启前"));
        assert_eq!(
            updated_desc,
            "[✅正常 (vfork)] 在软重启前自动重置。\\n第二行"
        );

        // 再用 Shell 模式更新
        let (new_prop_shell, updated_desc_shell) =
            format_updated_description(&new_prop, "[✅正常 (Shell)]");
        assert!(new_prop_shell.contains("description=[✅正常 (Shell)] 在软重启前自动重置。\\n第二行\n"));
        assert!(!new_prop_shell.contains("[✅正常 (vfork)]"));
        assert_eq!(
            updated_desc_shell,
            "[✅正常 (Shell)] 在软重启前自动重置。\\n第二行"
        );
    }

    #[test]
    fn test_candidate_ksud_paths() {
        const CANDIDATES: &[&str] = &[
            "/data/adb/ksud",
            "/data/adb/ksu/bin/ksud",
            "/system/bin/ksud",
            "/system/xbin/ksud",
        ];
        assert_eq!(CANDIDATES[0], "/data/adb/ksud");
    }
}

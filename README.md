# 软重启 PID 修复 (Soft Reboot PID Fix)

[![KernelSU](https://img.shields.io/badge/KernelSU-Supported-brightgreen.svg)](https://kernelsu.org/)
[![Build and Release](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml/badge.svg)](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

一个专为 Android（KernelSU）设计的超轻量级高效模块。在系统执行软重启（Soft Reboot）前，自动且极速地重置 Linux 内核 PID 计数器，使其回绕并重置到安全低位范围，从而有效避开部分安全软件或反作弊机制对软重启后异常高 PID 的检测。

---

## 📖 背景与原理

### 为什么需要修复？
- **软重启特性**：Android 系统的“软重启”（Soft Reboot，命令 `setprop ctl.restart zygote` ）并不会重启底层 Linux 内核。
- **PID 持续累加**：由于内核未重启，PID 计数器持续递增。软重启后重新启动的 `system_server`、`zygote` 以及各类关键应用，其分配到的 PID 往往高达数万。
- **安全与风控检测**：部分金融银行应用、企业安全控件或反作弊系统会检测“开机时间与 PID 的比例关系”或“关键进程 PID 异常偏高”，将其判定为环境异常、二次注入或多开容器。

### 本模块如何解决？
模块在软重启触发时自动执行原生核心组件 `pid_wrap`：
1. **极速通道 (Fast Wrap)**：临时将 `/proc/sys/kernel/pid_max` 动态收缩至当前 PID 附近，促使 Linux 内核在极少次 fork/clone 后瞬间触发顶峰自然回绕（重置到安全低位区间 `[300, 1024]`），随后立即通过 RAII 机制安全复原原始 `pid_max`。
2. **极速汇编兜底 (Inline Asm Fallback)**：若处于受限环境（如被强 SELinux 策略阻止修改 `procfs`），全自动切换至专属微架构内联汇编（`options(nostack)` 零栈开销直接触发系统调用）并绑定至 CPU 超大核，以每秒数万次的极速吞吐消耗 PID 实现回绕。
3. **低位免回绕**：若当前系统 PID 已处于安全低位（`< 1500`），则直接智能退出，避免不必要的系统开销。

---

## ✨ 核心特性

- **🚀 毫秒级极速重置**：核心采用 Rust 原生实现，利用内核自然回绕机制，执行耗时控制在微秒至毫秒级，软重启过程几乎零感知延迟。
- **🛡️ 健全的 RAII 安全机制**：
  - 基于 Rust `Drop` trait 实现 `PidMaxGuard`，在调整系统参数期间自动屏蔽进程信号，确保原始 `pid_max` 在任何异常路径下均 100% 自动还原。
  - 采用原子写入 `pwrite`，避免 procfs 写入偏移飘移。
- **⚡ 单一职责执行入口**：[`emulated-soft-reboot.sh`](emulated-soft-reboot.sh) 仅作为系统软重启事件的触发钩子，直接 `exec` 替换进程调用原生二进制，零多余子 Shell 开销。
- **📱 全主流架构覆盖**：原生适配 Android 全部四大主流指令集：
  - `ARM64` (`arm64-v8a` / `aarch64`)
  - `ARM32` (`armeabi-v7a`)
  - `x86_64` (`x64`)
  - `x86`
- **📊 模块可变简介（运行状态即时感知）**：集成 KernelSU 模块可变简介（`override.description`），状态在管理器中一目了然。

---

## 📁 项目结构

```text
soft_restart_fix/
├── Cargo.toml                 # Rust 项目依赖与编译配置
├── Cargo.lock                 # 锁定精确依赖版本
├── src/
│   └── main.rs                # 核心 Rust 源码（PID 回绕消耗器）
├── module.prop                # 模块元信息说明
├── emulated-soft-reboot.sh    # 软重启触发入口钩子脚本
├── customize.sh               # KernelSU 模块安装脚本（架构校验与安全拦截）
├── build.sh                   # 自动化跨架构构建与 Zip 打包脚本
└── bin/                       # 各架构预编译原生二进制文件
    ├── pid_wrap_arm64
    ├── pid_wrap_armeabi
    ├── pid_wrap_x86
    └── pid_wrap_x86_64
```

---

## 🛠️ 构建指南

本项目内置自适应跨平台构建脚本 [`build.sh`](build.sh)，支持在本地或 GitHub Actions CI 中自动化完成全架构编译与 strip 打包。

### 前置要求
- Linux / macOS 环境
- Rust 工具链（`rustc`、`cargo` 与 `rustup`）
- Android NDK（支持 r21 ~ r30+）
- Python 3 或 `zip` 命令

### 一键构建
```bash
# 自动扫描系统中已安装的最新 NDK 并编译所有 4 个 Android 目标
./build.sh

# 或显式指定 Android NDK 绝对路径
./build.sh /path/to/android-ndk
```

构建成功后将在根目录下生成开箱即用的 `soft_restart_fix.zip` 刷机包。

---

## 📲 安装与使用

1. 在 GitHub [Releases](https://github.com/youfeng11/soft_restart_fix/releases) 页面下载最新的 `soft_restart_fix.zip` 刷机包。
2. 打开 **KernelSU 管理器** $\rightarrow$ 进入 **模块** 页面 $\rightarrow$ 选择 **从本地安装**。
3. 安装成功后，执行一次软重启（例如在终端运行 `su -c 'setprop ctl.restart zygote'` 或通过工具箱执行软重启）。
4. 软重启完成后，返回 KernelSU 管理器查看模块简介，标签显示 `[✅已生效]` 即代表生效！

---

## 📄 开源协议

本项目采用 [Apache-2.0 License](LICENSE) 授权。

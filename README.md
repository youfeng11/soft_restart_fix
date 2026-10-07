# 软重启 PID 修复 (Soft Restart PID Fix)

[![KernelSU](https://img.shields.io/badge/KernelSU-Supported-brightgreen.svg)](https://kernelsu.org/)
[![Build and Release](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml/badge.svg)](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

一个专为 Android（KernelSU）设计的超轻量级高效模块。在系统执行软重启（Userspace Reboot）前，自动且极速地重置 Linux 内核 PID 计数器，使其回绕并重置到安全低位范围，从而有效避开部分安全软件或反作弊机制对软重启后异常高 PID 的检测。

---

## ✨ 核心特性

- **🚀 毫秒级极速重置**：核心采用 C 语言原生实现，利用 `/proc/sys/kernel/pid_max` 触发内核级瞬间回绕机制，重置耗时降至微秒/毫秒级，软重启几乎零感知延迟。
- **🛡️ 健全的安全与回退机制**：
  - 临时调整参数期间自动屏蔽进程信号，确保原始 `pid_max` 100% 立即恢复。
  - 若处于受限环境，全自动无缝回退至常规多核心 `vfork()` 循环模式。
  - 提供 Shell 原生备用兜底逻辑（[`emulated-soft-reboot.sh`](emulated-soft-reboot.sh)）。
- **📱 全架构适配**：支持主流所有 Android CPU 架构：
  - `ARM64` (`aarch64`)
  - `ARM` (`armeabi-v7a`)
  - `x86_64`
  - `x86`
- **📦 即刷即用**：符合 KernelSU 模块标准规范，安装时自动识别设备架构并剔除冗余文件，体积超小（仅几十 KB）。

---

## 📁 项目结构

```text
soft_restart_fix/
├── module.prop                # 模块元信息说明
├── pid_wrap.c                 # 核心 C 语言源码（PID 回绕消耗器）
├── emulated-soft-reboot.sh    # 软重启执行脚本（含兜底逻辑）
├── customize.sh               # KernelSU 模块安装脚本（架构适配）
├── build.sh                   # 跨环境构建与 Zip 打包脚本
└── bin/                       # 各架构预编译二进制文件
    ├── pid_wrap_arm64
    ├── pid_wrap_armeabi
    ├── pid_wrap_x86
    └── pid_wrap_x86_64
```

---

## 🛠️ 构建指南

本项目内置环境自适应构建脚本 [`build.sh`](build.sh)：

### 前置要求
- Linux / macOS 环境
- Android NDK（支持 r21 ~ r30+）
- Python 3 或 `zip` 命令

### 编译与打包
```bash
# 自动扫描并使用系统中已安装的最高版本 NDK
./build.sh

# 或者显式指定 NDK 路径
./build.sh /path/to/android-ndk
```

构建完成后将在根目录生成 `soft_restart_fix.zip` 刷机包。

---

## 📲 安装与使用

1. 在 Releases 页面下载最新的 `soft_restart_fix.zip`。
2. 打开 **KernelSU 管理器** $\rightarrow$ 点击 **模块** $\rightarrow$ 从本地安装并选择该 Zip 包。
3. 重启设备生效即可。

---

## 📄 开源协议

本项目采用 [Apache-2.0 License](LICENSE) 授权。

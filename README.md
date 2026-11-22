# 软重启 PID 修复（Soft Reboot PID Fix）

[![KernelSU](https://img.shields.io/badge/KernelSU-Module-brightgreen.svg)](https://kernelsu.org/)
[![Build and Release](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml/badge.svg)](https://github.com/youfeng11/soft_restart_fix/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](LICENSE)

用于 Android KernelSU 的 PID 回绕模块。在软重启前尝试将后续分配的 PID 降至较低区间，核心使用 Rust 实现，支持 ARM64、ARM32、x86_64 和 x86。

## 安装与使用

1. 从 [Releases](https://github.com/youfeng11/soft_restart_fix/releases) 下载 `soft_restart_fix.zip`。
2. 在 KernelSU 管理器的模块页面选择从本地安装。
3. 如管理器提示需要重启，先重启设备，再使用 KernelSU 的软重启功能。
4. 软重启后查看模块简介和日志。

需要支持 `emulated-soft-reboot.sh` 钩子的 KernelSU 版本。直接执行 `setprop ctl.restart zygote` 不会主动调用本模块。

## 查看结果

| 简介状态 | 含义 |
| --- | --- |
| `[⚠️未生效]` | 尚未写入执行结果 |
| `[✅已生效]` | 当前 PID 已较低，或快速路径检测到回绕 |
| `[✅已生效 (备用方案)]` | 备用路径检测到 PID 下降 |
| `[❌错误]` | 回绕失败 |
| `[❌错误，模块文件缺失]` | 未找到可执行的核心程序 |

状态表示最近一次执行结果，不保证关键进程的最终 PID 或特定应用的检测结果。日志位于：

```text
/data/adb/modules/soft_restart_fix/pid_wrap.log
```

## 工作原理

优先临时缩小 `pid_max`，通过创建立即退出的子进程触发 PID 回绕，随后尝试恢复原值。快速路径不可用时，改用循环创建子进程等待自然回绕；工具自身 PID 小于 `1280` 时直接跳过。

## 从源码构建

准备 Rust 工具链（含 rustup）和 Android NDK，在项目目录执行。

**Windows（PowerShell 5.1 或更新版本）：**

```powershell
.\build.ps1

# 或显式指定 NDK 路径
.\build.ps1 -NdkPath "D:\SDK\ndk\26.1.10909125"
```

**Linux / macOS（还需 Bash，以及 Python 3 或 `zip`）：**

```bash
./build.sh

# 或显式指定 NDK 路径
./build.sh /path/to/android-ndk
```

脚本编译四种架构，并在项目根目录生成 `soft_restart_fix.zip`。CI 使用 Ubuntu 和 NDK r26b。

## 开源协议

本项目采用 [Apache License 2.0](LICENSE)。

# DirectHCI

[English](../README.md)

DirectHCI 让 Windows 应用直接访问 USB 蓝牙控制器的原始 HCI 接口。
本地服务会将选中的控制器临时绑定到 WinUSB，收发 HCI 命令、事件和 ACL
数据，并在客户端释放会话或断开后恢复 Windows 蓝牙驱动。

仓库包含 Windows 服务、控制面板、命令行工具，以及用于原始 HCI 和
BLE Central/GATT 的 Rust 库。BLE 协议栈使用 TrouBLE。

**当前版本：`0.1.0-alpha.1`。** 开发测试主要使用 Windows 11 上的
Intel AX201，已有驱动接管、原始 HCI 通信和 Windows 恢复的实机记录。
BLE 生命周期回归问题，以及近期安装器和控制面板的改动，仍待实机复测。
具体结果见[兼容性记录](compatibility.md)。

DirectHCI 占用控制器期间，连接到这块控制器的 Windows 蓝牙设备会暂时
不可用。运行时同一时间只允许一个活动 HCI 会话。如果键鼠依赖这块控制器，
测试前请备好有线输入设备。

## 开始使用

安装器面向 Windows x64。从源码构建安装包需要 Rust、MinGW-w64、EWDK
和 Inno Setup，步骤见[安装文档](installation.md#build-the-installer)。

1. 运行安装包，从开始菜单打开 **DirectHCI Control Panel**。安装和启动
   面板都需要管理员权限。
2. 点击 **Start Service**，选择控制器。
3. 如果状态为 **Not prepared**，点击 **Prepare Controller**。确认本机
   证书信任后，服务会生成并暂存 WinUSB 驱动包。Windows 可能因签名策略
   拒绝该包，详见[控制器准备](installation.md#prepare-a-controller)。
4. 使用 CLI 或 Rust 客户端打开会话。启动服务和准备驱动包不会接管
   控制器；客户端申请会话时才会切换驱动。

服务运行后，可在 PowerShell 中查看状态和控制器列表：

```powershell
& "$env:ProgramFiles\DirectHCI\directhci.exe" status
& "$env:ProgramFiles\DirectHCI\directhci.exe" controllers
```

运行 `directhci.exe --help` 查看命令用法。BLE 操作使用
`directhci-ble.exe` 或 [BLE 库](../crates/directhci-ble/README.md)。
需要接管控制器的客户端也须以管理员身份运行。

关闭控制面板会停止服务；最小化则收起到托盘，服务继续运行。
如果异常退出后蓝牙未恢复，按[恢复步骤](installation.md#recovery)处理。

## Rust 库

| Crate | 用途 |
| --- | --- |
| [`directhci-client`](../crates/directhci-client) | 连接本地服务，查询控制器、申请和释放原始 HCI 会话 |
| [`directhci-bt-hci`](../crates/directhci-bt-hci) | 将 DirectHCI 会话适配为 `bt-hci` 控制器 |
| [`directhci-ble`](../crates/directhci-ble) | 异步 BLE 扫描、连接、GATT 读写和通知 |

这些 crate 尚未发布，使用时通过本地 checkout 的路径依赖引入。
客户端通过本地命名管道与 `directhcid` 通信，由服务管理 WinUSB 句柄、
驱动切换和恢复日志。会话与恢复流程见[架构文档](architecture.md)。

## 构建

需要 Rust 1.87 或更新版本。Windows 构建使用 `x86_64-pc-windows-gnu`
目标，MinGW-w64 工具（包括 `windres.exe`）须在 `PATH` 中。
在仓库根目录执行：

```powershell
rustup target add x86_64-pc-windows-gnu
$env:CARGO_TARGET_DIR = "$env:LOCALAPPDATA\DirectHCI\target"
cargo build --locked --release --workspace --target x86_64-pc-windows-gnu
```

可执行文件输出到
`%LOCALAPPDATA%\DirectHCI\target\x86_64-pc-windows-gnu\release`。
构建不会安装服务。前台运行 daemon、Linux 检查和安装包构建说明见
[开发文档](development.md)。

## 仓库结构

| 路径 | 内容 |
| --- | --- |
| `apps/` | Windows 服务、控制面板、诊断 CLI 和 BLE CLI |
| `crates/` | 客户端库、共享类型和 Windows 后端 |
| `driver/` | 按设备生成的 WinUSB INF 模板及旧版开发驱动包 |
| `installer/`、`scripts/windows/` | Windows 打包配置和构建脚本 |
| `docs/` | 安装、架构、恢复和硬件测试文档 |

## 文档

- [安装、控制器准备与恢复](installation.md)
- [开发](development.md)
- [硬件兼容性与已知问题](compatibility.md)
- [架构](architecture.md)
- [临时驱动切换与恢复机制](temporary-rebind.md)
- [Windows 实机测试](windows-test-plan.md)
- [独立 USB 控制器实验流程](dedicated-controller.md)
- [早期方案调研](ownership-survey.md)
- [依赖与参考资料](references.md)

## 许可证

DirectHCI 的第一方 crate 和应用当前采用 [GPL-3.0-only](../LICENSE.txt)。
第三方依赖的许可证见[参考资料](references.md)。

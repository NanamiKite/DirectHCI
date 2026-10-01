# DirectHCI 中文文档

本文是当前实现的中文总览，面向使用和开发。详细的 M1 驱动选择、
Journal 格式、WinUSB/UsbDk 调研与 API 来源仍见文末对应的英文专题文档。
**代码存在、过去在真机成功，以及当前版本已重新验收是三种不同状态。**

## 项目边界与现状

DirectHCI 是 Windows 蓝牙控制器的用户态所有权与 Raw HCI 基础设施，
不是某个 BLE 外设的专用协议项目。通用 BLE Central/GATT 能力位于独立
`directhci-ble` 库，由 TrouBLE 实现 Host Stack；`directhci-core` 和
`directhcid` 不包含设备专用 UUID、握手或业务状态。

```text
CLI / Control Panel / Rust consumer
            ↓ directhci-client（本地 Named Pipe）
         directhcid（唯一活动写会话）
            ↓ M1 临时 WinUSB 接管 + M2 RawHciSession
      Windows Bluetooth Controller

Rust BLE consumer
    ↓ directhci-ble → directhci-bt-hci → directhci-client
```

当前 AX201 主线是临时接管，不是把系统蓝牙永久绑定到 WinUSB：

```text
BTHUSB / WindowsOwned
  → 持久化恢复意图
  → 对准确 devnode 选择 DirectHCI WinUSB
  → Raw HCI 会话
  → 关闭 I/O
  → 重新观察、选择适用的 Windows 蓝牙驱动
  → BTHUSB / WindowsOwned
```

Dedicated 模式仅为另一块专用 USB dongle 的辅助路线。UsbDk 只有只读探测
代码，未在当前 AX201 宿主机安装或执行 redirect。

### 已记录的真机证据

同一台 Windows 11 AX201 宿主机曾完成
`BTHUSB/oem69.inf → DirectHCI WinUSB/oem183.inf → BTHUSB/oem69.inf`；
WinUSB readiness 为 `Ready`，接口 E0/01/01，Event IN `0x81`、
ACL IN `0x82`、ACL OUT `0x02`。这些端点号是实测结果，通用代码按
descriptor 发现，不应硬编码。HCI Reset 返回 `0x00`；还观察到 LE
连接、ATT MTU 交换、GATT discovery、被动 notification 与 Windows 恢复。
`oemNN.inf` 和驱动版本都只是当时的观察，不是永久恢复目标。

**当前版本尚待复测：** BLE CLI 提取为库后，后续真机报告出现过 GATT
timeout/disconnect 生命周期回归。较早的成功不等于最新 BLE 构建已经通过。
最近的安装包/面板 UAC 代码也尚无新一轮 Windows 宿主机验收记录。
请以 [兼容性记录](compatibility.md) 和新的实机结果为准。

## Windows 安装与使用

当前安装器只安装 Rust 可执行程序、注册 `DirectHCI` 服务并建立控制面板
快捷方式；它**不会**安装/签名/暂存开发用 AX201 WinUSB 驱动包，不会自动
改绑 AX201，也不会关闭 Secure Boot 或修改测试签名。新的 Windows 机器
若缺少已信任的开发包，不能仅凭运行安装器就获得 takeover 能力。

在 Windows 宿主机安装 Inno Setup 6.4 或更新版，进入仓库后运行：

```powershell
.\scripts\windows\build-installer.ps1
```

不要在修改源码后使用 `-SkipBuild`：它只检查 exe 是否存在，可能把旧版本
打进安装包。Cargo 构建目标默认在
`%LOCALAPPDATA%\DirectHCI\target\x86_64-pc-windows-gnu\release`；
新安装包在 `%LOCALAPPDATA%\DirectHCI\installer-output`。安装后程序位于
`C:\Program Files\DirectHCI`。三处不是同一个目录。当前开发版安装包
文件名固定为 `DirectHCI-Setup-0.1.0-alpha.1.exe`，不要误运行 Downloads
里的旧副本。

安装器的 `PrivilegesRequired=admin` 要求管理员安装。若安装器本来就是
通过已提权进程启动，Windows 可能不再显示第二次 UAC。安装程序的提权不会
传递给后来从开始菜单启动的控制面板；面板源码会检查自身令牌，并在必要时
请求 UAC，拒绝后不进入界面。**这项新行为须用重新打包、重新安装后的
Windows 版本确认**；普通 SDK/CLI 客户端不会因此自动提权。

打开 Control Panel 后可查看 SCM 服务状态、控制器列表、首选控制器、
所有权、活动客户端及恢复状态；可点击 Start/Stop Service、Refresh、
Restore Windows、Diagnostics。服务未安装或已停止时，面板仍应能打开并
显示不可用状态。面板不直接操作 SetupAPI/WinUSB，而是调用
`directhci-client` 和 SCM。

面板打开期间约每两秒自动刷新服务、控制器、活动客户端与恢复状态，
IPC 查询在后台执行，不阻塞窗口。正常关闭面板时，会先请求停止服务，
等待 SCM 确认 `Stopped` 后再退出；也可主动点击 **Stop Service**，
停止服务但保留面板。若有活动会话，停止前须确认，daemon 会关闭 Raw HCI
并尝试恢复 Windows 蓝牙；停止失败时面板保持打开并显示错误。普通客户端
断开只释放会话和恢复 Windows 蓝牙，**不会**自动停止服务。前台开发模式
`directhcid run` 仍由其自身的显式关闭操作结束。

### 首选控制器

首选项保存在受保护的 `%ProgramData%\DirectHCI\config.json`，由 daemon
验证 ControllerId 并原子写入，面板不直接编辑。只有一块控制器且没有已存
首选项时，daemon 可以自动选择；多块时必须人工选择；已保存的设备消失后
不会悄悄改选另一块。活动会话期间不能切换首选控制器。

ControllerId 是索引，不是授权凭据。真正 acquire 前必须重新枚举、确认
同一物理控制器、检查驱动与恢复候选。不要把旧 interface path、显示名称、
VID/PID 或 `oemNN.inf` 当成可靠的永久身份。

### 卸载与异常恢复

从 Windows“已安装的应用”卸载。卸载前置流程会尝试停止服务并调用现有
离线恢复；若无法确认 Windows Bluetooth 已恢复，就保留服务、文件及
Journal，不应手动删除它们。安装器升级也执行相应前置检查。

需要独立恢复时，在管理员终端运行已安装的：

```powershell
& "$env:ProgramFiles\DirectHCI\directhci.exe" recover --offline
```

此命令根据持久 Journal **加上新的 Windows 实际观察**进行 reconcile，
不会仅按历史 `oem69.inf` 名称盲目回放。若报告 identity ambiguous、
controller missing 或 unsafe journal path，应保留现场并分析，不能清空
`%ProgramData%\DirectHCI` 来“解除”保护。普通诊断可用
`directhci status` 和 `directhci controllers`（前提是服务可用）。

## 开发与真机边界

Linux VM 可编辑并做 Rust 静态检查；硬件、PnP、驱动与服务验收必须在
Windows 宿主机。不要把大量 Cargo `target/` 放入 VMware 共享目录：

```sh
export CARGO_TARGET_DIR=/tmp/directhci-target
cargo check --workspace --target x86_64-pc-windows-gnu
```

Windows 宿主机构建时让 `CARGO_TARGET_DIR` 指向本地磁盘，例如
`$env:LOCALAPPDATA\DirectHCI\target`。构建 exe 不等于已经安装服务；
构建目录 exe 也不等于 `C:\Program Files\DirectHCI` 中的已安装版本。
开发版 WinUSB INF、签名、证书信任与仅暂存 Driver Store 的流程见
[临时重绑定专题](temporary-rebind.md) 和
[驱动包说明](../driver/winusb-ax201-dev/README.md)；不应自动对系统 AX201
运行 Zadig，也不应把 `pnputil /add-driver` 改成带 `/install` 的命令。

通用 BLE 库可被另一个 Rust consumer 依赖：
`DirectHciBleCentral → BleConnection` 提供 scan、discover、read/write、
标准 CCCD subscribe、无 CCCD 被动 listen/listen_all 与显式 disconnect。
标准订阅和被动监听不会互相自动 fallback。详见
[BLE 库 README](../crates/directhci-ble/README.md)。在当前 BLE 生命周期
回归复测通过前，不要把旧的 notification 真机成功表述成最新构建已通过。

## 文档索引

| 主题 | 详细文档 |
| --- | --- |
| 架构、运行时与 BLE 边界 | [architecture.md](architecture.md) |
| 当前硬件证据与待复测项 | [compatibility.md](compatibility.md) |
| Windows 安装、升级、卸载 | [installation.md](installation.md) |
| VM / Windows 构建和运行 | [development.md](development.md) |
| AX201 临时重绑定与恢复 | [temporary-rebind.md](temporary-rebind.md) |
| Windows 验收阶段和记录 | [windows-test-plan.md](windows-test-plan.md) |
| Dedicated dongle 辅助流程 | [dedicated-controller.md](dedicated-controller.md) |
| 方案取舍与 UsbDk 风险 | [ownership-survey.md](ownership-survey.md) |
| 上游来源与许可证边界 | [references.md](references.md) |

本中文页是当前状态与操作的汇总，不复制各专题全部历史调查细节。
如文档与当前 Windows 真机观察冲突，以保存的实际输出为准，先核对构建
和安装的 exe 是否为同一版本，再更新相应验收记录。

## 许可证

项目当前暂定采用 GNU GPL 第 3 版，仅此版本（`GPL-3.0-only`）；
英文许可证全文见仓库根目录的 [LICENSE.txt](../LICENSE.txt)。
“暂定”表示未来版本仍可重新评估授权方案，并不表示已发布版本的授权可
追溯撤销。第三方依赖继续适用各自的许可证；来源和使用边界见
[references.md](references.md)。作为库依赖使用的 `directhci-ble`、
`directhci-client` 等第一方 crate 也采用此许可证；将其集成进其他项目
并分发前，应核对 GPL 义务和自身项目的授权兼容性。

# 在 Windows 上安装与恢复 DirectHCI

DirectHCI 面向 Windows x64。安装器提供运行时服务、Control Panel、命令行工具与按需准备 WinUSB 驱动包的组件；**安装本身不会切换蓝牙控制器的驱动**。普通用户不需要 EWDK、MSBuild 或驱动签名工具。当前动态准备与安装流程仍需要按具体 Windows 主机验收，见[兼容性记录（英文）](compatibility.md)。[English](installation.md)。

## 安装并启动

1. 若正在手动运行 `directhcid run`，先结束该前台进程，以免它占用命名管道。
2. 运行对应发布版本的 `DirectHCI-Setup-<version>.exe`，批准管理员权限请求。默认安装在 `C:\Program Files\DirectHCI`。
3. 从开始菜单打开 **DirectHCI Control Panel**。面板需要管理员授权；拒绝授权时会退出。
4. 点击 **Start Service**，在 **Preferred Controller** 中选择要使用的 USB 蓝牙控制器。

服务将所选 ControllerId 保存在 `%ProgramData%\DirectHCI\config.json`。若只发现一台控制器且尚无首选项，可以自动选中；若发现多台，需要用户选择。已保存的控制器不再可用时，面板不会擅自切换到另一台。活动会话期间不可切换首选控制器。

若设备管理器显示蓝牙设备，而面板没有可选控制器，先看[“面板看不到蓝牙控制器”](troubleshooting.zh-CN.md#面板看不到蓝牙控制器)。当前后端只识别符合条件的 USB 蓝牙设备节点，不是所有 Windows 蓝牙总线类型。

## 准备控制器

若所选设备显示 **Not prepared**，点击 **Prepare Controller** 并审阅本机证书信任提示。服务从该控制器实际 PnP 属性获取 Hardware ID，生成仅匹配它的 WinUSB 包，使用一次性本机证书签名，再请求 Windows 将包暂存到 Driver Store。以后新增符合条件的 USB 蓝牙控制器，可在面板中按需 Prepare，无须重装 DirectHCI。

**Prepare 不等于 Takeover。** 准备仅增加一个候选驱动，不会立即从 Windows 手中接管蓝牙。暂存成功后，运行时仍会检查原 Windows 蓝牙驱动是否保持活动，并重新计算接管安全计划。只有后续客户端申请会话才会尝试临时接管。

Windows 可能因代码完整性／签名策略拒绝本机签名包。DirectHCI 应报告真实错误，不会为此关闭 Secure Boot、启用 TESTSIGNING 或修改 BCD。失败时参见[安全排查](troubleshooting.zh-CN.md#prepare-controller-被-windows-拒绝)。

## 服务和面板

Control Panel 显示服务状态、首选控制器、活动客户端与恢复状态：

| 操作 | 预期行为 |
| --- | --- |
| 客户端释放会话或断开 | 运行时关闭 HCI 会话并尝试、验证 Windows 蓝牙恢复；服务继续运行 |
| 点击 **Stop Service** | 停止服务，面板仍打开 |
| 关闭面板或从托盘选择 **Exit Control Panel** | 请求停止服务，等待状态变为 Stopped 后退出 |
| 最小化面板 | 收入系统托盘，服务保持运行 |

活动会话期间停止服务需要确认；若停止失败，面板会保留并显示错误。点击托盘图标可重新打开窗口。**服务停止本身不能证明蓝牙已经恢复**，应以实际控制器状态为准。

查询状态可在 PowerShell 中运行：

```powershell
$cli = Join-Path $env:ProgramFiles 'DirectHCI\directhci.exe'
& $cli status
& $cli controllers
```

当前权限模型下，申请控制器或发送 Raw HCI 需要管理员身份。日常故障先使用面板；更多命令见 [CLI（英文）](cli.md)。

## 离线恢复

若运行时仍可用且面板显示需要恢复，先在面板点击 **Restore Windows**。若服务不可用，先确认服务已停止，再在管理员 PowerShell 中运行：

```powershell
$cli = Join-Path $env:ProgramFiles 'DirectHCI\directhci.exe'
& $cli recover --offline --json
```

恢复会重新枚举 journal 指向的原物理控制器和当前驱动状态。过期 journal 不等于驱动仍错误；只有新鲜观察确认 Windows 蓝牙已正常接管时才会清理记录。设备缺失、身份不明确、journal 目录不安全或恢复尚未确认时，流程会保留 journal。**不要为了消除错误而手动删除 journal。** 参见[常见问题与安全恢复](troubleshooting.zh-CN.md)。

## 升级或卸载

运行新版安装器升级，或通过 **Windows“已安装的应用” → DirectHCI → 卸载**。升级与卸载前必须停止服务并确认 Windows 蓝牙恢复；无法确认时应保留服务、可执行文件与 journal 供恢复。

当前卸载程序**不会**自动删除已经暂存的 WinUSB 包、受信任的一次性证书公钥或 `%ProgramData%\DirectHCI` 状态。清理这些对象需要识别其依赖关系；不要先删除可能仍被驱动包使用的证书。

开发者构建安装包的方法见[开发说明（英文）](development.md#build-the-installer)。只编译可执行文件不会自动安装服务。

# 常见问题与安全恢复

DirectHCI 会临时切换 Windows USB 蓝牙控制器的驱动。发生故障时，先确认控制器**实际被 Windows 枚举成什么状态**。不要为消除报错而删除 `%ProgramData%\DirectHCI\ownership-journal-v1.json`、强装其他设备的 INF，或关闭 Secure Boot／驱动签名检查。若键鼠依赖正在操作的蓝牙控制器，请备好非蓝牙输入设备。

日常安装见[中文安装与恢复说明](installation.zh-CN.md)；内部状态见[故障模型（英文）](failure-model.md)。[English](troubleshooting.md)。

## 面板看不到蓝牙控制器

- **现象：** 服务可能显示 Running，但 **Bluetooth Controller** 一栏为空或不可用。
- **查看哪里：** 点击面板 **Refresh**，查看 **Diagnostics**；在设备管理器中确认是否有当前存在、状态正常的 **USB 蓝牙控制器设备节点**。仅有“Bluetooth”字样并不能证明它属于 USB。若服务已停，先在面板中启动。高级诊断可用 `directhci controllers --direct --json`，该命令不会接管控制器。
- **安全下一步：** 若只看到非 USB 节点（例如 `BCMBTBUS\BLUETOOTH\...`），当前后端不能准备该节点。若确有正常的 USB/BTHUSB 控制器却仍为空，保留面板诊断与直接枚举结果供排查。不要在设备管理器里对任意设备强制安装 WinUSB。参见[兼容性条件](compatibility.md#compatibility-model)。

## Prepare Controller 被 Windows 拒绝

- **现象：** 面板显示准备失败或 `PackageStagingRejectedByWindows`。
- **查看哪里：** 复制面板完整错误，确认设备管理器中原来的 Windows 蓝牙驱动仍在使用。高级排查可查看 `%WINDIR%\INF\setupapi.dev.log` 中对应时间段；分享前检查日志里的设备标识与路径。
- **安全下一步：** Prepare 本身不会接管控制器。不要修改 Secure Boot、TESTSIGNING 或 BCD，不要用 `/install` 或手动强装 INF。若 Windows 拒绝本机签名的包，记录真实 Windows 错误与签名策略信息；该机器可能不接受这种准备方式。失败后可能留下已信任的一次性证书公钥。详见[驱动准备机制（英文）](internals/driver-provisioning.md)。

## 显示 RecoveryRequired

- **现象：** 面板或 `directhci status` 显示 `RecoveryRequired`，重启后也可能仍显示。
- **查看哪里：** 查看面板 **Recovery／Diagnostics**，并在设备管理器核对是否仍是同一物理控制器、当前服务／驱动及问题代码。保留的 journal 是恢复证据，不代表 WinUSB 一定仍在使用。
- **安全下一步：** 先结束活动客户端，在正在运行的面板中点击 **Restore Windows**。恢复流程会重新枚举 journal 指向的控制器；如果 Windows 驱动已经正常工作，可核对后清理过期记录。若服务不可用，按[离线恢复步骤](installation.zh-CN.md#离线恢复)使用管理员终端。若设备身份缺失或不明确，保留 journal 和诊断结果；不要选同 VID/PID 的另一台设备，也不要反复尝试接管。

## 服务已停止，但蓝牙没有恢复

- **现象：** DirectHCI 服务显示 Stopped，但 Windows 蓝牙仍关闭、设备管理器报错，或找不到控制器。
- **查看哪里：** 服务停止**不等于**已确认 `WindowsOwned`。检查设备管理器与面板／CLI 诊断。“设备描述符请求失败”的未知 USB 设备可能不再提供原来的 PnP 身份，恢复因此可能报告 `device_missing`。
- **安全下一步：** 暂停再次接管，在服务停止时以管理员身份运行[离线恢复](installation.zh-CN.md#离线恢复)。若结果是 `device_missing`、`RecoveryRequired` 或不能确认原控制器，保留 journal，记录完整结果与设备管理器问题代码。硬件断电重置或重启可能改变枚举状态，但不保证修复；之后仍需重新核对。不要删除 journal，也不要向未知 USB 设备强装驱动。

## 提交诊断信息

建议记录 DirectHCI 构建、Windows 版本、控制器型号／USB ID、失败前操作、完整错误和最终观察到的驱动／ownership 状态。优先使用面板 **Diagnostics → Copy**。公开 CLI 或 SetupAPI 日志前，检查设备实例 ID、序列号、机器／用户路径及证书标识；不确定时先私下保存原始日志。需要记录的字段见[Windows 实机测试计划（英文）](windows-test-plan.md)。

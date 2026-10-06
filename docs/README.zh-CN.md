# DirectHCI

[English](../README.md)

DirectHCI 是面向 USB 蓝牙控制器的 Windows 用户态 ownership 与 Raw HCI
运行时。本地服务可以将选中的控制器临时绑定到微软 WinUSB，收发 HCI 命令、
事件和 ACL 数据，并在会话结束后恢复 Windows 蓝牙驱动。

DirectHCI **不使用静态蓝牙 VID/PID 白名单**。**Prepare Controller**
会从新鲜枚举且符合条件的 USB 蓝牙控制器读取真实 Hardware ID，生成只匹配
该 ID 的 WinUSB 驱动包。准备操作仅将候选驱动暂存到 Driver Store，
**不会**立即接管控制器。Windows 签名策略和后续接管安全检查仍可能拒绝操作。

```text
Windows 蓝牙（BTHUSB / IBTUSB）
    │ Prepare Controller：暂存精确 Hardware ID 的 WinUSB 候选驱动
    │ Acquire：核对设备身份、驱动排名和恢复路径
    ▼
DirectHCI 接管控制器（WinUSB）
    │ HCI Command / Event / ACL
    │ Release、客户端断开或服务停止
    ▼
恢复 Windows 蓝牙
```

仓库包含 Windows 服务、控制面板、诊断 CLI，以及基于 TrouBLE 的 Raw HCI
和 BLE Central/GATT Rust 库。DirectHCI 不是某个厂商设备的专用协议，
也不自行替代 BLE Host Stack。

## 主要能力

- 带持久恢复日志的、受安全检查约束的临时 USB 蓝牙接管。
- 通过本地 Windows 服务和 SDK 提供 Raw HCI Command、Event、ACL。
- 按需生成精确 Hardware ID 的 WinUSB 驱动包，不维护静态 VID/PID 列表。
- 原生控制面板、诊断 CLI，以及可选的 BLE Central/GATT 库。

## 安装与快速开始

安装器面向 Windows x64。安装 DirectHCI 本身不会切换蓝牙驱动。

1. 安装 DirectHCI，从开始菜单打开 **DirectHCI Control Panel**。
   安装和打开面板会请求管理员授权。
2. 点击 **Start Service**，选择要使用的 USB 蓝牙控制器。
3. 若显示 **Not prepared**，点击 **Prepare Controller**，确认本机证书
   信任提示。若 Windows 拒绝本机签名的包，面板会报告错误，不会切换控制器。
4. 启动 DirectHCI 客户端。正常使用时由客户端申请会话开始临时接管；
   开发命令 `takeover ... --execute` 也可以显式切换驱动。
   释放会话、客户端断开或服务停止时会进入恢复流程。

控制面板显示服务、首选控制器、活动客户端和恢复状态。关闭面板会停止服务；
最小化会收入托盘并保持服务运行。日常使用见[中文安装与恢复说明](installation.zh-CN.md)，故障时见[常见问题与安全恢复](troubleshooting.zh-CN.md)。

DirectHCI 占用控制器期间，使用该控制器的 Windows 蓝牙设备暂时不可用。
若键鼠依赖这块控制器，实验时请备好非蓝牙输入设备。运行时同时只允许
一个活动 Raw HCI 会话。

## 当前状态

版本：`0.2.0`。用户已在 Intel AX201（`8087:0026`）、
AX200（`8087:0029`）和 BE200／Gale Peak 系列蓝牙（`8087:0036`）
的实机上报告 DirectHCI 正常运行。这些是常见硬件系列名称，不能仅凭
USB ID 断定设备的具体模块 SKU。其中 AX201 已有较详细的接管、
Raw HCI、BLE 和恢复记录；
另两组尚缺归档的构建、主机与逐阶段日志。Windows 是否接受本机签名的
驱动包仍取决于主机策略。详见[兼容性与验收记录](compatibility.md)。

后续 BLE library 改动曾出现 GATT 超时／断连回归报告。当前接收顺序与
生命周期修正仍需 Windows 真机回归验收；早期正常运行不代表本次构建已验收。

## 架构与开发

```text
Raw HCI consumer / CLI / Control Panel
                  ↓
           directhci-client
                  ↓ 本地命名管道
               directhcid
                  ├─ ownership 与恢复
                  └─ WinUSB Raw HCI
```

`directhcid` 持有驱动切换、WinUSB 句柄和恢复日志。BLE Host 是 Raw HCI
之上的可选 consumer，不属于特权运行时。详见[架构](architecture.md)、
[SDK](sdk.md) 和[开发指南](development.md)。Rust crate 暂未发布，
其他项目目前通过本地 checkout 的 path dependency 使用。

## 文档

- [安装与恢复（中文）](installation.zh-CN.md) · [常见问题与安全恢复（中文）](troubleshooting.zh-CN.md)
- [兼容性模型与硬件验证](compatibility.md)
- [架构](architecture.md)
- [Ownership 与恢复机制](ownership-and-recovery.md)
- [故障模型](failure-model.md)
- [CLI](cli.md) · [Rust SDK](sdk.md) · [开发](development.md)
- [驱动准备内部机制](internals/driver-provisioning.md)
- [Windows 实机测试计划](windows-test-plan.md)
- [依赖与参考资料](references.md)

## 许可证

DirectHCI 第一方 crate 和应用当前采用
[GPL-3.0-only](../LICENSE.txt)。第三方依赖的许可证见[参考资料](references.md)。

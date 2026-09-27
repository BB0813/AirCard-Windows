# AirCard (Windows) 🎴

> **iOS 18+ Apple 钱包卡片皮肤 & 锁屏密码键盘主题工具（无需越狱）**  
> Rust 编写的原生 Windows 客户端，基于 `airlift` 的 AirTraffic 同步漏洞实现。

---

## 功能特性

- 🎨 **自定义卡片皮肤**：为 Apple Pay 和 Apple Cash 卡片替换自定义图片、纹理或银行 Logo。
- 🔢 **锁屏密码键盘主题（.passthm）**：直接把 Cowabunga 和 Nugget 的 `.passthm` 主题包应用到 iOS 锁屏键盘按钮。
- ⚡ **原生 & 轻量**：单文件 `aircard.exe`（约 7.5 MB）。不需要 Python、不需要 Flet、不需要 webview、不需要臃肿的运行时。
- 🪟 **Material Design 3 界面**：基于 `egui` 和 `eframe` 构建的现代深色主题。
- 📱 **零门槛卡片识别**：连接 iPhone 后打开钱包点击卡片，通过 `syslog_relay` 实时捕获卡片 hash。
- 📶 **USB & WiFi 双通道**：通过 USB 或已配对的本地 WiFi 扫描卡片事件并写入资源。
- 🌐 **中文 / English 界面**：顶栏一键切换，选择会本地保存，下次启动自动生效。
- 🔄 **可回滚**：完整的 Books 状态快照与自动恢复机制，最大程度保留设备原有状态。
- 🚀 **无需越狱**：利用 Apple 自带的 AirTraffic 同步通道，不修改系统分区、不关闭安全机制。

---

## 环境要求

- **Windows 10 / 11（64 位）**
- **Apple Mobile Device Support 或 64 位 iTunes**（与 Apple 设备通信必需）。
- 首次配对需要用到 Lightning 或 USB-C 数据线。
- 使用 WiFi 模式时，需开启 WiFi 同步，并保证电脑和 iPhone 在同一局域网。

---

## ⚠️ 排查与驱动修复（什么都跑不起来时先看这里）

> [!TIP]
> **iPhone 识别不到、AirTraffic 同步卡住、或者操作失败？**
> Windows 上 Apple USB 驱动损坏或冲突是首要原因。
> 1. 下载并安装 **[3uTools](https://www.3u.com/)**。
> 2. **从电脑上拔掉 iPhone**。
> 3. 在 3uTools 里进入 **工具箱 ➔ 修复驱动**。
> 4. 点击 **立即修复**，等待 Apple 驱动重装完成。
> 5. 重新插上已解锁的 iPhone，点击 **信任**，然后启动 **AirCard**。

> [!WARNING]
> **3uTools 和 i4Tools 会带来另一个问题。** 它们安装的 Apple 驱动会覆盖
> `HKLM\SOFTWARE\Apple Inc.\CoreFP` 注册表项，把它指向自带的 `CoreFP.dll`，
> 而那个文件通常早已被删除。结果是能连上设备、能扫码，但一写入就超时。
> 如果遇到这种情况，请修复注册表，或直接改用 Apple 官方 iTunes，详见下一节。

---

## 卡在「Waiting for ReadyForSync」/「AirTraffic sync timed out」

如果日志停在 iPhone 发出 `SyncAllowed` 之后、然后传输超时，问题出在
FairPlay —— 既不是数据线，也不是本工具。

AirTraffic 需要 `CoreFP.dll`，它通过读取注册表
`HKLM\SOFTWARE\Apple Inc.\CoreFP` 来定位这个文件。`CoreFP.dll`
**不属于** Apple Mobile Device Support，它随 **iTunes** 一起安装。所以：

- 只装过 Apple Mobile Device Support 的机器，**根本没有**这个文件；
- 装过 **i4Tools / 3uTools** 的机器，注册表被改成指向它们自带的副本，而那个路径通常已经不存在。

两种情况都会导致主机无法生成 FairPlay 凭证，iPhone 于是接受会话（`SyncAllowed`），
之后就不再推进握手。v1.3.0 会在启动时检测这一点并给出明确提示，
而不是让你对着一个静默的超时干等 60 秒。

**先检查你的机器：**

```powershell
# 在管理员权限的 PowerShell 中运行
(Get-ItemProperty 'HKLM:\SOFTWARE\Apple Inc.\CoreFP' -ErrorAction SilentlyContinue | Format-List *)
Test-Path "$env:ProgramFiles\Common Files\Apple\Mobile Device Support\CoreFP.dll"
```

**一键诊断**（连上 iPhone 后运行，会打印设备侧的实际情况）：

```powershell
aircard.exe probe
```

### 方案 A — 用本工具自带的修复脚本（推荐）

```powershell
# 1) 下载 Apple 官方 iTunes 安装包，从中提取经 Apple 签名的 CoreFP.dll
#    （无需管理员权限，约 200 MB 下载）
powershell -ExecutionPolicy Bypass -File tools\Extract-CoreFP.ps1

# 2) 安装 DLL 并修复注册表（会弹一次 UAC）
powershell -ExecutionPolicy Bypass -File tools\Install-CoreFP.ps1
```

提取步骤会校验 DLL 的 `Apple Inc.` 签名后才写入；安装步骤会写好 `LibraryPath`、
清理掉残留的 `Libi4CFPath` / `LibiiiiPath`，并先把旧注册表项备份到
`%TEMP%\CoreFP-registry-backup.reg`，可随时回滚。

完成后**重启电脑**，重新连接 iPhone，打开一次 Apple 图书，再重试。

### 方案 B — 安装官方 iTunes

先卸载 i4Tools/3uTools 安装的 Apple 驱动，再从
[apple.com](https://www.apple.com/itunes/) 安装**官方 iTunes**。
微软商店版本不含 Mobile Device Support，装了也没用。

---

## 安装

### 使用预编译程序

1. 从 [Releases](https://github.com/BB0813/AirCard-Windows/releases) 下载 **`aircard.exe`**。
2. 用 USB 连接 iPhone，解锁屏幕，弹出提示时点击 **「信任此电脑」**。
3. 运行 **`aircard.exe`**。开启 WiFi 同步后，后续会话可以不插线。

---

## WiFi 连接配置

1. 先用 USB 连接 iPhone 完成配对。
2. 在「Apple 设备」或 iTunes 中勾选 **「当此 iPhone 接入 Wi-Fi 时显示」** / **「通过 Wi-Fi 与此 iPhone 同步」**。
3. 应用设置，保持 iPhone 与电脑在同一局域网。
4. 在 AirCard 中点 **刷新**，确认设备显示 **WiFi** 通道。
5. 拔掉数据线，再点一次 **刷新**，然后选择 **仅 WiFi**。需要自动回退时选 **自动（优先 USB）**。

两种通道都可用时，**自动** 会先用 USB，失败再退回 WiFi。
要确保整条链路都走 WiFi，请拔掉数据线、点 **刷新**、再选 **仅 WiFi** ——
这一步是必要的，因为 Apple 的 AirTraffic API 按 UDID 选路，不接受传输方式参数。

---

## 如何自定义 Apple 钱包卡片

1. 通过 USB 或已配对的 WiFi 连接 iPhone，并确保屏幕已解锁。
2. 在 AirCard 的 **钱包** 标签页点击 **扫描**。
3. 在 iPhone 上：
   - 打开 **Apple 钱包**（或连按右侧边键）。
   - 点击想自定义的那张卡片。
   - AirCard 会自动抓取并保存卡片 hash。点 **停止** 结束扫描。
4. 点 **选择图片...** 挑选图片（PNG、JPG、WebP —— 在预览区拖动调整裁切位置，会自动缩放到 `1536 × 969`）。
5. 点 **应用卡片皮肤**。
6. 在 iPhone 上从后台划掉 **钱包** App（底部上滑进多任务，把钱包卡片划走），重新打开钱包即可看到新皮肤。

### 关于「恢复原卡面」

> [!IMPORTANT]
> **在 iOS 26 / 27 上「恢复原卡面」不可用。**
>
> 这个功能需要 AFC 把设备上的原始图片读出来，而当前 iOS 封了这条路。
> 在 iPhone17,1 / iOS 26.7 上实测：
>
> ```
> com.apple.afc            -> 所有 /var 路径返回 error 8
> com.apple.afc2           -> AMDeviceSecureStartService: -402653150
> com.apple.mobilesync.AFC2-> AMDeviceSecureStartService: -402653150
> ```
>
> `com.apple.afc` 只暴露 Media 沙箱（`/DCIM`、`/Books`、`/Downloads`），
> 能提供完整文件系统的 AFC2 服务被系统直接拒绝启动，而卡面包正好在
> `/var/mobile/Library/Passes/Cards/`，在沙箱之外。
>
> **应用卡片皮肤不受影响** —— 那条路走 AirTraffic，依然能写进卡面包。
> 这也解释了为什么「能刷卡面，但不能备份原卡面」。
> 我也验证过让 AirTraffic 把文件写到 AFC 能读到位置再取回：写入报告成功，
> 但文件落在所有 AFC 可见路径之外，所以这条路也走不通。
>
> 运行 `aircard.exe probe` 可以看到你这台设备的实际情况。如果它报告某个服务
> 能读到卡片目录，那么下面的备份与恢复功能就是可用的。

在 AFC 能访问 `/var` 的系统上，首次应用皮肤会保存一份原卡面的本地备份，
之后 **恢复原卡面** 会把它写回去，并让钱包的缓存图片失效。

---

## 如何应用锁屏密码主题（.passthm）

1. 切换到 AirCard 的 **锁屏密码** 标签页。
2. 点 **选择 .passthm...**，挑选任意 `.passthm` 主题包（Cowabunga 或 Nugget 出品）。
3. 选择目标 iOS 版本缓存：
   - **自动 (TelephonyUI-10)** — iOS 18+（默认）
   - **TelephonyUI-9** — iOS 16 - 17
   - **TelephonyUI-8** — 更早的 iOS
4. 点 **应用密码主题**。
5. 锁定 iPhone 屏幕或打开拨号键盘，即可看到新的自定义按键。

> [!IMPORTANT]
> **请关闭「粗体文本」：**  
> 在 iPhone 上进入 **设置 ➔ 显示与亮度**，确认 **粗体文本** 已**关闭**。
> 开启粗体后，iOS 会忽略缓存的拨号键盘图片，改用系统矢量字体渲染。

---

## 从源码构建

前置条件：[Rust 工具链](https://rustup.rs/)（`stable-x86_64-pc-windows-msvc`）。

```powershell
# 克隆仓库
git clone https://github.com/BB0813/AirCard-Windows.git
cd AirCard-Windows

# 运行测试
cargo test

# 构建发布版
cargo build --release
```

产物在 `target\release\aircard.exe`。

---

## 贡献者

- **[@Lumid-Off](https://github.com/Lumid-Off)**（Windows 原生 Rust 移植 & 维护者）— [GitHub](https://github.com/Lumid-Off) · [Twitter / X](https://x.com/LumidOff)
- **[@mak5er](https://github.com/mak5er)**（原版 macOS 应用 & 漏洞研究）— [GitHub](https://github.com/mak5er) · [Twitter / X](https://x.com/mak5er)
- **[0xjohnnydev 的 AirLift](https://github.com/0xjohnnydev/airlift)**：`AirliftFFI` 背后的 AirTraffic / ATAirlock 沙箱逃逸原始实现与概念验证。

## 致谢

- 核心漏洞利用基于 `airlift`（AirTraffic 同步逃逸）。
- 主题格式参考 [Cowabunga](https://github.com/leminlimez/Cowabunga) 与 [Nugget](https://github.com/leminlimez/Nugget)。

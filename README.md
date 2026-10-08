# AlertApp 即时提醒

Slint + Rust 写的跨平台（**优先 Android 移动端**，桌面端用于开发调试）即时提醒应用。

采用 **MQTT 订阅/发布** 模式：手机、电脑、模拟器等任意设备订阅同一个 Topic，任何一台设备
发布消息，其他设备**实时收到**提醒，并按需执行 —— **亮屏 / 震动 / 响铃 / 弹窗**（可任意组合）。

- 消息链路：设备 A 发布 → MQTT Broker → 设备 B 订阅 → 触发提醒（秒级）
- 离线补发：`clean_session=false` + QoS1 持久会话，掉线期间的消息由 Broker 缓存，重连后自动送达
- 断线重连：rumqttc 事件循环自动重连（指数退避），重连后自动重新订阅
- 保活设计：见下文「后台保活」

## 目录结构（cargo workspace）

```
alertapp/
├── Cargo.toml            # workspace 根（release 优化配置）
├── app/                  # 应用主体（cdylib + rlib，无 bin —— cargo-apk 要求）
│   ├── Cargo.toml        # 依赖 + cargo-apk 打包配置（权限、图标、主题、双架构）
│   ├── build.rs          # slint-build 编译 UI
│   ├── ui/main.slint     # 界面：消息 / 发布 / 设置 三个页面（浅色卡片风）
│   ├── res/              # Android 资源：启动图标、主题、通知小图标
│   ├── tools/gen-icon.ps1  # 重新生成启动图标
│   └── src/
│       ├── lib.rs        # 核心逻辑 + android_main 入口
│       ├── config.rs     # 配置持久化（JSON）
│       ├── mqtt.rs       # MQTT 引擎（独立线程，自动重连/重订阅）
│       ├── alert.rs      # 提醒动作定义与分发（亮屏/震动/响铃/弹窗组合）
│       └── platform/
│           ├── android.rs  # Android：JNI 调系统能力（通知/全屏意图/震动/响铃/唤醒锁）
│           └── desktop.rs  # 桌面端最小实现（系统通知 + 提示音）
└── tools/                # 桌面程序与命令行工具
    └── src/
        ├── main.rs       # alertapp-desktop：桌面 GUI
        └── bin/send.rs   # alertapp-send：命令行发布工具
```

> 为什么拆成两个包：cargo-apk 按「包名 → cdylib 产物」收集打包对象，包里若还有 `[[bin]]`
> 会报 `Bin is not compatible with Cdylib`。所以 Android 打包的 `app` 包不带任何 bin，
> 桌面程序和 CLI 放在 `tools` 包里。

## 消息协议

发布到 Topic 的负载是 JSON：

```json
{
  "title": "服务器告警",
  "body": "CPU 使用率 95%",
  "sender": "alertapp-abc123",
  "actions": ["popup", "screen", "vibrate", "ring"]
}
```

- `actions` 可任意组合：`popup` 弹窗 / `screen` 亮屏 / `vibrate` 震动 / `ring` 响铃
- 不发 JSON、直接发纯文本也可以，按「默认动作的文本提醒」处理
- 生效动作 = 发送方请求 ∩ 接收方「消息页」底部的提醒方式开关（本机关掉的方式绝不打扰）
- 自己发的消息只进列表、不提醒（按 `sender` 识别）

## 快速开始（桌面端先跑通）

```powershell
cd F:\code\alertapp
cargo run -p alertapp-desktop     # 启动 GUI（默认连 broker.emqx.io:1883，主题 alertapp/demo/alerts）
```

再开一个终端发一条提醒：

```powershell
cd F:\code\alertapp
cargo run -p alertapp-tools --bin alertapp-send -- `
  --broker broker.emqx.io --topic alertapp/demo/alerts `
  --title "服务器告警" --body "CPU 95%" --actions popup,screen,vibrate,ring
```

桌面端表现：系统通知弹窗 + 蜂鸣提示音（亮屏/震动仅 Android 有效）。
两台设备都装上本应用后，也可以直接用「发布」页互相发提醒。

> ⚠️ 公共 Broker（broker.emqx.io）任何人都能订阅，**主题请用足够随机的长名字**（如
> `alertapp/你的私有串/alerts`）。涉及隐私的提醒请自建 Broker 或使用 EMQX Cloud 等带鉴权的托管服务。

## Android 构建

环境（已具备）：Android SDK/NDK、JDK、`cargo-apk`、Rust target `aarch64-linux-android`、`x86_64-linux-android`。

```powershell
cd F:\code\alertapp\app
cargo apk build            # 通用调试包（arm64 + x86_64），输出在 ..\target\debug\apk\
cargo apk build --release  # 发布包（需本机存在 release.keystore，见下）

adb install -r ..\target\debug\apk\alertapp.apk
# Android 13+ 先授通知权限，再启动（包名 cargo-apk 默认前缀为 rust.）：
adb shell pm grant rust.alertapp android.permission.POST_NOTIFICATIONS
adb shell monkey -p rust.alertapp -c android.intent.category.LAUNCHER 1
```

模拟器（x86_64）与真机（arm64）都能装这个通用包。

### release 签名密钥（不入库）

`app/release.keystore` 被 `.gitignore` 排除，**仓库里没有它**。`cargo apk build --release`
需要该文件存在；只做 debug 打包不受影响。

克隆仓库后若要出 release 包，用 keytool 重新生成一个（别名与密码须与
`app/Cargo.toml` 的 `[package.metadata.android.signing.release]` 一致）：

```powershell
cd F:\code\alertapp\app
keytool -genkeypair -v -keystore release.keystore -alias alertapp `
  -keyalg RSA -keysize 2048 -validity 10000 `
  -storepass alertapp2026 -keypass alertapp2026 `
  -dname "CN=alertapp, OU=dev, O=alertapp, L=, S=, C=CN"
```

> 注意：换新密钥后，已安装的旧版本无法覆盖安装（签名不同），需先卸载。
> 想让升级连续，请自行备份 `release.keystore`，不要提交到公开仓库。

安装后一次性配置（设置页里都有入口）：

1. **电池白名单**：点「电池白名单」按钮，允许本应用忽略电池优化
2. **通知权限**（Android 13+）：点「通知设置」，允许通知
3. **全屏意图**（Android 14+）：通知设置里允许「全屏Intent/全屏通知」，息屏亮屏才生效
4. **厂商 ROM**（小米/华为/荣耀等）：系统设置里允许「自启动」，最近任务里下拉锁定本应用

改 Topic / Broker / 账号密码后，点「保存并重连」立即生效。

## 后台保活的实现与边界

| 手段 | 说明 |
| --- | --- |
| MQTT 持久会话 | QoS1 + clean_session=false，断网/杀进程期间的消息由 Broker 补发 |
| 断线自动重连 | 网络恢复后秒级重连并重新订阅 |
| PARTIAL_WAKE_LOCK | 「后台保活」开关打开时长期持有，息屏后 CPU/网络不休眠，连接不断 |
| 电池优化白名单 | 避免系统冻结/杀死后台进程 |
| 全屏意图通知 | 息屏/锁屏状态直接点亮屏幕弹出提醒（像来电一样） |

**与微信的差距要说清楚**：微信靠厂商推送通道 + FCM（系统级常驻），本应用是自建长连接。
做到上述配置后，日常使用可以做到秒级实时；但极端情况（系统极端内存压力、厂商激进攻杀后台）
进程仍可能被杀。进程被杀不会丢消息——下次打开应用时 Broker 会把离线消息补发出来。

### 已实测验证（模拟器 + 本地 Broker）

- ✅ 前台/后台收到消息均秒级触发 弹窗+震动+响铃+亮屏（应用压到桌面后横幅通知照常弹出）
- ✅ `PARTIAL_WAKE_LOCK` 稳定持有：`dumpsys power` 锁列表长期可见 `alertapp:keepalive`
- ✅ 全屏意图 + `ACQUIRE_CAUSES_WAKEUP` 亮屏锁：息屏时点亮屏幕弹出（真实息屏请在真机上验证）
- ⚠️ 实现坑位记录：`WakeLock.acquire(i64::MAX)` 会因 Android 内部 `postDelayed(uptime+timeout)`
  整数溢出而**瞬间释放**，必须用有限超时（本项目用 7 天）+ 每 30 秒 `isHeld()` 自检重取

### 本地开发调试（无需公网 Broker）

```powershell
node broker.mjs                 # 启动本地 Broker（aedes，见 tmp 示例或自写 ~15 行）
adb reverse tcp:1883 tcp:1883   # 设备的 127.0.0.1:1883 → 电脑的 1883
# 应用设置里把服务器填 127.0.0.1、端口 1883，保存并重连即可收发
```

后续可演进：Gradle 壳 + Kotlin 前台服务（真正的 FGS 常驻通知，保活等级再上一档）、
开机自启、接入厂商推送通道。

## Broker 建议

| 方案 | 适用 |
| --- | --- |
| broker.emqx.io（公共测试） | 快速试用，注意主题随机化 |
| 自建 mosquitto（VPS/家庭服务器） | 推荐，`allow_anonymous false` + 账号密码，私聊级隐私 |
| EMQX Cloud 免费额度 | 免运维，支持 TLS（8883） |

自建 mosquitto 最小配置（`/etc/mosquitto/conf.d/alert.conf`）：

```
listener 8883
certfile /etc/mosquitto/certs/fullchain.pem
keyfile  /etc/mosquitto/certs/privkey.pem
allow_anonymous false
password_file /etc/mosquitto/passwd
```

应用里填：服务器地址、端口 8883、勾选 TLS；自签证书则把 CA 的 PEM 路径写进
`config.json` 的 `ca_path`（手机端放在应用 files 目录下并在配置文件里写绝对路径）。

## 常见问题

- **收不到提醒**：先点「测试提醒」确认本机通道；再核对两端 Topic 是否一致、Broker 地址端口是否正确。
- **息屏不亮屏**：Android 14+ 需在通知设置里开启全屏意图；锁屏密码界面下部分 ROM 只显示横幅。
- **锁屏后一段时间收不到**：检查电池白名单与厂商后台白名单；确认「后台保活」开关已打开。
- **桌面端没有系统通知**：Windows 通知依赖应用身份注册，失败时会退化为蜂鸣提示音，不影响 Android。

# Vial 集成：可行性、冲突规避与真机验证计划

> 状态：2026-11-22，仅模拟器与自动化测试，未接触真实键盘。
> 分支/worktree：`spike/vial-backend` @ `.worktrees/vial-spike`
> 目标：**让 zmk-tui 用同一套操作逻辑编辑 Vial 键盘的键位**，不是另做一个 Vial 编辑器。

## 1. 结论

| 问题 | 结论 |
| --- | --- |
| 协议层能不能接？ | **能，风险低。** Vial 是 32 字节定长 raw HID，`0xFE` 前缀扩展；读定义、读/写整个动态键位表都很直接。 |
| 能不能复用现在的 UI？ | **能，且已用模拟器验证。** 仍是同一个 `App`、同一套按键/弹窗/保存/丢弃/恢复流程；Vial 只是第二个设备后端。 |
| 键位模型冲突大不大？ | **中等，但可控。** 用 14 个伪 behavior 把 QMK 16 位键码投影到现有 `Binding` 模型；无法结构化的键码一律保留原始值，读改写不会丢数据。 |
| "Vial 不太稳定"是不是真的？ | **协议本身很稳；生态版本耦合才是风险。** Vial 固件和 GUI 对协议版本、QMK 键码编号有硬耦合；GUI 更新慢；只走 USB；宏/Tap Dance/Combos 等是另一套扩展。 |
| 今天做到哪一步？ | 模拟器端到端可编辑、可保存、可丢弃、可恢复；连接页自动识别 Studio / Vial；默认停留设备列表，可选 `--auto` 单候选自动连接；编辑器内按 `b` 可带未保存确认回到设备列表；105 个库测试 + 47 个 TUI 测试 + 13 个 Vial 集成测试通过，完整质量门通过。 |
| 明天真机先做什么？ | 只读探测（版本/UID/矩阵/定义/层数）→ `.vil` 备份 → 改一个空闲键 → 读回校验 → 恢复。清单见 §9。 |

**可行性评级：**

- 协议与传输：**高**（一天内可跑通，已有实现）。
- 同 UI 编辑 Vial 键位：**高**（模拟器已验证；真机差异主要在固件行为）。
- Vial 全功能对齐（宏、Tap Dance、Combos、Key Override、QMK Settings、编码器、灯效）：**低 / 不在本期范围**。
- 长期稳定性：**中**。不是协议会崩，而是版本、固件分支、键码编号和 GUI 生态会漂移；必须做版本门禁、原值保留和备份。

## 2. "同一套操作逻辑"具体指什么

没有新增 Vial 专用界面。连接成功后，Vial 键盘和 ZMK Studio 键盘进入同一个 `EditorState`：

- `Enter` 打开同一个 binding 编辑器；
- behavior 列表来自同一个 `Catalog`，只是内容换成 14 个 Vial 伪 behavior；
- 参数选择复用同一个 `ParamKind`（`HidUsage` / `LayerId` / `Range`）；
- `s` 保存、`d` 丢弃、`R` 恢复出厂、`q` 退出、未保存提示，全部走原逻辑；
- 只有"层管理"和"物理布局切换"通过 capability 隐藏/禁用，因为 Vial 的层数是固件编译期固定的。

连接页也不区分两套流程：USB 面板同时列出 `名称 (ZMK Studio)`（CDC-ACM 串口）和 `名称 (Vial)`（带 Vial serial magic 的 raw HID）两类候选，选中哪一行就用对应后端连接。若同一把键盘同时暴露两种接口，会出现两行，标准切换就是换一行选择，不需要改构建或换命令。

用户默认不需要关心标准：启动后停在设备列表，USB/HID 候选先出现，蓝牙扫描结果随后追加；每行标注 `(ZMK Studio)` / `(Vial)`，由用户决定连哪个。蓝牙扫描需要几秒，提前自动连 USB 会让人看不到蓝牙候选，所以自动连接改为显式 `--auto`：只在恰好一个候选时尝试一次，失败回到列表，不会在后台无限重试。

进入编辑器后按 `b` 随时回到设备列表。有未保存修改时会先确认：`s` 保存后返回、`d` 丢弃后返回、`b` 直接返回、`Esc` 取消。返回会释放当前串口/BLE/raw HID，并重新扫描设备；这样设置好一把键盘后可以直接选下一把。

设备差异被收口在 `DeviceClient` 一个接口后面：

```text
App / TUI / modals / render          （未改操作逻辑）
        │
        ▼
DeviceClient  ──────────────┬──────────────
   │                        │
StudioClient            VialClient
   │                        │
ZMK Studio RPC          VialKeyboard（暂存、校验、能力）
                            │
                        VialDevice（32 字节命令）
                            │
                  USB raw HID / SimulatedVial
```

> 关于 trait：当前 `DeviceClient` 是 enum 静态分派，方法签名与 Studio 客户端一一对应，等价于一个封闭 trait。它已经满足"同一套逻辑"，且不需要 `async_trait`/对象安全改造。若以后要接第三个后端，或希望插件式扩展，可以原位换成 `#[async_trait] trait Device` + `Arc<dyn Device>`；TUI 调用点（`EditorState.client`）不需要再动。这是有意保留的演进点，不是现在的阻塞项。

## 3. 今天已经实现并验证的部分

新增（全部是增量文件，Studio 路径零改动）：

| 文件 | 作用 |
| --- | --- |
| `src/device.rs` | `DeviceClient`：Studio / Vial 两个后端的统一方法面 + capability |
| `src/vial/protocol.rs` | VIA/Vial 命令、32 字节报文、版本握手、定义分页、解锁、宏计数 |
| `src/vial/definition.rs` | 解析固件内嵌的 `vial.json`（KLE 布局、旋转簇、矩阵、去重、编码器计数） |
| `src/vial/keycode.rs` | QMK 16 位键码 ↔ `Binding` 的双向映射、14 个伪 behavior、v5/v6 键码方言 |
| `src/vial/session.rs` | 读整个键位表、暂存编辑、保存（含读回校验）、丢弃、恢复、能力声明 |
| `src/vial/client.rs` | `VialClient`：把 Vial 错误映射成现有 `StudioError`，接进 UI |
| `src/vial/simulate.rs` | 内存 Vial 键盘（模拟 via.c/vial.c、EEPROM、解锁手势） |
| `src/vial/vil.rs` | 最小 `.vil` 备份导出/恢复（键位部分） |
| `examples/vial_probe.rs` | 只读诊断/真机探测工具（不是编辑器入口） |
| `tests/vial.rs` | 协议、会话、`.vil`、TUI 连接层集成测试 |

修改的既有文件（冲突面）：

| 文件 | 改动 |
| --- | --- |
| `src/lib.rs` | `pub mod device; pub mod vial;` |
| `src/tui/connection.rs` | `Connected.client` 类型改为 `DeviceClient`；新增 `UsbEntry` 分类、`connect_vial`、按路径打开 Vial |
| `src/tui/app.rs` | `StartOptions.vial/scan_vial`；`EditorState.client/capabilities`；启动分支；USB 候选分类；`L` 层管理禁用提示；提示文案 |
| `src/main.rs` | `--vial` / `--simulate-vial`；`--list` 增加 Vial 设备 |
| `src/transport/mod.rs` | `ConnectionKind::{Vial, VialSimulator}` |
| `src/tui/render.rs` | USB 面板标题改为 `USB / HID`，空状态提示两种标准 |
| `Cargo.toml` / `Cargo.lock` | `lzma-rs`、`serde_json`、`hidapi` |
| `tests/tui.rs` | 3 个 Vial TUI 流程测试 + 1 个 Studio/Vial 候选分类测试 |

没有改：`client.rs`、`protocol/frame.rs`、`transport/serial.rs`、`transport/ble.rs`、`simulator.rs`、`tui/modals.rs`、`tui/keyboard.rs`、`keycodes/`；`tui/render.rs` 只动了连接页标题和空状态。

## 4. Vial 协议要点（已对源码核对）

一手来源：`vial-kb/vial-qmk` 的 `quantum/via.h`、`quantum/via.c`、`quantum/vial.h`、`quantum/vial.c`，以及 `vial-kb/vial-gui` 的 `protocol/keyboard_comm.py`。

- HID 接口：usage page **0xFF60**、usage **0x61**；固定 **32 字节**报文，写一条读一条，无帧头/长度/请求 ID。
- 设备发现：HID serial number 含 `vial:f64c2b3c`（`build_vial.mk` 固定定义）；或 VID/PID 命中 VIA 定义库。Vial GUI 两者都认。本实现还支持对没有 serial magic 的 `0xFF60/0x61` 接口做一次只读 Vial ID 探测，以覆盖部分 2.4G 接收器/第三方构建。
- 版本：
  - VIA 协议：`0x01` 返回大端 16 位；`vial-qmk` 是 **0x0009**，上游 QMK 已到 **0x000D**（`via.h`）。
  - Vial 协议：`0xFE 0x00` 返回小端 32 位 + 8 字节 UID；当前固件是 **6**。
- 动态键位表：
  - 层数 `0x11`；
  - 单键读写 `0x04`/`0x05`（layer, row, col, 大端 u16）；
  - 整表分块 `0x12`/`0x13`（offset 大端 u16、单次最多 **28** 字节；按 layer → row → col 大端排列）；
  - 复位 `0x06`（恢复编译期 keymap）。
- 键盘定义：固件内嵌 **XZ 压缩** 的 `vial.json`；`0xFE 0x01` 取长度，`0xFE 0x02` 按 32 字节页取数据。布局的 KLE 第一个 legend 是 `"row,col"`。
- 解锁：`0xFE 0x05` 返回 `[unlocked, in_progress, row0, col0, ...]`；`0xFE 0x06` 开始，用户按住矩阵键，`0xFE 0x07` 轮询。**动态键位写入本身不要求解锁**；宏写入、bootloader、RESET 键码才受保护（官方 Security 文档）。
- 宏/Tap Dance/Combos/Key Override/QMK Settings：`0x0C..0x13`、`0xFE 0x0D` 等独立命令，本期不编辑，原样保留。

**键码方言**是最大的隐藏坑：Vial 协议 6 换用了 QMK 0.24+ 的新编号，协议 ≤5 是旧编号。例如：

| 常量 | 旧（≤5） | 新（6） |
| --- | --- | --- |
| `QK_MOD_TAP` | 0x6000 | 0x2000 |
| `QK_TO` | 0x5000 | 0x5200 |
| `QK_MOMENTARY` | 0x5100 | 0x5220 |
| `QK_TOGGLE_LAYER` | 0x5300 | 0x5260 |
| `QK_MACRO` | 0x7700（嵌在旧 mod-tap 区间内） | 0x7700 |

`vial-gui` 自己就维护 `keycodes_v5.py` / `keycodes_v6.py` 两套表。本实现按 `vial_protocol` 选择 `Dialect`；这也是真机探测必须记录版本号的原因。

## 5. 键位映射：怎么做到"不丢键"

QMK 一个格子是一个 16 位键码；现有模型是 `{behavior_id, param1, param2}`。映射表：

| QMK 范围 | 伪 behavior | 说明 |
| --- | --- | --- |
| `KC_NO` / `KC_TRNS` | None / Transparent | 精确 |
| `0x04..=0xA4`、`0xE0..=0xE7` | Key Press | HID keyboard page，取值为 ZMK 的 `0x0007xxxx` |
| `0xA5..=0xBE` 常见系统/媒体键 | Key Press | QMK 紧凑编号 → HID consumer usage 查表（静音/音量/播放/亮度等） |
| `QK_MODS`（修饰键+键） | Modified Key | mods 掩码 + 内层键码，精确 |
| `QK_MOD_TAP` | Mod-Tap (Vial) | mods 掩码 + tap 键，精确 |
| `QK_LAYER_TAP` | Layer-Tap | layer + tap 键，精确 |
| `QK_TO` / `QK_MOMENTARY` / `QK_TOGGLE_LAYER` / `QK_ONE_SHOT_LAYER` | To/Momentary/Toggle/Sticky Layer | 层 id，精确 |
| `QK_ONE_SHOT_MOD` | Sticky Modifier | mods 掩码，精确 |
| `QK_BOOTLOADER` / `QK_REBOOT` | Bootloader / Reset | 精确 |
| 其余（Tap Dance、宏、Key Override、Swap Hands、鼠标、自定义 `QK_KB`…） | Vial Keycode（Raw） | 原样保留 16 位，不解释、不重写 |

`tests/vial.rs` 有一条穷举测试：**两个方言下 0..=65535 每个键码 decode→encode 都必须回到原值**。这是"第三方编辑器不会悄悄改坏键位"的核心保证。

代价：Raw 键码在 UI 里显示成 `0x1234`，只能当数字改，不能结构化编辑。这是有意的取舍——宁可少编辑，不可乱改。

## 6. 冲突分析：会跟什么冲突，怎么避开

### 6.1 与上游/未来 Studio 改动的合并冲突

冲突面被压到 10 个既有文件、约 380 行（其中一半是测试）。协议、串口/BLE 传输、弹窗没动；渲染只改了连接页文案。即便如此，`app.rs`/`connection.rs` 仍是上游活跃区，未来 rebase 可能冲突。规避策略：

1. **不复制 Studio 逻辑**：不新建第二套 App/弹窗/保存状态机；
2. **错误类型不扩散**：`VialError` 在 `VialClient` 边界映射成现有 `StudioError`，`Msg` 一个都没加；
3. **能力而不是分支**：UI 只问 `capabilities()`，不写 `if is_vial` 散落各处；
4. **新代码在新文件**：`src/vial/**`、`src/device.rs` 可整体摘除；
5. **USB HID 依赖常驻**：自动识别需要 `hidapi` 才能枚举 raw HID；它只在连接页扫描时使用，不影响 Studio 路径；
6. 若上游大改 `client.rs`，只需改 `DeviceClient::Studio` 的转发，不动 Vial 模块。

### 6.2 与现有 Studio 行为的语义冲突

| 维度 | Studio | Vial | 处理 |
| --- | --- | --- | --- |
| 保存 | 编辑立即下发，`s` 持久化 | 固件逐键写 EEPROM，没有事务 | 我们在内存暂存，`s` 才写，写完逐键读回校验；`d` 重新读设备 |
| 锁定 | 锁定后拒绝编辑 | 键位写入仍允许；宏/bootloader 才受限 | 对 UI 报告 Unlocked，不弹 Studio 解锁框；probe 仍会打印固件锁定状态 |
| 层 | 可增删改名/移动，有 free slots | 层数编译期固定，无名字 | `manage_layers=false`，`L` 提示"fixed layer count"；层显示为 Layer 0..N |
| 物理布局 | 固件可报多套并切换 | 定义里只有一套，还有 layout options 复用矩阵格 | 只暴露一套；重复矩阵格去重并 warning；暂不编辑 layout options |
| 恢复出厂 | 恢复 Studio 设置 | `0x06` 只恢复键位，不碰宏/Tap Dance | 提示"Stock settings restored"沿用，但实际只复位键位；文档写明 |
| 编码器 | 协议不暴露 | Vial 有 `0xFE 03/04` | 本期不暴露，只统计数量并 warning |

### 6.3 运行时冲突

- **HID 独占**：同一时刻只能有一个进程打开 raw HID；Vial GUI/浏览器 WebHID/我们的 TUI 会互相抢。真机测试前先退出 Vial GUI。
- **多键盘/多接口**：连接页会列出所有候选；但 `--vial` 快捷方式仍取"第一个 serial 含 `vial:` 的接口，否则第一个 0xFF60/0x61 接口"。多把 Vial 键盘同时插入时用连接页选择，或先用 `--list` 确认。
- **平台权限**：macOS 可能要求终端有输入监控/蓝牙权限；Linux 需要 udev 规则（Vial 文档有）；Windows 未验证。
- **BLE 不可能**：Vial 的编辑通道是 USB raw HID；本实现也只做了 USB。不要把"无线 Vial"写进预期。
- **2.4G 接收器不一定透传 raw HID**：键盘通过 2.4G dongle 连接时，主机看到的可能只是普通键盘 HID 接口；如果 dongle 没有把 usage page `0xFF60`/usage `0x61` 的 Vial 端点桥接出来，任何主機工具（包括 Vial GUI）都改不了键位。先用 `vial_probe --hid` 看是否存在该接口；没有就必须有线连接。若 dongle 透传了 raw HID 但没有 Vial serial magic，本实现会自动做一次只读 `vial_get_keyboard_id` 探测兜底。

### 6.4 测试/质量门冲突

- 现有门禁要求 CRAP、覆盖率、变异测试、Kani。新增约 3.3k 行会拉低覆盖率、改变 CRAP 报告，因此做了两类调整：
  - 把 `src/vial/transport.rs`（hidapi FFI）和 `src/tui/connection.rs`（硬件连接分发）加入 CRAP 排除，和既有 `src/transport/**` 同一先例；行覆盖率仍然全量统计。
  - 把 `tests/vial.rs` 加入门禁的集成测试步骤，并把变异测试的 target dir 隔离到 `target/quality/mutants-target`，避免 cargo-mutants 恢复源码后留下陈旧的变异产物。
- `tools/quality-gate.sh` 已完整通过：覆盖率 **82.69%**（阈值 79%）、CRAP >30 **0**、CRAP >100 **0**、Kani 418 checks 通过、变异测试 19/20（95%，阈值 90%）。

## 7. "Vial 不稳定"到底指什么

先说结论：**这次集成里，Vial 的协议层不是主要风险；版本耦合和生态节奏才是。**

已核对的事实：

1. **版本门禁很硬**：Vial GUI 只接受 VIA 协议 **9** 和 Vial 协议 **0..=6**；上游 QMK 已到 VIA **0x000D**。用错版本不是降级，是直接拒绝。
2. **键码编号变过**：协议 6 重排了 QK_MOD_TAP/QK_TO 等；旧固件 + 新主机表会"显示对、写错键"。本实现按协议选表，未知版本直接报错，不猜。
3. **Vial 是 QMK 分支**：`vial-qmk` 的 `vial` 分支仍在维护（2026-07 仍有键盘合入），但它不是上游 QMK；VIA/Vial 命令集与 QMK 主线并不完全同步。
4. **GUI 节奏慢**：截至 2026-11-22，Vial GUI 最新 release 是 **v0.7.5（2025-08-02）**；固件仓库同期仍活跃。GUI 与固件不是同一节奏发布，"看起来不稳定"很多来自这种错位。
5. **只支持 raw HID/USB**：没有 BLE 配置通道；无线键盘要连线。
6. **写入是逐键 EEPROM**：固件没有事务；中途拔线可能只写了一半。Vial GUI 也是这个模型。我们的缓解是暂存 + 读回校验 + `.vil` 备份，但**不能提供原子性**。
7. **安全模型**：默认锁定；键位可写，宏/bootloader 受限。第三方工具如果不懂这一点，会误判"设备不响应"。

风险表：

| 风险 | 概率 | 影响 | 缓解 |
| --- | --- | --- | --- |
| 固件 Vial 协议 <5 或 >6 | 低 | 无法连接 | 明确报错；不猜测键码表 |
| 键码方言判断错 | 低 | 写错键 | 按协议选 v5/v6；穷举 round-trip 测试 |
| 定义 JSON 超出 KLE 子集 | 中 | 布局缺失/错位 | 支持 KLE 旋转簇（`r`/`rx`/`ry`），旋转键按旋转中心 x、原始行 y 归一化为 1u；去重 + warning；真机核对 |
| 锁定/宏写入被拒 | 中 | 保存失败 | 只编辑键位；读回校验；错误原样提示 |
| 中途断线 | 中 | 部分键已写 | 备份 + 重试 + `R` 恢复；文档写明无事务 |
| GUI/固件版本错位 | 中 | 显示/写入不一致 | 记录 VIA/Vial 版本；必要时用 Vial GUI 交叉验证 |
| 多设备选错 | 中 | 改错键盘 | `--list` 先确认；后续加选择界面 |
| 编码器/宏用户预期 | 高 | "怎么不能改" | UI warning + 文档明确边界 |

## 8. 今日模拟验证结果

```text
$ cargo run --example vial_probe
Vial probe — in-process simulator (no hardware)
  VIA 0x0009, Vial 6, UID 5649414C44454D4F
  definition "Simulated Vial 4x6": 4x6 matrix, 24 keys, 0 encoder(s), 0 duplicate cell(s)
  layers: 4  lock: locked  unsaved: false
  ...
  staged, unsaved=true
  saved, keyboard now reports 0x0005
  Backup written to .../zmk-tui-vial-backup.vil
  Reset simulator to stock: position 0 is 0x0004
```

TUI 模拟器路径（`--simulate-vial`）由 `tests/tui.rs` 覆盖：

- 连接后进入同一个编辑器，显示 `VIAL SIM`、`Simulated Vial 4x6`、4 层；
- `Enter` → "Key Press" → `B` → Review 显示 `&kp B` → 应用 → `s` 保存成功；
- 未保存时 `d` 丢弃，键位回到 `A`；
- `L` 不打开层管理，提示 "fixed layer count"。

当前测试统计：库 105、client 3、TUI 47、Vial 13、cucumber 4；`clippy -D warnings` 与 `fmt` 通过；默认构建即包含真实 Vial USB 传输。

## 9. 明日真机验证清单

> 目标顺序：**只读 → 备份 → 单键写 → 读回 → 恢复**。每一步都可回退。

### 9.1 构建与枚举

```bash
cd /Users/zhouye/projects/zmk/.worktrees/vial-spike
cargo build --release
cargo run --example vial_probe -- --list
```

期望：出现 usage page `0xFF60`/usage `0x61` 的接口；serial 含 `vial:f64c2b3c`（`is_vial=true`）。
若列表为空：固件可能只是 VIA 而不是 Vial，或系统权限/线材/接口不对；先别改键。

### 9.2 只读探测

```bash
cargo run --example vial_probe -- --real
```

记录并核对：

- [ ] VIA 协议 = `0x0009`（或 0x000A..0x000D，我们的兼容窗口）
- [ ] Vial 协议 = 6（若是 5，键码方言会切到旧表）
- [ ] UID 与 Vial GUI 显示/`vial.json` 一致
- [ ] 矩阵 rows×cols、物理键数、层数
- [ ] 前 12 个键的 label 与 Vial GUI 一致（尤其 LT/MT/媒体键）
- [ ] warnings：重复矩阵格、编码器数量、Raw 键码数量、锁定状态
- [ ] 若是 2.4G 接收器，先跑 `vial_probe --hid`，确认 usage `0xFF60/0x61` 存在；不存在就换有线
- [ ] 备份文件已写到 `/tmp/zmk-tui-vial-backup.vil`，复制到安全位置

### 9.3 单键写入

先退出 Vial GUI（raw HID 独占），然后：

```bash
cargo run -- --vial
```

- [ ] 选一个**空闲/可恢复**的键，改成明显不同的键码（例如 `A` → `B`）
- [ ] `s` 保存；观察是否出现 "Saved to keyboard"（内部已逐键读回校验）
- [ ] 在文本编辑器里按键验证；或重新打开 probe 确认 keycode 变化
- [ ] `R` 恢复出厂键位，再验证一次
- [ ] 若保存失败：记录错误；用 Vial GUI 解锁（按住矩阵键）后重试；仍失败则用 `.vil`/Vial GUI 恢复

### 9.4 锁定与宏边界

- [ ] 冷启动（默认锁定）时改键是否成功（预期：成功，因为固件只保护宏/bootloader）
- [ ] 如果失败，先用 Vial GUI 执行解锁手势，再重试
- [ ] 确认本工具**不会**写宏/Tap Dance/Combos/QMK Settings（代码层面未调用）

### 9.5 `.vil` 备份/恢复

```bash
# 恢复（probe 会暂存、保存并读回校验）
cargo run --example vial_probe -- --real --restore /path/to/backup.vil
```

- [ ] 用本工具导出的 `.vil` 能被 Vial GUI 打开/恢复（我们写的是 `0xNNNN` 十六进制键码，Vial 的 AnyKeycode 接受）
- [ ] 若 Vial GUI 拒绝，记录报错；我们只导出键位，宏/动态条目字段是空的，属于预期限制

### 9.6 异常场景

- [ ] 保存过程中拔线：记录哪些键已写；重连后 probe 读取；用备份恢复
- [ ] 同时打开 Vial GUI：确认 HID 独占导致的连接失败提示是否可理解
- [ ] 多把键盘同时插入：`--list` 确认 `--vial` 选中的是哪一把（当前是"第一个"，可能需要后续加选择）
- [ ] 中文/特殊键位：确认 Raw 键码显示为 `0xNNNN`，不被误改

### 9.7 记录模板

```text
键盘/固件：
VID:PID：
Vial 固件版本/构建：
VIA 协议：         Vial 协议：
矩阵：       层数：        编码器：
锁定状态：      UID：
定义解析：keys=   duplicates=   encoders=
Raw 键码数量/样例：
保存是否成功：   读回是否一致：
异常/恢复过程：
```

## 10. 已知限制（本期明确不做）

1. **宏、Tap Dance、Combos、Key Override、QMK Settings**：不读不写，键位里的引用键码原样保留。
2. **编码器**：只统计，不显示/不编辑。
3. **物理布局切换 / layout options**：只显示定义里的第一套；重复矩阵格去重。
4. **无线**：Vial 走 USB raw HID；BLE 无解。
5. **连接界面**：Vial 已并入 USB 候选列表，默认停留列表等用户选择；没有热插拔监听，设备插拔后按 `r` 重新扫描。`--vial` 快捷方式只打开第一个候选，`--auto` 是显式自动连接。
6. **保存原子性**：逐键写 + 读回校验；断线可能部分写入。
7. **`.vil`**：只覆盖键位，不含宏/动态条目；只接受数字键码（本工具导出的格式）。
8. **平台**：macOS 已编译验证；Linux/Windows 仅理论上 hidapi 可用，未测试。
9. **质量门**：完整 `tools/quality-gate.sh` 已通过（覆盖率、CRAP、Kani、变异）；但它验证的是主机协议/模拟器，不等于真机固件行为。
10. **跨标准热切换**：`b` 可带未保存确认回到设备列表重新选择（含换标准），但没有不离开编辑器、原地热切换后端的快捷键。
11. **自动连接是 opt-in**：默认不自动进入，避免在蓝牙扫描完成前抢连 USB；`--auto` 且只有一个候选时才自动连接，多候选仍停在列表。

## 11. 代码地图与回滚

```text
src/device.rs                 统一后端接口（Studio/Vial）
src/vial/protocol.rs          32 字节协议 + 版本门禁
src/vial/definition.rs        vial.json/KLE 解析
src/vial/keycode.rs           QMK ↔ Binding（v5/v6，穷举 round-trip）
src/vial/session.rs           读表、暂存、保存校验、丢弃、恢复、capability
src/vial/client.rs            Vial → StudioError 适配
src/vial/simulate.rs          内存设备（测试/演示）
src/vial/vil.rs               .vil 键位备份
examples/vial_probe.rs        真机只读/写入探测（非编辑器入口）
tests/vial.rs                 端到端测试
```

回滚：整个分支可以丢弃；若只想关掉功能，删除 `src/vial/`、`src/device.rs`，把 `Connected.client` 改回 `StudioClient`，移除 `main.rs` 的两个 flag 即可。Studio 路径没有行为变化。

## 12. 参考

- Vial 固件协议：<https://github.com/vial-kb/vial-qmk/blob/vial/quantum/via.h>、<https://github.com/vial-kb/vial-qmk/blob/vial/quantum/vial.h>、<https://github.com/vial-kb/vial-qmk/blob/vial/quantum/vial.c>
- Vial GUI 协议实现：<https://github.com/vial-kb/vial-gui/blob/main/src/main/python/protocol/keyboard_comm.py>
- Vial 安全模型：<https://get.vial.today/docs/security.html>
- Vial 键盘定义：<https://get.vial.today/docs/porting-to-via.html>
- Vial GUI 版本：<https://github.com/vial-kb/vial-gui/releases>
- Vial-QMK 提交：<https://github.com/vial-kb/vial-qmk/commits/vial>
- 上游 QMK VIA 协议：<https://github.com/qmk/qmk_firmware/blob/master/quantum/via.h>
- Vial 键码表 v5/v6：<https://github.com/vial-kb/vial-gui/tree/main/src/main/python/keycodes>

# Keyphore Windows 版实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 构建一个通过 Codex Hooks 驱动 NuPhy Air75 V3 有线主键区灯光的 Windows Tauri 2 托盘应用。

**Architecture:** Tauri 主进程是唯一 HID 持有者，Rust 共享核心负责协议、状态、设置和诊断；独立的 `keyphore-hook.exe` 只从标准输入接收事件并原子更新状态文件。Vue 3 前端仅调用受限 Tauri Command，不直接访问 HID 或文件系统。

**Tech Stack:** Tauri 2、Rust、Vue 3、TypeScript、pnpm、`hidapi`、Tauri Autostart、Windows WebView2

**Spec:** `docs/superpowers/specs/2026-10-08-keyphore-windows-design.md`

**2026-10-14 实施更新：** 本计划保留原始阶段划分，不代表所有步骤已执行。主键区亮度地址 1 的旧限制已由实机结果纠正；Hook 改用 `.codex-plugin/plugin.json` 和 10 类官方事件。Task 5 源码与用例已编写，Task 6 已完成启停、串行同步和恢复源码接线，新增部分尚未编译或运行验收。当前行为及已知边界以 [Codex 联动说明](../../codex-status-integration.md) 为准，不执行本计划中的自动提交或全局安装步骤。

**当前范围修订：** 用户要求五种全局状态（失败、权限等待、执行、全部完成、空闲），四种颜色分别自定义，侧灯 / 主键区 / 两者可选；空闲与停止恢复原灯效。页面采用无边框毛玻璃窗口和平台风格控制按钮。下文的仅主键区、旧配色与完成熄灭要求保留为历史计划，不用于覆盖当前需求；分区保护改为每次只写目标分区并校验另一分区不变。

## Global Constraints

- 支持 Windows 10/11 x64，不申请管理员权限，不安装驱动或 Windows Service。
- 首版只支持 NuPhy Air75 V3 原厂固件、USB 有线连接、USB ID `19f5:1028`。
- Tauri 主进程是唯一 HID 持有者；Hook 程序不得打开键盘或启动界面。
- 首次灯光写入必须由用户主动触发；只读探测不得改变键盘状态。
- 所有灯光写入必须回读验证，并保证侧边灯状态不变。
- 新增或修改的方法添加简体中文 JSDoc；Rust 公共接口添加简体中文文档注释。
- 不保存 Codex 提示词、正文、工具输入或工具输出，不上传遥测。
- 自动更新、多语言、无线连接和多键盘并行不在首版范围。

## Review Focus

- Windows 将控制集合报告为不同接口编号时，只读列出候选项并拒绝猜测写入；Task 3 覆盖。
- 多把相同键盘或多个控制接口同时存在时拒绝写入；Task 3 覆盖。
- Hook 同时写入、锁超时或状态文件损坏时不阻塞 Codex且不丢失有效状态；Task 5 覆盖。
- HID 写入成功但回读不匹配或侧边灯变化时停止后续写入并尝试熄灯；Task 4 覆盖。
- USB 断连、休眠唤醒或程序退出时释放句柄，重连后只重放当前聚合状态；Task 6 覆盖。

---

## 文件结构

- `package.json`、`pnpm-lock.yaml`：前端和 Tauri CLI 依赖。
- `src/`：Vue 设置界面、类型和 Tauri API 包装。
- `src-tauri/src/lib.rs`：Tauri 初始化、托盘、命令注册和运行时装配。
- `src-tauri/src/bin/keyphore-hook.rs`：Codex Hook 独立入口。
- `src-tauri/src/protocol.rs`：64 字节 NuPhy 报文及会话协议。
- `src-tauri/src/device.rs`：Air75 V3 身份与候选接口选择。
- `src-tauri/src/hid.rs`：Windows HID 枚举、收发和回读。
- `src-tauri/src/lighting.rs`：Air75 主键区状态、颜色和侧边灯保护。
- `src-tauri/src/status.rs`：Hook 事件、持久状态和聚合逻辑。
- `src-tauri/src/storage.rs`：`%LOCALAPPDATA%\Keyphore`、文件锁与原子 JSON 写入。
- `src-tauri/src/runtime.rs`：设备重连、状态同步和退出清理。
- `src-tauri/src/settings.rs`：配置模型、校验和持久化。
- `src-tauri/src/diagnostics.rs`：隐私安全的设备与协议诊断。
- `src-tauri/tests/`：协议、设备、状态、Hook、运行时集成测试。
- `plugin/`：Codex Plugin manifest、八个 Hook 定义及打包脚本。

### Task 1: 建立可编译的 Tauri 工作区

**Files:**
- Create: `package.json`
- Create: `pnpm-lock.yaml`
- Create: `src/**`
- Create: `src-tauri/Cargo.toml`
- Create: `src-tauri/tauri.conf.json`
- Create: `src-tauri/src/lib.rs`
- Create: `.gitignore`

**Interfaces:**
- Consumes: Node.js 24、pnpm 11、Rust stable、Windows C++ Build Tools、WebView2。
- Produces: `pnpm tauri dev` 可启动的 Vue 3 + TypeScript + Tauri 2 基础应用。

- [ ] **Step 1: 检查并安装缺失的 Rust stable 与 Windows Tauri 构建前置依赖**

  执行前单独取得用户授权；安装后记录 `rustc --version`、`cargo --version` 和 MSVC 工具链可用性。

- [ ] **Step 2: 使用官方 Vue + TypeScript 模板在当前仓库根目录生成 Tauri 2 项目**

  应用名使用 `Keyphore`，bundle identifier 使用 `com.barrywu.keyphore.windows`，包管理器使用 pnpm。

- [ ] **Step 3: 添加 Autostart 和 Single Instance 插件并设置最小权限**

  前端只允许调用本项目声明的 Commands 和 Autostart 的查询、启用、停用命令。

- [ ] **Step 4: 运行基础检查**

  Run: `pnpm build` 和 `pnpm tauri build --debug`
  Expected: Vue 类型检查通过，生成 Windows debug bundle。

- [ ] **Step 5: 提交**

  Run: `git add . && git commit -m "chore: scaffold Tauri Windows app"`

### Task 2: 移植 NuPhy 协议和 Air75 灯光模型

**Files:**
- Create: `src-tauri/src/protocol.rs`
- Create: `src-tauri/src/lighting.rs`
- Create: `src-tauri/tests/protocol_tests.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: 无。
- Produces: `NuPhyRequest::{read, write}`、`NuPhySessionKey`、`validate_session_response`、`validate_response`、`Air75MainState`、`merge_main_preserving_side`。

- [ ] **Step 1: 写协议失败测试**

  覆盖 64 字节报文、56 字节载荷上限、校验和、会话挑战响应、地址/长度/会话不匹配，以及 Air75 主状态 9 字节与受保护侧边状态 8 字节。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test protocol_tests`
  Expected: 因协议类型尚不存在而编译失败。

- [ ] **Step 3: 实现最小协议与灯光纯函数**

  Air75 状态长度固定为 17；写入地址为 0；不得执行地址 1 的额外亮度镜像写入。

- [ ] **Step 4: 验证测试通过**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test protocol_tests`
  Expected: 全部通过。

- [ ] **Step 5: 提交**

  Run: `git add src-tauri && git commit -m "feat: add Air75 protocol core"`

### Task 3: 实现 Windows Air75 只读探测

**Files:**
- Create: `src-tauri/src/device.rs`
- Create: `src-tauri/src/hid.rs`
- Create: `src-tauri/tests/device_tests.rs`
- Create: `src-tauri/src/bin/keyphore-probe.rs`
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 2 的协议解码和会话验证。
- Produces: `enumerate_air75_candidates() -> Result<Vec<DeviceDescriptor>>`、`select_control_interface(&[DeviceDescriptor]) -> Result<DeviceDescriptor>`、`probe_read_only() -> Result<ProbeReport>`。

- [ ] **Step 1: 写设备选择失败测试**

  覆盖准确身份、无线设备、未知产品名、多个候选、接口 3 正常匹配，以及接口属性变化时返回只读诊断而不选择写入目标。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test device_tests`
  Expected: 因设备接口尚不存在而编译失败。

- [ ] **Step 3: 实现 Windows `hidapi` 枚举和只读探测 CLI**

  只匹配 VID `0x19f5`、PID `0x1028`、允许的两个产品名和 USB 总线；探测只发送临时会话请求和状态读取请求，不发送写请求。

- [ ] **Step 4: 运行自动测试与本机只读探测**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test device_tests`
  Expected: 全部通过。

  Run: `cargo run --manifest-path src-tauri/Cargo.toml --bin keyphore-probe`
  Expected: 输出唯一控制接口和 17 字节状态，或输出所有候选接口及明确拒绝原因；不得改变灯光。

- [ ] **Step 5: 用户确认探测结果后提交**

  Run: `git add src-tauri && git commit -m "feat: add read-only Air75 probe"`

### Task 4: 实现实机灯光测试与安全回读

**Files:**
- Modify: `src-tauri/src/hid.rs`
- Modify: `src-tauri/src/lighting.rs`
- Create: `src-tauri/tests/lighting_tests.rs`
- Modify: `src-tauri/src/bin/keyphore-probe.rs`

**Interfaces:**
- Consumes: Task 2 协议、Task 3 唯一控制接口。
- Produces: `Air75Controller::{read_state, apply_and_verify, exercise}`。

- [ ] **Step 1: 写灯光失败测试**

  覆盖蓝、橙、绿、熄灭，写后回读不匹配、侧边灯变化、某一步失败后尝试熄灯，以及 Air75 不写亮度镜像地址。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test lighting_tests`
  Expected: 因控制器尚不存在而编译失败。

- [ ] **Step 3: 实现 `Air75Controller` 和显式 `--exercise` 参数**

  默认命令保持只读；只有 `--exercise` 才依次写入蓝、橙、绿、熄灭，每一步均回读并保护侧边状态。

- [ ] **Step 4: 运行自动测试并请求用户执行实体灯光确认**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test lighting_tests`
  Expected: 全部通过。

  Run: `cargo run --manifest-path src-tauri/Cargo.toml --bin keyphore-probe -- --exercise`
  Expected: 四步回读成功，用户确认实体颜色正确且侧边灯未变化。

- [ ] **Step 5: 提交**

  Run: `git add src-tauri && git commit -m "feat: control Air75 status lighting"`

### Task 5: 实现 Codex Hook 与持久状态

**Files:**
- Create: `src-tauri/src/status.rs`
- Create: `src-tauri/src/storage.rs`
- Create: `src-tauri/src/bin/keyphore-hook.rs`
- Create: `src-tauri/tests/status_tests.rs`
- Create: `src-tauri/tests/hook_tests.rs`
- Create: `plugin/.codex-plugin/plugin.json`
- Create: `plugin/hooks/hooks.json`

**Interfaces:**
- Consumes: `%LOCALAPPDATA%\Keyphore`。
- Produces: `handle_hook(input: &[u8], store: &StatusStore) -> Result<()>`、`StatusCore::aggregate_at`、十类 Hook 对 `keyphore-hook.exe` 的调用定义。

- [x] **Step 1: 写状态和 Hook 失败测试**

  覆盖八类事件、1 MiB 输入上限、状态优先级、并发写入、100 ms 锁超时、完成状态到期、损坏 JSON 备份恢复，以及不持久化内容字段。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test status_tests --test hook_tests`
  Expected: 因状态接口尚不存在而编译失败。

- [x] **Step 3: 实现原子状态存储、聚合器、Hook 二进制和 Plugin 文件**

  Hook 快速失败时返回非零退出码；日志只记录事件类型和匿名状态标识。

- [ ] **Step 4: 验证测试通过**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test status_tests --test hook_tests`
  Expected: 全部通过。

- [ ] **Step 5: 提交**

  Run: `git add src-tauri plugin && git commit -m "feat: integrate Codex lifecycle hooks"`

### Task 6: 实现后台同步、重连和退出清理

**Files:**
- Create: `src-tauri/src/runtime.rs`
- Create: `src-tauri/tests/runtime_tests.rs`
- Modify: `src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: Task 4 `Air75Controller`、Task 5 `StatusStore` 和 `StatusCore`。
- Produces: `KeyphoreRuntime::{start, poll_once, handle_resume, shutdown}` 与线程安全的 `RuntimeSnapshot`。

- [ ] **Step 1: 写运行时失败测试**

  使用假 HID 覆盖仅状态变化时写入、每秒重连、断连释放、唤醒重枚举、重连重放当前状态、单实例持有和退出熄灯。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime_tests`
  Expected: 因运行时尚不存在而编译失败。

- [ ] **Step 3: 实现后台运行时并接入 Tauri 生命周期**

  状态轮询 100 ms，未连接设备重探测 1 秒；所有 HID 操作串行执行。

- [ ] **Step 4: 验证测试通过**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test runtime_tests`
  Expected: 全部通过。

- [ ] **Step 5: 提交**

  Run: `git add src-tauri && git commit -m "feat: add resilient lighting runtime"`

### Task 7: 实现设置、诊断、托盘和 Vue 界面

**Files:**
- Create: `src-tauri/src/settings.rs`
- Create: `src-tauri/src/diagnostics.rs`
- Create: `src-tauri/tests/settings_tests.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/App.vue`
- Create: `src/api.ts`
- Create: `src/types.ts`
- Create: `src/components/StatusPanel.vue`
- Create: `src/components/LightingSettings.vue`
- Create: `src/components/HardwareTest.vue`
- Create: `src/components/AppSettings.vue`
- Modify: `src/style.css`

**Interfaces:**
- Consumes: Task 6 `RuntimeSnapshot`、Task 4 灯光测试、Task 5 Plugin 状态。
- Produces: `get_snapshot`、`get_settings`、`save_settings`、`probe_keyboard`、`exercise_keyboard`、`install_hooks`、`remove_hooks`、`get_diagnostics` Tauri Commands。

- [ ] **Step 1: 写设置失败测试**

  覆盖亮度 1–100、完成时间 1–60 秒、无效颜色、损坏配置回退和最后有效配置保留。

- [ ] **Step 2: 验证测试失败**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml --test settings_tests`
  Expected: 因设置接口尚不存在而编译失败。

- [ ] **Step 3: 实现设置、诊断、Commands 和托盘行为**

  关闭窗口隐藏到托盘；托盘提供显示、灯光测试和安全退出；退出调用 `shutdown`。

- [ ] **Step 4: 实现四区 Vue 界面和简体中文 JSDoc**

  前端不引入 UI 组件库，使用原生表单控件和 CSS；灯光测试按钮必须展示将改变实体键盘的确认提示。

- [ ] **Step 5: 运行 Rust 与前端检查**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml` 和 `pnpm build`
  Expected: 全部测试和类型检查通过。

- [ ] **Step 6: 提交**

  Run: `git add src src-tauri package.json pnpm-lock.yaml && git commit -m "feat: add tray settings experience"`

### Task 8: 安装、端到端验证和私有 GitHub 仓库

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- Create: `README.md`
- Create: `docs/verification/windows-air75-v3.md`
- Modify: `.gitignore`

**Interfaces:**
- Consumes: Tasks 1–7 的完整应用。
- Produces: Windows 安装包、验证记录和私有 GitHub 仓库。

- [ ] **Step 1: 配置 Windows 安装包和 Hook 二进制随包交付**

  安装范围保持当前用户；卸载时移除本应用管理的 Hook 和启动项，不删除其他 Codex 配置。

- [ ] **Step 2: 执行完整自动验证**

  Run: `cargo test --manifest-path src-tauri/Cargo.toml`、`pnpm build`、`pnpm tauri build`
  Expected: 全部通过并生成安装包。

- [ ] **Step 3: 执行 Air75 V3 和 Codex 端到端人工验证**

  验证安装、只读探测、三色灯光、侧边灯保护、Hook 状态、USB 重连、开机启动、托盘退出和卸载；结果写入验证记录。

- [ ] **Step 4: 检查仓库边界并提交发布验证**

  Run: `git status --short`、`git diff --check`，确认无日志、用户路径、构建产物或凭据。

  Run: `git add README.md docs src-tauri/tauri.conf.json .gitignore && git commit -m "docs: add Windows release verification"`

- [ ] **Step 5: 创建私有 GitHub 仓库并推送**

  安装并登录 GitHub CLI 或使用用户提供的既有 GitHub 认证；创建 `Keyphore-Windows` 私有仓库，设置 `origin` 并推送当前分支。创建远程仓库前再次确认目标 GitHub 账户。

- [ ] **Step 6: 验证远程仓库**

  Run: `git status --short` 和 `git ls-remote origin HEAD`
  Expected: 工作区干净，远程仓库返回当前默认分支提交。

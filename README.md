<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="AirPulse icon" width="112" height="112" />
</p>

<h1 align="center">AirPulse</h1>

<p align="center"><strong>English</strong> | <a href="README.zh-CN.md">简体中文</a></p>

<p align="center">Let your keyboard lighting tell you what Codex is doing.</p>

AirPulse is a Windows desktop app that turns Codex task status into customizable RGB lighting on the NuPhy Air75 V3 keyboard. It combines tracked tasks across multiple chats into one global status and controls the side lights, main key area, or both over USB. You can customize status colors and brightness, then restore the original lighting when control ends.

Built with Tauri 2, Vue 3, TypeScript, and Rust, AirPulse features a frameless window, Windows-style window controls, and Acrylic frosted glass effects.

## Download and install

Download a Windows x64 installer from [GitHub Releases](https://github.com/Hegg5577/AirPulse/releases):

- `AirPulse_0.1.0_x64-setup.exe`: recommended installer.
- `AirPulse_0.1.0_x64_en-US.msi`: MSI installer.
- `SHA256SUMS.txt`: SHA256 checksums for the installers.

After installation, open AirPulse and use **Codex 联动** (Codex integration) at the top of the window to install and connect the plugin. UI labels below include their Chinese text so you can find the corresponding controls in the app.

## Features

- **Global task status**: combines multiple Codex chats, prioritizing tool failures and permission requests.
- **Custom status colors**: color picker and hexadecimal input, with a brightness slider and integer input.
- **Three lighting zones**: side lights, main key area, or both; defaults to side lights at 30% brightness.
- **Original lighting restoration**: restores the controlled zones when idle or when integration stops, leaving unselected zones unchanged.
- **One-click Codex plugin installation**: includes the required components, detects and reuses existing installations, and reports readiness after receiving a real event.
- **Advanced manual controls**: a separate dialog for reading lighting settings, setting static RGB across a zone, adjusting brightness, turning lights off, and restoring them.
- **Saved preferences**: stores colors, brightness, and selected zones locally.
- **System tray support**: minimize to the taskbar, or choose between hiding to the tray and exiting when closing the window.

## Global status rules

Rules are evaluated in the following priority order:

| Global status | Default color | Rule |
| --- | --- | --- |
| Tool failure | Red `#FF3B30` | Any task reports a recognizable tool failure |
| Waiting for permission | Red `#FF3B30` | No tool failure, but at least one task is waiting for permission |
| Running | Yellow `#FFD60A` | No exceptional condition, and at least one task is running |
| All completed | Green `#34C759` | All tasks still being tracked have completed |
| Idle | Original lighting | No active tasks; restore the lighting configuration from before control began |

The completion color remains until a new task or lifecycle event changes the status; it does not turn off on a timer. Tool failure detection relies on explicit structured failure flags in tool results and cannot identify every tool's error format.

## Before you start

- Use Windows and a NuPhy Air75 V3 connected over **USB**. Other keyboard models and wireless connections have not been verified.
- Wake the keyboard and close other software that may change its lighting at the same time.
- Use a Codex version that supports plugins and Hook approval. AirPulse can detect a native `codex.exe` on PATH and the CLI bundled with the Windows desktop app.

### Connect Codex

1. Open AirPulse and click **Codex 联动** (Codex integration) at the top.
2. Click **安装 Codex 联动** (Install Codex integration). An existing installation is reused. If the CLI cannot be found or installation fails, the dialog explains why.
3. Review and trust the Hook in Codex on first use. In the CLI, you can use `/hooks`. If the plugin has not loaded, reopen Codex.
4. Send a new message in Codex. Once the dialog receives a real event, it displays **联动已就绪** (Integration ready).
5. Return to the main screen, configure colors, brightness, and lighting zones, then enable integration.

Installing the plugin does not automatically enable lighting or approve Hook trust on your behalf. Codex may require another review when Hook definitions change. Integration readiness means a real event was received recently; it does not confirm a keyboard connection or hardware verification of every status.

### Control and restoration

While integration is enabled, lighting configuration and manual lighting writes are locked. Click **停止并恢复** (Stop and restore) before changing settings. If restoration fails, keep the original device and mode, then retry.

The title bar's **Minimize** button minimizes the window to the taskbar. Clicking **Close** or pressing `Alt+F4` opens a choice dialog. **隐藏到托盘** (Hide to tray) keeps enabled integration running; **退出 AirPulse** (Exit AirPulse) closes the app and attempts to restore the original lighting; **取消** (Cancel) or `Esc` keeps the window open. Click the tray icon, select **显示窗口** (Show window) from its menu, or launch AirPulse again to reopen the existing window. You can also right-click the tray icon and choose **退出 AirPulse** (Exit AirPulse) directly.

Restore points exist only within the current desktop process. A normal exit attempts to restore the lighting used before automatic integration; restoration cannot be guaranteed after force termination or exiting with the keyboard disconnected. For manual changes, use the restore button in the manual control dialog. Main key area updates may briefly pass through 100% brightness. Individual per-key colors are not currently supported.

## Local development

You need Node.js 22.12 or a newer compatible version, pnpm, stable Rust, Visual Studio C++ Build Tools, and WebView2. See the [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for the Windows build environment.

Run these commands from the repository root:

```powershell
pnpm install --frozen-lockfile
pnpm tauri dev
```

Only the desktop app can access the HID backend. Running `pnpm dev` alone provides a UI preview.

When developing from source for the first time, prepare the receiver before using plugin installation:

```powershell
cargo build --release --bin keyphore-hook --manifest-path src-tauri/Cargo.toml --locked
powershell -NoProfile -ExecutionPolicy Bypass -File plugin/package.ps1
```

After changing Rust code, window configuration, native materials, or backend commands, rebuild and restart the desktop process. Refreshing the page alone does not update the backend. Follow the same procedure if the UI reports an outdated backend.

## Build installers

```powershell
pnpm tauri build
```

The preparation script builds the frontend, compiles the release Hook receiver, and prepares the plugin files before Tauri builds the desktop app and installers. Packaging stops if any step fails.

Installers are written to `src-tauri/target/release/bundle/`. Plugin resources are bundled with the app and copied to the current user's application data directory on a fresh installation, without depending on the source checkout. The repository does not track `node_modules`, `dist`, Rust `target`, or plugin binaries.

Application icons are stored in `src-tauri/icons/`; the page and title bar use `public/airpulse.png`. Desktop icons and the README use the same AirPulse assets.

## Project structure

```text
src/                       Vue UI and integration preferences
public/                    Static frontend assets
src-tauri/src/             HID protocol, status aggregation, lighting, and plugin installation
src-tauri/src/bin/         Hook receiver and hardware diagnostics
src-tauri/tests/           Protocol, status, receiver, and lighting tests
src-tauri/icons/           Application and installer icons
plugin/                    Codex plugin manifest, Hook definitions, and forwarding scripts
.agents/plugins/           Local plugin source manifest
scripts/                   Release build preparation scripts
docs/                      Integration guides, design notes, and hardware test records
```

## Data and compatibility

The Hook stores only the identifiers and phases needed to aggregate status. It does not store prompts, chat content, tool arguments, or tool result bodies, and it does not access the keyboard directly.

AirPulse was renamed from this project's earlier Keyphore Windows development version. The following internal identifiers remain unchanged to preserve compatibility with existing installations and settings:

| Item | Compatibility identifier |
| --- | --- |
| Application identifier | `com.barrywu.keyphore.windows` |
| Codex plugin | `keyphore-windows@keyphore-local` |
| Hook receiver | `keyphore-hook.exe` |
| Status file | `%LOCALAPPDATA%\Keyphore\status.json` |
| UI preferences key | `keyphore.status-light.preferences.v1` |

You may therefore still see the old name in the Codex plugin list or local data paths. Existing installations linked to a development directory are reused without automatic migration or upgrades; keep their plugin source directory available. The rename does not add support for other keyboard models or macOS, and platform-specific UI styling does not imply platform support.

See the [Codex integration guide](docs/codex-status-integration.md) and [lighting hardware test records](docs/lighting-hardware-tests.md) for details; these documents are in Chinese. `docs/superpowers/` preserves the original design and implementation history. Where older names or plans differ, follow the current README.

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

type LightingZone = "main" | "side";
type DisplayZone = LightingZone | "both";
type TaskStatus = "idle" | "running" | "permissionWaiting" | "toolFailed" | "completed";
type ColorStatus = Exclude<TaskStatus, "idle">;

interface LightingView {
  handle: number;
  state: number[];
  mainRestoreAvailable: boolean;
  sideRestoreAvailable: boolean;
}

interface StatusColors {
  toolFailed: number[];
  permissionWaiting: number[];
  running: number[];
  completed: number[];
}

interface RuntimeSnapshot {
  enabled: boolean;
  status: TaskStatus;
  brightness: number;
  zone: DisplayZone;
  colors: StatusColors;
  error: string | null;
  restorePending: boolean;
  lastEventMs: number | null;
}

interface IntegrationStatus {
  cliAvailable: boolean;
  installed: boolean;
  enabled: boolean;
}

interface CodexEventStatus {
  lastEventMs: number | null;
  received: boolean;
}

interface Preferences {
  brightness: number;
  zone: DisplayZone;
  colors: Record<ColorStatus, string>;
}

// 保留原配置键与应用标识，改名后继续读取用户已保存的颜色、亮度和灯区。
const STORAGE_KEY = "keyphore.status-light.preferences.v1";
const appWindow = getCurrentWindow();
const isMac = /Mac/i.test(navigator.platform);
const automatic = ref<RuntimeSnapshot | null>(null);
const preferences = ref<Preferences>({
  brightness: 30,
  zone: "side",
  colors: {
    toolFailed: "#FF3B30",
    permissionWaiting: "#FF3B30",
    running: "#FFD60A",
    completed: "#34C759",
  },
});
const statusRows: { id: ColorStatus; label: string; description: string }[] = [
  { id: "toolFailed", label: "工具失败", description: "任一任务出现明确的结构化失败" },
  { id: "permissionWaiting", label: "权限等待", description: "至少一个任务等待权限" },
  { id: "running", label: "执行中", description: "至少一个任务仍在执行" },
  { id: "completed", label: "全部完成", description: "所有跟踪任务均已完成" },
];
const statusLabels: Record<TaskStatus, string> = {
  idle: "空闲",
  running: "执行中",
  permissionWaiting: "权限等待",
  toolFailed: "工具失败",
  completed: "全部完成",
};
const zones: { id: LightingZone; label: string; offset: number }[] = [
  { id: "main", label: "主键区", offset: 0 },
  { id: "side", label: "左右侧灯", offset: 9 },
];
const zoneOptions: { id: DisplayZone; label: string }[] = [
  { id: "side", label: "侧灯" },
  { id: "main", label: "主键区" },
  { id: "both", label: "侧灯 + 主键区" },
];
const keyboardKeys = Array.from({ length: 42 }, (_, index) => index);
const controls = ref({
  main: { color: "#0000FF", brightness: 30 },
  side: { color: "#00FF00", brightness: 30 },
});
const current = ref<LightingView | null>(null);
const busy = ref(false);
const error = ref("");
const message = ref("");
const statusError = ref("");
const maximized = ref(false);
const previewStatus = ref<TaskStatus>("running");
const manualDialog = ref<HTMLDialogElement | null>(null);
const integrationDialog = ref<HTMLDialogElement | null>(null);
const closeDialog = ref<HTMLDialogElement | null>(null);
const closingWindow = ref(false);
const closeWindowError = ref("");
const integration = ref<IntegrationStatus | null>(null);
const codexEvent = ref<CodexEventStatus | null>(null);
const checkingIntegration = ref(false);
const installingIntegration = ref(false);
const integrationError = ref("");
const eventError = ref("");
let statusTimer: ReturnType<typeof setInterval> | undefined;
let integrationTimer: ReturnType<typeof setInterval> | undefined;
let unlistenResize: (() => void) | undefined;
let unlistenClose: (() => void) | undefined;
let pollingStatus = false;
let pollingEvent = false;

const manualLocked = computed(() => automatic.value?.enabled || automatic.value?.restorePending);
const currentStatus = computed(() => automatic.value?.status ?? "idle");
const displayedStatus = computed(() => automatic.value?.enabled ? currentStatus.value : previewStatus.value);
const previewLit = computed(() => displayedStatus.value !== "idle" && preferences.value.brightness > 0);
const activeColor = computed(() => displayedStatus.value === "idle"
  ? "#8A939F"
  : preferences.value.colors[displayedStatus.value]);
const connection = computed(() => {
  if (automatic.value?.enabled) return "Codex 联动运行中";
  if (error.value) return "键盘状态待确认";
  if (!current.value) return "尚未读取键盘";
  return `Air75 V3 · USB · ${current.value.handle === 0 ? "Mac" : "Windows"} 模式`;
});
const selectedMain = computed(() => preferences.value.zone !== "side");
const selectedSide = computed(() => preferences.value.zone !== "main");
const integrationReady = computed(() => integration.value?.installed && integration.value.enabled
  && codexEvent.value?.received && !integrationError.value && !eventError.value);
const integrationLabel = computed(() => {
  if (installingIntegration.value) return "正在安装联动组件";
  if (checkingIntegration.value) return "正在检测 Codex";
  if (integrationError.value) return "联动安装状态待确认";
  if (!integration.value) return "等待读取安装状态";
  if (!integration.value.cliAvailable) return "未找到 Codex";
  if (!integration.value.installed) return "尚未安装联动组件";
  if (!integration.value.enabled) return "插件已安装，尚未启用";
  return integrationReady.value ? "联动已就绪" : "已安装，等待真实事件";
});

/** 安装使用独立弹窗，避免把首次配置流程挤进日常灯光控制页面。 */
async function openCodexIntegration() {
  integrationDialog.value?.showModal();
  await refreshCodexIntegration();
}

/** 关闭仅收起引导，已经开始的安装任务继续完成。 */
function closeCodexIntegration() {
  integrationDialog.value?.close();
}

/** 旧后台需要重新启动桌面进程，刷新页面无法增加安装命令。 */
function integrationErrorMessage(cause: unknown): string {
  const detail = String(cause);
  return /Command\s+(get_codex_integration|get_codex_event|install_codex_integration)\s+not found/i.test(detail)
    ? "当前后台尚未包含一键安装功能，请重新编译并启动桌面应用。"
    : detail;
}

/** 仅在打开引导或主动刷新时启动 CLI，日常轮询不反复创建进程。 */
async function refreshCodexIntegration() {
  if (checkingIntegration.value || installingIntegration.value) return;
  checkingIntegration.value = true;
  integrationError.value = "";
  try {
    integration.value = await invoke<IntegrationStatus>("get_codex_integration");
  } catch (cause) {
    integration.value = null;
    integrationError.value = integrationErrorMessage(cause);
  } finally {
    checkingIntegration.value = false;
  }
  await refreshCodexEvent();
}

/** 只读检测真实事件，独立于灯光开关，也不会用旧事件验证新安装。 */
async function refreshCodexEvent() {
  if (pollingEvent || checkingIntegration.value || installingIntegration.value
    || !integrationDialog.value?.open || !integration.value?.installed) return;
  pollingEvent = true;
  try {
    codexEvent.value = await invoke<CodexEventStatus>("get_codex_event");
    eventError.value = "";
  } catch (cause) {
    codexEvent.value = null;
    eventError.value = integrationErrorMessage(cause);
  } finally {
    pollingEvent = false;
  }
}

/** 安装由用户主动触发，成功只表示组件已安装，首次信任保留在 Codex 内完成。 */
async function installCodexIntegration() {
  if (checkingIntegration.value || installingIntegration.value
    || !integration.value?.cliAvailable || integration.value.installed) return;
  installingIntegration.value = true;
  integrationError.value = "";
  eventError.value = "";
  codexEvent.value = null;
  try {
    integration.value = await invoke<IntegrationStatus>("install_codex_integration");
  } catch (cause) {
    integrationError.value = integrationErrorMessage(cause);
  } finally {
    installingIntegration.value = false;
  }
  await refreshCodexEvent();
}

/** 使用原生模态对话框约束焦点，避免手动操作时误触页面联动开关。 */
function openManualControl() {
  manualDialog.value?.showModal();
}

/** 关闭仅收起界面，不重置输入或恢复灯效，焦点由原生对话框返回入口。 */
function closeManualControl() {
  manualDialog.value?.close();
}

/** 从本地恢复界面配置，异常或旧格式数据直接使用安全默认值。 */
function loadPreferences() {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null") as Partial<Preferences> | null;
    if (!saved || !["side", "main", "both"].includes(saved.zone ?? "")) return;
    if (!Number.isInteger(saved.brightness) || (saved.brightness ?? -1) < 0
      || (saved.brightness ?? 101) > 100) return;
    if (!saved.colors || statusRows.some(({ id }) => !/^#[0-9A-F]{6}$/i.test(saved.colors?.[id] ?? ""))) return;
    preferences.value = saved as Preferences;
  } catch {
    // 存储不可用时继续使用默认配置，避免阻断灯光控制界面。
    message.value = "无法读取本地配置，本次使用默认值。";
  }
}

/** 配置只保存非敏感的颜色、灯区和亮度，不写入任务内容。 */
function savePreferences() {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences.value));
  } catch {
    message.value = "配置本次可用，但无法保存到本地。";
  }
}

/** 将严格的十六进制颜色转换为后台命令需要的 RGB 数组。 */
function hexToRgb(color: string): number[] {
  return [1, 3, 5].map((index) => Number.parseInt(color.slice(index, index + 2), 16));
}

/** 用真实回读更新手动输入，避免把请求值显示为设备实际值。 */
function updateCurrent(view: LightingView) {
  current.value = view;
  for (const zone of zones) {
    const rgbOffset = zone.id === "main" ? 6 : 14;
    controls.value[zone.id] = {
      color: `#${view.state.slice(rgbOffset, rgbOffset + 3)
        .map((byte) => byte.toString(16).padStart(2, "0")).join("").toUpperCase()}`,
      brightness: view.state[zone.offset + 1],
    };
  }
}

/** 串行执行手动命令；失败后只读刷新，防止界面显示未落盘的值。 */
async function perform(command: string, args: Record<string, unknown>, success: string) {
  if (busy.value) return;
  busy.value = true;
  error.value = "";
  message.value = "";
  try {
    updateCurrent(await invoke<LightingView>(command, args));
    message.value = success;
  } catch (cause) {
    error.value = String(cause);
    try {
      updateCurrent(await invoke<LightingView>("read_lighting"));
    } catch {
      current.value = null;
    }
  } finally {
    busy.value = false;
  }
}

/** 读取不会写灯，可在唤醒键盘或重新连接 USB 后重试。 */
async function readLighting() {
  await perform("read_lighting", {}, "已读取键盘灯光参数");
}

/** 校验手动参数后设置单个整区，熄灭时仍保留用户所选颜色。 */
async function applyLighting(zone: LightingZone, off = false) {
  if (busy.value || manualLocked.value) return;
  const control = controls.value[zone];
  const brightness = off ? 0 : Number(control.brightness);
  if (!/^#[0-9A-F]{6}$/i.test(control.color) || !Number.isInteger(brightness)
    || brightness < 0 || brightness > 100) {
    error.value = "请输入 #RRGGBB 颜色和 0 到 100 的整数亮度。";
    return;
  }
  await perform("set_lighting", { zone, color: hexToRgb(control.color), brightness },
    "参数写入并回读成功");
}

/** 只恢复指定分区，保持另一分区当前灯效。 */
async function restoreLighting(zone: LightingZone) {
  if (manualLocked.value) return;
  await perform("restore_lighting", { zone }, "已恢复该区域修改前的灯效");
}

/** 轮询仅读取软件状态，旧后台缺少命令时明确提示重新编译，避免误报空闲。 */
async function refreshStatus() {
  if (pollingStatus) return;
  pollingStatus = true;
  try {
    automatic.value = await invoke<RuntimeSnapshot>("get_status");
    statusError.value = "";
  } catch (cause) {
    const detail = String(cause);
    statusError.value = /Command\s+get_status\s+not found/i.test(detail)
      ? "当前运行的后台版本过旧，请重新编译并启动桌面应用；仅刷新页面无法更新后台。"
      : `无法读取联动状态：${detail}`;
  } finally {
    pollingStatus = false;
  }
}

/** 校验配置后启停联动；停止会同步恢复所有被接管区域。 */
async function toggleAutomatic(start: boolean) {
  if (busy.value || (start && (!automatic.value || statusError.value))) return;
  const brightness = Number(preferences.value.brightness);
  if (start && (!Number.isInteger(brightness) || brightness < 0 || brightness > 100
    || statusRows.some(({ id }) => !/^#[0-9A-F]{6}$/i.test(preferences.value.colors[id])))) {
    error.value = "请检查状态颜色和 0 到 100 的整数亮度。";
    return;
  }
  busy.value = true;
  error.value = "";
  message.value = "";
  try {
    const colors = Object.fromEntries(statusRows.map(({ id }) => [id, hexToRgb(preferences.value.colors[id])]));
    automatic.value = await invoke<RuntimeSnapshot>(start ? "start_automatic" : "stop_automatic",
      start ? { brightness, zone: preferences.value.zone, colors } : {});
    message.value = start ? "状态灯联动已开启" : "联动已停止，原灯效已恢复";
  } catch (cause) {
    error.value = String(cause);
  } finally {
    await refreshStatus();
    busy.value = false;
  }
}

/** 恢复按钮只在当前设备和模式存在对应快照时可用。 */
function canRestore(zone: LightingZone) {
  return zone === "main" ? current.value?.mainRestoreAvailable : current.value?.sideRestoreAvailable;
}

/** 显示真实灯效编号，避免把动态灯效误标为静态。 */
function stateDescription(offset: number) {
  if (!current.value) return "等待读取";
  return `灯效 ${current.value.state[offset]} · ${current.value.state[offset + 1]}%`;
}

/** 标题栏空白区域拖动窗口，交互控件不会触发拖动。 */
async function dragWindow(event: MouseEvent) {
  if (event.button === 0 && event.target === event.currentTarget) await appWindow.startDragging();
}

/** 切换最大化并回读真实窗口状态。 */
async function toggleMaximize() {
  await appWindow.toggleMaximize();
  maximized.value = await appWindow.isMaximized();
}

/** 双击仅作用于标题栏空白，防止窗口按钮触发额外最大化。 */
function maximizeFromTitlebar(event: MouseEvent) {
  if (event.target === event.currentTarget) void toggleMaximize();
}

/** 每次关闭都让用户选择，重复关闭请求不重置当前弹窗或操作。 */
function openCloseDialog() {
  if (!closeDialog.value || closeDialog.value.open) return;
  closeWindowError.value = "";
  closeDialog.value.showModal();
}

/** 取消只收起选择弹窗，避免误操作导致窗口隐藏或联动停止。 */
function cancelWindowClose() {
  if (!closingWindow.value) closeDialog.value?.close();
}

/** 隐藏保留联动和灯效快照，退出交由后台正常清理；失败时保留弹窗供重试。 */
async function confirmWindowClose(action: "hide" | "exit") {
  if (closingWindow.value) return;
  closingWindow.value = true;
  closeWindowError.value = "";
  try {
    if (action === "hide") {
      await appWindow.hide();
      closeDialog.value?.close();
    } else {
      await invoke("exit_application");
    }
  } catch (cause) {
    closeWindowError.value = String(cause);
  } finally {
    closingWindow.value = false;
  }
}

/** 优先监听系统关闭请求，再读取配置与状态，让 Alt+F4 使用相同的选择弹窗。 */
onMounted(async () => {
  unlistenClose = await appWindow.onCloseRequested((event) => {
    event.preventDefault();
    openCloseDialog();
  });
  loadPreferences();
  maximized.value = await appWindow.isMaximized();
  unlistenResize = await appWindow.onResized(async () => {
    maximized.value = await appWindow.isMaximized();
  });
  await Promise.all([readLighting(), refreshStatus()]);
  statusTimer = setInterval(refreshStatus, 1000);
  integrationTimer = setInterval(refreshCodexEvent, 2000);
});

/** 离开页面释放轮询和窗口监听，后台联动生命周期不受页面影响。 */
onUnmounted(() => {
  if (statusTimer !== undefined) clearInterval(statusTimer);
  if (integrationTimer !== undefined) clearInterval(integrationTimer);
  unlistenResize?.();
  unlistenClose?.();
});

watch(preferences, savePreferences, { deep: true });
</script>

<template>
  <div class="window-shell" :class="{ mac: isMac, maximized }" :aria-busy="busy">
    <header class="titlebar" @mousedown="dragWindow" @dblclick="maximizeFromTitlebar">
      <div v-if="isMac" class="traffic-lights">
        <button class="traffic close" title="关闭" aria-label="关闭" @click.stop="openCloseDialog" />
        <button class="traffic minimize" title="最小化" aria-label="最小化" @click.stop="appWindow.minimize()" />
        <button class="traffic maximize" title="最大化" aria-label="最大化" @click.stop="toggleMaximize" />
      </div>
      <img class="brand-mark" src="/airpulse.png" alt="" draggable="false" />
      <span class="window-title">AirPulse</span>
      <div v-if="!isMac" class="window-controls">
        <button title="最小化" aria-label="最小化" @click.stop="appWindow.minimize()"><span class="min-icon" /></button>
        <button :title="maximized ? '还原' : '最大化'" :aria-label="maximized ? '还原' : '最大化'" @click.stop="toggleMaximize">
          <span :class="maximized ? 'restore-icon' : 'max-icon'" />
        </button>
        <button class="close-window" title="关闭" aria-label="关闭" @click.stop="openCloseDialog"><span>×</span></button>
      </div>
    </header>

    <div class="app-layout">
      <main>
        <section class="overview" aria-labelledby="overview-title">
          <div>
            <h1 id="overview-title">全局状态灯</h1>
            <p>{{ connection }}</p>
          </div>
          <div class="overview-actions">
            <button class="link-button" aria-haspopup="dialog" @click="openCodexIntegration">Codex 联动</button>
            <button class="link-button" aria-haspopup="dialog" @click="openManualControl">手动控制</button>
            <button class="link-button" :disabled="busy" title="重新读取键盘" @click="readLighting">
              <span class="refresh-icon">↻</span> 读取键盘
            </button>
          </div>
        </section>

        <p v-if="error || statusError || automatic?.error" class="notice error" role="alert">
          {{ error || statusError || automatic?.error }}
        </p>
        <p v-if="message" class="notice success" role="status">{{ message }}</p>

        <div class="keyboard-stage" :style="{ '--active-color': activeColor }">
          <div class="section-heading">
            <h2>灯区预览</h2>
            <span class="preview-badge">配置示意</span>
          </div>
          <div class="keyboard" :class="{ 'main-active': selectedMain && previewLit }">
            <div class="side-light left" :class="{ active: selectedSide && previewLit }" />
            <div class="key-grid">
              <span v-for="key in keyboardKeys" :key="key" class="key" />
            </div>
            <div class="side-light right" :class="{ active: selectedSide && previewLit }" />
          </div>
          <div class="preview-meta">
            <label class="preview-picker">
              状态
              <select v-if="!automatic?.enabled" v-model="previewStatus" aria-label="预览状态">
                <option v-for="row in statusRows" :key="row.id" :value="row.id">{{ row.label }}</option>
                <option value="idle">空闲 · 原灯效</option>
              </select>
              <span v-else>{{ statusLabels[currentStatus] }}</span>
            </label>
            <span>{{ zoneOptions.find((item) => item.id === preferences.zone)?.label }}</span>
            <span>{{ preferences.brightness }}% 亮度</span>
          </div>
          <div class="live-status" role="status">
            <span class="section-label">当前联动</span>
            <h3>{{ statusError ? '后台未就绪' : !automatic ? '正在连接后台' : automatic.enabled ? statusLabels[currentStatus] : '联动已停止' }}</h3>
            <p v-if="statusError">配置可以编辑，联动暂不可用</p>
            <p v-else-if="automatic?.restorePending && !automatic.enabled">请先恢复原灯效，再重新开启联动</p>
            <p v-else-if="!automatic?.enabled">开启右侧联动后，灯光自动跟随任务状态</p>
            <p v-else-if="currentStatus === 'idle'">没有活跃任务，键盘保持原灯效</p>
            <p v-else>正在监听 Codex，按最高优先级状态展示</p>
          </div>
        </div>

        <section class="settings" aria-labelledby="settings-title">
          <div class="section-heading">
            <div>
              <span class="section-label">联动规则</span>
              <h3 id="settings-title">状态映射</h3>
            </div>
            <button class="switch-row" role="switch" :aria-checked="automatic?.enabled ?? false"
              :disabled="busy || (!automatic?.enabled && (!automatic || !!statusError || !!automatic?.restorePending))"
              @click="toggleAutomatic(!automatic?.enabled)">
              <span>{{ automatic?.enabled ? "已开启" : "已关闭" }}</span>
              <span class="switch-track"><span /></span>
            </button>
          </div>

          <fieldset :disabled="busy || automatic?.enabled || automatic?.restorePending">
            <legend class="sr-only">状态灯配置</legend>
            <div class="setting-block">
              <label class="setting-label">展示区域</label>
              <div class="segmented" role="radiogroup" aria-label="展示区域">
                <button v-for="option in zoneOptions" :key="option.id" type="button" role="radio"
                  :aria-checked="preferences.zone === option.id"
                  :class="{ selected: preferences.zone === option.id }"
                  @click="preferences.zone = option.id">{{ option.label }}</button>
              </div>
            </div>

            <div class="setting-block brightness-block">
              <label for="brightness">状态亮度</label>
              <input id="brightness" v-model.number="preferences.brightness" type="range" min="0" max="100" step="1" />
              <label class="brightness-value">
                <input v-model.number="preferences.brightness" type="number" min="0" max="100" step="1"
                  aria-label="精确亮度百分比" />%
              </label>
            </div>

            <div class="color-list">
              <p class="priority-hint">优先级从上到下，出现多个状态时取最高项</p>
              <div v-for="row in statusRows" :key="row.id" class="color-row">
                <span class="color-swatch" :style="{ backgroundColor: preferences.colors[row.id] }" />
                <div class="color-copy">
                  <label :for="`${row.id}-color`">{{ row.label }}</label>
                  <span>{{ row.description }}</span>
                </div>
                <input :id="`${row.id}-color`" v-model="preferences.colors[row.id]" type="color"
                  :aria-label="`${row.label}颜色`" />
                <input v-model="preferences.colors[row.id]" class="hex-input" type="text" maxlength="7"
                  spellcheck="false" :aria-label="`${row.label}十六进制颜色`" />
              </div>
              <div class="color-row idle-row">
                <span class="color-swatch idle-swatch" />
                <div class="color-copy">
                  <span class="row-title">空闲</span>
                  <span>没有活跃任务时恢复接管前灯效</span>
                </div>
                <span class="original-effect">原灯效</span>
              </div>
            </div>
          </fieldset>

          <button v-if="automatic?.restorePending && !automatic.enabled" class="restore-action"
            :disabled="busy" @click="toggleAutomatic(false)">重试恢复原灯效</button>
        </section>
      </main>
    </div>

    <dialog ref="closeDialog" class="manual-dialog close-dialog" aria-labelledby="close-title"
      aria-describedby="close-description" :aria-busy="closingWindow" @cancel="closingWindow && $event.preventDefault()">
      <div class="section-heading">
        <h2 id="close-title">关闭 AirPulse</h2>
        <button class="dialog-close" aria-label="取消关闭" title="取消" :disabled="closingWindow"
          @click="cancelWindowClose">×</button>
      </div>
      <p id="close-description">你希望隐藏窗口继续运行，还是退出程序？</p>
      <p class="close-note">隐藏到托盘会保留已开启的灯光联动；退出将停止联动，并尝试恢复原灯效。</p>
      <p v-if="closeWindowError" class="notice error" role="alert">{{ closeWindowError }}</p>
      <div class="dialog-footer">
        <button class="link-button" :disabled="closingWindow" autofocus @click="cancelWindowClose">取消</button>
        <button class="link-button" :disabled="closingWindow" @click="confirmWindowClose('exit')">退出 AirPulse</button>
        <button class="link-button install-action" :disabled="closingWindow" @click="confirmWindowClose('hide')">隐藏到托盘</button>
      </div>
    </dialog>

    <dialog ref="integrationDialog" class="manual-dialog integration-dialog" aria-labelledby="integration-title"
      aria-describedby="integration-description" :aria-busy="checkingIntegration || installingIntegration">
      <div class="section-heading">
        <div>
          <span class="section-label">首次配置</span>
          <h3 id="integration-title">连接 Codex</h3>
        </div>
        <button class="dialog-close" aria-label="关闭 Codex 联动设置" title="关闭" autofocus
          @click="closeCodexIntegration">×</button>
      </div>
      <p id="integration-description">安装一次，让键盘跟随 Codex 的任务状态。首次信任需要你在 Codex 中确认。</p>
      <div class="integration-summary" :class="{ ready: integrationReady }" role="status">
        <span class="integration-dot" />
        <div>
          <strong>{{ integrationLabel }}</strong>
          <p v-if="integrationReady">已收到真实 Hook 事件，可返回主页开启灯光联动。</p>
          <p v-else-if="integration && !integration.cliAvailable">请安装或更新 Codex Windows 桌面版，然后重新检测。</p>
          <p v-else-if="integration?.installed && !integration.enabled">请在 Codex 的插件设置中启用 AirPulse 的联动插件（keyphore-windows），再重新检测。</p>
          <p v-else-if="integration?.installed">若已信任，在 Codex 中发一条新消息即可验证，无需重复安装。</p>
          <p v-else>自动准备组件并安装到当前用户，无需手动复制文件或运行命令。</p>
        </div>
      </div>
      <p v-if="integrationError || eventError" class="notice error" role="alert">{{ integrationError || eventError }}</p>
      <ol class="integration-steps">
        <li :class="{ complete: integration?.installed }">
          <span class="step-number">1</span>
          <div><h4>安装联动组件</h4><p>随应用提供本地插件，已安装时直接复用。</p></div>
          <span v-if="integration?.installed" class="step-result">已安装</span>
        </li>
        <li>
          <span class="step-number">2</span>
          <div><h4>在 Codex 中确认信任</h4><p>按 Codex 的 Hook 审核提示操作；CLI 可通过 <code>/hooks</code> 审核。若未加载插件，请重新打开 Codex。</p></div>
        </li>
        <li :class="{ complete: integrationReady }">
          <span class="step-number">3</span>
          <div><h4>发送一条消息</h4><p>在 Codex 中发送新消息，收到任务事件后，这里会自动显示“联动已就绪”。</p></div>
          <span v-if="integrationReady" class="step-result">已接收</span>
        </li>
      </ol>
      <p class="integration-note">安装不会自动开启灯光，也不会代替你信任 Hook。Hook 定义发生变化后，Codex 可能要求重新审核。</p>
      <p v-if="codexEvent?.lastEventMs" class="integration-note">最近事件：{{ new Date(codexEvent.lastEventMs).toLocaleString('zh-CN') }}</p>
      <div class="dialog-footer">
        <button class="link-button" :disabled="checkingIntegration || installingIntegration" @click="refreshCodexIntegration">
          {{ checkingIntegration ? '检测中…' : '重新检测' }}
        </button>
        <button class="link-button install-action"
          :disabled="checkingIntegration || installingIntegration || !integration?.cliAvailable || integration?.installed || !!integrationError"
          @click="installCodexIntegration">
          {{ installingIntegration ? '正在安装…' : integration?.installed ? '已安装' : '安装 Codex 联动' }}
        </button>
      </div>
    </dialog>

    <dialog ref="manualDialog" class="manual-dialog" aria-labelledby="manual-title" aria-describedby="manual-description">
      <div class="section-heading">
        <h2 id="manual-title">高级手动控制</h2>
        <button class="dialog-close" aria-label="关闭手动控制" title="关闭" autofocus @click="closeManualControl">×</button>
      </div>
      <p id="manual-description">用于独立验证整区 RGB，不修改全局状态配置。关闭弹窗不会恢复灯效。</p>
      <p v-if="manualLocked" class="notice error" role="status">联动或恢复期间不可手动写灯，请先停止联动并恢复原灯效。</p>
      <p v-if="error" class="notice error" role="alert">{{ error }}</p>
      <p v-if="message" class="notice success" role="status">{{ message }}</p>
      <div class="manual-zones">
        <fieldset v-for="zone in zones" :key="zone.id" :disabled="busy || !current || manualLocked">
          <legend>{{ zone.label }} <small>{{ stateDescription(zone.offset) }}</small></legend>
          <div class="manual-inputs">
            <input v-model="controls[zone.id].color" type="color" :aria-label="`${zone.label}颜色`" />
            <input v-model="controls[zone.id].color" type="text" maxlength="7" spellcheck="false"
              :aria-label="`${zone.label}十六进制颜色`" />
            <input v-model.number="controls[zone.id].brightness" type="number" min="0" max="100"
              :aria-label="`${zone.label}亮度`" />
          </div>
          <div class="manual-actions">
            <button class="primary-action" @click="applyLighting(zone.id)">应用</button>
            <button @click="applyLighting(zone.id, true)">熄灭</button>
            <button :disabled="!canRestore(zone.id)" @click="restoreLighting(zone.id)">恢复</button>
          </div>
        </fieldset>
      </div>
      <div class="dialog-footer">
        <button class="link-button" :disabled="busy" @click="readLighting">读取键盘</button>
        <button class="link-button" @click="closeManualControl">完成</button>
      </div>
    </dialog>
  </div>
</template>

<style>
:root {
  font-family: "Segoe UI Variable", "Segoe UI", "Microsoft YaHei", sans-serif;
  color: #15191f;
  background: transparent;
  font-synthesis: none;
  text-rendering: optimizeLegibility;
  letter-spacing: 0;
}

* { box-sizing: border-box; }
html, body, #app { width: 100%; height: 100%; margin: 0; overflow: hidden; }
button, input { font: inherit; letter-spacing: 0; }
button { color: inherit; }

.window-shell {
  width: 100%;
  height: 100%;
  overflow: hidden;
  /* 桌面模糊由 Tauri 原生材质提供，低透明度底色避免盖住 Acrylic。 */
  background: rgba(235, 239, 243, 0.22);
  border: 1px solid rgba(255, 255, 255, 0.54);
  border-radius: 10px;
}
.window-shell.maximized { border-radius: 0; }

.titlebar {
  position: relative;
  z-index: 10;
  height: 40px;
  display: flex;
  align-items: center;
  padding-left: 14px;
  background: rgba(255, 255, 255, 0.24);
  color: #252b33;
  user-select: none;
}

.brand-mark {
  width: 24px;
  height: 24px;
  display: block;
  object-fit: contain;
  border-radius: 5px;
  pointer-events: none;
}

.window-title { margin-left: 9px; font-size: 12px; font-weight: 600; pointer-events: none; }
.window-controls { height: 100%; margin-left: auto; display: flex; }
.window-controls button {
  width: 46px;
  height: 100%;
  border: 0;
  background: transparent;
  color: #252b33;
  display: grid;
  place-items: center;
}
.window-controls button:hover { background: rgba(38, 47, 59, 0.08); }
.window-controls .close-window:hover { background: #c42b1c; color: #fff; }
.min-icon { width: 10px; height: 1px; background: currentColor; }
.max-icon { width: 10px; height: 10px; border: 1px solid currentColor; }
.restore-icon { position: relative; width: 9px; height: 9px; border: 1px solid currentColor; }
.restore-icon::after { content: ""; position: absolute; width: 8px; height: 8px; top: -4px; right: -4px; border-top: 1px solid currentColor; border-right: 1px solid currentColor; }
.close-window span { font-size: 18px; line-height: 1; font-weight: 300; }

.traffic-lights { display: flex; gap: 8px; margin-right: 14px; }
.traffic { width: 13px; height: 13px; padding: 0; border: 0; border-radius: 50%; }
.traffic.close { background: #ff5f57; }
.traffic.minimize { background: #febc2e; }
.traffic.maximize { background: #28c840; }

.app-layout { height: calc(100% - 40px); }
.section-label { font-size: 10px; line-height: 1; font-weight: 700; color: #586574; }
main {
  height: 100%;
  min-width: 0;
  overflow: auto;
  padding: 22px 24px;
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1.15fr);
  align-content: start;
  gap: 16px;
  scrollbar-color: #aeb5bd transparent;
}
.overview { grid-column: 1 / -1; display: flex; align-items: center; justify-content: space-between; gap: 20px; }
.overview h1 { margin: 0 0 6px; font-size: 27px; line-height: 1.2; font-weight: 680; }
.overview p { margin: 0; color: #4f5c6b; font-size: 12px; }
.overview-actions { display: flex; align-items: center; gap: 8px; }
.link-button {
  height: 34px;
  padding: 0 12px;
  border: 1px solid rgba(71, 80, 90, 0.18);
  border-radius: 6px;
  background: rgba(255, 255, 255, 0.52);
  cursor: pointer;
}
.refresh-icon { display: inline-block; margin-right: 4px; font-size: 16px; }

.keyboard-stage, .settings {
  min-width: 0;
  padding: 18px;
  border: 1px solid rgba(255, 255, 255, 0.62);
  border-radius: 14px;
  background: rgba(255, 255, 255, 0.38);
  box-shadow: 0 8px 28px rgba(31, 44, 61, 0.06), inset 0 1px rgba(255, 255, 255, 0.6);
}
.keyboard-stage { display: flex; flex-direction: column; }
.section-heading h2 { margin: 0; font-size: 16px; font-weight: 650; }
.preview-badge { padding: 4px 8px; border: 1px solid rgba(71, 80, 90, 0.12); border-radius: 20px; color: #586574; font-size: 10px; }
.keyboard {
  position: relative;
  width: 100%;
  height: 150px;
  margin: 38px auto 0;
  padding: 13px 19px;
  border: 1px solid rgba(36, 43, 50, 0.23);
  border-radius: 8px;
  background: rgba(35, 40, 47, 0.92);
  box-shadow: 0 20px 36px rgba(22, 28, 34, 0.18), inset 0 1px rgba(255, 255, 255, 0.08);
}
.key-grid { height: 100%; display: grid; grid-template-columns: repeat(12, 1fr); grid-template-rows: repeat(4, 1fr); gap: 4px; }
.key {
  min-width: 0;
  border-radius: 4px;
  background: #5b636d;
  border: 1px solid #6e7782;
  box-shadow: inset 0 -3px rgba(0, 0, 0, 0.19);
}
.keyboard.main-active .key { background: color-mix(in srgb, var(--active-color) 76%, #2b3037); border-color: var(--active-color); box-shadow: 0 0 8px color-mix(in srgb, var(--active-color) 60%, transparent); }
.side-light { position: absolute; top: 24px; bottom: 24px; width: 5px; border-radius: 2px; background: #68717b; }
.side-light.left { left: 7px; }
.side-light.right { right: 7px; }
.side-light.active { background: var(--active-color); box-shadow: 0 0 15px 3px var(--active-color); }
.preview-meta { width: 100%; margin: 16px auto 0; display: flex; flex-wrap: wrap; align-items: center; justify-content: space-between; gap: 10px; color: #4f5c6b; font-size: 11px; }
.preview-picker { display: flex; align-items: center; gap: 7px; }
.preview-picker select { border: 0; border-radius: 4px; padding: 2px 4px; background: rgba(255, 255, 255, 0.55); color: #414951; font: inherit; }
.live-status { margin-top: auto; padding-top: 36px; }
.live-status h3 { margin: 8px 0; font-size: 24px; font-weight: 650; }
.live-status p { margin: 0; color: #4f5c6b; font-size: 12px; line-height: 1.6; }

.notice { grid-column: 1 / -1; margin: 0; padding: 10px 12px; border-radius: 8px; font-size: 12px; overflow-wrap: anywhere; }
.notice.error { color: #8d2018; background: rgba(255, 223, 218, 0.72); border: 1px solid rgba(196, 43, 28, 0.16); }
.notice.success { color: #22623a; background: rgba(220, 245, 227, 0.72); border: 1px solid rgba(52, 199, 89, 0.18); }

.section-heading { display: flex; align-items: center; justify-content: space-between; gap: 18px; }
.section-heading h3 { margin: 5px 0 0; font-size: 18px; font-weight: 650; }
.switch-row { display: flex; align-items: center; gap: 9px; border: 0; background: transparent; cursor: pointer; font-size: 12px; }
.switch-track { width: 34px; height: 19px; padding: 2px; border-radius: 10px; background: #8c959e; transition: background 160ms ease; }
.switch-track span { display: block; width: 15px; height: 15px; border-radius: 50%; background: #fff; transition: transform 160ms ease; }
.switch-row[aria-checked="true"] .switch-track { background: #20784a; }
.switch-row[aria-checked="true"] .switch-track span { transform: translateX(15px); }
fieldset { min-width: 0; margin: 0; padding: 0; border: 0; }
.setting-block { margin-top: 18px; }
.setting-label, .brightness-block label { display: block; margin-bottom: 9px; color: #414951; font-size: 12px; font-weight: 600; }
.segmented { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); padding: 3px; border-radius: 7px; background: rgba(114, 124, 134, 0.13); }
.segmented button { min-height: 34px; border: 0; border-radius: 5px; background: transparent; color: #69727c; cursor: pointer; font-size: 12px; }
.segmented button.selected { background: rgba(255, 255, 255, 0.88); color: #171b20; box-shadow: 0 1px 4px rgba(27, 33, 39, 0.13); }
.brightness-block { display: grid; grid-template-columns: 90px 1fr 76px; align-items: center; gap: 12px; }
.brightness-block label { margin: 0; }
input[type="range"] { width: 100%; accent-color: #2b333c; }
.brightness-value { display: flex !important; align-items: center; gap: 4px; color: #4f5862; font-variant-numeric: tabular-nums; }
.brightness-value input { width: 58px; height: 30px; padding-left: 6px; border: 1px solid rgba(71, 80, 90, 0.18); border-radius: 5px; background: rgba(255, 255, 255, 0.58); font-size: 12px; }
.color-list { margin-top: 16px; border-top: 1px solid rgba(71, 80, 90, 0.11); }
.priority-hint { margin: 12px 0 4px; color: #586574; font-size: 10px; }
.color-row { min-height: 52px; display: grid; grid-template-columns: 10px 1fr 30px 80px; align-items: center; gap: 10px; border-bottom: 1px solid rgba(71, 80, 90, 0.1); }
.color-swatch { width: 11px; height: 31px; border-radius: 3px; box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.09); }
.color-copy { min-width: 0; display: flex; flex-direction: column; gap: 3px; }
.color-copy label, .row-title { font-size: 12px; font-weight: 650; }
.color-copy span { color: #586574; font-size: 10px; white-space: normal; }
.color-row input[type="color"] { width: 30px; height: 30px; padding: 2px; border: 1px solid rgba(71, 80, 90, 0.18); border-radius: 5px; background: rgba(255, 255, 255, 0.6); cursor: pointer; }
.hex-input { width: 80px; height: 30px; padding: 0 8px; border: 1px solid rgba(71, 80, 90, 0.18); border-radius: 5px; background: rgba(255, 255, 255, 0.58); font-family: "Cascadia Mono", Consolas, monospace; font-size: 11px; text-transform: uppercase; }
.idle-row { grid-template-columns: 10px 1fr auto; border-bottom: 0; }
.idle-swatch { background: #89929c; }
.original-effect { color: #5f6973; font-size: 11px; }
.restore-action { margin-top: 16px; min-height: 34px; padding: 0 13px; border: 0; border-radius: 5px; background: #8d2018; color: #fff; cursor: pointer; }

.manual-dialog {
  width: min(660px, calc(100% - 48px));
  max-height: calc(100% - 80px);
  padding: 24px;
  overflow: auto;
  border: 1px solid rgba(255, 255, 255, 0.8);
  border-radius: 16px;
  background: rgba(239, 243, 248, 0.94);
  color: #15191f;
  box-shadow: 0 24px 80px rgba(15, 23, 36, 0.28);
  backdrop-filter: blur(20px);
}
.manual-dialog::backdrop { background: rgba(17, 25, 38, 0.28); backdrop-filter: blur(5px); }
.manual-dialog > p { margin: 12px 0 16px; font-size: 12px; line-height: 1.6; }
.manual-dialog > #manual-description { color: #586574; }
.dialog-close { width: 32px; height: 32px; border: 0; border-radius: 6px; background: transparent; color: #586574; font-size: 24px; cursor: pointer; }
.dialog-close:hover { background: rgba(38, 47, 59, 0.08); }
.dialog-footer { display: flex; justify-content: flex-end; gap: 8px; margin-top: 22px; }
.integration-dialog { width: min(600px, calc(100% - 48px)); }
.close-dialog { width: min(480px, calc(100% - 48px)); }
.close-dialog > .close-note { color: #586574; }
.integration-summary { display: flex; gap: 12px; align-items: flex-start; margin: 18px 0; padding: 16px; border-radius: 10px; background: rgba(114, 124, 134, 0.1); }
.integration-summary.ready { background: rgba(52, 199, 89, 0.1); }
.integration-dot { width: 9px; height: 9px; margin-top: 5px; flex-shrink: 0; border-radius: 50%; background: #87919c; }
.integration-summary.ready .integration-dot { background: #20784a; }
.integration-summary strong { font-size: 14px; font-weight: 650; }
.integration-summary p { margin: 6px 0 0; color: #586574; font-size: 12px; line-height: 1.6; }
.integration-steps { margin: 20px 0; padding: 0; list-style: none; }
.integration-steps li { display: flex; align-items: flex-start; gap: 12px; padding: 14px 0; border-bottom: 1px solid rgba(71, 80, 90, 0.1); }
.integration-steps li > div { flex: 1; min-width: 0; }
.step-number { display: grid; place-items: center; flex-shrink: 0; width: 26px; height: 26px; border: 1px solid rgba(71, 80, 90, 0.2); border-radius: 50%; color: #586574; font-size: 12px; }
.complete .step-number { background: #20784a; color: white; border-color: #20784a; }
.integration-steps h4 { margin: 3px 0 5px; font-size: 13px; font-weight: 650; }
.integration-steps p { margin: 0; color: #586574; font-size: 12px; line-height: 1.6; }
.step-result { padding-top: 5px; color: #20784a; font-size: 11px; white-space: nowrap; }
.integration-dialog > .integration-note { margin: 8px 0; color: #586574; font-size: 11px; }
.install-action { background: #29313a; color: #fff; border-color: #29313a; }
.manual-zones { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 18px; margin-top: 20px; }
.manual-zones fieldset { padding: 14px; border: 1px solid rgba(71, 80, 90, 0.14); border-radius: 10px; }
.manual-zones legend { width: 100%; margin-bottom: 11px; font-size: 12px; font-weight: 650; }
.manual-zones small { float: right; color: #7c858e; font-weight: 400; }
.manual-inputs { display: grid; grid-template-columns: 34px 1fr 62px; gap: 7px; }
.manual-inputs input { min-width: 0; height: 32px; padding: 0 7px; border: 1px solid rgba(71, 80, 90, 0.2); border-radius: 5px; background: rgba(255, 255, 255, 0.6); }
.manual-inputs input[type="color"] { width: 34px; padding: 2px; }
.manual-actions { margin-top: 9px; display: flex; gap: 7px; }
.manual-actions button { min-height: 30px; padding: 0 9px; border: 1px solid rgba(71, 80, 90, 0.2); border-radius: 5px; background: rgba(255, 255, 255, 0.55); cursor: pointer; font-size: 11px; }
.manual-actions .primary-action { background: #29313a; color: #fff; border-color: #29313a; }

button:disabled, fieldset:disabled { opacity: 0.48; cursor: not-allowed; }
button:focus-visible, input:focus-visible, select:focus-visible { outline: 2px solid #1473e6; outline-offset: 2px; }
.sr-only { position: absolute; width: 1px; height: 1px; padding: 0; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }

@media (max-width: 930px) {
  main { padding: 22px; grid-template-columns: minmax(0, 0.85fr) minmax(0, 1.15fr); gap: 14px; }
  .keyboard-stage, .settings { padding: 16px; }
  .keyboard { height: 130px; }
  .color-row { gap: 7px; }
}

@media (max-width: 760px) {
  main { grid-template-columns: 1fr; padding: 20px; }
  .keyboard { max-width: 460px; }
  .manual-zones { grid-template-columns: 1fr; }
}

@media (max-height: 700px) {
  main { padding: 16px 20px; gap: 12px; }
  .keyboard-stage, .settings { padding: 16px; }
  .setting-block { margin-top: 14px; }
  .color-list { margin-top: 12px; }
  .color-row { min-height: 44px; }
  .priority-hint { margin-top: 8px; }
  .keyboard { height: 130px; margin-top: 24px; }
  .live-status { padding-top: 24px; }
}
</style>

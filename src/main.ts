import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./style.css";

type InteractionMode = "hold" | "toggle";
type RuntimePhase =
  | "idle"
  | "connecting"
  | "listening"
  | "finalizing"
  | "inserting"
  | "complete"
  | "error";

type AppConfig = {
  app_id: string;
  secret_key: string;
  shortcut: string;
  interaction_mode: InteractionMode;
  microphone: string;
  auto_insert: boolean;
  endpoint: string;
  resource_id: string;
};

type Microphone = {
  id: string;
  name: string;
  is_default: boolean;
};

type RuntimeSnapshot = {
  phase: RuntimePhase;
  transcript: string;
  message: string;
};

type LevelPayload = {
  level: number;
};

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("Missing #app root");

const windowLabel = getCurrentWindow().label;
if (windowLabel === "dictation" || new URLSearchParams(location.search).get("window") === "dictation") {
  void mountDictationOverlay(app);
} else {
  void mountSettings(app);
}

async function mountSettings(root: HTMLDivElement) {
  document.body.className = "settings-body";
  root.innerHTML = `
    <main class="shell">
      <header class="settings-header">
        <div class="brand-lockup">
          <div class="brand-mark" aria-hidden="true">
            <span></span><span></span><span></span><span></span><span></span>
          </div>
          <div class="brand-copy">
            <h1>Voice Flow</h1>
            <div class="compact-status">
              <span class="status-dot" id="status-dot"></span>
              <strong id="status-title">正在初始化</strong>
              <small id="status-detail">读取本地配置</small>
            </div>
          </div>
        </div>
        <button class="ghost-button" id="test-button" type="button">
          <span class="button-record-dot"></span>
          <span id="test-label">试说一次</span>
        </button>
      </header>

      <div class="signal-demo" aria-hidden="true">
        ${Array.from({ length: 19 }, (_, index) => `<i style="--i:${index}"></i>`).join("")}
      </div>

      <div class="content-scroll">
          <section class="panel credentials-panel">
            <div class="panel-heading">
              <div>
                <h3>连接语音服务</h3>
              </div>
              <span class="local-badge">仅存本机</span>
            </div>

            <div class="field-grid">
              <label class="field">
                <span>APP ID <em>旧版鉴权需要</em></span>
                <input id="app-id" type="text" autocomplete="off" placeholder="留空则使用 API Key 鉴权" />
              </label>
              <label class="field">
                <span>Secret Key / API Key</span>
                <div class="secret-field">
                  <input id="secret-key" type="password" autocomplete="off" placeholder="输入 Access Token 或 API Key" />
                  <button id="reveal-secret" type="button" aria-label="显示密钥">显示</button>
                </div>
              </label>
            </div>
            <p class="field-note">填写 APP ID 时按旧版 APP ID + Access Token 鉴权；留空则按新版 API Key 鉴权。</p>
          </section>

          <section class="panel input-panel">
            <div class="panel-heading compact">
              <div>
                <h3>说话方式</h3>
              </div>
            </div>

            <div class="input-layout">
              <label class="field microphone-field">
                <span>麦克风</span>
                <select id="microphone">
                  <option value="">系统默认麦克风</option>
                </select>
                <small id="microphone-note">跟随系统当前默认设备</small>
              </label>

              <div class="field shortcut-field">
                <span>全局快捷键</span>
                <button id="shortcut-capture" class="shortcut-capture" type="button">
                  <kbd id="shortcut-value">L⌘  L⇧  Space</kbd>
                  <small>支持单键并区分左右</small>
                </button>
              </div>
            </div>

            <fieldset class="mode-fieldset">
              <legend>触发模式</legend>
              <label class="mode-card" data-mode="hold">
                <input type="radio" name="mode" value="hold" checked />
                <span class="mode-icon hold-icon" aria-hidden="true"></span>
                <span>
                  <strong>按住说话</strong>
                  <small>按下开始，松开即停止并插入</small>
                </span>
                <b>默认</b>
              </label>
              <label class="mode-card" data-mode="toggle">
                <input type="radio" name="mode" value="toggle" />
                <span class="mode-icon toggle-icon" aria-hidden="true"></span>
                <span>
                  <strong>按键切换</strong>
                  <small>按一次开始，再按一次结束</small>
                </span>
              </label>
            </fieldset>
          </section>

          <section class="panel behavior-panel">
            <div class="behavior-row">
              <div class="behavior-copy">
                <span class="cursor-symbol" aria-hidden="true"></span>
                <div>
                  <strong>结束后插入光标</strong>
                  <p>不抢焦点，松开快捷键后自动粘贴到当前应用。</p>
                </div>
              </div>
              <label class="switch">
                <input id="auto-insert" type="checkbox" checked />
                <span></span>
              </label>
            </div>
          </section>

          <details class="advanced-panel">
            <summary>高级连接设置 <span>用于切换服务资源</span></summary>
            <div class="advanced-grid">
              <label class="field">
                <span>WebSocket Endpoint</span>
                <input id="endpoint" type="url" />
              </label>
              <label class="field">
                <span>Resource ID</span>
                <input id="resource-id" type="text" />
              </label>
            </div>
          </details>

          <div class="preview-panel" id="preview-panel">
            <div class="preview-meta">
              <span>实时文字</span>
              <span id="preview-state">等待试说</span>
            </div>
            <p id="preview-text">实时识别的内容会同时显示在这里。</p>
          </div>
        </div>

      <footer class="action-bar">
        <p id="save-message">修改只保存在这台设备上</p>
        <button class="primary-button" id="save-button" type="button">
          <span>保存并启用</span>
          <i aria-hidden="true">→</i>
        </button>
      </footer>
    </main>
  `;

  const appId = element<HTMLInputElement>("#app-id");
  const secretKey = element<HTMLInputElement>("#secret-key");
  const microphone = element<HTMLSelectElement>("#microphone");
  const shortcutCapture = element<HTMLButtonElement>("#shortcut-capture");
  const shortcutValue = element<HTMLElement>("#shortcut-value");
  const autoInsert = element<HTMLInputElement>("#auto-insert");
  const endpoint = element<HTMLInputElement>("#endpoint");
  const resourceId = element<HTMLInputElement>("#resource-id");
  const saveButton = element<HTMLButtonElement>("#save-button");
  const saveMessage = element<HTMLElement>("#save-message");
  const testButton = element<HTMLButtonElement>("#test-button");
  const testLabel = element<HTMLElement>("#test-label");
  const previewText = element<HTMLElement>("#preview-text");
  const previewState = element<HTMLElement>("#preview-state");
  const previewPanel = element<HTMLElement>("#preview-panel");

  let config: AppConfig;
  let runtime: RuntimeSnapshot;
  let capturingShortcut = false;
  let capturedShortcutKeys: string[] = [];
  let active = false;
  let unlistenRuntime: UnlistenFn | undefined;

  try {
    const [loadedConfig, loadedRuntime, microphones] = await Promise.all([
      invoke<AppConfig>("get_config"),
      invoke<RuntimeSnapshot>("get_runtime"),
      invoke<Microphone[]>("list_microphones").catch(() => []),
    ]);
    config = loadedConfig;
    runtime = loadedRuntime;
    populateMicrophones(microphone, microphones, config.microphone);
    applyConfig(config);
    applyRuntime(runtime);
  } catch (error) {
    showSaveMessage(asMessage(error), true);
    setStatus("error", "初始化失败", asMessage(error));
    return;
  }

  unlistenRuntime = await listen<RuntimeSnapshot>("voice-flow://runtime", ({ payload }) => {
    runtime = payload;
    applyRuntime(runtime);
  });
  window.addEventListener("beforeunload", () => unlistenRuntime?.());

  document.querySelectorAll<HTMLInputElement>('input[name="mode"]').forEach((input) => {
    input.addEventListener("change", updateModeCards);
  });

  element<HTMLButtonElement>("#reveal-secret").addEventListener("click", (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    const revealed = secretKey.type === "text";
    secretKey.type = revealed ? "password" : "text";
    button.textContent = revealed ? "显示" : "隐藏";
  });

  shortcutCapture.addEventListener("click", () => {
    if (capturingShortcut) {
      finishShortcutCapture(config.shortcut);
      return;
    }
    capturingShortcut = true;
    capturedShortcutKeys = [];
    shortcutCapture.classList.add("is-capturing");
    shortcutValue.textContent = "请按键…";
    shortcutCapture.querySelector("small")!.textContent = "松开完成，再次点击取消";
  });

  window.addEventListener("keydown", (event) => {
    if (!capturingShortcut) return;
    event.preventDefault();
    event.stopPropagation();
    const key = shortcutKeyFromCode(event.code);
    if (!key) {
      shortcutValue.textContent = `暂不支持 ${event.code}`;
      return;
    }
    if (!capturedShortcutKeys.includes(key)) capturedShortcutKeys.push(key);
    shortcutValue.textContent = prettyShortcut(capturedShortcutKeys.join("+"));
  });

  window.addEventListener("keyup", (event) => {
    if (!capturingShortcut || capturedShortcutKeys.length === 0) return;
    event.preventDefault();
    event.stopPropagation();
    config.shortcut = capturedShortcutKeys.join("+");
    finishShortcutCapture(config.shortcut);
  });

  saveButton.addEventListener("click", async () => {
    saveButton.disabled = true;
    showSaveMessage("正在保存并注册快捷键…");
    try {
      config = await invoke<AppConfig>("save_config", { config: readConfig() });
      applyConfig(config);
      showSaveMessage("已保存，快捷键现在可以在任意应用中使用");
      setStatus("ready", "语音输入已就绪", prettyShortcut(config.shortcut));
    } catch (error) {
      showSaveMessage(asMessage(error), true);
    } finally {
      saveButton.disabled = false;
    }
  });

  testButton.addEventListener("click", async () => {
    testButton.disabled = true;
    try {
      await invoke(active ? "stop_dictation" : "start_dictation");
    } catch (error) {
      showSaveMessage(asMessage(error), true);
    } finally {
      testButton.disabled = false;
    }
  });

  function applyConfig(next: AppConfig) {
    appId.value = next.app_id;
    secretKey.value = next.secret_key;
    microphone.value = next.microphone;
    shortcutValue.textContent = prettyShortcut(next.shortcut);
    autoInsert.checked = next.auto_insert;
    endpoint.value = next.endpoint;
    resourceId.value = next.resource_id;
    const selectedMode = document.querySelector<HTMLInputElement>(`input[name="mode"][value="${next.interaction_mode}"]`);
    if (selectedMode) selectedMode.checked = true;
    updateModeCards();
  }

  function readConfig(): AppConfig {
    const interactionMode = document.querySelector<HTMLInputElement>('input[name="mode"]:checked')?.value as InteractionMode | undefined;
    return {
      app_id: appId.value.trim(),
      secret_key: secretKey.value.trim(),
      shortcut: config.shortcut,
      interaction_mode: interactionMode ?? "hold",
      microphone: microphone.value,
      auto_insert: autoInsert.checked,
      endpoint: endpoint.value.trim(),
      resource_id: resourceId.value.trim(),
    };
  }

  function applyRuntime(next: RuntimeSnapshot) {
    active = ["connecting", "listening", "finalizing", "inserting"].includes(next.phase);
    testLabel.textContent = active ? "结束并插入" : "试说一次";
    testButton.classList.toggle("is-recording", active);
    previewPanel.dataset.phase = next.phase;
    previewState.textContent = phaseLabel(next.phase);
    if (next.transcript) previewText.textContent = next.transcript;
    if (next.phase === "idle" && !next.transcript) {
      setStatus(secretKey.value ? "ready" : "setup", secretKey.value ? "语音输入已就绪" : "等待完成配置", prettyShortcut(config.shortcut));
    } else if (next.phase === "error") {
      setStatus("error", "语音输入出错", next.message);
      previewText.textContent = next.message;
    } else {
      setStatus(next.phase, phaseLabel(next.phase), next.message);
    }
  }

  function finishShortcutCapture(shortcut: string) {
    capturingShortcut = false;
    shortcutCapture.classList.remove("is-capturing");
    shortcutValue.textContent = prettyShortcut(shortcut);
    shortcutCapture.querySelector("small")!.textContent = "支持单键并区分左右";
  }

  function showSaveMessage(message: string, isError = false) {
    saveMessage.textContent = message;
    saveMessage.classList.toggle("error-text", isError);
  }
}

async function mountDictationOverlay(root: HTMLDivElement) {
  document.body.className = "overlay-body";
  root.innerHTML = `
    <section class="dictation-ribbon" data-phase="idle">
      <div class="ribbon-signal" aria-hidden="true">
        <div class="mic-orbit"><span></span></div>
        <div class="level-bars">
          ${Array.from({ length: 11 }, (_, index) => `<i data-bar="${index}"></i>`).join("")}
        </div>
      </div>
      <div class="ribbon-copy">
        <div class="ribbon-meta">
          <span id="ribbon-phase">正在连接</span>
          <span class="live-mark"><i></i> LIVE</span>
        </div>
        <p id="ribbon-transcript">准备好后直接开始说话…</p>
        <small id="ribbon-message">松开快捷键即可插入</small>
      </div>
      <div class="ribbon-key" id="ribbon-key">HOLD</div>
    </section>
  `;

  const ribbon = element<HTMLElement>(".dictation-ribbon");
  const transcript = element<HTMLElement>("#ribbon-transcript");
  const phase = element<HTMLElement>("#ribbon-phase");
  const message = element<HTMLElement>("#ribbon-message");
  const key = element<HTMLElement>("#ribbon-key");
  const bars = Array.from(document.querySelectorAll<HTMLElement>("[data-bar]"));

  const config = await invoke<AppConfig>("get_config").catch(() => undefined);
  key.textContent = config?.interaction_mode === "toggle" ? "TOGGLE" : "HOLD";

  const applyRuntime = (next: RuntimeSnapshot) => {
    ribbon.dataset.phase = next.phase;
    phase.textContent = phaseLabel(next.phase);
    message.textContent = next.message;
    if (next.transcript) {
      transcript.textContent = next.transcript;
      transcript.scrollTop = transcript.scrollHeight;
    } else if (next.phase === "error") {
      transcript.textContent = "无法开始语音输入";
    } else if (next.phase === "connecting") {
      transcript.textContent = "准备好后直接开始说话…";
    }
  };

  const runtime = await invoke<RuntimeSnapshot>("get_runtime").catch(() => undefined);
  if (runtime) applyRuntime(runtime);

  await listen<RuntimeSnapshot>("voice-flow://runtime", ({ payload }) => applyRuntime(payload));
  await listen<LevelPayload>("voice-flow://level", ({ payload }) => {
    const energy = Math.min(1, Math.max(0.05, payload.level * 5.5));
    bars.forEach((bar, index) => {
      const center = 1 - Math.abs(index - (bars.length - 1) / 2) / (bars.length / 2);
      const ripple = 0.58 + Math.sin(index * 1.7 + performance.now() / 150) * 0.18;
      bar.style.height = `${6 + energy * center * ripple * 38}px`;
    });
  });
}

function populateMicrophones(select: HTMLSelectElement, microphones: Microphone[], selected: string) {
  for (const microphone of microphones) {
    const option = document.createElement("option");
    option.value = microphone.id;
    option.textContent = microphone.is_default ? `${microphone.name}（当前默认）` : microphone.name;
    select.append(option);
  }
  if (selected && !microphones.some((microphone) => microphone.id === selected)) {
    const unavailable = document.createElement("option");
    unavailable.value = selected;
    unavailable.textContent = `${selected}（当前不可用）`;
    select.append(unavailable);
  }
}

function updateModeCards() {
  document.querySelectorAll<HTMLElement>(".mode-card").forEach((card) => {
    const input = card.querySelector<HTMLInputElement>('input[type="radio"]');
    card.classList.toggle("is-selected", Boolean(input?.checked));
  });
}

function shortcutKeyFromCode(code: string): string | undefined {
  const aliases: Record<string, string> = {
    MetaLeft: "Command",
    MetaRight: "RCommand",
    ControlLeft: "LControl",
    ControlRight: "RControl",
    ShiftLeft: "LShift",
    ShiftRight: "RShift",
    AltLeft: "LOption",
    AltRight: "ROption",
    ArrowUp: "Up",
    ArrowDown: "Down",
    ArrowLeft: "Left",
    ArrowRight: "Right",
    Backquote: "Grave",
    BracketLeft: "LeftBracket",
    BracketRight: "RightBracket",
    Backslash: "BackSlash",
    Quote: "Apostrophe",
    Period: "Dot",
    NumpadEqual: "NumpadEquals",
  };
  if (aliases[code]) return aliases[code];
  if (code.startsWith("Key") && code.length === 4) return code.slice(3);
  if (code.startsWith("Digit") && code.length === 6) return `Key${code.slice(5)}`;
  if (/^F(?:[1-9]|1\d|20)$/.test(code)) return code;
  const supported = new Set([
    "Escape", "Space", "Enter", "Backspace", "CapsLock", "Tab", "Home", "End", "PageUp", "PageDown", "Insert", "Delete",
    "Numpad0", "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9",
    "NumpadSubtract", "NumpadAdd", "NumpadDivide", "NumpadMultiply", "NumpadEnter", "NumpadDecimal",
    "Minus", "Equal", "Semicolon", "Comma", "Slash",
  ]);
  return supported.has(code) ? code : undefined;
}

function prettyShortcut(shortcut: string): string {
  const labels: Record<string, string> = {
    Command: "L⌘",
    RCommand: "R⌘",
    LControl: "L⌃",
    RControl: "R⌃",
    LShift: "L⇧",
    RShift: "R⇧",
    LOption: "L⌥",
    ROption: "R⌥",
    Up: "↑",
    Down: "↓",
    Left: "←",
    Right: "→",
    Space: "Space",
    Escape: "Esc",
    Key0: "0",
    Key1: "1",
    Key2: "2",
    Key3: "3",
    Key4: "4",
    Key5: "5",
    Key6: "6",
    Key7: "7",
    Key8: "8",
    Key9: "9",
  };
  return shortcut
    .split("+")
    .map((key) => labels[key] ?? key)
    .join("  ");
}

function phaseLabel(phase: RuntimePhase): string {
  const labels: Record<RuntimePhase, string> = {
    idle: "就绪",
    connecting: "连接中",
    listening: "正在聆听",
    finalizing: "正在收尾",
    inserting: "正在插入",
    complete: "已完成",
    error: "出现错误",
  };
  return labels[phase];
}

function setStatus(kind: string, title: string, detail: string) {
  const dot = document.querySelector<HTMLElement>("#status-dot");
  const titleElement = document.querySelector<HTMLElement>("#status-title");
  const detailElement = document.querySelector<HTMLElement>("#status-detail");
  if (!dot || !titleElement || !detailElement) return;
  dot.dataset.state = kind;
  titleElement.textContent = title;
  detailElement.textContent = detail;
}

function element<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing element: ${selector}`);
  return found;
}

function asMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

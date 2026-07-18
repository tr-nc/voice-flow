import { getCurrentWindow } from "@tauri-apps/api/window";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { toPreviewFrame } from "./preview-adapter";
import type { PreviewFrame } from "./preview-model";
import { PreviewRenderer } from "./preview-renderer";
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
  secret_key: string;
  shortcut: string;
  interaction_mode: InteractionMode;
  microphone: string;
  auto_insert: boolean;
};

type Microphone = {
  id: string;
  name: string;
  is_default: boolean;
};

type RuntimeSnapshot = {
  phase: RuntimePhase;
  transcript: string;
  segments: TranscriptSegment[];
  message: string;
};

type TranscriptSegment = {
  text: string;
  definite: boolean;
};

const app = document.querySelector<HTMLDivElement>("#app");
if (!app) throw new Error("Missing #app root");
const isLinux = navigator.userAgent.includes("Linux");

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
      <div class="content-scroll">
          <section class="panel credentials-panel">
            <div class="panel-heading">
              <div>
                <h3>连接语音服务</h3>
              </div>
              <span class="local-badge">仅存本机</span>
            </div>

            <label class="field">
              <span>Secret Key</span>
              <div class="secret-field">
                <input id="secret-key" type="password" autocomplete="off" placeholder="输入 Secret Key" />
                <button id="reveal-secret" type="button" aria-label="显示密钥">显示</button>
              </div>
            </label>
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
                  <kbd id="shortcut-value">读取中…</kbd>
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

        </div>
    </main>
  `;

  const secretKey = element<HTMLInputElement>("#secret-key");
  const microphone = element<HTMLSelectElement>("#microphone");
  const microphoneNote = element<HTMLElement>("#microphone-note");
  const shortcutCapture = element<HTMLButtonElement>("#shortcut-capture");
  const shortcutValue = element<HTMLElement>("#shortcut-value");
  const autoInsert = element<HTMLInputElement>("#auto-insert");

  let config: AppConfig;
  let runtime: RuntimeSnapshot;
  let capturingShortcut = false;
  let capturedShortcutKeys: string[] = [];
  let active = false;
  let saveRevision = 0;
  let persistedRevision = 0;
  let saveTimer: number | undefined;
  let saveLoop: Promise<void> | undefined;
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
    console.error("Failed to initialize settings", error);
    return;
  }

  unlistenRuntime = await listen<RuntimeSnapshot>("voice-flow://runtime", ({ payload }) => {
    runtime = payload;
    applyRuntime(runtime);
  });
  window.addEventListener("beforeunload", () => unlistenRuntime?.());

  document.querySelectorAll<HTMLInputElement>('input[name="mode"]').forEach((input) => {
    input.addEventListener("change", () => {
      updateModeCards();
      scheduleConfigSave();
    });
  });

  element<HTMLButtonElement>("#reveal-secret").addEventListener("click", (event) => {
    const button = event.currentTarget as HTMLButtonElement;
    const revealed = secretKey.type === "text";
    secretKey.type = revealed ? "password" : "text";
    button.textContent = revealed ? "显示" : "隐藏";
  });

  secretKey.addEventListener("input", () => scheduleConfigSave(350));
  microphone.addEventListener("change", () => {
    updateMicrophoneNote();
    scheduleConfigSave();
  });
  autoInsert.addEventListener("change", () => scheduleConfigSave());

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
    scheduleConfigSave();
  });

  function applyConfig(next: AppConfig) {
    secretKey.value = next.secret_key;
    microphone.value = next.microphone;
    updateMicrophoneNote();
    shortcutValue.textContent = prettyShortcut(next.shortcut);
    autoInsert.checked = next.auto_insert;
    const selectedMode = document.querySelector<HTMLInputElement>(`input[name="mode"][value="${next.interaction_mode}"]`);
    if (selectedMode) selectedMode.checked = true;
    updateModeCards();
  }

  function updateMicrophoneNote() {
    microphoneNote.textContent = microphone.value ? "已自动保存此录音设备" : "跟随系统当前默认设备";
  }

  function readConfig(): AppConfig {
    const interactionMode = document.querySelector<HTMLInputElement>('input[name="mode"]:checked')?.value as InteractionMode | undefined;
    return {
      secret_key: secretKey.value.trim(),
      shortcut: config.shortcut,
      interaction_mode: interactionMode ?? "hold",
      microphone: microphone.value,
      auto_insert: autoInsert.checked,
    };
  }

  function applyRuntime(next: RuntimeSnapshot) {
    active = ["connecting", "listening", "finalizing", "inserting"].includes(next.phase);
    if (next.phase === "idle" && persistedRevision < saveRevision) void flushConfigSave();
    if (next.phase === "error") console.error("Dictation failed:", next.message);
  }

  function finishShortcutCapture(shortcut: string) {
    capturingShortcut = false;
    shortcutCapture.classList.remove("is-capturing");
    shortcutValue.textContent = prettyShortcut(shortcut);
    shortcutCapture.querySelector("small")!.textContent = "支持单键并区分左右";
  }

  function scheduleConfigSave(delay = 0) {
    saveRevision += 1;
    if (saveTimer !== undefined) window.clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => {
      saveTimer = undefined;
      void flushConfigSave();
    }, delay);
  }

  function flushConfigSave(): Promise<void> {
    if (saveTimer !== undefined) {
      window.clearTimeout(saveTimer);
      saveTimer = undefined;
    }
    if (saveLoop) return saveLoop;
    if (persistedRevision >= saveRevision || active) return Promise.resolve();

    saveLoop = persistPendingConfig().finally(() => {
      saveLoop = undefined;
      if (!active && persistedRevision < saveRevision) void flushConfigSave();
    });
    return saveLoop;
  }

  async function persistPendingConfig() {
    while (!active && persistedRevision < saveRevision) {
      const targetRevision = saveRevision;
      try {
        config = await invoke<AppConfig>("save_config", { config: readConfig() });
        persistedRevision = targetRevision;
      } catch (error) {
        persistedRevision = targetRevision;
        console.error("Failed to save settings:", asMessage(error));
      }
    }
  }
}

async function mountDictationOverlay(root: HTMLDivElement) {
  document.body.className = "overlay-body";
  root.innerHTML = `<p class="dictation-text"></p>`;

  const transcript = element<HTMLElement>(".dictation-text");
  const preview = new PreviewRenderer(transcript);
  const overlayWindow = getCurrentWindow();
  const minOverlayHeight = 72;
  const maxOverlayHeight = 280;
  // Eight pixels around the panel plus its one-pixel border on each side.
  const overlayVerticalPadding = 18;
  let layoutFrame: number | undefined;
  let previewFrame: number | undefined;
  let pendingPreview: PreviewFrame | undefined;
  let desiredHeight = minOverlayHeight;
  let appliedHeight = 0;
  let resizeInFlight = false;

  const flushOverlayResize = async () => {
    if (resizeInFlight || desiredHeight === appliedHeight) return;
    resizeInFlight = true;
    try {
      while (desiredHeight !== appliedHeight) {
        const targetHeight = desiredHeight;
        await invoke("resize_dictation_overlay", { height: targetHeight });
        appliedHeight = targetHeight;
      }
    } catch (error) {
      console.error("Failed to resize the dictation overlay:", asMessage(error));
    } finally {
      resizeInFlight = false;
    }
  };

  const updateOverlayLayout = () => {
    if (layoutFrame !== undefined) window.cancelAnimationFrame(layoutFrame);
    layoutFrame = window.requestAnimationFrame(() => {
      layoutFrame = undefined;
      desiredHeight = Math.min(
        maxOverlayHeight,
        Math.max(minOverlayHeight, Math.ceil(transcript.scrollHeight) + overlayVerticalPadding),
      );
      void flushOverlayResize().finally(() => {
        // When the capped preview overflows, keep the newest recognized words visible.
        transcript.scrollTop = transcript.scrollHeight;
      });
    });
  };

  const applyRuntime = (next: RuntimeSnapshot) => {
    pendingPreview = toPreviewFrame(next.transcript, next.segments ?? []);
    if (previewFrame !== undefined) return;
    previewFrame = window.requestAnimationFrame(() => {
      previewFrame = undefined;
      if (!pendingPreview) return;
      preview.render(pendingPreview);
      pendingPreview = undefined;
      updateOverlayLayout();
    });
  };

  const runtime = await invoke<RuntimeSnapshot>("get_runtime").catch(() => undefined);
  if (runtime) applyRuntime(runtime);
  await listen<RuntimeSnapshot>("voice-flow://runtime", ({ payload }) => applyRuntime(payload));

  void overlayWindow.onScaleChanged(updateOverlayLayout);
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
    Command: isLinux ? "L Super" : "L⌘",
    RCommand: isLinux ? "R Super" : "R⌘",
    LControl: "L⌃",
    RControl: "R⌃",
    LShift: "L⇧",
    RShift: "R⇧",
    LOption: isLinux ? "L Alt" : "L⌥",
    ROption: isLinux ? "R Alt" : "R⌥",
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

function element<T extends Element>(selector: string): T {
  const found = document.querySelector<T>(selector);
  if (!found) throw new Error(`Missing element: ${selector}`);
  return found;
}

function asMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

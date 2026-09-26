import { invoke } from "@tauri-apps/api/core";

/** 窗口内按键：绑 PTT 不靠全局钩子。输入框里的字不往下传。 */
export function attachPttKeys(): () => void {
  const downCodes = new Set<string>();
  let captureActive = false;
  const go = (pressed: boolean) => (e: KeyboardEvent) => {
    if (e.repeat) return;
    const el = e.target as HTMLElement | null;
    if (
      el &&
      (el.tagName === "INPUT" ||
        el.tagName === "TEXTAREA" ||
        el.tagName === "SELECT" ||
        el.isContentEditable)
    ) {
      return;
    }
    // PTT binding capture owns keyboard events until it reports completion.
    // The capture key still reaches the native watcher, but it must not key
    // the microphone through this window listener.
    if (captureActive) return;
    if (pressed) downCodes.add(e.code);
    else downCodes.delete(e.code);
    void invoke("ptt_ui_key", { code: e.code, pressed });
  };
  const down = go(true);
  const up = go(false);
  window.addEventListener("keydown", down);
  window.addEventListener("keyup", up);
  const release = () => {
    for (const code of downCodes) void invoke("ptt_ui_key", { code, pressed: false });
    downCodes.clear();
  };
  const capture = (e: Event) => {
    captureActive = e instanceof CustomEvent && e.detail === true;
    if (captureActive) release();
  };
  window.addEventListener("blur", release);
  document.addEventListener("visibilitychange", release);
  window.addEventListener("can-voice:ptt-capture", capture);
  return () => {
    release();
    window.removeEventListener("keydown", down);
    window.removeEventListener("keyup", up);
    window.removeEventListener("blur", release);
    document.removeEventListener("visibilitychange", release);
    window.removeEventListener("can-voice:ptt-capture", capture);
  };
}

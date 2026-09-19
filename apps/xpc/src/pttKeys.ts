import { invoke } from "@tauri-apps/api/core";

/** 窗口内按键：绑 PTT 不靠全局钩子。输入框里的字不往下传。 */
export function attachPttKeys(): () => void {
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
    void invoke("ptt_ui_key", { code: e.code, pressed });
  };
  const down = go(true);
  const up = go(false);
  window.addEventListener("keydown", down);
  window.addEventListener("keyup", up);
  return () => {
    window.removeEventListener("keydown", down);
    window.removeEventListener("keyup", up);
  };
}

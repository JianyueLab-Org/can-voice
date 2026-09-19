import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { applyLanguage, type LanguageChoice } from "./i18n";

/**
 * 主题、置顶、精简（#45），界面语言（#29）。四个客户端逐字节相同的一份。
 *
 * **真相在 Rust 侧的设置文件里**，这里是它的一个映像。localStorage 里另存一份
 * 主题只为了一件事：挂载之前就能上色——等 `invoke("settings")` 回来再上色，
 * 深色用户每次启动都会先被白屏闪一下。
 */

/** 和 Rust 侧 `can_voice_settings::Theme` 一一对应。 */
export type Theme = "system" | "light" | "dark";

/** 和 Rust 侧 `can_voice_settings::Appearance` 一一对应。 */
export interface Appearance {
  theme: Theme;
  always_on_top: boolean;
  compact: boolean;
  /** 措辞和切换在 `i18n.ts`，这里只是它在设置文件里的那一格。 */
  language: LanguageChoice;
}

const STORED = "can-voice.theme";
const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

function stored(): Theme {
  try {
    const t = localStorage.getItem(STORED);
    return t === "light" || t === "dark" || t === "system" ? t : "dark";
  } catch {
    return "system";
  }
}

export const appearance = ref<Appearance>({
  theme: stored(),
  always_on_top: false,
  compact: false,
  language: "system",
});

function paint(theme: Theme) {
  const dark = theme === "dark" || (theme === "system" && systemDark.matches);
  document.documentElement.classList.toggle("dark", dark);
  try {
    localStorage.setItem(STORED, theme);
  } catch {
    // 存不下只是下次启动会闪一下，不值得打断任何事。
  }
}

/** 挂载之前同步调用一次。 */
export function paintStoredTheme() {
  paint(stored());
}

// 跟随系统时，系统半路切了深浅色要跟上——值班到天黑时系统自动切，是常事。
systemDark.addEventListener("change", () => paint(appearance.value.theme));

/** 从设置文件读回来。每个窗口挂载时调一次。 */
export async function loadAppearance() {
  const s = await invoke<{ appearance?: Partial<Appearance> }>("settings");
  if (s.appearance) appearance.value = { ...appearance.value, ...s.appearance };
  // 原来 voice 只有深色。以前默认「跟随系统」的，第一次读过来改成深色并写回去。
  if (appearance.value.theme === "system") {
    appearance.value = { ...appearance.value, theme: "dark" };
    await invoke("set_appearance", { appearance: appearance.value });
  }
  paint(appearance.value.theme);
  applyLanguage(appearance.value.language);
}

/** 改一项并存下去。置顶和精简由 Rust 侧动窗口。 */
export async function setAppearance(patch: Partial<Appearance>) {
  appearance.value = { ...appearance.value, ...patch };
  paint(appearance.value.theme);
  applyLanguage(appearance.value.language);
  await invoke("set_appearance", { appearance: appearance.value });
}

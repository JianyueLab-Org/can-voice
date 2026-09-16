import { computed, ref, watchEffect } from "vue";
import { createTranslator } from "@jianyuelab-org/can-ui/i18n";
import commonZh from "./locales/common.zh.json";
import commonEn from "./locales/common.en.json";
import appZh from "./locales/app.zh.json";
import appEn from "./locales/app.en.json";

/**
 * 界面语言（#29）。四个客户端逐字节相同的一份。
 *
 * **字典是 JSON 不是 TS**：`crates/can-voice-i18n/tests/dictionaries.rs` 要在 CI 里读它——
 * 两种语言 key 一样多、占位符对得上、英文里没有汉字、代码里用到的 key 都在、界面代码里
 * 没有写死的中文。前端没有测试框架，这几条只能在 Rust 那边跑。
 *
 * - `locales/common.*.json`：四个共用组件和共用 crate 的话，四个客户端逐字节相同。
 * - `locales/app.*.json`：这个客户端自己的。两份的顶层命名空间不许重名。
 *
 * Rust 侧交过来的错误是 `{ key, values }`（`can_voice_i18n::Message`），用 `errorText` 翻。
 */

/** 能显示的语言。 */
export type Language = "zh" | "en";
/** 设置里存的那一项。和 Rust 侧 `can_voice_settings::Language` 一一对应。 */
export type LanguageChoice = "system" | Language;

const zh = { ...commonZh, ...appZh };
const en = { ...commonEn, ...appEn };

type Paths<T> = {
  [K in keyof T & string]: T[K] extends string ? K : `${K}.${Paths<T[K]>}`;
}[keyof T & string];

/**
 * 字典里的一个 key。**写错了编译不过**，这就是它存在的理由。
 *
 * 拼出来的 key（`` `voice.link.${x}` as Key ``）逃得过这一关，也逃得过 Rust 那边的扫描，
 * 写的时候对着字典自己核一遍。
 */
export type Key = Paths<typeof zh>;

const STORED = "can-voice.language";

/**
 * 系统语言。**中文和英文之外的一律中文**：中文是这个网络的产品语言，旧版 can-audio
 * 也是这么定的。按系统给的偏好顺序找第一个认得的。
 */
export function systemLanguage(): Language {
  const tags = navigator.languages?.length ? navigator.languages : [navigator.language];
  for (const tag of tags) {
    const primary = (tag ?? "").toLowerCase().split(/[-_]/)[0];
    if (primary === "zh" || primary === "en") return primary;
  }
  return "zh";
}

/**
 * localStorage 里另存一份，理由和主题同一条：挂载之前就要知道用哪种语言——等
 * `invoke("settings")` 回来再换，英文用户每次启动都会先看到一闪中文。
 */
function storedChoice(): LanguageChoice {
  try {
    const v = localStorage.getItem(STORED);
    return v === "zh" || v === "en" ? v : "system";
  } catch {
    return "system";
  }
}

const choice = ref<LanguageChoice>(storedChoice());
const system = ref<Language>(systemLanguage());
// 跟随系统时，系统半路换了语言要跟上。
window.addEventListener("languagechange", () => {
  system.value = systemLanguage();
});

/** 此刻在用的语言。 */
export const language = computed<Language>(() =>
  choice.value === "system" ? system.value : choice.value,
);

// 读屏软件和拼写检查看的是这个属性。
watchEffect(() => {
  document.documentElement.lang = language.value === "en" ? "en" : "zh-CN";
});

// 英文缺了哪个 key 就退回中文：半句中文比一个裸 key 好认。字典测试保证这种事不发生。
const translators = { zh: createTranslator(zh), en: createTranslator(en, zh) };

/**
 * 翻一个 key。**在模板、computed 或者函数体里调**：它读的是响应式的语言，切换时
 * 跟着重算。存进模块级常量里的翻译会冻在加载那一刻的语言上——旧版踩过。
 */
export function t(key: Key, values?: Record<string, string | number>): string {
  return translators[language.value](key, values);
}

/** Rust 侧 `can_voice_i18n::Message` 的形状。 */
export interface Message {
  key: string;
  values?: Record<string, string>;
}

function isMessage(e: unknown): e is Message {
  return typeof e === "object" && e !== null && typeof (e as Message).key === "string";
}

/**
 * 命令失败时给人看的那一句。
 *
 * Rust 侧交过来的可能是一条 `Message`、一串 `Message`（地址校验一次报全），也可能
 * 是一个 JS 异常。**别再写 `String(e)`**：一条 `Message` 被 `String()` 出来是
 * `[object Object]`。
 */
export function errorText(e: unknown): string {
  if (Array.isArray(e)) return e.map(errorText).join(t("common.separator.sentence"));
  if (isMessage(e)) return translators[language.value](e.key, e.values);
  return String(e);
}

/** 换语言。`appearance.ts` 在读回设置和用户改设置时调。 */
export function applyLanguage(next: LanguageChoice) {
  choice.value = next;
  try {
    localStorage.setItem(STORED, next);
  } catch {
    // 存不下只是下次启动会闪一下，不值得打断任何事。
  }
}

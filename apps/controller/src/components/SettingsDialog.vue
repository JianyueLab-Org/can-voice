<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { appearance, setAppearance, type Theme } from "../appearance";
import { errorText, t, type LanguageChoice } from "../i18n";

/**
 * 设置对话框（#45）。四个客户端逐字节相同的一份。
 *
 * 语音服务器、FSD、can-api 这几个地址此前只能靠环境变量改——一个装了 msi 的人
 * 没有地方改它们。**每个客户端用哪几格由 Rust 侧报**（`endpoint_fields`），
 * 默认值也从那里来：这里再抄一份默认地址，迟早和真正生效的那个对不上。
 */
const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ close: [] }>();

/** 和 Rust 侧 `can_voice_settings::endpoints::Field` 一一对应。 */
interface Field {
  key: string;
  default: string;
  overridden: boolean;
}

/**
 * 一格地址叫什么。**每次渲染时现翻**，不做成模块级的表：表里的字会冻在加载那一刻的
 * 语言上。Rust 侧报了一格这里不认识的，就显示它的字段名。
 */
function fieldLabel(key: string): string {
  switch (key) {
    case "api_origin":
      return t("settings.endpoint.api_origin");
    case "voice_server":
      return t("settings.endpoint.voice_server");
    case "fsd_server":
      return t("settings.endpoint.fsd_server");
    case "datafeed_url":
      return t("settings.endpoint.datafeed_url");
    case "metar_url":
      return t("settings.endpoint.metar_url");
    case "atis_config_url":
      return t("settings.endpoint.atis_config_url");
    default:
      return key;
  }
}

const fields = ref<Field[]>([]);
/** 整份 `Endpoints`。**这个客户端不显示的那几格也在里面**，存的时候原样带回去。 */
const values = ref<Record<string, string>>({});
const debugLog = ref(false);
/** Rust 侧交回来的原样，渲染时才翻——切了语言，这一段跟着换。 */
const problem = ref<unknown>(null);
const saved = ref(false);
const box = ref<HTMLElement | null>(null);

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    problem.value = null;
    saved.value = false;
    fields.value = await invoke<Field[]>("endpoint_fields");
    const s = await invoke<{ endpoints?: Record<string, string>; debug_log?: boolean }>(
      "settings",
    );
    values.value = { ...(s.endpoints ?? {}) };
    debugLog.value = s.debug_log ?? false;
    await nextTick();
    box.value?.focus();
  },
);

async function saveEndpoints() {
  problem.value = null;
  saved.value = false;
  try {
    values.value = await invoke<Record<string, string>>("set_endpoints", {
      endpoints: values.value,
    });
    saved.value = true;
  } catch (e) {
    // 填得不对就没存，Rust 侧说哪里不对。
    problem.value = e;
  }
}

function pickTheme(event: Event) {
  void setAppearance({ theme: (event.target as HTMLSelectElement).value as Theme });
}

/** 当场切，不用重开窗口。 */
function pickLanguage(event: Event) {
  void setAppearance({
    language: (event.target as HTMLSelectElement).value as LanguageChoice,
  });
}

async function toggleDebug() {
  await invoke("set_debug_log", { on: debugLog.value });
}
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-3"
    @click.self="emit('close')"
  >
    <div
      ref="box"
      tabindex="-1"
      class="flex max-h-full w-[30rem] flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      @keyup.escape="emit('close')"
    >
      <h2 class="font-semibold">{{ t("settings.title") }}</h2>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">{{ t("settings.appearance") }}</p>
        <!-- 语言放在最前面：切错了语言的人，要在一堆读不懂的字里找到它。 -->
        <label class="flex items-center gap-2">
          <span class="w-20 opacity-70">{{ t("settings.language") }}</span>
          <select
            :value="appearance.language"
            class="rounded border px-2 py-1"
            @change="pickLanguage"
          >
            <option value="system">{{ t("settings.language_system") }}</option>
            <!-- 每种语言写它自己的名字：看不懂当前语言的人也认得出自己那一项。 -->
            <option value="zh">{{ t("language.zh") }}</option>
            <option value="en">{{ t("language.en") }}</option>
          </select>
        </label>
        <label class="flex items-center gap-2">
          <span class="w-20 opacity-70">{{ t("settings.theme") }}</span>
          <select :value="appearance.theme" class="rounded border px-2 py-1" @change="pickTheme">
            <option value="system">{{ t("settings.theme_system") }}</option>
            <option value="light">{{ t("settings.theme_light") }}</option>
            <option value="dark">{{ t("settings.theme_dark") }}</option>
          </select>
        </label>
        <label class="flex items-center gap-2">
          <input
            type="checkbox"
            :checked="appearance.always_on_top"
            @change="setAppearance({ always_on_top: !appearance.always_on_top })"
          />
          {{ t("settings.always_on_top") }}
        </label>
        <label class="flex items-center gap-2">
          <input
            type="checkbox"
            :checked="appearance.compact"
            @change="setAppearance({ compact: !appearance.compact })"
          />
          {{ t("settings.compact") }}
        </label>
      </section>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">{{ t("settings.endpoints") }}</p>
        <label v-for="f in fields" :key="f.key" class="flex flex-col gap-1">
          <span class="flex items-center gap-2">
            <span class="opacity-70">{{ fieldLabel(f.key) }}</span>
            <!-- 被环境变量盖着的格子改了也不生效。不标出来的话，看起来就是设置坏了。 -->
            <span v-if="f.overridden" class="text-amber-700">{{ t("settings.overridden") }}</span>
          </span>
          <input
            v-model="values[f.key]"
            :placeholder="f.default"
            spellcheck="false"
            class="rounded border px-2 py-1 font-mono"
            @input="saved = false"
            @keyup.enter="saveEndpoints"
          />
        </label>
        <div class="flex items-center gap-2">
          <button class="rounded border px-3 py-1" @click="saveEndpoints">
            {{ t("settings.save_endpoints") }}
          </button>
          <span v-if="saved" class="text-green-700">{{ t("settings.saved") }}</span>
        </div>
        <p v-if="problem" class="rounded border border-red-400 px-2 py-1 text-red-600">
          {{ errorText(problem) }}
        </p>
      </section>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">{{ t("settings.troubleshooting") }}</p>
        <label class="flex items-center gap-2">
          <input v-model="debugLog" type="checkbox" @change="toggleDebug" />
          {{ t("settings.debug_log") }}
          <span class="opacity-60">{{ t("settings.debug_log_note") }}</span>
        </label>
      </section>

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('close')">
          {{ t("common.close") }}
        </button>
      </div>
    </div>
  </div>
</template>

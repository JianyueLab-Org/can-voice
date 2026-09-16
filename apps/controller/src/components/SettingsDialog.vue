<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { appearance, setAppearance, type Theme } from "../appearance";

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

const LABELS: Record<string, string> = {
  api_origin: "can-api",
  voice_server: "语音服务器",
  fsd_server: "FSD 服务器",
  datafeed_url: "数据源",
  metar_url: "气象源",
  atis_config_url: "通播配置",
};

const fields = ref<Field[]>([]);
/** 整份 `Endpoints`。**这个客户端不显示的那几格也在里面**，存的时候原样带回去。 */
const values = ref<Record<string, string>>({});
const debugLog = ref(false);
const problem = ref("");
const saved = ref(false);
const box = ref<HTMLElement | null>(null);

watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    problem.value = "";
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
  problem.value = "";
  saved.value = false;
  try {
    values.value = await invoke<Record<string, string>>("set_endpoints", {
      endpoints: values.value,
    });
    saved.value = true;
  } catch (e) {
    // 填得不对就没存，Rust 侧说哪里不对。
    problem.value = String(e);
  }
}

function pickTheme(event: Event) {
  void setAppearance({ theme: (event.target as HTMLSelectElement).value as Theme });
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
      <h2 class="font-semibold">设置</h2>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">外观</p>
        <label class="flex items-center gap-2">
          <span class="w-20 opacity-70">主题</span>
          <select :value="appearance.theme" class="rounded border px-2 py-1" @change="pickTheme">
            <option value="system">跟随系统</option>
            <option value="light">浅色</option>
            <option value="dark">深色</option>
          </select>
        </label>
        <label class="flex items-center gap-2">
          <input
            type="checkbox"
            :checked="appearance.always_on_top"
            @change="setAppearance({ always_on_top: !appearance.always_on_top })"
          />
          窗口置顶
        </label>
        <label class="flex items-center gap-2">
          <input
            type="checkbox"
            :checked="appearance.compact"
            @change="setAppearance({ compact: !appearance.compact })"
          />
          精简模式
        </label>
      </section>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">服务器地址 · 留空用默认 · 下次连接生效</p>
        <label v-for="f in fields" :key="f.key" class="flex flex-col gap-1">
          <span class="flex items-center gap-2">
            <span class="opacity-70">{{ LABELS[f.key] ?? f.key }}</span>
            <!-- 被环境变量盖着的格子改了也不生效。不标出来的话，看起来就是设置坏了。 -->
            <span v-if="f.overridden" class="text-amber-700">此刻被环境变量盖着，改了不生效</span>
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
          <button class="rounded border px-3 py-1" @click="saveEndpoints">保存地址</button>
          <span v-if="saved" class="text-green-700">已保存</span>
        </div>
        <p v-if="problem" class="rounded border border-red-400 px-2 py-1 text-red-600">
          {{ problem }}
        </p>
      </section>

      <section class="flex flex-col gap-2 text-xs">
        <p class="opacity-60">排障</p>
        <label class="flex items-center gap-2">
          <input v-model="debugLog" type="checkbox" @change="toggleDebug" />
          调试级日志
          <span class="opacity-60">（下次启动生效。和命令行加 --debug 一样）</span>
        </label>
      </section>

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('close')">关闭</button>
      </div>
    </div>
  </div>
</template>

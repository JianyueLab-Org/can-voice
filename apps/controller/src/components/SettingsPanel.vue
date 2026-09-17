<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import { t } from "../i18n";

/// 已经存下来的 CAN 号，寄日志时预填，省得再打一遍。
const props = defineProps<{ cid?: string }>();

/** 一个绑定加上 Rust 侧算好的短标识。措辞只有一份，前端不自己拼。 */
interface BindingView {
  token: string;
  unresolved: boolean;
  binding: unknown;
}

interface Settings {
  cid: string;
  input_device: string | null;
  output_device: string | null;
}

const inputs = ref<string[]>([]);
const outputs = ref<string[]>([]);
const input = ref<string>("");
const output = ref<string>("");
const bindings = ref<BindingView[]>([]);
const capturing = ref(false);
const mouseOk = ref(true);
const keyboardOk = ref(true);

let captureTimer: number | undefined;

async function load() {
  const devices = await invoke<{ input: string[]; output: string[] }>("audio_devices");
  inputs.value = devices.input;
  outputs.value = devices.output;
  const s = await invoke<Settings>("settings");
  input.value = s.input_device ?? "";
  output.value = s.output_device ?? "";
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
}

onMounted(async () => {
  mouseOk.value = await invoke<boolean>("mouse_ptt_supported");
  keyboardOk.value = await invoke<boolean>("keyboard_ptt_supported");
  await load();
});
onUnmounted(() => window.clearInterval(captureTimer));

async function applyDevices() {
  // 空串是"跟系统默认"，传 null 过去。
  await invoke("set_audio_devices", {
    input: input.value || null,
    output: output.value || null,
  });
}

async function push() {
  await invoke("set_ptt_bindings", { bindings: bindings.value.map((b) => b.binding) });
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
}

/**
 * 「按一下你要的键」。
 *
 * 先把当前这组推下去：监听器是**懒起**的，没推过就根本没有东西在听，
 * 于是录制永远录不到——而界面上看不出任何异常。
 */
async function capture() {
  if (capturing.value) return;
  capturing.value = true;
  await invoke("set_ptt_bindings", { bindings: bindings.value.map((b) => b.binding) });
  await invoke("begin_ptt_capture");

  let waited = 0;
  captureTimer = window.setInterval(async () => {
    const got = await invoke<unknown | null>("take_captured_binding");
    waited += 150;
    if (got) {
      window.clearInterval(captureTimer);
      capturing.value = false;
      bindings.value = [...bindings.value, { token: "", unresolved: false, binding: got }];
      await push();
      return;
    }
    // 十秒还没按就收手：一个停不下来的录制状态会把下一次点击也吃掉。
    if (waited >= 10_000) {
      window.clearInterval(captureTimer);
      capturing.value = false;
    }
  }, 150);
}

async function remove(i: number) {
  bindings.value = bindings.value.filter((_, n) => n !== i);
  await push();
}
</script>

<template>
  <section class="flex flex-col gap-4 rounded border p-3 text-xs">
    <div class="flex flex-col gap-2">
      <h2 class="font-semibold">{{ t("audio.title") }}</h2>
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("audio.microphone") }}</span>
        <select v-model="input" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("audio.system_default") }}</option>
          <option v-for="d in inputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("audio.headset") }}</span>
        <select v-model="output" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("audio.system_default") }}</option>
          <option v-for="d in outputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <p class="opacity-60">{{ t("audio.applies_now") }}</p>
    </div>

    <div class="flex flex-col gap-2">
      <h2 class="font-semibold">{{ t("ptt.title") }}</h2>
      <ul v-if="bindings.length" class="flex flex-col gap-1">
        <li
          v-for="(b, i) in bindings"
          :key="i"
          class="flex items-center gap-2 rounded border px-2 py-1"
        >
          <span class="font-mono">{{ b.token || "…" }}</span>
          <span v-if="b.unresolved" class="text-red-600">
            {{ t("ptt.unresolved") }}
          </span>
          <button class="ml-auto rounded border px-2" @click="remove(i)">
            {{ t("ptt.delete") }}
          </button>
        </li>
      </ul>
      <p v-else class="opacity-60">{{ t("ptt.none") }}</p>

      <button class="self-start rounded border px-3 py-1" :disabled="capturing" @click="capture">
        {{ capturing ? t("ptt.capturing") : t("ptt.record") }}
      </button>

      <p v-if="!keyboardOk" class="text-red-600">
        {{ t("ptt.wayland") }}
      </p>
      <p v-if="!mouseOk" class="opacity-70">{{ t("ptt.no_mouse") }}</p>
    </div>

    <LogPanel :cid="props.cid" />
  </section>
</template>

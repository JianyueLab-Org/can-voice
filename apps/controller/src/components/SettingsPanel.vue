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
  mic_volume: number;
  speaker_volume: number;
}

const inputs = ref<string[]>([]);
const outputs = ref<string[]>([]);
const input = ref<string>("");
const output = ref<string>("");
const mic = ref(100);
const speaker = ref(100);
const bindings = ref<BindingView[]>([]);
const capturing = ref(false);
const mouseOk = ref(true);
const keyboardOk = ref(true);
const testing = ref<"speaker" | "mic" | null>(null);
const testErr = ref("");

let captureTimer: number | undefined;

async function load() {
  const devices = await invoke<{ input: string[]; output: string[] }>("audio_devices");
  inputs.value = devices.input;
  outputs.value = devices.output;
  const s = await invoke<Settings>("settings");
  input.value = s.input_device ?? "";
  output.value = s.output_device ?? "";
  mic.value = s.mic_volume ?? 100;
  speaker.value = s.speaker_volume ?? 100;
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
  await dropGoneDevices();
}

async function refreshDevices() {
  const devices = await invoke<{ input: string[]; output: string[] }>("audio_devices");
  inputs.value = devices.input;
  outputs.value = devices.output;
  await dropGoneDevices();
}

/** 下拉框里已经没有的设备当成拔掉了，改回系统默认。 */
async function dropGoneDevices() {
  let changed = false;
  if (input.value && !inputs.value.includes(input.value)) {
    input.value = "";
    changed = true;
  }
  if (output.value && !outputs.value.includes(output.value)) {
    output.value = "";
    changed = true;
  }
  if (changed) await applyDevices();
}

let deviceTimer: number | undefined;
onMounted(async () => {
  mouseOk.value = await invoke<boolean>("mouse_ptt_supported");
  keyboardOk.value = await invoke<boolean>("keyboard_ptt_supported");
  await load();
  deviceTimer = window.setInterval(() => void refreshDevices(), 2000);
});
onUnmounted(() => {
  window.clearInterval(captureTimer);
  window.clearInterval(deviceTimer);
});

async function applyDevices() {
  // 空串是"跟系统默认"，传 null 过去。
  await invoke("set_audio_devices", {
    input: input.value || null,
    output: output.value || null,
  });
}

async function applyVolume() {
  await invoke("set_master_volume", { mic: mic.value, speaker: speaker.value });
}

async function testSpeaker() {
  if (testing.value) return;
  testing.value = "speaker";
  testErr.value = "";
  try {
    await invoke("test_speaker");
  } catch (e) {
    testErr.value = String(e);
  } finally {
    testing.value = null;
  }
}

async function testMic() {
  if (testing.value) return;
  testing.value = "mic";
  testErr.value = "";
  try {
    await invoke("test_mic");
  } catch (e) {
    testErr.value = String(e);
  } finally {
    testing.value = null;
  }
}

async function push() {
  await invoke("set_ptt_bindings", { bindings: bindings.value.map((b) => b.binding) });
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
}

/**
 * 「按一下你要的键」。
 *
 * 先把当前这组推下去，再开始录。空栈也要听得到正在绑的那个键。
 */
async function capture() {
  if (capturing.value) return;
  capturing.value = true;
  (document.activeElement as HTMLElement | null)?.blur();
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
      void invoke("cancel_ptt_capture");
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
      <div class="flex items-center gap-2">
        <button class="rounded border px-3 py-1" :disabled="!!testing" @click="testSpeaker">
          {{ testing === "speaker" ? t("audio.testing") : t("audio.test_speaker") }}
        </button>
        <button class="rounded border px-3 py-1" :disabled="!!testing" @click="testMic">
          {{ testing === "mic" ? t("audio.testing_mic") : t("audio.test_mic") }}
        </button>
      </div>
      <p v-if="testErr" class="text-red-600">{{ testErr }}</p>
      <label class="flex items-center gap-2">
        {{ t("audio.mic_volume") }}
        <input
          type="range"
          min="0"
          max="200"
          step="1"
          v-model.number="mic"
          class="flex-1"
          @change="applyVolume"
        />
        <span class="w-10 text-right font-mono">{{ mic }}%</span>
      </label>
      <label class="flex items-center gap-2">
        {{ t("audio.speaker_volume") }}
        <input
          type="range"
          min="0"
          max="200"
          step="1"
          v-model.number="speaker"
          class="flex-1"
          @change="applyVolume"
        />
        <span class="w-10 text-right font-mono">{{ speaker }}%</span>
      </label>
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

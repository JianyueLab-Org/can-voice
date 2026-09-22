<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import InstallWizard from "./InstallWizard.vue";
import { t } from "../i18n";

/**
 * 设置对话框里 xpc 自己那几段：音频 / 网络 / 他机三页（spec §6）。
 *
 * **它整个挂在 `SettingsDialog` 的 `v-if="open"` 里面**，每次打开都是新挂一次，
 * 关掉就销毁。所以 `onMounted` 就是「打开的时候读一遍」，`onUnmounted` 就是
 * 「关掉的时候停掉」——不要改成 watch，外层 `v-if` 已经决定了生命周期，再套一层
 * 只会多一条走不到的路（`SettingsCommon` 那条 `{ immediate: true }` 是因为它读的
 * 是自己的属性，不是自己的挂载）。管制端的 `SettingsPanel.vue` 是同一个形状。
 *
 * 「网络」那一页只有寄日志：服务器地址那几格在 `SettingsCommon` 里，就在这个枢轴
 * 的正上方；真实姓名和连不连在主界面的连接卡片上。
 *
 * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
 */

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
/// `csl` 是扫模型那一侧的现状，由 App.vue 那份轮询回来的快照带进来。
const props = defineProps<{ cid?: string; csl?: CslView }>();
import type { CslView, Settings } from "../types";

/** 枢轴停在哪一页。默认音频，和 can-audio 的 `setCurrentItem("audio")` 一样。 */
const page = ref<"audio" | "network" | "traffic">("audio");

interface BindingView {
  token: string;
  unresolved: boolean;
  binding: unknown;
}

interface DeviceInfo {
  id: string;
  name: string;
  is_default: boolean;
}

const inputs = ref<DeviceInfo[]>([]);
const outputs = ref<DeviceInfo[]>([]);
const input = ref("");
const output = ref("");
const inject = ref(true);
const chime = ref(true);
const chimeAll = ref(false);
const chimeVolume = ref(100);
const mic = ref(100);
const speaker = ref(100);
const range = ref(200);
const cslDir = ref("");
const bindings = ref<BindingView[]>([]);
const capturing = ref(false);
const keyboardOk = ref(true);
const mouseOk = ref(true);
const testing = ref<"speaker" | "mic" | null>(null);
const testErr = ref("");

let captureTimer: number | undefined;
let deviceTimer: number | undefined;

onMounted(async () => {
  keyboardOk.value = await invoke<boolean>("keyboard_ptt_supported");
  mouseOk.value = await invoke<boolean>("mouse_ptt_supported");
  const devices = await invoke<{ input: DeviceInfo[]; output: DeviceInfo[] }>("audio_devices");
  inputs.value = devices.input ?? [];
  outputs.value = devices.output ?? [];
  const s = await invoke<Settings>("settings");
  input.value = s.input_device ?? "";
  output.value = s.output_device ?? "";
  inject.value = s.inject;
  chime.value = s.message_sound;
  chimeAll.value = s.message_sound_all;
  chimeVolume.value = s.message_sound_volume;
  mic.value = s.mic_volume ?? 100;
  speaker.value = s.speaker_volume ?? 100;
  range.value = s.traffic_range_nm;
  cslDir.value = s.csl_dir;
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
  await dropGoneDevices();
  deviceTimer = window.setInterval(() => void refreshDevices(), 2000);
});
onUnmounted(() => {
  window.clearInterval(captureTimer);
  window.clearInterval(deviceTimer);
  // 关掉对话框就销毁这个组件，所以「录到一半」是关得掉的——而清掉那个 150 ms
  // 轮询并不会让 Rust 侧退出录制。不取消的话，对话框关着的时候按下的键会留在
  // 那里，下次一点「录制」立刻抓到它。
  if (capturing.value) void invoke("cancel_ptt_capture");
});

const applyDevices = () =>
  invoke("set_audio_devices", { input: input.value || null, output: output.value || null });

async function refreshDevices() {
  const devices = await invoke<{ input: DeviceInfo[]; output: DeviceInfo[] }>("audio_devices");
  inputs.value = devices.input ?? [];
  outputs.value = devices.output ?? [];
  await dropGoneDevices();
}

async function dropGoneDevices() {
  let changed = false;
  if (input.value && !inputs.value.some((d) => d.id === input.value)) {
    input.value = "";
    changed = true;
  }
  if (output.value && !outputs.value.some((d) => d.id === output.value)) {
    output.value = "";
    changed = true;
  }
  if (changed) await applyDevices();
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

const applyVolume = () =>
  invoke("set_master_volume", { mic: mic.value, speaker: speaker.value });

const applyInject = () => invoke("set_injection", { on: inject.value });

const applyChime = () => invoke("set_message_sound", { on: chime.value });
const applyChimeAll = () => invoke("set_message_sound_all", { on: chimeAll.value });

/** 夹过的那个数要回填：填 9999 之后该看到 200。 */
async function applyChimeVolume() {
  chimeVolume.value = await invoke<number>("set_message_sound_volume", {
    percent: Math.round(chimeVolume.value),
  });
}

/**
 * 试听。**用当前选着的设备和音量**，不是已经存下来的那份——用户多半正是刚换了
 * 耳机才来点这一下。所以先把设备和音量应用下去，再放。
 */
async function previewChime() {
  await applyDevices();
  await applyChimeVolume();
  await invoke("preview_chime");
}

/** 夹过的那个数要回填到框里：填 9999 之后该看到 500，而不是自己填的那个。 */
async function applyRange() {
  range.value = await invoke<number>("set_traffic_range", { nm: Math.round(range.value) });
}

/** 改完立刻重扫。不重扫的话，填对了路径的人做的这件事看起来毫无反应。 */
const applyCslDir = () => invoke("set_csl_dir", { dir: cslDir.value });

async function push() {
  await invoke("set_ptt_bindings", { bindings: bindings.value.map((b) => b.binding) });
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
}

/** 空栈也要听得到正在绑的那个键。 */
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
    } else if (waited >= 10_000) {
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
  <section class="flex flex-col gap-3 text-xs">
    <!-- 枢轴就是 `PilotPanel` 那个页签的写法：一个 ref、几个按钮、v-if/v-else。
         三页值不上一个分页组件。 -->
    <div class="flex gap-2">
      <button class="rounded border px-2 py-1" :class="page === 'audio' ? 'border-sky-500' : ''" @click="page = 'audio'">
        {{ t("local.audio") }}
      </button>
      <button class="rounded border px-2 py-1" :class="page === 'network' ? 'border-sky-500' : ''" @click="page = 'network'">
        {{ t("local.network") }}
      </button>
      <button class="rounded border px-2 py-1" :class="page === 'traffic' ? 'border-sky-500' : ''" @click="page = 'traffic'">
        {{ t("local.traffic") }}
      </button>
    </div>

    <div v-if="page === 'audio'" class="flex flex-col gap-3">
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("local.microphone") }}</span>
        <select v-model="input" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("local.system_default") }}</option>
          <option v-for="d in inputs" :key="d.id" :value="d.id">{{ d.name }}</option>
        </select>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("local.headset") }}</span>
        <select v-model="output" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("local.system_default") }}</option>
          <option v-for="d in outputs" :key="d.id" :value="d.id">{{ d.name }}</option>
        </select>
      </label>
      <p class="opacity-60">{{ t("local.devices_note") }}</p>
      <div class="flex items-center gap-2">
        <button class="rounded border px-3 py-1" :disabled="!!testing" @click="testSpeaker">
          {{ testing === "speaker" ? t("local.testing") : t("local.test_speaker") }}
        </button>
        <button class="rounded border px-3 py-1" :disabled="!!testing" @click="testMic">
          {{ testing === "mic" ? t("local.testing_mic") : t("local.test_mic") }}
        </button>
      </div>
      <p v-if="testErr" class="text-red-600">{{ testErr }}</p>
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("local.mic_volume") }}</span>
        <input type="range" min="0" max="200" step="1" v-model.number="mic" class="flex-1" @change="applyVolume" />
        <span class="w-10 text-right font-mono">{{ mic }}%</span>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("local.speaker_volume") }}</span>
        <input type="range" min="0" max="200" step="1" v-model.number="speaker" class="flex-1" @change="applyVolume" />
        <span class="w-10 text-right font-mono">{{ speaker }}%</span>
      </label>

      <div class="flex flex-col gap-2">
        <span class="font-semibold">{{ t("ptt.title") }}</span>
        <ul v-if="bindings.length" class="flex flex-col gap-1">
          <li v-for="(b, i) in bindings" :key="i" class="flex items-center gap-2 rounded border px-2 py-1">
            <span class="font-mono">{{ b.token || "…" }}</span>
            <span v-if="b.unresolved" class="text-red-600">{{ t("ptt.unresolved") }}</span>
            <button class="ml-auto rounded border px-2" @click="remove(i)">{{ t("ptt.remove") }}</button>
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
    </div>

    <div v-else-if="page === 'network'" class="flex flex-col gap-3">
      <LogPanel :cid="props.cid" />
    </div>

    <div v-else class="flex flex-col gap-3">
      <label class="flex items-center gap-2">
        <input v-model="inject" type="checkbox" @change="applyInject" />
        <span>{{ t("local.inject") }}</span>
        <span class="opacity-60">{{ t("local.inject_note") }}</span>
      </label>

      <label class="flex items-center gap-2">
        <input v-model="chime" type="checkbox" @change="applyChime" />
        <span>{{ t("local.chime") }}</span>
      </label>
      <label class="flex items-center gap-2">
        <input v-model="chimeAll" type="checkbox" @change="applyChimeAll" />
        <span>{{ t("local.chime_all") }}</span>
        <span class="opacity-60">{{ t("local.chime_all_note") }}</span>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("local.chime_volume") }}</span>
        <input
          v-model.number="chimeVolume"
          type="range"
          min="0"
          max="200"
          class="flex-1"
          @change="applyChimeVolume"
        />
        <span class="w-10 text-right opacity-70">{{ chimeVolume }}%</span>
        <button class="rounded border px-2 py-1" @click="previewChime">{{ t("local.preview") }}</button>
      </label>
      <p class="opacity-60">
        {{ t("local.chime_note") }}
      </p>

      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("local.range") }}</span>
        <input
          v-model.number="range"
          type="number"
          min="5"
          max="500"
          class="w-20 rounded border px-2 py-1"
          @change="applyRange"
        />
        <span class="opacity-60">
          {{ t("local.range_note") }}
        </span>
      </label>

      <!-- CSL 扫到几个要显示出来：扫不到的表现是"天上是空的"，和没装插件、
           和 UDP 不通长得一模一样，而三者要做的事完全不同。 -->
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("csl.dir") }}</span>
        <input
          v-model="cslDir"
          :placeholder="t('csl.dir_placeholder')"
          class="flex-1 rounded border px-2 py-1 font-mono"
          @change="applyCslDir"
          @keyup.enter="applyCslDir"
        />
        <button class="rounded border px-2 py-1" @click="applyCslDir">{{ t("local.rescan") }}</button>
      </label>
      <p v-if="props.csl" class="opacity-60">
        <template v-if="props.csl.loading">{{ t("csl.loading", { path: props.csl.root }) }}</template>
        <template v-else-if="props.csl.models">
          {{ t("csl.found", { count: props.csl.models, path: props.csl.root }) }}
        </template>
        <span v-else class="text-amber-700">
          {{ t("csl.empty", { path: props.csl.root }) }}
        </span>
      </p>

      <InstallWizard />
    </div>
  </section>
</template>

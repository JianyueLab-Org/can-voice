<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import InstallWizard from "./InstallWizard.vue";
import { t } from "../i18n";

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
/// `csl` 是扫模型那一侧的现状，由 App.vue 那份轮询回来的快照带进来；
/// `observer` 是观察员模式开着没有——观察员不上 FSD，拍发不了计划。
const props = defineProps<{ cid?: string; csl?: CslView; observer?: boolean }>();
import type { CslView, FlightPlan, Settings } from "../types";
import { emptyFlightPlan } from "../types";

interface BindingView {
  token: string;
  unresolved: boolean;
  binding: unknown;
}

const tab = ref<"plan" | "settings">("plan");
const plan = ref<FlightPlan>(emptyFlightPlan());
/**
 * 上一次拍发的结果，`null` = 还没拍发过。**存的是哪一种结果不是那句话**：
 * 存成句子的话，切了语言那一行还停在旧语言上。
 */
const filed = ref<"filed" | "offline" | null>(null);
const inputs = ref<string[]>([]);
const outputs = ref<string[]>([]);
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

let captureTimer: number | undefined;

onMounted(async () => {
  keyboardOk.value = await invoke<boolean>("keyboard_ptt_supported");
  mouseOk.value = await invoke<boolean>("mouse_ptt_supported");
  const devices = await invoke<{ input: string[]; output: string[] }>("audio_devices");
  inputs.value = devices.input;
  outputs.value = devices.output;
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
  plan.value.aircraft = s.aircraft;
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
});
onUnmounted(() => window.clearInterval(captureTimer));

async function file() {
  filed.value = (await invoke<boolean>("file_flight_plan", { plan: plan.value }))
    ? "filed"
    : "offline";
}

const applyDevices = () =>
  invoke("set_audio_devices", { input: input.value || null, output: output.value || null });

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

/** 捕获会把键盘监听拉起来：空栈按「只起已绑定的来源」是不会起 rdev 的。 */
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
    } else if (waited >= 10_000) {
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
  <section class="flex flex-col gap-3 rounded border p-3 text-xs">
    <div class="flex gap-2">
      <button class="rounded border px-2 py-1" :class="tab === 'plan' ? 'border-sky-500' : ''" @click="tab = 'plan'">
        {{ t("plan.tab") }}
      </button>
      <button class="rounded border px-2 py-1" :class="tab === 'settings' ? 'border-sky-500' : ''" @click="tab = 'settings'">
        {{ t("local.tab") }}
      </button>
    </div>

    <div v-if="tab === 'plan'" class="grid grid-cols-4 gap-2">
      <label class="flex flex-col gap-1">
        <span class="opacity-60">{{ t("plan.rules") }}</span>
        <select v-model="plan.rules" class="rounded border px-2 py-1">
          <option value="I">{{ t("plan.rules_i") }}</option>
          <option value="V">{{ t("plan.rules_v") }}</option>
          <option value="Y">{{ t("plan.rules_y") }}</option>
          <option value="Z">{{ t("plan.rules_z") }}</option>
        </select>
      </label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.aircraft") }}</span>
        <input v-model="plan.aircraft" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.cruise_speed") }}</span>
        <input v-model="plan.cruise_speed" placeholder="N0450" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.cruise_altitude") }}</span>
        <input v-model="plan.cruise_altitude" placeholder="F350" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.departure") }}</span>
        <input v-model="plan.departure" placeholder="ZSPD" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.arrival") }}</span>
        <input v-model="plan.arrival" placeholder="ZBAA" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.alternate") }}</span>
        <input v-model="plan.alternate" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.departure_time") }}</span>
        <input v-model="plan.departure_time" placeholder="1230" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.enroute_hours") }}</span>
        <input v-model="plan.enroute_hours" placeholder="02" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.enroute_minutes") }}</span>
        <input v-model="plan.enroute_minutes" placeholder="15" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.fuel_hours") }}</span>
        <input v-model="plan.fuel_hours" placeholder="04" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.fuel_minutes") }}</span>
        <input v-model="plan.fuel_minutes" placeholder="00" class="rounded border px-2 py-1" /></label>

      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">{{ t("plan.route") }}</span>
        <input v-model="plan.route" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">{{ t("plan.remarks") }}</span>
        <input v-model="plan.remarks" class="rounded border px-2 py-1" /></label>

      <div class="col-span-4 flex items-center gap-2">
        <!-- 观察员没有 FSD 连接，计划由机长那一端拍发。 -->
        <button class="rounded border px-3 py-1" :disabled="props.observer" @click="file">
          {{ t("plan.file") }}
        </button>
        <span v-if="props.observer" class="opacity-70">{{ t("plan.observer") }}</span>
        <span v-else-if="filed" class="opacity-70">
          {{ filed === "filed" ? t("plan.filed") : t("plan.offline") }}
        </span>
      </div>
    </div>

    <div v-else class="flex flex-col gap-3">
      <label class="flex items-center gap-2">
        <input v-model="inject" type="checkbox" @change="applyInject" />
        <span>{{ t("local.inject") }}</span>
        <span class="opacity-60">{{ t("local.inject_note") }}</span>
      </label>

      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("local.microphone") }}</span>
        <select v-model="input" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("local.system_default") }}</option>
          <option v-for="d in inputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">{{ t("local.headset") }}</span>
        <select v-model="output" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">{{ t("local.system_default") }}</option>
          <option v-for="d in outputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <p class="opacity-60">{{ t("local.devices_note") }}</p>
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

      <InstallWizard />

      <LogPanel :cid="props.cid" />
    </div>
  </section>
</template>

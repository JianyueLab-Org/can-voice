<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import { t } from "../i18n";

/**
 * 设置对话框里 msfs 自己那几段：音频 / 网络 / 他机三页（spec §6）。
 *
 * **它整个挂在 `SettingsDialog` 的 `v-if="open"` 里面**，每次打开都是新挂一次，
 * 关掉就销毁。所以 `onMounted` 就是「打开的时候读一遍」，`onUnmounted` 就是
 * 「关掉的时候停掉」——不要改成 watch，外层 `v-if` 已经决定了生命周期，再套一层
 * 只会多一条走不到的路。
 *
 * 「网络」那一页只有寄日志：服务器地址那几格在 `SettingsCommon` 里，就在这个枢轴
 * 的正上方；真实姓名和连不连在主界面的连接卡片上。
 *
 * **和 xpc 的同名文件是两份，不是一份，所以两份都不进 `SHARED_FRONTEND`。**
 * 三处真实差异：这里是机库不是 CSL（四种状态，而且一条都不印路径——路径就在同一行
 * 的输入框里）；**没有显示距离**（msfs 的 Rust 侧根本没有 `set_traffic_range` 这个
 * 命令，`Settings` 里也没有 `traffic_range_nm`，距离是常量 `MAX_RANGE_NM`）；
 * **没有安装向导**（msfs 走 SimConnect，没有插件这回事）。所以「他机」这一页比 xpc
 * 的少两格——**这是真实差异，不是漏了**，别照着 xpc 那份去补。
 */

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
/// `hangar` 是扫机库那一侧的现状，由 App.vue 那份轮询回来的快照带进来。
const props = defineProps<{ cid?: string; hangar?: HangarView }>();
import type { HangarView, Settings } from "../types";

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
const packagesDir = ref("");
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
  packagesDir.value = s.packages_dir;
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

/** 改完立刻重扫。不重扫的话，填对了路径的人做的这件事看起来毫无反应。 */
const applyPackagesDir = () => invoke("set_packages_dir", { dir: packagesDir.value });

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
    <!-- 枢轴沿用这个文件从前身 `PilotPanel.vue` 带过来的写法：一个 ref、几个按钮、
         v-if/v-else。三页值不上一个分页组件。

         **`page` 只能由下面这三个 @click 改，这一条是有承重的。** 他机页上那个机库
         目录输入框只在原生 change / keyup.enter 时才落盘，而切页的 v-if 会把它连同
         没提交的编辑一起拆掉。今天不丢，是因为点按钮必然先在 mousedown 把焦点挪走，
         blur → change 先跑，拆子树的 Vue handler 后跑。
         哪天有人用代码改 `page`（比如机库空的时候一键跳到他机页——本分支新加的那两条
         机库横幅正会让人想这么干），焦点不动，这一条就不成立了，编辑会无声丢掉，而且
         什么都不会报错。`SettingsDialog.vue` 里那个 close() 先 blur 再 emit 的写法，
         就是同一个坑在 Esc 上的解法。真要加代码改页，照着它先 blur。 -->
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

      <!-- 扫不到机模的表现是「他机全是同一架第一方飞机」，和机型码对不上长得差不多，
           所以三个数都要显示出来。**四种状态**：正在扫 / 扫到了 / 扫到涂装但一个机型码
           都没有（琥珀）/ 什么都没扫到（琥珀）。第三种是 msfs 独有的：读到的全是附加件
           那类没有机型码的配置，光看一个总数分不出它和「目录指错了」。
           **一条都不印路径**：路径就在上面那个输入框里，印第二遍只会把这一行撑长。 -->
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("hangar.dir") }}</span>
        <input
          v-model="packagesDir"
          :placeholder="t('hangar.dir_placeholder')"
          class="flex-1 rounded border px-2 py-1 font-mono"
          @change="applyPackagesDir"
          @keyup.enter="applyPackagesDir"
        />
        <button class="rounded border px-2 py-1" @click="applyPackagesDir">{{ t("local.rescan") }}</button>
      </label>
      <p v-if="props.hangar" class="opacity-60">
        <template v-if="props.hangar.loading">{{ t("hangar.loading") }}</template>
        <template v-else-if="props.hangar.types">
          {{
            t("hangar.found", {
              liveries: props.hangar.liveries,
              types: props.hangar.types,
              files: props.hangar.files,
            })
          }}
        </template>
        <span v-else-if="props.hangar.liveries" class="text-amber-700">
          {{ t("hangar.no_types", { liveries: props.hangar.liveries }) }}
        </span>
        <span v-else class="text-amber-700">
          {{ t("hangar.empty") }}
        </span>
      </p>
    </div>
  </section>
</template>

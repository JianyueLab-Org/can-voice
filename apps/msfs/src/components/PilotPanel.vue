<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";

/// 已经存下来的 CAN 号，寄日志时预填，省得再打一遍。
const props = defineProps<{ cid?: string; hangar?: HangarView }>();
import type { FlightPlan, Settings, HangarView } from "../types";
import { emptyFlightPlan } from "../types";

interface BindingView {
  token: string;
  unresolved: boolean;
  binding: unknown;
}

const tab = ref<"plan" | "settings">("plan");
const plan = ref<FlightPlan>(emptyFlightPlan());
const filed = ref("");
const inputs = ref<string[]>([]);
const outputs = ref<string[]>([]);
const input = ref("");
const output = ref("");
const inject = ref(true);
const packagesDir = ref("");
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
  packagesDir.value = s.packages_dir;
  plan.value.aircraft = s.aircraft;
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
});
onUnmounted(() => window.clearInterval(captureTimer));

async function file() {
  filed.value = (await invoke<boolean>("file_flight_plan", { plan: plan.value }))
    ? "已拍发"
    : "还没上线，先上线再拍发";
}

const applyDevices = () =>
  invoke("set_audio_devices", { input: input.value || null, output: output.value || null });

const applyInject = () => invoke("set_injection", { on: inject.value });

/** 改完立刻重扫。不重扫的话，填对了路径的人做的这件事看起来毫无反应。 */
const applyPackagesDir = () => invoke("set_packages_dir", { dir: packagesDir.value });

async function push() {
  await invoke("set_ptt_bindings", { bindings: bindings.value.map((b) => b.binding) });
  bindings.value = await invoke<BindingView[]>("ptt_bindings");
}

/** 监听器是**懒起**的：没推过绑定就没有东西在听，录制会永远录不到。 */
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
        飞行计划
      </button>
      <button class="rounded border px-2 py-1" :class="tab === 'settings' ? 'border-sky-500' : ''" @click="tab = 'settings'">
        设置
      </button>
    </div>

    <div v-if="tab === 'plan'" class="grid grid-cols-4 gap-2">
      <label class="flex flex-col gap-1">
        <span class="opacity-60">规则</span>
        <select v-model="plan.rules" class="rounded border px-2 py-1">
          <option value="I">I 仪表</option>
          <option value="V">V 目视</option>
          <option value="Y">Y 先仪后目</option>
          <option value="Z">Z 先目后仪</option>
        </select>
      </label>
      <label class="flex flex-col gap-1"><span class="opacity-60">机型</span>
        <input v-model="plan.aircraft" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">巡航速度</span>
        <input v-model="plan.cruise_speed" placeholder="N0450" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">巡航高度</span>
        <input v-model="plan.cruise_altitude" placeholder="F350" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">起飞机场</span>
        <input v-model="plan.departure" placeholder="ZSPD" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">目的机场</span>
        <input v-model="plan.arrival" placeholder="ZBAA" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">备降</span>
        <input v-model="plan.alternate" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">预计起飞 (UTC)</span>
        <input v-model="plan.departure_time" placeholder="1230" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">航路时间 时</span>
        <input v-model="plan.enroute_hours" placeholder="02" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">航路时间 分</span>
        <input v-model="plan.enroute_minutes" placeholder="15" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">燃油 时</span>
        <input v-model="plan.fuel_hours" placeholder="04" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">燃油 分</span>
        <input v-model="plan.fuel_minutes" placeholder="00" class="rounded border px-2 py-1" /></label>

      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">航路</span>
        <input v-model="plan.route" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">备注</span>
        <input v-model="plan.remarks" class="rounded border px-2 py-1" /></label>

      <div class="col-span-4 flex items-center gap-2">
        <button class="rounded border px-3 py-1" @click="file">拍发</button>
        <span class="opacity-70">{{ filed }}</span>
      </div>
    </div>

    <div v-else class="flex flex-col gap-3">
      <label class="flex items-center gap-2">
        <input v-model="inject" type="checkbox" @change="applyInject" />
        <span>把他机注入模拟器</span>
        <span class="opacity-60">关掉之后天上就只剩自己，语音不受影响</span>
      </label>

      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">麦克风</span>
        <select v-model="input" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">跟随系统默认</option>
          <option v-for="d in inputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <label class="flex items-center gap-2">
        <span class="w-16 opacity-70">耳机</span>
        <select v-model="output" class="flex-1 rounded border px-2 py-1" @change="applyDevices">
          <option value="">跟随系统默认</option>
          <option v-for="d in outputs" :key="d" :value="d">{{ d }}</option>
        </select>
      </label>
      <p class="opacity-60">换设备立刻生效。</p>

      <!-- 扫不到机模的表现是"他机都是同一架小飞机"，和没装机模、和机型码
           对不上长得差不多，所以三个数都要显示出来。 -->
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">包目录</span>
        <input
          v-model="packagesDir"
          placeholder="留空就自己去找（读 UserCfg.opt）"
          class="flex-1 rounded border px-2 py-1 font-mono"
          @change="applyPackagesDir"
          @keyup.enter="applyPackagesDir"
        />
        <button class="rounded border px-2 py-1" @click="applyPackagesDir">重扫</button>
      </label>
      <p v-if="props.hangar" class="opacity-60">
        <template v-if="props.hangar.loading">正在扫本机机库…</template>
        <template v-else-if="props.hangar.types">
          扫到 {{ props.hangar.liveries }} 个涂装、{{ props.hangar.types }} 种机型（读了
          {{ props.hangar.files }} 个 aircraft.cfg）
        </template>
        <span v-else-if="props.hangar.liveries" class="text-amber-700">
          扫到 {{ props.hangar.liveries }} 个涂装，但一个机型码都没有——多半读到的是
          附加件而不是飞机，把包目录填对再试。
        </span>
        <span v-else class="text-amber-700">
          没扫到任何机模，他机会全部退到内置的那几个第一方机型。包目录常常在另一块盘上，
          那就把路径填在这里。
        </span>
      </p>

      <div class="flex flex-col gap-2">
        <span class="font-semibold">按键发话（PTT）</span>
        <ul v-if="bindings.length" class="flex flex-col gap-1">
          <li v-for="(b, i) in bindings" :key="i" class="flex items-center gap-2 rounded border px-2 py-1">
            <span class="font-mono">{{ b.token || "…" }}</span>
            <span v-if="b.unresolved" class="text-red-600">这个绑定在本系统上认不出来，请重新录</span>
            <button class="ml-auto rounded border px-2" @click="remove(i)">删除</button>
          </li>
        </ul>
        <p v-else class="opacity-60">还没有绑定。</p>
        <button class="self-start rounded border px-3 py-1" :disabled="capturing" @click="capture">
          {{ capturing ? "按一下你要的键…" : "录制一个绑定" }}
        </button>
        <p v-if="!keyboardOk" class="text-red-600">
          本系统是 Wayland，普通程序不允许监听全局按键，键盘 PTT 不会响。请改用手柄。
        </p>
        <p v-if="!mouseOk" class="opacity-70">本系统不支持鼠标侧键作 PTT。</p>
      </div>

      <LogPanel :cid="props.cid" />
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import StationEditor from "./components/StationEditor.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import LogPanel from "./components/LogPanel.vue";
import NameDialog from "./components/NameDialog.vue";
import type { Live, Rendered, Station } from "./types";
import { callsignOf, stateText } from "./types";

const profiles = ref<{ names: string[]; active: string }>({ names: [], active: "" });
const stations = ref<Station[]>([]);
const selected = ref("");
const presetName = ref("");
const live = ref<Record<string, Live>>({});
const error = ref("");

const cid = ref("");
const password = ref("");
const sampleMetar = ref("ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG");
const preview = ref<Rendered | null>(null);

/** 当前打开的是哪个对话框。`null` 是没开。 */
const asking = ref<"profile" | "rename" | "station" | null>(null);

let timer: number | undefined;

const station = computed(() => stations.value.find((s) => callsignOf(s) === selected.value));
const current = computed<Live | undefined>(() => live.value[selected.value]);
const onAir = computed(() => selected.value in live.value);

/** 在播时看的是真正上线的那份，没上线时看预览。 */
const shown = computed<Rendered | null>(() => (current.value ? current.value : preview.value));

async function reload() {
  profiles.value = await invoke("profiles");
  stations.value = await invoke("stations");
  if (!stations.value.some((s) => callsignOf(s) === selected.value)) {
    selected.value = stations.value.length ? callsignOf(stations.value[0]) : "";
  }
  syncPreset();
  await refreshLive();
  await renderPreview();
}

async function refreshLive() {
  live.value = await invoke("live");
}

function syncPreset() {
  const s = station.value;
  if (!s) {
    presetName.value = "";
  } else if (!s.presets.some((p) => p.name === presetName.value)) {
    presetName.value = s.presets[0]?.name ?? "";
  }
}

/** 不连网也能看结果——配模板时要求先上线，等于让人对着猜出来的稿子调格式。 */
async function renderPreview() {
  const s = station.value;
  if (!s) {
    preview.value = null;
    return;
  }
  try {
    preview.value = await invoke("preview", {
      callsign: callsignOf(s),
      preset: presetName.value,
      metar: sampleMetar.value,
      letter: current.value?.letter ?? s.letter,
    });
  } catch (e) {
    error.value = String(e);
  }
}

async function guard(fn: () => Promise<unknown>) {
  error.value = "";
  try {
    await fn();
  } catch (e) {
    error.value = String(e);
  }
  await reload();
}

const save = () =>
  guard(async () => {
    const s = station.value;
    if (!s) return;
    // 呼号可能因为改了机场或类型而变，所以要把**原来那个**一起送过去。
    await invoke("save_station", { callsign: selected.value, station: s });
    selected.value = callsignOf(s);
  });

const addStation = (icao: string) =>
  guard(async () => {
    asking.value = null;
    const made = await invoke<Station>("add_station", { identifier: icao });
    selected.value = callsignOf(made);
  });

const removeStation = () =>
  guard(async () => {
    if (onAir.value) throw new Error("先停掉再删");
    await invoke("remove_station", { callsign: selected.value });
    selected.value = "";
  });

const start = () =>
  guard(() =>
    invoke("start", {
      callsign: selected.value,
      preset: presetName.value,
      cid: cid.value,
      password: password.value,
    }),
  );

const stop = () => guard(() => invoke("stop", { callsign: selected.value }));
const refresh = () => guard(() => invoke("refresh", { callsign: selected.value }));

const addProfile = (name: string) =>
  guard(async () => {
    asking.value = null;
    await invoke("add_profile", { name });
    await invoke("select_profile", { name });
  });

/** 改名。**命令一直都在**，只是没有地方按——建错名字的配置改不掉也删不掉。 */
const renameProfile = (name: string) =>
  guard(async () => {
    asking.value = null;
    await invoke("rename_profile", { old: profiles.value.active, new: name });
  });

const removeProfile = () =>
  guard(async () => {
    // 删掉的是一整份配置，问一句。这是这个界面上唯一不可撤销的动作。
    if (!window.confirm(`删除配置「${profiles.value.active}」？`)) return;
    await invoke("remove_profile", { name: profiles.value.active });
  });

const pickProfile = (name: string) => guard(() => invoke("select_profile", { name }));

watch([selected, presetName, sampleMetar], () => {
  syncPreset();
  void renderPreview();
});

// 上次用的 CAN 号预填。密码不存：它换的是一张短寿命的票。
async function loadSettings() {
  cid.value = (await invoke<{ cid: string }>("settings")).cid;
}

onMounted(async () => {
  await loadSettings();
  await reload();
  // 在播的那几路状态一直在变，轮询比订阅省事，也不会漏掉挂载之前发生的事。
  timer = window.setInterval(refreshLive, 1000);
});
onUnmounted(() => window.clearInterval(timer));
</script>

<template>
  <main class="mx-auto flex h-screen max-w-6xl flex-col gap-3 p-4 text-sm">
    <header class="flex items-center gap-3">
      <h1 class="text-base font-semibold">通播制作</h1>
      <select
        :value="profiles.active"
        class="rounded border px-2 py-1 text-xs"
        @change="pickProfile(($event.target as HTMLSelectElement).value)"
      >
        <option v-for="n in profiles.names" :key="n">{{ n }}</option>
      </select>
      <button class="rounded border px-2 py-1 text-xs" @click="asking = 'profile'">新配置</button>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="!profiles.active"
        @click="asking = 'rename'"
      >
        改名
      </button>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="profiles.names.length < 2"
        @click="removeProfile"
      >
        删除
      </button>
      <div class="ml-auto flex items-center gap-2">
        <input v-model="cid" placeholder="CAN 号" class="w-24 rounded border px-2 py-1 text-xs" />
        <input
          v-model="password"
          type="password"
          placeholder="密码"
          class="w-28 rounded border px-2 py-1 text-xs"
        />
      </div>
    </header>

    <UpdateBanner />

    <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ error }}
    </p>

    <div class="flex min-h-0 flex-1 gap-4">
      <!-- 席位列表 -->
      <aside class="flex w-52 flex-col gap-1 overflow-auto">
        <button
          v-for="s in stations"
          :key="callsignOf(s)"
          class="flex items-center gap-2 rounded border px-2 py-1 text-left font-mono text-xs"
          :class="callsignOf(s) === selected ? 'border-sky-500' : ''"
          @click="selected = callsignOf(s)"
        >
          <span
            class="h-2 w-2 shrink-0 rounded-full"
            :class="live[callsignOf(s)]?.state === 'Online' ? 'bg-green-500' : 'bg-neutral-300'"
          />
          <span class="truncate">{{ callsignOf(s) }}</span>
          <span v-if="live[callsignOf(s)]" class="ml-auto opacity-60">
            {{ live[callsignOf(s)].letter }}
          </span>
        </button>
        <button class="rounded border border-dashed px-2 py-1 text-xs" @click="asking = 'station'">
          + 新席位
        </button>

        <!-- 折叠着：平时不占地方，出了问题才展开。 -->
        <details class="mt-auto rounded border px-2 py-1 text-xs">
          <summary class="cursor-pointer opacity-70">日志</summary>
          <div class="pt-2">
            <LogPanel :cid="cid" />
          </div>
        </details>
      </aside>

      <!-- 编辑 -->
      <div class="flex min-w-0 flex-1 flex-col gap-3 overflow-auto">
        <StationEditor
          v-if="station"
          :station="station"
          :preset-name="presetName"
          @change="save"
          @pick="(n) => (presetName = n)"
          @remove="removeStation"
        />
        <p v-else class="py-8 text-center text-xs opacity-50">左边挑一个席位，或者新建一个</p>
      </div>

      <!-- 稿子 -->
      <aside class="flex w-80 flex-col gap-2 overflow-auto border-l pl-4">
        <div class="flex items-center gap-2">
          <span class="text-xs font-semibold">{{ stateText(current) }}</span>
          <span v-if="current" class="font-mono text-xs opacity-60">{{ current.letter }}</span>
          <button
            v-if="!onAir"
            class="ml-auto rounded border px-3 py-1 text-xs"
            :disabled="!station"
            @click="start"
          >
            上线
          </button>
          <template v-else>
            <button class="ml-auto rounded border px-2 py-1 text-xs" @click="refresh">
              取报文
            </button>
            <button class="rounded border px-2 py-1 text-xs" @click="stop">停止</button>
          </template>
        </div>

        <label class="flex flex-col gap-1">
          <span class="text-xs opacity-60">
            {{ onAir ? "服务端给的报文" : "试算用的报文" }}
          </span>
          <textarea
            v-if="!onAir"
            v-model="sampleMetar"
            rows="3"
            class="rounded border px-2 py-1 font-mono text-xs"
          />
          <pre v-else class="rounded border px-2 py-1 font-mono text-xs whitespace-pre-wrap">{{
            current?.metar || "还没拿到"
          }}</pre>
        </label>

        <template v-if="shown">
          <div>
            <p class="text-xs opacity-60">文字通播（飞行员读的）</p>
            <pre class="rounded border px-2 py-1 text-xs whitespace-pre-wrap">{{ shown.text }}</pre>
          </div>
          <div>
            <p class="text-xs opacity-60">英文语音稿</p>
            <pre class="rounded border px-2 py-1 text-xs whitespace-pre-wrap">{{
              shown.voice_en
            }}</pre>
          </div>
          <div v-if="station && station.voice_language !== 'en'">
            <p class="text-xs opacity-60">中文语音稿</p>
            <pre class="rounded border px-2 py-1 text-xs whitespace-pre-wrap">{{
              shown.voice_zh
            }}</pre>
          </div>
          <!-- 声音归服务端机队。这一支只做稿子，所以要让人看见线上那份长什么样。 -->
          <details>
            <summary class="cursor-pointer text-xs opacity-60">发上 FSD 的那一份</summary>
            <pre class="rounded border px-2 py-1 font-mono text-xs whitespace-pre-wrap">{{
              shown.wire
            }}</pre>
          </details>
        </template>
      </aside>
    </div>

    <NameDialog
      :open="asking === 'profile'"
      title="新配置的名字"
      placeholder="例如：浦东"
      @confirm="addProfile"
      @cancel="asking = null"
    />
    <NameDialog
      :open="asking === 'rename'"
      title="改配置的名字"
      :initial="profiles.active"
      @confirm="renameProfile"
      @cancel="asking = null"
    />
    <NameDialog
      :open="asking === 'station'"
      title="新席位"
      placeholder="机场四字码，例如 ZSPD"
      @confirm="addStation"
      @cancel="asking = null"
    />
  </main>
</template>

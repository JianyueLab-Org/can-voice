<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import StationEditor from "./components/StationEditor.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import LogPanel from "./components/LogPanel.vue";
import NameDialog from "./components/NameDialog.vue";
import NetworkDialog from "./components/NetworkDialog.vue";
import type { ImportReport, Live, Merged, NetworkPreview, Rendered, Station } from "./types";
import { callsignOf, describeMerge, stateText } from "./types";

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);

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
/** 模板里认不出来的变量。认不出的是**照字面念出去**的，所以要说出来。 */
const problems = ref<string[]>([]);
const refreshSecs = ref(300);
const rating = ref(0);

/** 导入 / 取配置之后给人看的结果。和 `error` 分开：这不是出错。 */
const notice = ref<string[]>([]);
/** 正在跑的那个外部请求。按钮据此变灰，免得连点出两次请求。 */
const busy = ref<"metar" | "vatis" | "online" | "network" | null>(null);
const network = ref<NetworkPreview | null>(null);
const vatisFile = ref<HTMLInputElement | null>(null);

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
    problems.value = [];
    return;
  }
  // 模板拼错要当场说。`[RWY]` 打成 `[RUNWAY]` 的话，飞行员听到的是一句
  // "runway" 后面跟着中括号里那个词，而稿子看起来一切正常。
  const template = s.presets.find((p) => p.name === presetName.value)?.template ?? "";
  problems.value = await invoke<string[]>("template_problems", { template });
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

/** 在播时换构型。**不必停掉重上**——重上的那几十秒里飞行员查不到通播。 */
const switchPreset = (name: string) =>
  guard(() => invoke("set_preset", { callsign: selected.value, preset: name }));

/** 手动推进一格字母。播错了、或者报文没变但场面条件变了，都靠它。 */
const bumpLetter = () => guard(() => invoke("advance_letter", { callsign: selected.value }));

/** 夹过的那个数要回填：填 5 之后界面上该看到 60。 */
async function applyRefresh() {
  refreshSecs.value = await invoke<number>("set_metar_refresh", {
    secs: Math.round(refreshSecs.value),
  });
}

const applyRating = () => invoke("set_rating", { rating: rating.value });

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

/** 跑一个外部请求：清掉上一次的提示、按钮变灰、失败了说出为什么。 */
async function fetching(kind: NonNullable<typeof busy.value>, fn: () => Promise<void>) {
  error.value = "";
  notice.value = [];
  busy.value = kind;
  try {
    await fn();
  } catch (e) {
    error.value = String(e);
  } finally {
    busy.value = null;
  }
  await reload();
}

/**
 * 取一份真实报文来试算，**不上线也能取**。
 *
 * 没有它的话"先把稿子写好再上线"做不成：只能对着一份编出来的电码调模板。
 */
const fetchMetar = () =>
  fetching("metar", async () => {
    const s = station.value;
    if (!s) return;
    sampleMetar.value = await invoke<string>("fetch_metar", { icao: s.identifier });
  });

/**
 * 导入 vATIS 配置。文件由这一侧读出来递给 Rust——为一个选文件的框引对话框插件、
 * 再开一条文件系统权限，不值得。
 */
async function importVatis(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0];
  // 清掉，否则同一个文件改完再选一次不会触发 change。
  input.value = "";
  if (!file) return;
  await fetching("vatis", async () => {
    const report = await invoke<ImportReport>("import_vatis", { body: await file.text() });
    notice.value = [
      `从 vATIS 配置「${report.source || file.name}」导入`,
      ...describeMerge(report.merged),
      ...report.notes,
    ];
  });
}

/** 数据源上此刻在线的通播席位。**只有机场和频率**，模板和构型它给不了。 */
const importOnline = () =>
  fetching("online", async () => {
    const merged = await invoke<Merged>("import_online");
    notice.value = [
      "从数据源导入在线通播席位",
      ...describeMerge(merged),
      ...(merged.added.length ? ["新加的席位用的是默认模板：构型和 NOTAM 要自己补，或者取一次网络配置。"] : []),
    ];
  });

/** 取全网配置，**只看差异**。动手在对话框里。 */
const checkNetwork = () =>
  fetching("network", async () => {
    network.value = await invoke<NetworkPreview>("check_network_config");
  });

const applyNetwork = (addMissing: boolean, overwrite: boolean) =>
  fetching("network", async () => {
    network.value = null;
    const merged = await invoke<Merged>("apply_network_config", { addMissing, overwrite });
    notice.value = ["并入全网通播配置", ...describeMerge(merged)];
  });

watch([selected, presetName, sampleMetar], () => {
  syncPreset();
  void renderPreview();
});

// 在播时换构型就真的换过去。挂着的席位改了下拉框却没生效，是那种"点了没反应"
// 的故障——而它恰好发生在管制员正忙着换跑道的时候。
watch(presetName, (name, was) => {
  if (name && was && name !== was && onAir.value) void switchPreset(name);
});

// 上次用的 CAN 号预填。密码不存：它换的是一张短寿命的票。
async function loadSettings() {
  const s = await invoke<{ cid: string; metar_refresh_secs: number; rating: number }>("settings");
  cid.value = s.cid;
  refreshSecs.value = s.metar_refresh_secs;
  rating.value = s.rating;
}

onMounted(async () => {
  void loadAppearance();
  await loadSettings();
  await reload();
  // 在播的那几路状态一直在变，轮询比订阅省事，也不会漏掉挂载之前发生的事。
  timer = window.setInterval(refreshLive, 1000);
});
onUnmounted(() => window.clearInterval(timer));
</script>

<template>
  <main
    class="mx-auto flex h-screen max-w-6xl flex-col text-sm"
    :class="compact ? 'gap-2 p-2' : 'gap-3 p-4'"
  >
    <header class="flex items-center gap-3">
      <template v-if="!compact">
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
      </template>
      <!-- 精简时也在：藏掉的话精简之后就切不回来了。 -->
      <WindowToggles class="ml-auto" @settings="showPrefs = true" />
    </header>

    <UpdateBanner v-if="!compact" />

    <p v-if="error" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
      {{ error }}
    </p>
    <div
      v-if="notice.length"
      class="flex items-start gap-2 rounded border border-sky-400 px-3 py-2 text-xs"
    >
      <div class="flex flex-col gap-0.5">
        <p v-for="(line, i) in notice" :key="i" :class="i === 0 ? 'font-semibold' : ''">
          {{ line }}
        </p>
      </div>
      <button class="ml-auto opacity-60" title="关掉" @click="notice = []">×</button>
    </div>

    <div class="flex min-h-0 flex-1 gap-4">
      <!-- 席位列表 -->
      <!-- 精简时只留席位列表：在播的哪几个、各自是哪个字母，就是值班时要盯的。 -->
      <aside class="flex flex-col gap-1 overflow-auto" :class="compact ? 'flex-1' : 'w-52'">
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
        <button
          v-if="!compact"
          class="rounded border border-dashed px-2 py-1 text-xs"
          @click="asking = 'station'"
        >
          + 新席位
        </button>
        <details class="rounded border px-2 py-1 text-xs">
          <summary class="cursor-pointer opacity-70">导入</summary>
          <div class="flex flex-col gap-1 pt-2">
            <!-- 配置本身：席位、频率、构型预设、模板、中文用词。先看差异再并。 -->
            <button
              class="rounded border px-2 py-1 text-left"
              :disabled="busy !== null"
              @click="checkNetwork"
            >
              {{ busy === "network" ? "正在取…" : "全网通播配置…" }}
            </button>
            <button
              class="rounded border px-2 py-1 text-left"
              :disabled="busy !== null"
              @click="vatisFile?.click()"
            >
              {{ busy === "vatis" ? "正在导入…" : "vATIS 配置文件…" }}
            </button>
            <input
              ref="vatisFile"
              type="file"
              accept=".json,application/json"
              class="hidden"
              @change="importVatis"
            />
            <!-- 运行状态，不是配置：只省掉查机场和频率这一步。 -->
            <button
              class="rounded border px-2 py-1 text-left"
              :disabled="busy !== null"
              title="只有机场和频率，模板和构型要另外补"
              @click="importOnline"
            >
              {{ busy === "online" ? "正在取…" : "此刻在线的通播席位" }}
            </button>
          </div>
        </details>

        <!-- 折叠着：平时不占地方。 -->
        <details v-if="!compact" class="mt-auto rounded border px-2 py-1 text-xs">
          <summary class="cursor-pointer opacity-70">播出</summary>
          <div class="flex flex-col gap-2 pt-2">
            <label class="flex items-center gap-2">
              <span class="opacity-60">报文周期</span>
              <input
                v-model.number="refreshSecs"
                type="number"
                min="60"
                max="3600"
                class="w-20 rounded border px-1 py-0.5"
                @change="applyRefresh"
              />
              <span class="opacity-60">秒</span>
            </label>
            <label class="flex items-center gap-2">
              <span class="opacity-60">登录等级</span>
              <select
                v-model.number="rating"
                class="flex-1 rounded border px-1 py-0.5"
                @change="applyRating"
              >
                <!-- 自动是默认：写死观察员的话，一个 C1 开的通播在雷达图上
                     显示成观察员，而管制席位上的同一个人是 C1。 -->
                <option :value="0">自动（跟随本人）</option>
                <option :value="1">OBS</option>
                <option :value="2">S1</option>
                <option :value="3">S2</option>
                <option :value="4">S3</option>
                <option :value="5">C1</option>
                <option :value="7">C3</option>
                <option :value="8">I1</option>
                <option :value="10">I3</option>
                <option :value="11">SUP</option>
              </select>
            </label>
            <p class="opacity-50">等级下次上线才生效。</p>
          </div>
        </details>

        <details v-if="!compact" class="rounded border px-2 py-1 text-xs">
          <summary class="cursor-pointer opacity-70">日志</summary>
          <div class="pt-2">
            <LogPanel :cid="cid" />
          </div>
        </details>
      </aside>

      <!-- 编辑 -->
      <div v-if="!compact" class="flex min-w-0 flex-1 flex-col gap-3 overflow-auto">
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
      <aside v-if="!compact" class="flex w-80 flex-col gap-2 overflow-auto border-l pl-4">
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
            <!-- 在播时也能换构型：停掉重上的那几十秒里飞行员查不到通播，
                 而那恰恰是管制员正忙着换跑道的时候。换构型连带推进字母——
                 跑道变了就是另一份通播。 -->
            <select
              v-model="presetName"
              class="ml-auto rounded border px-2 py-1 text-xs"
              title="换一套跑道构型（会推进一格字母）"
            >
              <option v-for="p in station?.presets ?? []" :key="p.name">{{ p.name }}</option>
            </select>
            <button
              class="rounded border px-2 py-1 text-xs"
              title="播错了，或者报文没变但场面条件变了"
              @click="bumpLetter"
            >
              推字母
            </button>
            <button class="rounded border px-2 py-1 text-xs" @click="refresh">取报文</button>
            <button class="rounded border px-2 py-1 text-xs" @click="stop">停止</button>
          </template>
        </div>

        <!-- 认不出的变量是照字面念出去的：`[RWY]` 打成 `[RUNWAY]`，飞行员听到的
             就是一句 "runway" 后面跟着中括号里那个词，而稿子看起来一切正常。 -->
        <p
          v-if="problems.length"
          class="rounded border border-amber-400 px-2 py-1 text-xs text-amber-700"
        >
          模板里这几个变量认不出来，会被原样念出去：{{ problems.join("、") }}
        </p>

        <label class="flex flex-col gap-1">
          <span class="flex items-center text-xs opacity-60">
            {{ onAir ? "服务端给的报文" : "试算用的报文" }}
            <button
              v-if="!onAir"
              class="ml-auto rounded border px-2 py-0.5"
              :disabled="!station || busy !== null"
              title="从气象源取这个机场此刻的真实报文，不用上线"
              @click.prevent="fetchMetar"
            >
              {{ busy === "metar" ? "正在取…" : "取真实报文" }}
            </button>
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
    <NetworkDialog :preview="network" @apply="applyNetwork" @cancel="network = null" />
    <NameDialog
      :open="asking === 'station'"
      title="新席位"
      placeholder="机场四字码，例如 ZSPD"
      @confirm="addStation"
      @cancel="asking = null"
    />
    <SettingsDialog :open="showPrefs" @close="showPrefs = false" />
  </main>
</template>

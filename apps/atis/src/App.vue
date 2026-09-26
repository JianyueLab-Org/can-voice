<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import WindowToggles from "./components/WindowToggles.vue";
import Splitter from "./components/Splitter.vue";
import StationList from "./components/StationList.vue";
import SettingsDialog from "./components/SettingsDialog.vue";
import { appearance, loadAppearance } from "./appearance";
import { invoke } from "@tauri-apps/api/core";
import StationDialog from "./components/StationDialog.vue";
import PresetDialog from "./components/PresetDialog.vue";
import UpdateBanner from "./components/UpdateBanner.vue";
import StartupGate from "./components/StartupGate.vue";
import AiringPanel from "./components/AiringPanel.vue";
import NameDialog from "./components/NameDialog.vue";
import NetworkDialog from "./components/NetworkDialog.vue";
import type {
  ImportReport,
  Live,
  Merged,
  NetworkPreview,
  Notice,
  Rendered,
  Station,
} from "./types";
import { callsignOf, noticeLines, stateText } from "./types";
import { errorText, t, type Key, type Message } from "./i18n";

/** 设置对话框开没开。 */
const showPrefs = ref(false);
/** 精简模式：只留值班时要盯的东西。开关在 WindowToggles 里，真相在设置文件里。 */
const compact = computed(() => appearance.value.compact);
/** 左栏宽度。**只活在会话里**，和 can-audio 一样不写进设置文件。 */
const paneWidth = ref(260);
// 退出精简时回到 260，和 can-audio 的 `splitter.setSizes([260, 640])` 同一个动作
// （`atis/gui.py:513`）：精简里左栏被拉成整个窗口宽，出来之后不重置就是一栏顶天。
watch(compact, (on) => {
  if (!on) paneWidth.value = 260;
});

const profiles = ref<{ names: string[]; active: string }>({ names: [], active: "" });
const stations = ref<Station[]>([]);
const selected = ref("");
const presetName = ref("");
const live = ref<Record<string, Live>>({});
/**
 * 上一次失败交回来的东西，**原样存着**（#29）：Rust 给的是 `Message`，在模板里用
 * `errorText` 现翻，切了语言跟着变。`null` 是没出错。
 */
const error = ref<unknown>(null);

const cid = ref("");
const password = ref("");
const sampleMetar = ref("ZSPD 251300Z 09004MPS 9999 FEW030 SCT100 25/18 Q1013 NOSIG");
const preview = ref<Rendered | null>(null);
/** 模板里认不出来的变量。认不出的是**照字面念出去**的，所以要说出来。 */
const problems = ref<string[]>([]);

/** 导入 / 取配置之后给人看的结果。和 `error` 分开：这不是出错。 */
const notice = ref<Notice | null>(null);
/** 提示的那几行。computed 里现翻，所以切了语言跟着变。 */
const noticeText = computed(() => (notice.value ? noticeLines(notice.value) : []));
/** 正在跑的那个外部请求。按钮据此变灰，免得连点出两次请求。 */
const busy = ref<"metar" | "vatis" | "online" | "network" | null>(null);
const network = ref<NetworkPreview | null>(null);
const vatisFile = ref<HTMLInputElement | null>(null);

/** 当前打开的是哪个对话框。`null` 是没开。一次只可能开一个。 */
const asking = ref<"profile" | "rename" | "station" | "edit" | "preset" | null>(null);

let timer: number | undefined;
let refreshInFlight = false;
let reloadInFlight = false;

const station = computed(() => stations.value.find((s) => callsignOf(s) === selected.value));
/**
 * 对话框绑的是**这一个对象**，不是每次拿 `station` 现查。改机场或类型是直接改
 * `station.identifier`/`atis_type`，字母还没打完 `callsignOf` 就先变了，
 * `station` 计算属性眼下按 `selected` 查不到——`<StationDialog v-if="station">`
 * 会连着输入光标一起卸载重建，`save()` 也会因为拿到 `undefined` 而静悄悄地不存。
 *
 * 这里缓存"上一次查到的那个对象"，只在查得到的时候才更新，查不到的间隙里维持
 * 原值——原值还是同一个对象，只是暂时按呼号查不到。`save()` 已经先把 `selected`
 * 指到新呼号，`reload()` 把 `stations` 换成新数组之后 `station` 会用新呼号重新
 * 查到，这里的 watch 跟着自动指到新对象，不用另外写重新指向的代码。
 */
const editingStation = ref<Station | undefined>(undefined);
watch(station, (s) => {
  if (s) {
    editingStation.value = s;
  } else if (editingStation.value && !stations.value.includes(editingStation.value)) {
    // 按 `selected` 查不到分两种情况：打字打到一半、派生呼号暂时对不上——这时
    // `editingStation` 指的对象还在 `stations` 数组里，原样留着，等 `save()` 把
    // `selected` 指到新呼号就会自然查到。真正的情况是这个位置已经不在当前
    // `stations` 里了（切换了配置、或者这个位置被删掉了），这时候才清掉，
    // 不然对话框会挂着一个属于别的配置的对象继续可编辑。
    editingStation.value = undefined;
  }
});
const current = computed<Live | undefined>(() => live.value[selected.value]);
const onAir = computed(() => selected.value in live.value);
const editedPreset = computed(() => station.value?.presets.find((p) => p.name === presetName.value));
const chineseShown = computed(() => station.value?.voice_language !== "en");

/** 字母行那句话。席位一定有字母，但在播那一份可能还没报上来。 */
const letterText = computed(() => {
  const letter = current.value?.letter ?? station.value?.letter;
  return letter ? t("draft.letter", { letter }) : t("draft.letter_none");
});

/** 在播时看的是真正上线的那份，没上线时看预览。 */
const shown = computed<Rendered | null>(() => (current.value ? current.value : preview.value));

async function reload() {
  if (reloadInFlight) return;
  reloadInFlight = true;
  try {
    profiles.value = await invoke("profiles");
    stations.value = await invoke("stations");
    if (!stations.value.some((s) => callsignOf(s) === selected.value)) {
      selected.value = stations.value.length ? callsignOf(stations.value[0]) : "";
    }
    syncPreset();
    await refreshLive();
    await renderPreview();
  } catch (e) {
    error.value = e;
  } finally {
    reloadInFlight = false;
  }
}

async function refreshLive() {
  if (refreshInFlight) return;
  refreshInFlight = true;
  const request = invoke<Record<string, Live>>("live");
  void request.then(
    () => { refreshInFlight = false; },
    () => { refreshInFlight = false; },
  );
  try {
    live.value = await Promise.race([
      request,
      new Promise<never>((_, reject) => window.setTimeout(() => reject(new Error("refresh timeout")), 1500)),
    ]);
  } catch (e) {
    error.value = e;
  }
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
    error.value = e;
  }
}

async function guard(fn: () => Promise<unknown>) {
  error.value = null;
  try {
    await fn();
  } catch (e) {
    error.value = e;
  }
  await reload();
}

const save = () =>
  guard(async () => {
    // 用 `editingStation`，不用 `station`：改机场或类型的时候，打字打到一半
    // `station` 会按新呼号查不到东西，直接用它会白白吞掉这次编辑。
    const s = editingStation.value;
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
    // 抛的是 key 不是一句话：和 Rust 交回来的错误同一个形状，显示时才翻。
    if (onAir.value) throw { key: "problem.station.stop_first" satisfies Key } satisfies Message;
    asking.value = null;
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
    if (!window.confirm(t("profile.confirm_remove", { name: profiles.value.active }))) return;
    await invoke("remove_profile", { name: profiles.value.active });
  });

const pickProfile = (name: string) =>
  guard(async () => {
    // 切配置就把开着的对话框关掉：模态没有焦点陷阱，键盘能直接跳到这个下拉框，
    // 不清掉的话对话框会带着上一个配置的位置继续开着，改一下就存进新配置里。
    asking.value = null;
    await invoke("select_profile", { name });
  });

/** 跑一个外部请求：清掉上一次的提示、按钮变灰、失败了说出为什么。 */
async function fetching(kind: NonNullable<typeof busy.value>, fn: () => Promise<void>) {
  error.value = null;
  notice.value = null;
  busy.value = kind;
  try {
    await fn();
  } catch (e) {
    error.value = e;
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
    notice.value = { kind: "vatis", source: report.source || file.name, report };
  });
}

/** 数据源上此刻在线的通播席位。**只有机场和频率**，模板和构型它给不了。 */
const importOnline = () =>
  fetching("online", async () => {
    const merged = await invoke<Merged>("import_online");
    notice.value = { kind: "online", merged };
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
    notice.value = { kind: "network", merged };
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
  const s = await invoke<{ cid: string }>("settings");
  cid.value = s.cid;
}

onMounted(() => {
  void init();
});

async function init() {
  void loadAppearance().catch((e) => { error.value = e; });
  timer = window.setInterval(() => void refreshLive(), 1000);
  try {
    await loadSettings();
    await reload();
  } catch (e) {
    error.value = e;
  }
}
onUnmounted(() => window.clearInterval(timer));
</script>

<template>
  <StartupGate>
    <main
      class="app-shell mx-auto flex h-screen max-w-6xl flex-col text-sm"
      :class="compact ? 'gap-2 p-2' : 'gap-3 p-4'"
    >
      <!-- 精简时两个分支都是假：账号行藏在 !compact 里，WindowToggles 只给了
           settings 一个钮，而它自己在精简模式下也不画。整个头就不挂，省下一份
           flex gap——320px 高的窗口里这一条不是小事。置顶和精简两个钮不在这，
           在左栏标题行，精简模式下切回来走的是那条路。 -->
      <header v-if="!compact" class="flex shrink-0 items-center gap-2">
        <select
          :value="profiles.active"
          class="rounded border px-2 py-1 text-xs"
          @change="pickProfile(($event.target as HTMLSelectElement).value)"
        >
          <option v-for="n in profiles.names" :key="n">{{ n }}</option>
        </select>
        <button class="rounded border px-2 py-1 text-xs" @click="asking = 'profile'">
          {{ t("profile.new") }}
        </button>
        <button
          class="rounded border px-2 py-1 text-xs"
          :disabled="!profiles.active"
          @click="asking = 'rename'"
        >
          {{ t("profile.rename") }}
        </button>
        <button
          class="rounded border px-2 py-1 text-xs"
          :disabled="profiles.names.length < 2"
          @click="removeProfile"
        >
          {{ t("profile.remove") }}
        </button>
        <span class="ml-auto text-xs opacity-60">{{ t("login.account") }}</span>
        <input
          v-model="cid"
          :placeholder="t('login.cid')"
          class="w-[110px] rounded border px-2 py-1 text-xs"
        />
        <input
          v-model="password"
          type="password"
          :placeholder="t('login.password')"
          class="w-[160px] rounded border px-2 py-1 text-xs"
        />
        <WindowToggles class="ml-auto" :only="['settings']" @settings="showPrefs = true" />
      </header>

      <UpdateBanner v-if="!compact" />

      <p v-if="error !== null" class="rounded border border-red-400 px-3 py-2 text-xs text-red-600">
        {{ errorText(error) }}
      </p>
      <div
        v-if="noticeText.length"
        class="flex items-start gap-2 rounded border border-sky-400 px-3 py-2 text-xs"
      >
        <div class="flex flex-col gap-0.5">
          <p v-for="(line, i) in noticeText" :key="i" :class="i === 0 ? 'font-semibold' : ''">
            {{ line }}
          </p>
        </div>
        <button class="ml-auto opacity-60" :title="t('dialog.dismiss')" @click="notice = null">
          ×
        </button>
      </div>

      <Splitter v-model:width="paneWidth" :collapsed="compact">
        <template #left>
          <div class="flex min-h-0 flex-1 flex-col gap-2">
            <header class="flex shrink-0 items-center gap-2">
              <h2 class="text-sm font-semibold">{{ t("station.list_title") }}</h2>
              <WindowToggles class="ml-auto" :only="['on_top', 'compact']" />
            </header>

            <StationList
              :stations="stations"
              :live="live"
              :selected="selected"
              @pick="selected = $event"
            />

            <!-- 钉成固定高度：不钉的话它会和席位列表抢纵向空间，窗口一矮列表就没了
                 （can-audio 的 `setSizePolicy(Preferred, Fixed)`，`atis/gui.py:368`）。 -->
            <div v-if="!compact" class="flex shrink-0 flex-col gap-1">
              <div class="flex gap-1">
                <button class="flex-1 rounded border px-2 py-1 text-xs" @click="asking = 'station'">
                  {{ t("station.new") }}
                </button>
                <button
                  class="flex-1 rounded border px-2 py-1 text-xs"
                  :disabled="!station"
                  @click="asking = 'edit'"
                >
                  {{ t("station.edit") }}
                </button>
                <button
                  class="flex-1 rounded border px-2 py-1 text-xs"
                  :disabled="!station"
                  @click="removeStation"
                >
                  {{ t("station.delete") }}
                </button>
              </div>
              <button
                class="rounded border px-2 py-1 text-left text-xs"
                :disabled="busy !== null"
                :title="t('import.network_tip')"
                @click="checkNetwork"
              >
                {{ busy === "network" ? t("busy.fetching") : t("import.network") }}
              </button>
              <button
                class="rounded border px-2 py-1 text-left text-xs"
                :disabled="busy !== null"
                :title="t('import.online_tip')"
                @click="importOnline"
              >
                {{ busy === "online" ? t("busy.fetching") : t("import.online") }}
              </button>
              <button
                class="rounded border px-2 py-1 text-left text-xs"
                :disabled="busy !== null"
                @click="vatisFile?.click()"
              >
                {{ busy === "vatis" ? t("busy.importing") : t("import.vatis") }}
              </button>
              <input
                ref="vatisFile"
                type="file"
                accept=".json,application/json"
                class="hidden"
                @change="importVatis"
              />
            </div>
          </div>
        </template>

        <template #right>
          <!-- 稿子 -->
          <aside class="flex min-h-0 flex-1 flex-col gap-2">
            <template v-if="station">
              <!-- 1. 预设行 -->
              <div class="flex shrink-0 items-center gap-2">
                <span class="text-xs opacity-60">{{ t("preset.label") }}</span>
                <select
                  v-model="presetName"
                  class="min-w-0 flex-1 rounded border px-2 py-1 text-xs"
                  :title="t('draft.preset_tip')"
                >
                  <option v-for="p in station.presets" :key="p.name" :value="p.name">{{ p.name }}</option>
                </select>
                <button
                  class="rounded border px-2 py-1 text-xs"
                  :disabled="!editedPreset"
                  @click="asking = 'preset'"
                >
                  {{ t("preset.edit") }}
                </button>
              </div>

              <!-- 2. 字母行 -->
              <div class="flex shrink-0 items-center gap-2">
                <span class="text-sm font-semibold">{{ letterText }}</span>
                <span class="ml-auto"></span>
                <button
                  class="rounded border px-2 py-1 text-xs"
                  :disabled="!onAir"
                  :title="t('draft.bump_tip')"
                  @click="bumpLetter"
                >
                  {{ t("draft.bump") }}
                </button>
                <button
                  class="rounded border px-2 py-1 text-xs"
                  :disabled="busy !== null"
                  :title="onAir ? undefined : t('draft.fetch_metar_tip')"
                  @click="onAir ? refresh() : fetchMetar()"
                >
                  {{ busy === "metar" ? t("busy.fetching") : onAir ? t("draft.refresh") : t("draft.fetch_metar") }}
                </button>
              </div>

              <!-- 3. METAR：在播看服务端那一份（只读），不在播是可以改的试算电码 -->
              <label class="flex shrink-0 flex-col gap-1">
                <span class="text-xs opacity-60">{{ onAir ? t("draft.metar_live") : t("draft.metar_sample") }}</span>
                <textarea
                  v-if="!onAir"
                  v-model="sampleMetar"
                  rows="2"
                  class="w-full rounded border px-2 py-1 font-mono text-xs"
                />
                <pre v-else class="w-full whitespace-pre-wrap rounded border px-2 py-1 font-mono text-xs">{{
                  current?.metar || t("draft.no_metar")
                }}</pre>
              </label>

              <!-- 4. 文字通播：固定 90px，can-audio 的 setFixedHeight(90) -->
              <div class="flex shrink-0 flex-col gap-1">
                <span class="text-xs font-semibold">{{ t("draft.text") }}</span>
                <pre
                  class="h-[90px] w-full overflow-auto whitespace-pre-wrap rounded border px-2 py-1 font-mono text-xs"
                  >{{ shown?.text ?? "" }}</pre
                >
              </div>

              <!-- 5. 语音稿：占满剩下的高度，装中英两份 -->
              <div class="flex min-h-0 flex-1 flex-col gap-1">
                <span class="shrink-0 text-xs font-semibold">{{ t("draft.voice") }}</span>
                <div class="flex min-h-0 flex-1 flex-col gap-1 overflow-auto rounded border p-2">
                  <span class="text-xs opacity-60">{{ t("draft.voice_en") }}</span>
                  <p class="whitespace-pre-wrap text-xs">{{ shown?.voice_en ?? "" }}</p>
                  <template v-if="chineseShown">
                    <span class="mt-2 text-xs opacity-60">{{ t("draft.voice_zh") }}</span>
                    <p class="whitespace-pre-wrap text-xs">{{ shown?.voice_zh ?? "" }}</p>
                  </template>
                </div>
              </div>

              <!-- 6. 播出行 -->
              <div class="flex shrink-0 items-center gap-2">
                <button
                  class="rounded px-3 py-1 text-xs text-white"
                  :style="{ background: onAir ? 'var(--can-muted)' : 'var(--can-on)' }"
                  @click="onAir ? stop() : start()"
                >
                  {{ onAir ? t("draft.stop") : t("draft.start") }}
                </button>
                <span class="min-w-0 flex-1 truncate text-xs opacity-70">{{ stateText(current) }}</span>
              </div>

              <!-- 告警和 wire 转储：can-audio 没有，留在播出行下面 -->
              <!-- 认不出的变量是照字面念出去的：`[RWY]` 打成 `[RUNWAY]`，飞行员听到的
                   就是一句 "runway" 后面跟着中括号里那个词，而稿子看起来一切正常。 -->
              <p v-if="problems.length" class="shrink-0 text-xs" :style="{ color: 'var(--can-active)' }">
                {{ t("draft.unknown_variables", { list: problems.join(t("common.separator.list")) }) }}
              </p>
              <!-- 声音归服务端机队。这一支只做稿子，所以要让人看见线上那份长什么样。 -->
              <details v-if="shown" class="shrink-0 text-xs">
                <summary class="cursor-pointer opacity-60">{{ t("draft.wire") }}</summary>
                <pre class="mt-1 whitespace-pre-wrap font-mono text-xs">{{ shown.wire }}</pre>
              </details>
            </template>
            <p v-else class="py-8 text-center text-xs opacity-50">{{ t("station.pick") }}</p>
          </aside>
        </template>
      </Splitter>

      <NameDialog
        :open="asking === 'profile'"
        :title="t('profile.new_title')"
        :placeholder="t('profile.new_placeholder')"
        @confirm="addProfile"
        @cancel="asking = null"
      />
      <NameDialog
        :open="asking === 'rename'"
        :title="t('profile.rename_title')"
        :initial="profiles.active"
        @confirm="renameProfile"
        @cancel="asking = null"
      />
      <NetworkDialog :preview="network" @apply="applyNetwork" @cancel="network = null" />
      <NameDialog
        :open="asking === 'station'"
        :title="t('station.new_title')"
        :placeholder="t('station.new_placeholder')"
        @confirm="addStation"
        @cancel="asking = null"
      />
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <AiringPanel :cid="cid" />
      </SettingsDialog>
      <StationDialog
        v-if="editingStation"
        :open="asking === 'edit'"
        :station="editingStation"
        :preset-name="presetName"
        :error="error"
        @close="asking = null"
        @change="save"
        @pick="presetName = $event"
        @remove="removeStation"
      />
      <PresetDialog
        v-if="editedPreset"
        :open="asking === 'preset'"
        :preset="editedPreset"
        :chinese-shown="chineseShown"
        :error="error"
        @close="asking = null"
        @change="save"
      />
    </main>
  </StartupGate>
</template>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import { t } from "../i18n";

/// 已经存下来的 CAN 号，寄日志时预填。
const props = defineProps<{ cid?: string }>();

const refreshSecs = ref(300);
const rating = ref(0);

// 这个组件挂在 `SettingsDialog` 的 `v-if="open"` 里面，**每次打开都是新挂一次**,
// 所以 `onMounted` 就是"打开的时候读一遍"。不要改成 watch——外层 `v-if` 已经决定了
// 生命周期，再套一层只会多一条走不到的路（`SettingsCommon` 那条 `{ immediate: true }`
// 是因为它读的是自己的属性，不是自己的挂载）。
onMounted(async () => {
  const s = await invoke<{ metar_refresh_secs: number; rating: number }>("settings");
  refreshSecs.value = s.metar_refresh_secs;
  rating.value = s.rating;
});

/** 夹过的那个数要回填：填 5 之后界面上该看到 60。 */
async function applyRefresh() {
  refreshSecs.value = await invoke<number>("set_metar_refresh", {
    secs: Math.round(refreshSecs.value),
  });
}

const applyRating = () => invoke("set_rating", { rating: rating.value });
</script>

<template>
  <section class="flex flex-col gap-4 text-xs">
    <div class="flex flex-col gap-2">
      <h3 class="font-semibold">{{ t("airing.title") }}</h3>
      <label class="flex items-center gap-2">
        <span class="opacity-60">{{ t("airing.refresh") }}</span>
        <input
          v-model.number="refreshSecs"
          type="number"
          min="60"
          max="3600"
          class="w-20 rounded border px-1 py-0.5"
          @change="applyRefresh"
        />
        <span class="opacity-60">{{ t("airing.seconds") }}</span>
      </label>
      <label class="flex items-center gap-2">
        <span class="opacity-60">{{ t("airing.rating") }}</span>
        <select
          v-model.number="rating"
          class="flex-1 rounded border px-1 py-0.5"
          @change="applyRating"
        >
          <!-- 自动是默认：写死观察员的话，一个 C1 开的通播在雷达图上
               显示成观察员，而管制席位上的同一个人是 C1。 -->
          <option :value="0">{{ t("airing.rating_auto") }}</option>
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
      <p class="opacity-50">{{ t("airing.rating_note") }}</p>
    </div>

    <LogPanel :cid="props.cid" />
  </section>
</template>

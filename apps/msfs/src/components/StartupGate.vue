<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { t } from "../i18n";

/** 和 Rust 侧 `can_voice_autoupdate::State` 一一对应。 */
type State =
  | { phase: "checking" }
  | { phase: "downloading"; received: number; total: number }
  | { phase: "installing" }
  | { phase: "done" };

const open = ref(false);
const state = ref<State>({ phase: "checking" });

let unlisten: UnlistenFn | null = null;
let firstEvent: ReturnType<typeof setTimeout> | null = null;

function mb(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(0)} MB`;
}

const message = () => {
  const s = state.value;
  if (s.phase === "downloading") {
    return s.total > 0
      ? t("update.downloading_sized", { done: mb(s.received), total: mb(s.total) })
      : t("update.downloading");
  }
  if (s.phase === "installing") return t("update.installing");
  return t("update.checking");
};

onMounted(async () => {
  // 只有 `done` 放行。**但如果 Rust 那边一个事件都没发**——它崩了，或者这个
  // 版本根本没装更新器——就不能永远挡着：启动不能被更新拖住。所以第一个事件
  // 之前有一道短的兜底，收到任何事件之后就取消，免得把一个正常的长下载切断。
  firstEvent = setTimeout(() => {
    open.value = true;
  }, 8_000);

  try {
    unlisten = await listen<State>("update://state", (event) => {
      if (firstEvent) {
        clearTimeout(firstEvent);
        firstEvent = null;
      }
      state.value = event.payload;
      if (event.payload.phase === "done") open.value = true;
    });
  } catch {
    open.value = true;
  }
});

onUnmounted(() => {
  if (firstEvent) clearTimeout(firstEvent);
  unlisten?.();
});
</script>

<template>
  <slot v-if="open" />
  <p v-else class="startup">{{ message() }}</p>
</template>

<style scoped>
.startup {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100vh;
  margin: 0;
  font-size: 0.875rem;
  opacity: 0.7;
}
</style>

<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { t } from "../i18n";

/** 和 Rust 侧 `can_voice_autoupdate::State` 一一对应。 */
type State =
  | { phase: "checking" }
  | { phase: "downloading"; received: number; total: number }
  | { phase: "installing" }
  | { phase: "done" };

const open = ref(false);
const state = ref<State>({ phase: "checking" });

let timer: number | null = null;
const MAX_WAIT_MS = 10_000;
let startedAt = 0;
let pollInFlight = false;

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

function stop() {
  if (timer !== null) {
    window.clearInterval(timer);
    timer = null;
  }
}

/**
 * 问一次 Rust 那边走到哪了。
 *
 * **轮询而不是听事件。** `start` 在 `.setup()` 里跑，最常见的两条路——这个包
 * 不能自己替换自己、或者还没配 `plugins.updater`——几微秒就完事了，那时 webview
 * 连自己的包都还没加载完，`onMounted` 没跑，监听器不存在，而 tauri 不会为还没
 * 起来的页面补发事件。状态是拉过来的，晚到多久都读得到。
 *
 * 问不出来就放行：启动不能被更新拖住。
 */
async function poll() {
  if (pollInFlight) return;
  if (startedAt !== 0 && Date.now() - startedAt >= MAX_WAIT_MS) {
    open.value = true;
    stop();
    return;
  }
  pollInFlight = true;
  const request = invoke<State>("update_state");
  void request.then(
    () => {
      pollInFlight = false;
    },
    () => {
      pollInFlight = false;
    },
  );
  try {
    state.value = await Promise.race([
      request,
      new Promise<never>((_, reject) =>
        window.setTimeout(() => reject(new Error("update check timeout")), 1500),
      ),
    ]);
    if (state.value.phase === "done") {
      open.value = true;
      stop();
    }
  } catch {
    open.value = true;
    stop();
  }
}

onMounted(() => {
  startedAt = Date.now();
  void poll();
  // 和各应用主刷新循环同一拍。
  timer = window.setInterval(() => void poll(), 200);
});

onUnmounted(stop);
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

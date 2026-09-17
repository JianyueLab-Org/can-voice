<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { errorText, t } from "../i18n";

/** 和 Rust 侧 `can_voice_update::Latest` 一一对应。 */
interface Latest {
  version: string;
  /** 下载地址。**指向 can-api 的中转**，不是 GitHub——大陆连 GitHub 拉 60 MB 经常断。 */
  download: string;
  notes: string;
  size: number;
}

const latest = ref<Latest | null>(null);
/** 打不开时 Rust 交回来的原样，渲染时才翻——切了语言跟着变。 */
const problem = ref<unknown>(null);

// 启动时查一次。**查不到就安静**：连不上更新服务不值得一个对话框，
// 更不该拖慢启动——Rust 侧每条错误路径都返回"没有更新"，这里只是再兜一层。
onMounted(async () => {
  try {
    latest.value = await invoke<Latest | null>("check_update");
  } catch {
    latest.value = null;
  }
});

const size = computed(() =>
  latest.value?.size ? `${(latest.value.size / 1_000_000).toFixed(0)} MB` : "",
);

/** 交给系统浏览器。Rust 侧只放行 https，别的地址会被挡回来。 */
async function open(url: string) {
  try {
    await invoke("open_download", { url });
  } catch (e) {
    problem.value = e;
  }
}

const download = () => (latest.value ? open(latest.value.download) : undefined);

/** 跳过的是**这一个版本**，不是从此闭嘴：下一版照样提示。 */
async function skip() {
  if (!latest.value) return;
  await invoke("skip_update", { version: latest.value.version });
  latest.value = null;
}
</script>

<template>
  <p
    v-if="latest"
    class="flex flex-wrap items-center gap-2 rounded border border-sky-400 px-3 py-2 text-xs"
  >
    <!-- 括号在译文里：中文是全角括号，英文不是。 -->
    <span class="flex-1">
      {{
        size
          ? t("update.available_sized", { version: latest.version, size })
          : t("update.available", { version: latest.version })
      }}
    </span>
    <!-- 不用 `<a target="_blank">`：在 webview 里那会把应用自己导航走，
         整个界面被一个发行说明页顶掉，而且回不来。 -->
    <button v-if="latest.notes" class="underline underline-offset-2" @click="open(latest.notes)">
      {{ t("update.notes") }}
    </button>
    <button class="rounded border px-2 py-0.5" @click="download">{{ t("update.download") }}</button>
    <button class="rounded border px-2 py-0.5" @click="skip">{{ t("update.skip") }}</button>
  </p>
  <p v-if="problem" class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700">
    {{ errorText(problem) }}
  </p>
</template>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { errorText, t } from "../i18n";

const props = defineProps<{ cid?: string }>();

const path = ref<string | null>(null);
const cid = ref("");
const password = ref("");
const busy = ref(false);
/**
 * 发送结果。**存的是"哪一种结果"而不是那句话**：存成句子的话，切了语言那一行
 * 还停在旧语言上。
 */
const outcome = ref<{ ok: true } | { ok: false; error: unknown } | null>(null);

onMounted(async () => {
  path.value = await invoke<string | null>("log_file");
  cid.value = props.cid ?? "";
});

/**
 * 把日志寄回去。
 *
 * **要密码**：can-api 的 `/api/v1/logs` 认的是 CAN 号加网络密码，不是会话——
 * 桌面端手里只有这一对。用完就丢，不进设置文件。
 */
async function send() {
  busy.value = true;
  outcome.value = null;
  try {
    await invoke("send_log", { cid: cid.value, password: password.value });
    outcome.value = { ok: true };
    password.value = "";
  } catch (e) {
    outcome.value = { ok: false, error: e };
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <span class="font-semibold">{{ t("log.title") }}</span>
    <!-- 路径要显示出来：用户要自己去翻那个文件的时候，"在哪"是第一个问题。 -->
    <p v-if="path" class="break-all font-mono text-xs opacity-70">{{ path }}</p>
    <p v-else class="text-xs text-amber-700">{{ t("log.no_file") }}</p>
    <p class="text-xs opacity-60">{{ t("log.explain") }}</p>
    <div class="flex flex-wrap items-center gap-2">
      <input v-model="cid" :placeholder="t('log.cid')" class="w-24 rounded border px-2 py-1 text-xs" />
      <input
        v-model="password"
        type="password"
        :placeholder="t('log.password')"
        class="w-32 rounded border px-2 py-1 text-xs"
        @keyup.enter="send"
      />
      <button
        class="rounded border px-3 py-1 text-xs"
        :disabled="busy || !path || !cid || !password"
        @click="send"
      >
        {{ busy ? t("log.sending") : t("log.send") }}
      </button>
    </div>
    <p v-if="outcome" class="text-xs" :class="outcome.ok ? 'opacity-70' : 'text-amber-700'">
      {{ outcome.ok ? t("log.sent") : errorText(outcome.error) }}
    </p>
  </div>
</template>

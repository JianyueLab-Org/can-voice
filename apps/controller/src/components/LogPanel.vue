<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";

const props = defineProps<{ cid?: string }>();

const path = ref<string | null>(null);
const cid = ref("");
const password = ref("");
const busy = ref(false);
const note = ref("");
const ok = ref(false);

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
  note.value = "";
  ok.value = false;
  try {
    await invoke("send_log", { cid: cid.value, password: password.value });
    note.value = "日志已经寄出去了。";
    ok.value = true;
    password.value = "";
  } catch (e) {
    note.value = String(e);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <span class="font-semibold">日志</span>
    <!-- 路径要显示出来：用户要自己去翻那个文件的时候，"在哪"是第一个问题。 -->
    <p v-if="path" class="break-all font-mono text-xs opacity-70">{{ path }}</p>
    <p v-else class="text-xs text-amber-700">
      这台机器上没能建起日志文件（目录不可写）。程序照常运行，但出了问题没有记录可发。
    </p>
    <p class="text-xs opacity-60">
      出了问题可以把日志寄给维护者。要 CAN 号和密码，因为服务端认的是这一对；
      密码只用这一次，不会存下来。
    </p>
    <div class="flex flex-wrap items-center gap-2">
      <input v-model="cid" placeholder="CAN 号" class="w-24 rounded border px-2 py-1 text-xs" />
      <input
        v-model="password"
        type="password"
        placeholder="密码"
        class="w-32 rounded border px-2 py-1 text-xs"
        @keyup.enter="send"
      />
      <button
        class="rounded border px-3 py-1 text-xs"
        :disabled="busy || !path || !cid || !password"
        @click="send"
      >
        {{ busy ? "寄送中…" : "发送日志" }}
      </button>
    </div>
    <p v-if="note" class="text-xs" :class="ok ? 'opacity-70' : 'text-amber-700'">{{ note }}</p>
  </div>
</template>

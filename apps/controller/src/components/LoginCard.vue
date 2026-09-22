<script setup lang="ts">
import { ref, watch } from "vue";
import { t } from "../i18n";

/**
 * 登录页。can-audio `controller/gui.py:470-524` 的第 0 页：居中一张 340px 的卡片。
 *
 * **独立成页，不挤在顶栏里。** 换客户端的那一天，人第一眼看到的东西要和旧版
 * 一样——旧版是两页 QStackedWidget，连上之后才换成台面。
 *
 * 卡片下面那行字既是状态也是报错，旧版就是这么用的一个 CaptionLabel。
 *
 * 只有管制端用：通播端把账号放在顶栏一行里，两个飞行员端放在"连接"卡片里。
 */
const props = defineProps<{
  /** 预填的 CAN 号。设置文件里存着上次那个。 */
  cid: string;
  status: string;
  /** 报错时着红。 */
  failed: boolean;
  busy: boolean;
  /** 卡片外面那行版本号。不翻译——要和日志里那一行对得上。 */
  version: string;
}>();

const emit = defineEmits<{ connect: [cid: string, password: string] }>();

const cid = ref(props.cid);
const password = ref("");

watch(
  () => props.cid,
  (v) => {
    // 只在用户还没动过这一格时填：settings 那一趟比 StartupGate 慢，
    // 填晚了也不能把人正在打的字盖掉。
    if (!cid.value) cid.value = v;
  },
);

function submit() {
  if (props.busy) return;
  emit("connect", cid.value, password.value);
  // 密码用过就丢：它只换一张 60 秒的票。
  password.value = "";
}
</script>

<template>
  <!-- `flex-1 min-h-0`，不是 `h-full`：顶上还有一排窗口开关和更新横幅，
       写死 100% 高的话这张卡会把它们顶出窗口。 -->
  <div class="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 overflow-auto">
    <!-- `max-w-full`：窗口可以比这张卡窄——存着的精简模式在启动时就应用上去了
         （`src-tauri/src/lib.rs` 的 setup），于是登录页会出现在一个 248px 的窗口里，
         固定 340px 会横向溢出到看不见按钮。 -->
    <div class="flex w-[340px] max-w-full flex-col gap-3 rounded-lg border px-6 py-5">
      <h1 class="text-center text-base font-semibold">{{ t("app.title") }}</h1>
      <input
        v-model="cid"
        :placeholder="t('login.cid')"
        class="rounded border px-2 py-1"
        @keyup.enter="submit"
      />
      <input
        v-model="password"
        type="password"
        :placeholder="t('login.password')"
        class="rounded border px-2 py-1"
        @keyup.enter="submit"
      />
      <button
        class="rounded border px-3 py-1 font-semibold text-white"
        :style="{ background: 'var(--can-theme)' }"
        :class="busy ? 'opacity-60' : ''"
        :disabled="busy"
        @click="submit"
      >
        {{ t("login.connect") }}
      </button>
      <p class="text-center text-xs" :class="failed ? 'text-red-600' : 'opacity-60'">
        {{ status }}
      </p>
    </div>
    <p class="text-xs opacity-50">{{ version }}</p>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { InstallStatus } from "../types";
import { installText } from "../types";

const status = ref<InstallStatus | null>(null);
const installs = ref<string[]>([]);
const root = ref("");
const busy = ref(false);
const note = ref("");
const ok = ref(false);

onMounted(async () => {
  installs.value = await invoke<string[]>("xplane_installs");
  await look(null);
  // Rust 那边已经把"记住的、再不然自动探测到的"那个挑出来了，照着回填，
  // 免得自动探测成功的人还要自己再填一遍。
  root.value = status.value?.root ?? "";
});

/** 看一眼现状。`null` = 让 Rust 那边决定看哪个目录。 */
async function look(which: string | null) {
  status.value = await invoke<InstallStatus>("plugin_install_status", { root: which });
}

const inspect = () => look(root.value || null);

function pick(path: string) {
  root.value = path;
  void look(path);
}

async function install() {
  busy.value = true;
  note.value = "";
  ok.value = false;
  try {
    const path = await invoke<string>("install_plugin", { root: root.value });
    note.value = `装好了：${path}。X-Plane 正开着的话，要重启它才会加载。`;
    ok.value = true;
  } catch (e) {
    note.value = String(e);
  } finally {
    busy.value = false;
    await look(root.value || null);
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <span class="font-semibold">X-Plane 他机插件</span>
    <p class="opacity-60">
      没有它，语音和上线都正常，但天上一架别人的飞机都不会出现。
    </p>

    <p>
      <span :class="status?.state === 'Current' ? 'text-green-700' : ''">
        {{ installText(status) }}
      </span>
    </p>

    <!-- XPPython3 是另一个包，这里不代装：它是编译出来的二进制，版本还跟
         模拟器绑（XP12 要 v4.x，11.52 要 v3.1.5）。所以只把话说清楚。 -->
    <p v-if="status && !status.xppython3 && status.state !== 'NoRoot' && status.state !== 'NotXplane'"
       class="rounded border border-amber-400 px-2 py-1 text-amber-700">
      这个目录里没有 XPPython3。插件装了也不会跑——它跑在 XPPython3 里。
      请先去装 XPPython3（X-Plane 12 用 v4.x，11.52 用 v3.1.5，装错大版本是静默不工作）。
    </p>

    <!-- 协议号对不上时插件静默丢弃每一帧，两端日志都干净。这是最难自查的
         一种故障，所以单独说，而不是只讲一句"版本旧"。 -->
    <p v-if="status?.protocol_mismatch" class="rounded border border-red-400 px-2 py-1 text-red-600">
      装着的那份说协议 {{ status.installed_protocol }}，这个客户端说
      {{ status.bundled_protocol }}。对不上时插件会丢掉每一帧，天上不会有任何飞机，
      而两边日志都是干净的——按下面的按钮更新。
    </p>

    <!-- 自动探测经常什么也探不到（绿色版、搬过目录、装在另一块盘上），
         所以自己填那一栏必须一直留着，不是探测失败才出现。 -->
    <label class="flex items-center gap-2">
      <span class="w-20 shrink-0 opacity-70">X-Plane 目录</span>
      <input
        v-model="root"
        placeholder="例如 /Applications/X-Plane 12"
        class="flex-1 rounded border px-2 py-1 font-mono"
        @keyup.enter="inspect"
      />
      <button class="rounded border px-2 py-1" @click="inspect">看一下</button>
    </label>

    <ul v-if="installs.length" class="flex flex-wrap gap-1">
      <li v-for="p in installs" :key="p">
        <button class="rounded border px-2 py-0.5 font-mono opacity-80" @click="pick(p)">
          {{ p }}
        </button>
      </li>
    </ul>

    <div class="flex items-center gap-2">
      <button
        class="rounded border px-3 py-1"
        :disabled="busy || !status?.can_install"
        @click="install"
      >
        {{ status?.state === "Missing" ? "安装" : "重新安装" }}
      </button>
      <span v-if="status?.path" class="font-mono opacity-60">{{ status.path }}</span>
    </div>

    <p v-if="note" :class="ok ? 'text-green-700' : 'text-red-600'">{{ note }}</p>
  </div>
</template>

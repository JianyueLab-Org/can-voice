<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import type { InstallStatus } from "../types";
import { installText } from "../types";
import { errorText, t } from "../i18n";

const status = ref<InstallStatus | null>(null);
const installs = ref<string[]>([]);
const root = ref("");
const busy = ref(false);
/**
 * 上一次安装的结果。**存的是装到了哪、或者原样的错误，不是那句话**：
 * 存成句子的话，切了语言那一行还停在旧语言上。
 */
const outcome = ref<{ ok: true; path: string } | { ok: false; error: unknown } | null>(null);
/**
 * 安装目录里带着的那份插件所在的文件夹。装不进去时叫人自己去拷它。
 * `null` = 没找到——那就不指，指着一个空文件夹比不指更糟。
 */
const bundledDir = ref<string | null>(null);
const copied = ref(false);
const bundledPath = ref<HTMLElement | null>(null);

onMounted(async () => {
  bundledDir.value = await invoke<string | null>("bundled_plugin_dir");
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

/**
 * 把那个文件夹的路径放进剪贴板，让人贴进文件管理器的地址栏。
 *
 * 给的是文件夹不是文件：Windows 上把一个 `.py` 的路径贴进资源管理器，关联了
 * Python 的机器会直接**运行**它。打开文件夹要多装一个 Tauri 插件，不值得。
 */
async function copyBundledDir() {
  if (!bundledDir.value) return;
  try {
    await navigator.clipboard.writeText(bundledDir.value);
    copied.value = true;
  } catch {
    // webview 不给剪贴板时退一步：把路径整段选中，让人自己按复制。
    if (bundledPath.value) window.getSelection()?.selectAllChildren(bundledPath.value);
  }
}

async function install() {
  busy.value = true;
  outcome.value = null;
  copied.value = false;
  try {
    const path = await invoke<string>("install_plugin", { root: root.value });
    outcome.value = { ok: true, path };
  } catch (e) {
    outcome.value = { ok: false, error: e };
  } finally {
    busy.value = false;
    await look(root.value || null);
  }
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <span class="font-semibold">{{ t("plugin.title") }}</span>
    <p class="opacity-60">
      {{ t("plugin.why") }}
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
      {{ t("plugin.no_xppython3") }}
    </p>

    <!-- 协议号对不上时插件静默丢弃每一帧，两端日志都干净。这是最难自查的
         一种故障，所以单独说，而不是只讲一句"版本旧"。 -->
    <p v-if="status?.protocol_mismatch" class="rounded border border-red-400 px-2 py-1 text-red-600">
      {{
        t("plugin.protocol_mismatch", {
          installed: status.installed_protocol ?? "—",
          bundled: status.bundled_protocol,
        })
      }}
    </p>

    <!-- 自动探测经常什么也探不到（绿色版、搬过目录、装在另一块盘上），
         所以自己填那一栏必须一直留着，不是探测失败才出现。 -->
    <label class="flex items-center gap-2">
      <span class="w-20 shrink-0 opacity-70">{{ t("plugin.root") }}</span>
      <input
        v-model="root"
        :placeholder="t('plugin.root_placeholder')"
        class="flex-1 rounded border px-2 py-1 font-mono"
        @keyup.enter="inspect"
      />
      <button class="rounded border px-2 py-1" @click="inspect">{{ t("plugin.inspect") }}</button>
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
        {{ status?.state === "Missing" ? t("plugin.install") : t("plugin.reinstall") }}
      </button>
      <span v-if="status?.path" class="font-mono opacity-60">{{ status.path }}</span>
    </div>

    <p v-if="outcome" :class="outcome.ok ? 'text-green-700' : 'text-red-600'">
      {{ outcome.ok ? t("plugin.installed", { path: outcome.path }) : errorText(outcome.error) }}
    </p>

    <!-- 装不进去多半是权限：X-Plane 在 Program Files 这类要管理员权限的地方，程序
         自己提不了权，人可以。安装包里带着一份（tauri.conf.json 的 bundle.resources），
         所以就地告诉他在哪、拷到哪，而不是叫他回下载页找那个 zip。 -->
    <div
      v-if="outcome && !outcome.ok && bundledDir"
      class="flex flex-col gap-1 rounded border border-amber-400 px-2 py-1"
    >
      <p>
        {{
          t("plugin.manual.explain", {
            file: "PI_XpcTraffic.py",
            target: "Resources/plugins/PythonPlugins/",
          })
        }}
      </p>
      <div class="flex items-center gap-2">
        <!-- 插值贴着标签写：两边留了空白，选中复制出来的路径就带着空格。 -->
        <span ref="bundledPath" class="flex-1 select-all break-all font-mono opacity-80">{{ bundledDir }}</span>
        <button class="shrink-0 rounded border px-2 py-0.5" @click="copyBundledDir">
          {{ copied ? t("plugin.manual.copied") : t("plugin.manual.copy") }}
        </button>
      </div>
    </div>
  </div>
</template>

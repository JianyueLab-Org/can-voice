<script setup lang="ts">
import { ref, watch } from "vue";
import type { NetworkPreview } from "../types";

/**
 * 全网通播配置：**先看差异，再勾要并的**。
 *
 * 补缺和覆盖分开勾。补缺几乎总是想要的；覆盖却会盖掉值班时手改过的临时构型和
 * NOTAM，所以它默认不勾，不能被一个"确定"顺手点掉。
 */
const props = defineProps<{ preview: NetworkPreview | null }>();
const emit = defineEmits<{ apply: [addMissing: boolean, overwrite: boolean]; cancel: [] }>();

const addMissing = ref(true);
const overwrite = ref(false);

watch(
  () => props.preview,
  () => {
    addMissing.value = true;
    overwrite.value = false;
  },
);

const list = (names: string[]) =>
  names.length > 12 ? `${names.slice(0, 12).join("、")} 等 ${names.length} 个` : names.join("、");
</script>

<template>
  <div
    v-if="preview"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
    @click.self="emit('cancel')"
  >
    <div class="flex w-[28rem] flex-col gap-3 rounded border bg-white p-4 text-sm dark:bg-neutral-900">
      <h2 class="font-semibold">全网通播配置 {{ preview.label }}</h2>
      <p v-if="preview.notes" class="text-xs">{{ preview.notes }}</p>
      <p v-if="preview.previous && preview.previous !== preview.version" class="text-xs opacity-60">
        上次并到的是 {{ preview.previous }}
      </p>
      <p
        v-if="preview.problems.length"
        class="rounded border border-amber-400 px-2 py-1 text-xs text-amber-700"
      >
        有 {{ preview.problems.length }} 个席位读不进来：{{ preview.problems.slice(0, 3).join("；") }}
      </p>

      <p v-if="!preview.missing.length && !preview.differing.length" class="text-xs">
        本地已经是这一版（{{ preview.same.length }} 个席位一致）。
      </p>

      <label v-if="preview.missing.length" class="flex items-start gap-2 text-xs">
        <input v-model="addMissing" type="checkbox" class="mt-0.5" />
        <span>
          补上本地没有的 {{ preview.missing.length }} 个：
          <span class="font-mono">{{ list(preview.missing) }}</span>
        </span>
      </label>

      <label v-if="preview.differing.length" class="flex items-start gap-2 text-xs">
        <input v-model="overwrite" type="checkbox" class="mt-0.5" />
        <span>
          用网络版覆盖内容不同的 {{ preview.differing.length }} 个：
          <span class="font-mono">{{ list(preview.differing) }}</span>
          <br />
          <span class="opacity-60">
            会盖掉本地改过的构型、模板和 NOTAM。情报字母保留本地的。
          </span>
        </span>
      </label>
      <p v-if="preview.on_air.length" class="text-xs opacity-60">
        {{ list(preview.on_air) }} 正在播出，这次不会动它们。
      </p>

      <div class="flex justify-end gap-2">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('cancel')">
          {{ preview.missing.length || preview.differing.length ? "取消" : "关闭" }}
        </button>
        <button
          v-if="preview.missing.length || preview.differing.length"
          class="rounded border px-3 py-1 text-xs"
          :disabled="!addMissing && !overwrite"
          @click="emit('apply', addMissing, overwrite)"
        >
          并进来
        </button>
      </div>
    </div>
  </div>
</template>

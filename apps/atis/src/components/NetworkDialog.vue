<script setup lang="ts">
import { ref, watch } from "vue";
import { errorText, messageText, t } from "../i18n";
import type { NetworkPreview } from "../types";
import { nameList } from "../types";

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

/** 在模板里调，所以切了语言跟着变。 */
const list = (names: string[]) => nameList(names, 12);
</script>

<template>
  <div
    v-if="preview"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30"
    @click.self="emit('cancel')"
  >
    <div class="flex w-[28rem] flex-col gap-3 rounded border bg-white p-4 text-sm dark:bg-neutral-900">
      <h2 class="font-semibold">{{ t("network.title", { label: messageText(preview.label) }) }}</h2>
      <p v-if="preview.notes" class="text-xs">{{ preview.notes }}</p>
      <p v-if="preview.previous && preview.previous !== preview.version" class="text-xs opacity-60">
        {{ t("network.previous", { version: preview.previous }) }}
      </p>
      <p
        v-if="preview.problems.length"
        class="rounded border border-amber-400 px-2 py-1 text-xs text-amber-700"
      >
        {{
          t("network.problems", {
            count: preview.problems.length,
            list: preview.problems.slice(0, 3).map(errorText).join(t("common.separator.sentence")),
          })
        }}
      </p>

      <p v-if="!preview.missing.length && !preview.differing.length" class="text-xs">
        {{ t("network.up_to_date", { count: preview.same.length }) }}
      </p>

      <label v-if="preview.missing.length" class="flex items-start gap-2 text-xs">
        <input v-model="addMissing" type="checkbox" class="mt-0.5" />
        <span>
          {{ t("network.add_missing", { count: preview.missing.length }) }}
          <span class="font-mono">{{ list(preview.missing) }}</span>
        </span>
      </label>

      <label v-if="preview.differing.length" class="flex items-start gap-2 text-xs">
        <input v-model="overwrite" type="checkbox" class="mt-0.5" />
        <span>
          {{ t("network.overwrite", { count: preview.differing.length }) }}
          <span class="font-mono">{{ list(preview.differing) }}</span>
          <br />
          <span class="opacity-60">
            {{ t("network.overwrite_note") }}
          </span>
        </span>
      </label>
      <p v-if="preview.on_air.length" class="text-xs opacity-60">
        {{ t("network.on_air", { list: list(preview.on_air) }) }}
      </p>

      <div class="flex justify-end gap-2">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('cancel')">
          {{ preview.missing.length || preview.differing.length ? t("dialog.cancel") : t("common.close") }}
        </button>
        <button
          v-if="preview.missing.length || preview.differing.length"
          class="rounded border px-3 py-1 text-xs"
          :disabled="!addMissing && !overwrite"
          @click="emit('apply', addMissing, overwrite)"
        >
          {{ t("network.apply") }}
        </button>
      </div>
    </div>
  </div>
</template>

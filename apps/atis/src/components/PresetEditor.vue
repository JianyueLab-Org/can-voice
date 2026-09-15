<script setup lang="ts">
import type { Preset } from "../types";

defineProps<{ preset: Preset; chineseShown: boolean }>();
defineEmits<{ change: [] }>();
</script>

<template>
  <div class="flex flex-col gap-2">
    <label class="flex flex-col gap-1">
      <span class="text-xs opacity-60">模板</span>
      <!-- 变量写成 [WIND]，加 :VOX 取语音形态。认不出的变量会原样留在稿子里
           ——那是故意的，好让拼错看得见。 -->
      <textarea
        v-model="preset.template"
        rows="3"
        class="rounded border px-2 py-1 font-mono text-xs"
        @change="$emit('change')"
      />
    </label>

    <label class="flex flex-col gap-1">
      <span class="text-xs opacity-60">机场条件（ARPT_COND）</span>
      <!-- 文字稿保留缩写原样，语音稿会展开：APCH → approach、RWY 16L →
           runway one six left。所以这里照着真实通播抄就行。 -->
      <textarea
        v-model="preset.airport_conditions"
        rows="2"
        class="rounded border px-2 py-1 text-xs"
        @change="$emit('change')"
      />
    </label>

    <div class="grid grid-cols-2 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">NOTAM</span>
        <input
          v-model="preset.notams"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">过渡高度层（TL）</span>
        <input
          v-model="preset.transition_level"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
    </div>

    <label class="flex flex-col gap-1">
      <span class="text-xs opacity-60">收尾语（留空用内置那句）</span>
      <input
        v-model="preset.closing"
        placeholder="advise on initial contact you have information [ATIS_LETTER]"
        class="rounded border px-2 py-1 text-xs"
        @change="$emit('change')"
      />
    </label>

    <!-- 中文那两格只在这份席位会播中文时才有意义。 -->
    <div v-if="chineseShown" class="grid grid-cols-2 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">中文跑道（留空用席位那个）</span>
        <!-- 跟着预设走而不是席位：切到"北向"时英文稿的 ARR RWY 会变，
             中文稿要是还念着南向的跑道，同一份通播里两种语言互相矛盾。 -->
        <input
          v-model="preset.chinese_runway"
          placeholder="三五左"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">中文附加说明</span>
        <input
          v-model="preset.chinese_extra"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
    </div>
  </div>
</template>

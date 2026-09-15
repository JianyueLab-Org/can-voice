<script setup lang="ts">
import { computed } from "vue";
import PresetEditor from "./PresetEditor.vue";
import type { Station } from "../types";
import { callsignOf } from "../types";

const props = defineProps<{ station: Station; presetName: string }>();
const emit = defineEmits<{ change: []; pick: [name: string]; remove: [] }>();

const chineseShown = computed(() => props.station.voice_language !== "en");
const preset = computed(
  () => props.station.presets.find((p) => p.name === props.presetName) ?? props.station.presets[0],
);

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("");

function addPreset() {
  const name = `构型 ${props.station.presets.length + 1}`;
  props.station.presets.push({
    ...JSON.parse(JSON.stringify(props.station.presets[0])),
    name,
  });
  emit("pick", name);
  emit("change");
}

function removePreset() {
  // 最后一份不许删——删光了这个席位就渲染不出任何稿子。
  if (props.station.presets.length <= 1) return;
  const i = props.station.presets.findIndex((p) => p.name === props.presetName);
  props.station.presets.splice(i, 1);
  emit("pick", props.station.presets[0].name);
  emit("change");
}
</script>

<template>
  <section class="flex flex-col gap-3">
    <header class="flex items-baseline gap-3">
      <h2 class="font-mono text-sm font-semibold">{{ callsignOf(station) }}</h2>
      <span class="text-xs opacity-60">{{ station.frequency }} MHz</span>
      <button class="ml-auto text-xs opacity-60 hover:opacity-100" @click="$emit('remove')">
        删除席位
      </button>
    </header>

    <div class="grid grid-cols-3 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">机场</span>
        <input
          v-model="station.identifier"
          class="rounded border px-2 py-1 font-mono text-xs uppercase"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">频率</span>
        <input
          v-model="station.frequency"
          class="rounded border px-2 py-1 font-mono text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">类型</span>
        <select
          v-model="station.atis_type"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        >
          <option value="combined">综合</option>
          <option value="departure">离场</option>
          <option value="arrival">进场</option>
        </select>
      </label>
    </div>

    <div class="grid grid-cols-3 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">机场名（语音念这个）</span>
        <!-- 语音念全名，文字稿留 ICAO：念 "Z S P D" 听着像在拼写。 -->
        <input
          v-model="station.name"
          placeholder="Shanghai Pudong International Airport"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">播报语言</span>
        <select
          v-model="station.voice_language"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        >
          <option value="en">英文</option>
          <option value="zh">中文</option>
          <option value="both">中英双语</option>
        </select>
      </label>
      <div class="flex flex-col gap-1">
        <span class="text-xs opacity-60">情报字母范围</span>
        <!-- 离场和进场分用不同字母段，避免飞行员把两份通播搞混。
             允许跨 Z 回绕，例如 Y..B。 -->
        <div class="flex items-center gap-1">
          <select
            v-model="station.code_range[0]"
            class="rounded border px-1 py-1 text-xs"
            @change="$emit('change')"
          >
            <option v-for="l in LETTERS" :key="l">{{ l }}</option>
          </select>
          <span class="text-xs opacity-50">–</span>
          <select
            v-model="station.code_range[1]"
            class="rounded border px-1 py-1 text-xs"
            @change="$emit('change')"
          >
            <option v-for="l in LETTERS" :key="l">{{ l }}</option>
          </select>
        </div>
      </div>
    </div>

    <div v-if="chineseShown" class="grid grid-cols-2 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">中文台名</span>
        <input
          v-model="station.chinese_name"
          placeholder="上海浦东"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">中文跑道（席位默认）</span>
        <input
          v-model="station.chinese_runway"
          placeholder="三五左"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
    </div>

    <div class="flex items-center gap-2 border-t pt-3">
      <span class="text-xs opacity-60">构型</span>
      <select
        :value="presetName"
        class="rounded border px-2 py-1 text-xs"
        @change="$emit('pick', ($event.target as HTMLSelectElement).value)"
      >
        <option v-for="p in station.presets" :key="p.name" :value="p.name">{{ p.name }}</option>
      </select>
      <input
        v-if="preset"
        v-model="preset.name"
        class="w-28 rounded border px-2 py-1 text-xs"
        @change="$emit('change')"
      />
      <button class="rounded border px-2 py-1 text-xs" @click="addPreset">新增</button>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="station.presets.length <= 1"
        @click="removePreset"
      >
        删除
      </button>
    </div>

    <PresetEditor
      v-if="preset"
      :preset="preset"
      :chinese-shown="chineseShown"
      @change="$emit('change')"
    />
  </section>
</template>

<script setup lang="ts">
import { computed } from "vue";
import { t } from "../i18n";
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
  // 起名用的是**此刻的界面语言**，而且存下来之后就不再变：这是用户的数据，和他
  // 自己起的名字一样，不是一句跟着语言切换的界面文字（#29）。
  const name = t("preset.new_name", { n: props.station.presets.length + 1 });
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
        {{ t("station.remove") }}
      </button>
    </header>

    <div class="grid grid-cols-3 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.airport") }}</span>
        <input
          v-model="station.identifier"
          class="rounded border px-2 py-1 font-mono text-xs uppercase"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.frequency") }}</span>
        <input
          v-model="station.frequency"
          class="rounded border px-2 py-1 font-mono text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.type") }}</span>
        <select
          v-model="station.atis_type"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        >
          <option value="combined">{{ t("station.type_combined") }}</option>
          <option value="departure">{{ t("station.type_departure") }}</option>
          <option value="arrival">{{ t("station.type_arrival") }}</option>
        </select>
      </label>
    </div>

    <div class="grid grid-cols-3 gap-2">
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.name") }}</span>
        <!-- 语音念全名，文字稿留 ICAO：念 "Z S P D" 听着像在拼写。 -->
        <input
          v-model="station.name"
          placeholder="Shanghai Pudong International Airport"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.voice_language") }}</span>
        <select
          v-model="station.voice_language"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        >
          <!-- 值是播给飞行员的内容语言，和界面语言无关；只有这几个字跟着界面翻。 -->
          <option value="en">{{ t("station.voice_en") }}</option>
          <option value="zh">{{ t("station.voice_zh") }}</option>
          <option value="both">{{ t("station.voice_both") }}</option>
        </select>
      </label>
      <div class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.code_range") }}</span>
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
        <span class="text-xs opacity-60">{{ t("station.chinese_name") }}</span>
        <input
          v-model="station.chinese_name"
          :placeholder="t('station.chinese_name_hint')"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
      <label class="flex flex-col gap-1">
        <span class="text-xs opacity-60">{{ t("station.chinese_runway") }}</span>
        <input
          v-model="station.chinese_runway"
          :placeholder="t('station.chinese_runway_hint')"
          class="rounded border px-2 py-1 text-xs"
          @change="$emit('change')"
        />
      </label>
    </div>

    <div class="flex items-center gap-2 border-t pt-3">
      <span class="text-xs opacity-60">{{ t("station.presets") }}</span>
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
      <button class="rounded border px-2 py-1 text-xs" @click="addPreset">
        {{ t("station.add_preset") }}
      </button>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="station.presets.length <= 1"
        @click="removePreset"
      >
        {{ t("station.remove_preset") }}
      </button>
    </div>
  </section>
</template>

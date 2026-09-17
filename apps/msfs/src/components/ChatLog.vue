<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import type { ChatMessage } from "../types";
import { recipientText } from "../types";
import { t } from "../i18n";

const props = defineProps<{ messages: ChatMessage[] }>();
const emit = defineEmits<{ (e: "reply", callsign: string): void }>();

const box = ref<HTMLElement | null>(null);

// **新消息要自己滚到底**：不滚的话，管制员的回话贴在看不见的下面，
// 而飞行员正盯着屏幕等它。
watch(
  () => props.messages.length,
  async () => {
    await nextTick();
    const el = box.value;
    if (el) el.scrollTop = el.scrollHeight;
  },
);
</script>

<template>
  <div ref="box" class="flex flex-col gap-1 overflow-auto text-xs">
    <div v-if="!messages.length" class="py-6 text-center opacity-50">
      {{ t("lists.no_messages") }}
    </div>
    <div
      v-for="(m, i) in messages"
      :key="`${m.at}-${i}`"
      class="rounded border px-2 py-1"
      :class="m.outbound ? 'border-neutral-300 opacity-70' : 'border-sky-400'"
    >
      <p class="flex items-baseline gap-2">
        <!-- 点一下发件人就把他填进收件人框：管制员叫你的时候，回话要快。 -->
        <button
          class="font-mono font-semibold underline-offset-2 hover:underline"
          @click="emit('reply', m.outbound ? m.to : m.from)"
        >
          {{ m.outbound ? "→ " + recipientText(m.to) : m.from }}
        </button>
        <span v-if="!m.outbound" class="font-mono opacity-50">{{ recipientText(m.to) }}</span>
      </p>
      <p class="whitespace-pre-wrap break-words">{{ m.text }}</p>
    </div>
  </div>
</template>

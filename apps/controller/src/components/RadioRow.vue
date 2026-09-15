<script setup lang="ts">
interface Radio {
  freq_khz: number;
  rx: boolean;
  tx: boolean;
  xc: boolean;
  gain: number;
  selected: boolean;
}

const props = defineProps<{
  radio: Radio;
  receiving: boolean;
  txDenied: boolean;
  rxDenied: boolean;
}>();

defineEmits<{
  switch: [name: "rx" | "tx" | "xc", on: boolean];
  volume: [gain: number];
  select: [];
  remove: [];
}>();

const mhz = (khz: number) => (khz / 1000).toFixed(3);
</script>

<template>
  <div class="flex items-center gap-3 rounded border px-3 py-2" :class="radio.selected ? 'border-sky-500' : ''">
    <!-- `selected` 是界面标记，**不发给服务端**：它和服务端的"主频率"是两件
         毫不相干的事，所以字段不叫 primary。 -->
    <button class="w-4 text-sky-600" :title="'选中这一行'" @click="$emit('select')">
      {{ radio.selected ? "▸" : "" }}
    </button>

    <span class="w-20 font-mono tabular-nums">{{ mhz(radio.freq_khz) }}</span>

    <!-- RX 灯：这个频率上**有人在讲**，不是"最后一个开口的人还在讲"。 -->
    <span
      class="h-2.5 w-2.5 rounded-full"
      :class="receiving ? 'bg-green-500' : 'bg-neutral-300'"
      :title="receiving ? '正在接收' : ''"
    />

    <!-- 三个开关。**耦合规则在核心库里**，这里只发意图：
         关 RX 会连带清掉 TX/XC，开 TX 会强制开 RX，开 XC 会强制开 RX+TX。
         前端不要自己实现一遍，否则同一条规则就有了两份。 -->
    <label v-for="s in (['rx', 'tx', 'xc'] as const)" :key="s" class="flex items-center gap-1 uppercase">
      <input type="checkbox" :checked="radio[s]" @change="$emit('switch', s, ($event.target as HTMLInputElement).checked)" />
      {{ s }}
    </label>

    <input
      type="range"
      min="0"
      max="2"
      step="0.05"
      :value="radio.gain"
      class="w-24"
      @input="$emit('volume', Number(($event.target as HTMLInputElement).value))"
    />

    <!-- 被拒要说出来：一个设好了却不生效、又不知道为什么的开关，
         正是整个重写要逃离的那类故障。 -->
    <span v-if="txDenied" class="text-xs text-amber-600" title="服务端没有给这个频率的发射权">发射被拒</span>
    <span v-if="rxDenied" class="text-xs text-amber-600" title="服务端没有给这个频率的接收">接收被拒</span>

    <button class="ml-auto text-xs opacity-60 hover:opacity-100" @click="$emit('remove')">移除</button>
  </div>
</template>

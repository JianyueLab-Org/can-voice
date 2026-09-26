<script setup lang="ts">
/**
 * 飞行计划对话框（spec §6）。飞行计划页搬进 `Modal`，
 * 从原生菜单「文件 → 飞行计划…」进，不再是主界面上的一个页签。
 *
 * 用到它的客户端：xpc、msfs。**登记在 `SHARED_FRONTEND` 上**——不登记的副本
 * 会无声地漂开。
 */
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import Modal from "./Modal.vue";
import { t } from "../i18n";
import type { FlightPlan, Settings } from "../types";
import { emptyFlightPlan } from "../types";

/// `observer` 是观察员模式开着没有——观察员不上 FSD，拍发不了计划。
const props = defineProps<{ open: boolean; observer?: boolean }>();
const emit = defineEmits<{ close: [] }>();

const plan = ref<FlightPlan>(emptyFlightPlan());
/**
 * 上一次拍发的结果，`null` = 还没拍发过。**存的是哪一种结果不是那句话**：
 * 存成句子的话，切了语言那一行还停在旧语言上。
 */
const filed = ref<"filed" | "offline" | null>(null);

/**
 * 机型从设置里预填。**这里自己读一次 `settings`**，不把 App.vue 登录框里那个
 * `aircraft` 传进来：那一个是跟着用户打字变的活值，而这一格此前取的是启动时
 * 存着的那一份。启动时多调一次 `settings` 可以忽略不计，换成活值则是行为变化。
 */
onMounted(async () => {
  const s = await invoke<Settings>("settings");
  plan.value.aircraft = s.aircraft;
});

async function file() {
  filed.value = (await invoke<boolean>("file_flight_plan", { plan: plan.value }))
    ? "filed"
    : "offline";
}
</script>

<template>
  <!-- 宽度用 `Modal` 的默认 44rem，四列表单在 980 宽的窗口里正好。外面补一个
       `text-xs`：`Modal` 的盒子是 `text-sm`，而这一页此前使用的
       `text-xs` 里，不补的话整张表单比今天大一号。 -->
  <Modal :open="props.open" :title="t('plan.tab')" @close="emit('close')">
    <div class="grid grid-cols-4 gap-2 text-xs">
      <label class="flex flex-col gap-1">
        <span class="opacity-60">{{ t("plan.rules") }}</span>
        <select v-model="plan.rules" class="rounded border px-2 py-1">
          <option value="I">{{ t("plan.rules_i") }}</option>
          <option value="V">{{ t("plan.rules_v") }}</option>
          <option value="Y">{{ t("plan.rules_y") }}</option>
          <option value="Z">{{ t("plan.rules_z") }}</option>
        </select>
      </label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.aircraft") }}</span>
        <input v-model="plan.aircraft" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.cruise_speed") }}</span>
        <input v-model="plan.cruise_speed" placeholder="N0450" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.cruise_altitude") }}</span>
        <input v-model="plan.cruise_altitude" placeholder="F350" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.departure") }}</span>
        <input v-model="plan.departure" placeholder="ZSPD" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.arrival") }}</span>
        <input v-model="plan.arrival" placeholder="ZBAA" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.alternate") }}</span>
        <input v-model="plan.alternate" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.departure_time") }}</span>
        <input v-model="plan.departure_time" placeholder="1230" class="rounded border px-2 py-1" /></label>

      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.enroute_hours") }}</span>
        <input v-model="plan.enroute_hours" placeholder="02" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.enroute_minutes") }}</span>
        <input v-model="plan.enroute_minutes" placeholder="15" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.fuel_hours") }}</span>
        <input v-model="plan.fuel_hours" placeholder="04" class="rounded border px-2 py-1" /></label>
      <label class="flex flex-col gap-1"><span class="opacity-60">{{ t("plan.fuel_minutes") }}</span>
        <input v-model="plan.fuel_minutes" placeholder="00" class="rounded border px-2 py-1" /></label>

      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">{{ t("plan.route") }}</span>
        <input v-model="plan.route" class="rounded border px-2 py-1 font-mono uppercase" /></label>
      <label class="col-span-4 flex flex-col gap-1"><span class="opacity-60">{{ t("plan.remarks") }}</span>
        <input v-model="plan.remarks" class="rounded border px-2 py-1" /></label>

      <div class="col-span-4 flex items-center gap-2">
        <!-- 观察员没有 FSD 连接，计划由机长那一端拍发。 -->
        <button class="rounded border px-3 py-1" :disabled="props.observer" @click="file">
          {{ t("plan.file") }}
        </button>
        <span v-if="props.observer" class="opacity-70">{{ t("plan.observer") }}</span>
        <span v-else-if="filed" class="opacity-70">
          {{ filed === "filed" ? t("plan.filed") : t("plan.offline") }}
        </span>
      </div>
    </div>
  </Modal>
</template>

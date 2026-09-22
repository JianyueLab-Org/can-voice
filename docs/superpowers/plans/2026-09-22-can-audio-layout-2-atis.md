# 通播端搬回 can-audio 排布 — 实施计划（4 之 2）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `apps/atis` 的三栏加折叠块改成 can-audio 的两栏加可拖分割条，席位编辑器和构型编辑器变成模态，播出设置和日志并进设置对话框，窗口几何跟上。

**Architecture:** 只动前端排布、窗口几何常量和字典。Rust 命令面一条不加不改，唯一的 Rust 改动是 `COMPACT_MIN` / `COMPACT_SIZE` 两个常量。`App.vue` 的 script 段（状态、命令调用、watch）基本原样保留——变的是它渲染成什么形状，不是它做什么。

**Tech Stack:** Vue 3.5 SFC + Vite 7 + TypeScript + Tailwind CSS v4（CSS 配置，没有配置文件）；Tauri v2；`crates/can-voice-i18n` 的字典测试是唯一的自动化护栏。

**Spec:** `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md`，本计划落地的是 §5（并用到 §3 的共用件和 §7 的 i18n 规矩）。

**Prior plan:** `docs/superpowers/plans/2026-09-22-can-audio-layout-1-controller.md` 已经落地（分支 `design/can-audio-layout`）。调色板、`StateToggle.vue`、`StatusBar.vue`、`SHARED_FRONTEND` 清单都已经在树上，本计划**建立在它们最终的样子之上**，不要按计划 1 的文字去回忆它们长什么样——**读文件**。

## Global Constraints

这一节的每一条都隐含在每个任务的要求里。

- **逐字节复制，不抽包。** 共用前端文件在四个端各一份逐字节相同的副本，清单在 `crates/can-voice-i18n/tests/dictionaries.rs` 的 `SHARED_FRONTEND`。改了其中一份，四份都要改，否则 `shared_frontend_files_are_identical_in_every_app_that_carries_them` 红。
- **界面代码里不许出现硬编码中文。** `the_interface_code_has_no_hardcoded_chinese` 扫 `apps/*/src/**`。每一句人看得见的话都走 `t("…")`。
- **每个新键中英成对。** `apps/atis/src/locales/app.zh.json` 和 `app.en.json` 键集必须一致，占位符也必须一致（`both_languages_have_the_same_keys_and_none_is_empty`、`placeholders_agree_between_the_languages`）。英文那份里不许出现中文（`english_has_no_chinese_in_it`）。
- **`t()` 用得到的键必须存在。** `every_key_the_interface_asks_for_exists` 会扫出模板里每一个字面量键。键名只能是字面量字符串，不能是拼出来的。
- **调色板只从 CSS 自定义属性取。** `--can-off / --can-on / --can-active / --can-muted / --can-theme / --can-idle / --can-surface / --can-window`，写法 `var(--can-on)`。不要再写十六进制字面量，`every_app_declares_the_can_audio_palette` 看的是 `style.css`，但新写的颜色一样要走变量，否则以后调色调不动。
- **Rust 命令面不动。** 不加 `#[tauri::command]`，不改签名，不改 `invoke_handler!` 的列表。本计划对 `src-tauri` 的唯一改动是两个常量和 `tauri.conf.json` 的最小尺寸。
- **版本号不动。** 已经在 `27.0.4`，本计划不碰十二个版本文件。
- **每个任务结束前的门**，四条全绿才算完：
  - `cd apps/atis && bun run build`（= `vue-tsc --noEmit && vite build`）
  - `cargo test -p can-voice-i18n`
  - `cargo fmt --all --check`
  - 动过 `apps/*/src` 里共用文件的任务，四个端都要 `bun run build`
- 动过 `apps/atis/src-tauri` 的任务另加 `cd apps/atis/src-tauri && cargo check --all-targets` 和 `cargo clippy --workspace --all-targets -- -D warnings`。
- **没有前端测试框架。** 仓库里没有 vitest / jest，也没有 `lint` 脚本。`vue-tsc` 和 Rust 字典测试就是全部的自动化覆盖——所以每个任务的"验证"步骤里要有一条**人工要看什么**，写清楚。
- **提交签名不能绕过。** `commit.gpgsign=true` 且是硬件密钥，每次提交都要等实体按键，可能等很久。不要 `--no-gpg-sign`，不要改 git 配置，不要杀掉卡住的提交重来。
- **不要 `git add -A` 或 `git add .`**，只 `git add` 点名的路径。
- 临时文件放 `<项目>/.temp/`，做完删掉。不许用 `/tmp`。

---

## File Structure

| 文件 | 动作 | 职责 |
|---|---|---|
| `apps/{controller,atis,xpc,msfs}/src/components/WindowToggles.vue` | 改（四份逐字节相同） | 加一个 `only` 属性，让调用方挑显示哪几个钮。通播端要把置顶/精简和设置拆到两处。 |
| `apps/atis/src/locales/app.{zh,en}.json` | 改 | 本计划用到的全部新键，一次加齐。 |
| `apps/atis/src/components/Modal.vue` | 新建 | 通用模态外壳：遮罩、标题、宽度属性、Esc 关闭、内容插槽、底部一个关闭钮。只有通播端用。 |
| `apps/atis/src/components/StationDialog.vue` | 新建 | `Modal` + `StationEditor`。取代中间那一栏。 |
| `apps/atis/src/components/PresetDialog.vue` | 新建 | `Modal` + `PresetEditor`。从右栏「编辑预设」进。 |
| `apps/atis/src/components/StationEditor.vue` | 改 | 去掉自带的外框和标题行（它现在是模态的内容），去掉内嵌的 `PresetEditor`——构型改走 `PresetDialog`。 |
| `apps/atis/src/components/Splitter.vue` | 新建 | 两栏可拖分割条，左栏宽度只活在会话里。 |
| `apps/atis/src/components/StationList.vue` | 新建 | 席位列表，每行一句 can-audio 的单行摘要。 |
| `apps/atis/src/components/AiringPanel.vue` | 新建 | 播出设置（报文周期、登录等级）加 `LogPanel`，塞进 `SettingsDialog` 的插槽。对应管制端的 `SettingsPanel.vue`。 |
| `apps/atis/src/types.ts` | 改 | 加 `stationSummary()`——纯函数，摘要那一行怎么拼。 |
| `apps/atis/src/App.vue` | 改（五个任务） | 顶栏、两栏骨架、左栏、右栏、设置插槽。script 段只减不加。 |
| `apps/atis/src-tauri/src/lib.rs` | 改 | `COMPACT_MIN` / `COMPACT_SIZE` 两个常量。 |
| `apps/atis/src-tauri/tauri.conf.json` | 改 | 最小尺寸 720×480 → 900×600。 |
| `docs/manual-test.md` | 改 | §4 的通播步骤按新排布重写，加一条精简模式。 |
| `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md` | 改 | §5 补记两处刻意的偏离（见任务 3 和任务 7）。 |

**不新建的东西：** `SettingsDialog.vue` 不改——它早就有 `<slot />`，通播端只是开始往里塞东西，所以它在 `SHARED_FRONTEND` 里那一行**留着**。`Panel.vue` 不建——§3 明说通播端不用卡片。`StateToggle.vue` / `StatusBar.vue` 通播端用不上，不碰。

---

### Task 1: `WindowToggles` 学会只显示一部分钮

can-audio 的通播端把**置顶和精简放在左栏标题行**、**设置留在顶栏**（`atis/gui.py:340-358` 对 `:323`）。理由是精简模式下顶栏整条藏起来，置顶和精简必须活在藏不掉的地方。现在 `WindowToggles.vue` 三个钮焊在一起，所以先让它可以拆开用。

**Files:**
- Modify: `apps/controller/src/components/WindowToggles.vue`
- Modify: `apps/atis/src/components/WindowToggles.vue`
- Modify: `apps/xpc/src/components/WindowToggles.vue`
- Modify: `apps/msfs/src/components/WindowToggles.vue`
- Test: `crates/can-voice-i18n/tests/dictionaries.rs`（不改，跑它）

**Interfaces:**
- Produces: `WindowToggles` 新增可选属性 `only?: ("on_top" | "compact" | "settings")[]`。不传等于三个都显示，所以另外三个端一个字不用改调用处。事件 `settings: []` 不变。

- [ ] **步骤 1：先读一遍现在的文件**

```bash
cat apps/controller/src/components/WindowToggles.vue
```

52 行，`defineEmits<{ settings: [] }>()`，没有属性。三个 `<button>`：置顶、精简、设置；设置那个已经有 `v-if="!appearance.compact"`。

- [ ] **步骤 2：在 controller 那一份上改**

`<script setup>` 里把 `defineEmits` 那一行换成下面这三行（注释照抄，它解释了为什么要有这个属性）：

```ts
/**
 * `only` 挑显示哪几个钮，不传是三个都显示。
 *
 * 通播端要把置顶/精简放进左栏标题行、设置留在顶栏——can-audio 就是这么放的
 * （`atis/gui.py:340-358` 对 `:323`），为的是精简模式下整条顶栏藏起来之后，
 * 置顶和精简还在。另外三个端不传，行为和以前一模一样。
 */
const props = defineProps<{ only?: ("on_top" | "compact" | "settings")[] }>();
const shows = (key: "on_top" | "compact" | "settings") => !props.only || props.only.includes(key);
defineEmits<{ settings: [] }>();
```

模板里给三个 `<button>` 各加一个条件：

- 置顶那个加 `v-if="shows('on_top')"`
- 精简那个加 `v-if="shows('compact')"`
- 设置那个把现有的 `v-if="!appearance.compact"` 改成 `v-if="shows('settings') && !appearance.compact"`

**不要动别的**：类名、`:aria-pressed`、`title`、`t()` 的键、那两条中文注释都原样留着。

- [ ] **步骤 3：复制到另外三个端**

```bash
for app in atis xpc msfs; do
  cp apps/controller/src/components/WindowToggles.vue "apps/$app/src/components/WindowToggles.vue"
done
git diff --stat apps/*/src/components/WindowToggles.vue
```

四个文件都要出现在 `git diff --stat` 里。只出现一个就是复制没跑。

- [ ] **步骤 4：先看它会失败**

故意只改一份，确认护栏是真的：

```bash
git stash push apps/atis/src/components/WindowToggles.vue
cargo test -p can-voice-i18n shared_frontend_files_are_identical_in_every_app_that_carries_them
```

预期：**FAIL**，信息里点名 `apps/atis/src/components/WindowToggles.vue differs from apps/controller's`。

```bash
git stash pop
```

- [ ] **步骤 5：四条门**

```bash
cargo test -p can-voice-i18n
cargo fmt --all --check
for app in controller atis xpc msfs; do (cd "apps/$app" && bun run build) || echo "$app FAILED"; done
```

四个端全 PASS。`vue-tsc` 这一步就是这个任务的类型检查——`only` 传错字符串会在这里红。

- [ ] **步骤 6：人工要看什么**

这个任务没有可见变化（没人传 `only`）。跑一次 `cd apps/controller && bun run tauri dev`，确认顶栏还是置顶/精简/设置三个钮，点精简之后设置钮消失、另外两个变成方图标——**和改之前一样**。

- [ ] **步骤 7：提交**

```bash
git add apps/controller/src/components/WindowToggles.vue apps/atis/src/components/WindowToggles.vue apps/xpc/src/components/WindowToggles.vue apps/msfs/src/components/WindowToggles.vue
git commit -m "$(cat <<'MSG'
feat(apps): let WindowToggles show a subset of its buttons

通播端要把置顶/精简和设置放在两处。不传 only 的三个端行为不变。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

提交要等硬件密钥按一下，可能等很久。不要中断，不要重来。

---

### Task 2: 一次把新文案加齐

后面六个任务用到的键全在这里加完。分散到各任务里加的话，每个任务都要碰同两个 JSON，而且 `every_key_the_interface_asks_for_exists` 会在**半路**红——那个红是噪音，不是信号。

**Files:**
- Modify: `apps/atis/src/locales/app.zh.json`
- Modify: `apps/atis/src/locales/app.en.json`

**Interfaces:**
- Produces: 下表的键，后面每个任务在模板里直接 `t("…")` 用。

- [ ] **步骤 1：确认 JSON 的格式能原样写回**

```bash
python3 - <<'PY'
import json
for p in ("apps/atis/src/locales/app.zh.json", "apps/atis/src/locales/app.en.json"):
    src = open(p, encoding="utf-8").read()
    out = json.dumps(json.loads(src), indent=2, ensure_ascii=False) + "\n"
    print(p, "identical" if out == src else "DIFFERS — 停下来手工改")
PY
```

两行都必须是 `identical`。是 `DIFFERS` 就别用脚本，手工改。

- [ ] **步骤 2：加键**

`station.new` 是**改文案**（`"+ 新席位"` → `"新建"`），因为它从列表下方那个长按钮变成了一行三个里的第一个。其余都是新增。

```bash
python3 - <<'PY'
import json

ZH = {
    "login": {"account": "账号"},
    "station": {
        "list_title": "通播席位",
        "new": "新建",
        "edit": "编辑",
        "delete": "删除",
        "title": "席位设置",
        "none": "还没有席位",
    },
    "preset": {
        "label": "构型",
        "edit": "编辑构型",
        "title": "构型「{name}」",
    },
    "draft": {
        "letter": "情报字母：{letter}",
        "letter_none": "情报字母：--",
        "voice": "语音稿",
    },
    "import": {
        "network_tip": "从 can 取全网通播配置：席位、频率、跑道构型预设、模板和中文播报用词。"
        "本地已有的席位默认不动，可以选择用网络版覆盖。",
    },
}

EN = {
    "login": {"account": "Account"},
    "station": {
        "list_title": "ATIS Positions",
        "new": "New",
        "edit": "Edit",
        "delete": "Delete",
        "title": "Position settings",
        "none": "No positions yet",
    },
    "preset": {
        "label": "Preset",
        "edit": "Edit preset",
        "title": "Preset “{name}”",
    },
    "draft": {
        "letter": "Information {letter}",
        "letter_none": "Information --",
        "voice": "Voice script",
    },
    "import": {
        "network_tip": "Pull the network-wide ATIS configuration from can: positions, "
        "frequencies, runway presets, templates and Chinese readback wording. Positions you "
        "already have are left alone by default; you can choose to overwrite them.",
    },
}

for path, add in (("apps/atis/src/locales/app.zh.json", ZH), ("apps/atis/src/locales/app.en.json", EN)):
    doc = json.loads(open(path, encoding="utf-8").read())
    for ns, entries in add.items():
        doc[ns].update(entries)
    open(path, "w", encoding="utf-8").write(json.dumps(doc, indent=2, ensure_ascii=False) + "\n")
    print("wrote", path)
PY
```

- [ ] **步骤 3：核一遍**

```bash
git diff apps/atis/src/locales/
```

对着看三件事：中英两份加的键**完全一样**；英文那份里**一个汉字都没有**（`english_has_no_chinese_in_it` 会查，但自己先看一眼便宜）；`preset.title` 两边都有 `{name}`、`draft.letter` 两边都有 `{letter}`，别的键两边都没有占位符。

- [ ] **步骤 4：跑字典测试**

```bash
cargo test -p can-voice-i18n
```

全绿。`every_key_the_interface_asks_for_exists` 不会因为"加了没人用的键"而红——它只查用到的键存不存在，反向没有断言。

- [ ] **步骤 5：提交**

```bash
git add apps/atis/src/locales/app.zh.json apps/atis/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
i18n(atis): add the wording the can-audio layout needs

station.new 从「+ 新席位」改成「新建」：它变成一行三个按钮里的第一个。
import.network_tip 是 can-audio 那条硬编码的长提示，按键搬过来。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 3: 席位编辑器和构型编辑器变成模态

can-audio 的席位编辑是 `StationDialog`（`atis/gui.py:88-237`），构型编辑是 `PresetDialog`（`atis/gui.py:1266-1330`），都不是内联栏。中间那一栏因此消失——这是两栏排布腾出空间的方式。

**刻意的偏离，落地时要记进 spec §5。** can-audio 的 `StationDialog` 编的是一份**拷贝**，底部 `确定 / 取消`，取消就丢弃；`PresetDialog` 编的是**原对象**，底部 `保存 / 取消`。can-voice 两个编辑器都直接改 `props` 上那个活对象，`@change` 立刻 `save()` 落盘——改成拷贝加提交是**重写行为**，不是搬排布，§10 把本轮限定在排布上。所以两个模态底部都只有一个**关闭**钮，没有取消。落地之后在 spec §5 里补一句记下来。

**Files:**
- Create: `apps/atis/src/components/Modal.vue`
- Create: `apps/atis/src/components/StationDialog.vue`
- Create: `apps/atis/src/components/PresetDialog.vue`
- Modify: `apps/atis/src/components/StationEditor.vue`
- Modify: `apps/atis/src/App.vue`

**Interfaces:**
- Produces:
  - `Modal`：`defineProps<{ open: boolean; title: string; width?: string }>()`，`defineEmits<{ close: [] }>()`，一个默认插槽。`width` 是一个 Tailwind 宽度类名字符串，不传是 `w-[44rem]`。
  - `StationDialog`：`{ open: boolean; station: Station; presetName: string }`，事件 `close: []`、`change: []`、`pick: [name: string]`、`remove: []`——后三个原样转发 `StationEditor` 的。
  - `PresetDialog`：`{ open: boolean; preset: Preset; chineseShown: boolean }`，事件 `close: []`、`change: []`。
- Consumes: `StationEditor`（现有，属性 `station` / `presetName`）、`PresetEditor`（现有，属性 `preset` / `chineseShown`）。

- [ ] **步骤 1：读三个现成文件**

```bash
cat apps/atis/src/components/SettingsDialog.vue
sed -n '1,50p;148,184p' apps/atis/src/components/StationEditor.vue
sed -n '1,12p' apps/atis/src/components/PresetEditor.vue
```

`SettingsDialog.vue` 就是要照着抄的模态骨架（遮罩、`@click.self`、`tabindex="-1"`、`@keyup.escape`、开的时候聚焦）。**照它的样子写，别自己发明。**

- [ ] **步骤 2：写 `Modal.vue`**

```vue
<script setup lang="ts">
import { nextTick, ref, watch } from "vue";
import { t } from "../i18n";

/**
 * 通用模态外壳。`SettingsDialog` 的骨架，加一个宽度属性和一个标题。
 *
 * 只有通播端有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 xpc 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
 */
const props = defineProps<{ open: boolean; title: string; width?: string }>();
const emit = defineEmits<{ close: [] }>();

const box = ref<HTMLElement | null>(null);

// 这个组件本身一直挂着，`v-if` 在它自己的根节点上，所以普通 watch 就够了。
// （`SettingsCommon` 要 `{ immediate: true }`，是因为它整个被挂在外层的
// `v-if` 里面，构造出来的那一刻 `open` 已经是 true，watch 永远不会触发。）
watch(
  () => props.open,
  async (open) => {
    if (!open) return;
    await nextTick();
    box.value?.focus();
  },
);
</script>

<template>
  <div
    v-if="open"
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/30 p-3"
    @click.self="emit('close')"
  >
    <div
      ref="box"
      tabindex="-1"
      class="flex max-h-full flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      :class="width ?? 'w-[44rem]'"
      @keyup.escape="emit('close')"
    >
      <h2 class="font-semibold">{{ title }}</h2>

      <slot />

      <div class="flex justify-end">
        <button class="rounded border px-3 py-1 text-xs" @click="emit('close')">
          {{ t("common.close") }}
        </button>
      </div>
    </div>
  </div>
</template>
```

- [ ] **步骤 3：写两个对话框**

`StationDialog.vue`：

```vue
<script setup lang="ts">
import Modal from "./Modal.vue";
import StationEditor from "./StationEditor.vue";
import { t } from "../i18n";
import type { Station } from "../types";

defineProps<{ open: boolean; station: Station; presetName: string }>();
const emit = defineEmits<{ close: []; change: []; pick: [name: string]; remove: [] }>();
</script>

<template>
  <Modal :open="open" :title="t('station.title')" @close="emit('close')">
    <StationEditor
      :station="station"
      :preset-name="presetName"
      @change="emit('change')"
      @pick="emit('pick', $event)"
      @remove="emit('remove')"
    />
  </Modal>
</template>
```

`PresetDialog.vue`：

```vue
<script setup lang="ts">
import Modal from "./Modal.vue";
import PresetEditor from "./PresetEditor.vue";
import { t } from "../i18n";
import type { Preset } from "../types";

const props = defineProps<{ open: boolean; preset: Preset; chineseShown: boolean }>();
const emit = defineEmits<{ close: []; change: [] }>();
</script>

<template>
  <Modal
    :open="open"
    :title="t('preset.title', { name: props.preset.name })"
    width="w-[40rem]"
    @close="emit('close')"
  >
    <PresetEditor :preset="preset" :chinese-shown="chineseShown" @change="emit('change')" />
  </Modal>
</template>
```

`w-[40rem]` 是 640px，就是 can-audio `PresetDialog` 那个 `setMinimumSize(640, 520)` 的宽。

- [ ] **步骤 4：`StationEditor` 里去掉内嵌的 `PresetEditor`**

删掉模板末尾那个 `<PresetEditor … />` 块（`:177-182`）和第 3 行的 `import PresetEditor from "./PresetEditor.vue";`。

**留着不动的**：`chineseShown` 计算属性（上面那个中文网格还在用）、`preset` 计算属性（构型改名那个输入框还在用）、整条构型工具条（下拉 / 改名 / 新增 / 删除）——席位对话框管的是**构型这份名单**，构型对话框管的是**一份构型的内容**。

- [ ] **步骤 5：`App.vue` 接上**

1. `asking` 的联合类型加两个成员，注释跟着改：

```ts
/** 当前打开的是哪个对话框。`null` 是没开。一次只可能开一个。 */
const asking = ref<"profile" | "rename" | "station" | "edit" | "preset" | null>(null);
```

2. 加两个 import（`StationDialog`、`PresetDialog`），删掉 `StationEditor` 的 import。

3. 加一个计算属性，给 `PresetDialog` 用：

```ts
const editedPreset = computed(() => station.value?.presets.find((p) => p.name === presetName.value));
const chineseShown = computed(() => station.value?.voice_language !== "en");
```

4. 删掉中间那一栏——模板 `:481-491` 整个 `<div v-if="!compact" class="flex min-w-0 flex-1 …">`，含里面的 `<StationEditor>` 和 `station.pick` 空状态。空状态搬进右栏（下一步）。

5. 右栏 `<aside>` 的 `w-80` 改成 `min-w-0 flex-1`，并在它最外层套一个 `v-if`/`v-else`：选中了席位才画内容，没选就画 `<p class="opacity-60">{{ t("station.pick") }}</p>`。`station.pick` 的中文是「左边挑一个席位，或者新建一个」——席位列表**还在左边**，所以这句话不用改。

6. 左栏那个「+ 新席位」按钮旁边加一个**临时**的编辑钮，任务 5 会把它挪进正式的按钮区：

```vue
<button class="rounded border px-2 py-1 text-xs" :disabled="!station" @click="asking = 'edit'">
  {{ t("station.edit") }}
</button>
```

7. 右栏的控制行里加一个编辑构型钮，任务 6 会把它挪进预设行：

```vue
<button class="rounded border px-2 py-1 text-xs" :disabled="!editedPreset" @click="asking = 'preset'">
  {{ t("preset.edit") }}
</button>
```

8. 和另外四个对话框并排，在 `</main>` 前加两个：

```vue
<StationDialog
  v-if="station"
  :open="asking === 'edit'"
  :station="station"
  :preset-name="presetName"
  @close="asking = null"
  @change="save"
  @pick="presetName = $event"
  @remove="removeStation"
/>
<PresetDialog
  v-if="editedPreset"
  :open="asking === 'preset'"
  :preset="editedPreset"
  :chinese-shown="chineseShown"
  @close="asking = null"
  @change="save"
/>
```

`v-if="station"` 在外、`:open` 在内是**故意的**：`StationEditor` 的属性 `station` 不可为空，而 `station` 计算属性会是 `undefined`。反过来写（`:open` 在外层 `v-if` 上）会让 `StationEditor` 在没选席位时拿到 `undefined`，`vue-tsc` 当场红。

9. `@remove="removeStation"` 之后席位没了，对话框要关掉——`removeStation` 结尾把 `selected` 置空，但 `asking` 还是 `"edit"`。在 `removeStation` 里加一行 `asking.value = null;`，放在 `invoke("remove_station", …)` 之前那一行，和 `addStation` 里 `asking.value = null;` 同一个位置。

- [ ] **步骤 6：四条门**

```bash
(cd apps/atis && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

- [ ] **步骤 7：人工要看什么**

`cd apps/atis && bun run tauri dev`：

1. 挑一个席位 → 点「编辑」→ 席位表单在模态里出来，标题是「席位设置」。改 ICAO 里一个字母 → 关掉 → 列表里那一条的呼号跟着变了（说明 `@change="save"` 落盘了）。
2. 点「编辑构型」→ 构型表单在模态里出来，标题带构型名。
3. 两个模态都要：按 Esc 关得掉，点遮罩关得掉，底部「关掉」关得掉。
4. **没选席位时**：右栏是「左边挑一个席位，或者新建一个」，「编辑」钮是灰的。
5. 在席位对话框里点「删除席位」→ 对话框关掉，席位从列表消失。**这条最容易漏**：不关的话，对话框会挂着一个已经不存在的席位。

- [ ] **步骤 8：提交**

```bash
git add apps/atis/src/components/Modal.vue apps/atis/src/components/StationDialog.vue apps/atis/src/components/PresetDialog.vue apps/atis/src/components/StationEditor.vue apps/atis/src/App.vue
git commit -m "$(cat <<'MSG'
refactor(atis): move the station and preset editors into modals

中间那一栏没了，腾给两栏排布。两个模态底部只有一个关闭钮:
编辑器改的是活对象、改一下存一下,没有可取消的草稿。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 4: 两栏之间放一根可拖的分割条

can-audio：`QSplitter(Qt.Orientation.Horizontal)`，`setSizes([260, 640])`（`atis/gui.py:329,461`），没设 `setStretchFactor`，没设 `setHandleWidth`，退出精简时重置回 `[260, 640]`（`:513`）。宽度**不持久化**——can-audio 也不存。

**Files:**
- Create: `apps/atis/src/components/Splitter.vue`
- Modify: `apps/atis/src/App.vue`

**Interfaces:**
- Produces: `Splitter`，`v-model:width`（数字，像素），可选 `min` / `max`（默认 180 / 520），可选 `collapsed`（真时左栏占满、分割条和右栏都不画）。两个具名插槽 `left` / `right`。

- [ ] **步骤 1：写 `Splitter.vue`**

```vue
<script setup lang="ts">
import { ref } from "vue";

/**
 * 两栏可拖分割条。左栏宽度由调用方持有，**只活在会话里**——存下来的话，
 * 换一台屏幕小的机器就得先去拖窗口才看得见右栏。can-audio 也不存
 * （`atis/gui.py:461`）。
 */
const props = withDefaults(
  defineProps<{ width: number; min?: number; max?: number; collapsed?: boolean }>(),
  { min: 180, max: 520, collapsed: false },
);
const emit = defineEmits<{ "update:width": [value: number] }>();

const root = ref<HTMLElement | null>(null);

function grab(event: PointerEvent) {
  const handle = event.currentTarget as HTMLElement;
  // 指针捕获：拖到右栏上面、拖出窗口，事件还是回到这根条上。没有它的话，
  // 手快一点就会"拖着拖着松开了"，而松手那一下发生在别人身上。
  handle.setPointerCapture(event.pointerId);
  const left = root.value?.getBoundingClientRect().left ?? 0;
  const move = (e: PointerEvent) =>
    emit("update:width", Math.min(props.max, Math.max(props.min, e.clientX - left)));
  const done = () => {
    handle.releasePointerCapture(event.pointerId);
    handle.removeEventListener("pointermove", move);
    handle.removeEventListener("pointerup", done);
    handle.removeEventListener("pointercancel", done);
  };
  handle.addEventListener("pointermove", move);
  handle.addEventListener("pointerup", done);
  // 系统抢走指针（触控板手势、窗口失焦）时也要收工，否则条子会黏在手上。
  handle.addEventListener("pointercancel", done);
}
</script>

<template>
  <div ref="root" class="flex min-h-0 flex-1">
    <div
      class="flex min-w-0 flex-col overflow-hidden"
      :class="collapsed ? 'flex-1' : ''"
      :style="collapsed ? undefined : { width: `${width}px`, flex: '0 0 auto' }"
    >
      <slot name="left" />
    </div>
    <div
      v-if="!collapsed"
      class="mx-2 w-1 shrink-0 cursor-col-resize rounded hover:bg-[var(--can-theme)]"
      role="separator"
      aria-orientation="vertical"
      @pointerdown="grab"
    />
    <div v-if="!collapsed" class="flex min-w-0 flex-1 flex-col overflow-hidden">
      <slot name="right" />
    </div>
  </div>
</template>
```

- [ ] **步骤 2：`App.vue` 用上它**

1. 加状态，紧挨着 `compact` 那个计算属性：

```ts
/** 左栏宽度。**只活在会话里**，和 can-audio 一样不写进设置文件。 */
const paneWidth = ref(260);
// 退出精简时回到 260，和 can-audio 的 `splitter.setSizes([260, 640])` 同一个动作
// （`atis/gui.py:513`）：精简里左栏被拉成整个窗口宽，出来之后不重置就是一栏顶天。
watch(compact, (on) => {
  if (!on) paneWidth.value = 260;
});
```

2. 把模板里那个三栏容器 `<div class="flex min-h-0 flex-1 gap-4">` 换成：

```vue
<Splitter v-model:width="paneWidth" :collapsed="compact">
  <template #left> … 现在那个 <aside> 的内容 … </template>
  <template #right> … 现在那个右 <aside> 的内容 … </template>
</Splitter>
```

左 `<aside>` 上的 `:class="compact ? 'flex-1' : 'w-52'"` 去掉——宽度归 `Splitter` 管了；保留 `class="flex flex-col gap-1 overflow-auto"`。右 `<aside>` 上任务 3 改出来的 `min-w-0 flex-1` 和 `v-if="!compact"` 都去掉——`collapsed` 已经不画右栏了；`border-l pl-4` 也去掉，分割条本身就是那条线。

- [ ] **步骤 3：门**

```bash
(cd apps/atis && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

- [ ] **步骤 4：人工要看什么**

`cd apps/atis && bun run tauri dev`：

1. 左栏默认约 260px 宽。
2. 拖分割条：左栏跟手，**到 180px 停住，到 520px 停住**，不会拖没。
3. 拖到最右边再松手，鼠标移到右栏上面——分割条没有黏在手上（指针捕获生效）。
4. 快速地拖出窗口外再松手回来：同样不黏手。
5. 点精简 → 只剩左栏，占满窗口宽，没有分割条。再点回来 → **左栏回到 260**，不是精简里被拉开的那个宽度。
6. 拖到 400、关掉客户端、再开：回到 260（不持久化，这是要的）。

- [ ] **步骤 5：提交**

```bash
git add apps/atis/src/components/Splitter.vue apps/atis/src/App.vue
git commit -m "$(cat <<'MSG'
feat(atis): split the workspace into two draggable panes

默认 260 / 其余,退出精简时重置回 260。宽度只活在会话里。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 5: 左栏——标题行、单行摘要的席位表、固定高度的按钮区

can-audio 的左栏（`atis/gui.py:332-404`）自上而下：标题 `通播席位` + 撑开 + 置顶 + 精简 → 席位列表（唯一会伸缩的东西）→ 一块钉成 `Fixed` 高度的按钮区：一行 `新建 | 编辑 | 删除`，再竖排三个 `从网络更新配置` / `取在线席位` / `导入 vATIS 配置…`。

列表行用 `atis/script.py:81-105` 的单行摘要：`identifier`（离场 / 进场加一个 ` D` / ` A` 后缀）+ 字母 +（有报文时）风组 + 气压组，**两个空格**分隔，在播时前面加 `● `。

**Files:**
- Modify: `apps/atis/src/types.ts`
- Create: `apps/atis/src/components/StationList.vue`
- Modify: `apps/atis/src/App.vue`

**Interfaces:**
- Produces:
  - `types.ts` 导出 `stationSummary(s: Station, live: Live | undefined): string`。
  - `StationList`：`{ stations: Station[]; live: Record<string, Live>; selected: string }`，事件 `pick: [callsign: string]`。

- [ ] **步骤 1：`types.ts` 加摘要函数**

加在 `callsignOf` 下面（同一块，都是"从 `Station` 算出一句话"）：

```ts
/**
 * METAR 里的风组和气压组。can-audio 那边是解析过的 METAR 对象
 * （`atis/script.py:99`），这边只有原文，所以现抓两组。
 *
 * **抓不到就不显示**——和 can-audio 的 `if text:` 一样。列表那一行是给人扫一眼的，
 * 宁可少两段，不能显示一段错的。
 */
const WIND = /\b(?:VRB|\d{3})\d{2,3}(?:G\d{2,3})?(?:MPS|KT|KMH)\b/;
const QNH = /\b(?:Q\d{3,4}|A\d{4})\b/;

/**
 * 席位列表里的那一行：`ZSPD  J  09004MPS  Q1013`。
 *
 * 没上线（`live` 是 undefined）就只有机场和字母——can-audio 的 `metar=None` 那条路
 * （`atis/script.py:98`）。圆点不在这里，颜色要按状态画，归组件。
 */
export function stationSummary(s: Station, live: Live | undefined): string {
  const marker = s.atis_type === "departure" ? " D" : s.atis_type === "arrival" ? " A" : "";
  const parts = [s.identifier + marker, live?.letter ?? s.letter];
  for (const pattern of [WIND, QNH]) {
    const found = (live?.metar ?? "").match(pattern);
    if (found) parts.push(found[0]);
  }
  return parts.join("  ");
}
```

- [ ] **步骤 2：写 `StationList.vue`**

```vue
<script setup lang="ts">
import type { Live, Station } from "../types";
import { callsignOf, stationSummary } from "../types";
import { t } from "../i18n";

const props = defineProps<{
  stations: Station[];
  live: Record<string, Live>;
  selected: string;
}>();
defineEmits<{ pick: [callsign: string] }>();

/**
 * 在播那个圆点的颜色。can-audio 的圆点不上色（`atis/script.py:105` 就是一个字符），
 * 这边上——状态是 can-voice 有而 can-audio 没有的东西，§1 说留着。
 */
function dot(callsign: string): string | null {
  const state = props.live[callsign]?.state;
  if (!state) return null;
  if (state === "Online") return "var(--can-on)";
  if (state === "Connecting" || state === "Reconnecting") return "var(--can-active)";
  if (state === "Error" || state === "Offline") return "var(--can-muted)";
  return "var(--can-idle)";
}
</script>

<template>
  <div class="flex min-h-0 flex-1 flex-col overflow-auto">
    <button
      v-for="s in stations"
      :key="callsignOf(s)"
      class="flex items-center gap-2 rounded px-2 py-1 text-left font-mono text-xs"
      :class="callsignOf(s) === selected ? 'bg-[var(--can-off)] text-white' : 'hover:opacity-80'"
      @click="$emit('pick', callsignOf(s))"
    >
      <span
        class="w-2 shrink-0"
        :style="{ color: dot(callsignOf(s)) ?? 'transparent' }"
        aria-hidden="true"
        >●</span
      >
      <span class="truncate">{{ stationSummary(s, live[callsignOf(s)]) }}</span>
    </button>
    <p v-if="!stations.length" class="px-2 py-1 text-xs opacity-60">{{ t("station.none") }}</p>
  </div>
</template>
```

圆点那一格**不上线时也占着**（`transparent`），不是 `v-if`——否则在播和不在播的两行会错开两个字符，一列对不齐的等宽文字比没有圆点还难扫。

- [ ] **步骤 3：`App.vue` 重画左栏**

`Splitter` 的 `#left` 插槽整块换成：

```vue
<template #left>
  <div class="flex min-h-0 flex-1 flex-col gap-2">
    <header class="flex shrink-0 items-center gap-2">
      <h2 class="text-sm font-semibold">{{ t("station.list_title") }}</h2>
      <WindowToggles class="ml-auto" :only="['on_top', 'compact']" />
    </header>

    <StationList
      :stations="stations"
      :live="live"
      :selected="selected"
      @pick="selected = $event"
    />

    <!-- 钉成固定高度：不钉的话它会和席位列表抢纵向空间，窗口一矮列表就没了
         （can-audio 的 `setSizePolicy(Preferred, Fixed)`，`atis/gui.py:368`）。 -->
    <div v-if="!compact" class="flex shrink-0 flex-col gap-1">
      <div class="flex gap-1">
        <button class="flex-1 rounded border px-2 py-1 text-xs" @click="asking = 'station'">
          {{ t("station.new") }}
        </button>
        <button
          class="flex-1 rounded border px-2 py-1 text-xs"
          :disabled="!station"
          @click="asking = 'edit'"
        >
          {{ t("station.edit") }}
        </button>
        <button
          class="flex-1 rounded border px-2 py-1 text-xs"
          :disabled="!station"
          @click="removeStation"
        >
          {{ t("station.delete") }}
        </button>
      </div>
      <button
        class="rounded border px-2 py-1 text-left text-xs"
        :disabled="busy !== null"
        :title="t('import.network_tip')"
        @click="checkNetwork"
      >
        {{ busy === "network" ? t("busy.fetching") : t("import.network") }}
      </button>
      <button
        class="rounded border px-2 py-1 text-left text-xs"
        :disabled="busy !== null"
        :title="t('import.online_tip')"
        @click="importOnline"
      >
        {{ busy === "online" ? t("busy.fetching") : t("import.online") }}
      </button>
      <button
        class="rounded border px-2 py-1 text-left text-xs"
        :disabled="busy !== null"
        @click="vatisFile?.click()"
      >
        {{ busy === "vatis" ? t("busy.importing") : t("import.vatis") }}
      </button>
      <input
        ref="vatisFile"
        type="file"
        accept=".json"
        class="hidden"
        @change="importVatis"
      />
    </div>
  </div>
</template>
```

读一遍**现在**那三个导入按钮（`App.vue:394-429` 的 `<details>` 块）再写，把它们的 `:disabled`、`busy` 文案和 `@click` **照搬**过来——上面这段是形状，真值以现有代码为准。改错一个 `busy` 判定，按钮就会在跑的时候可以连点。

删掉的东西：整个 `<details>` 导入折叠块，列表下面原来那个 `+ 新席位` 按钮，以及任务 3 临时加的那个编辑钮。隐藏的 `<input type="file">` **必须跟着搬进来**，别留在删掉的 `<details>` 里——`vatisFile?.click()` 找不到它就是"点了没反应"。

**Airing 和 Log 两个 `<details>` 块不在这份删除名单里，原样留在 `#left` 插槽末尾。** 任务 7 才把它们并进 `SettingsDialog`；本任务只重画导入折叠块和按钮区。提前删掉的话，METAR 刷新间隔的设置、登录等级和寄日志的表单在任务 5 和任务 7 之间的两个提交里都够不着。

- [ ] **步骤 4：门**

```bash
(cd apps/atis && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

- [ ] **步骤 5：人工要看什么**

`cd apps/atis && bun run tauri dev`：

1. 左栏标题是「通播席位」，右端是置顶和精简两个钮，**没有设置钮**。
2. 席位行是一行等宽字：没上线的显示 `ZSPD  J`（前面留一格空位），上线之后变成 `● ZSPD  J  09004MPS  Q1013`，圆点是绿的。
3. 建一个离场席位（`_D_ATIS`）→ 那一行显示 `ZSPD D  J`。
4. 三个导入按钮：鼠标停在「全网通播配置…」上要出那条长提示；点一个，三个都变灰直到回来。
5. 选 vATIS 文件那个还能弹出选文件框。
6. 把窗口拖到最矮：按钮区**不动**，被压扁的是席位列表。
7. 点精简：按钮区消失，标题行和列表还在。

- [ ] **步骤 6：提交**

```bash
git add apps/atis/src/types.ts apps/atis/src/components/StationList.vue apps/atis/src/App.vue
git commit -m "$(cat <<'MSG'
feat(atis): rebuild the left pane the way can-audio arranges it

席位行改成单行摘要,导入折叠块化成固定高度的按钮区,置顶和精简
搬进标题行——精简模式下顶栏整条藏起来,它们必须在藏不掉的地方。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 6: 右栏——预设行、字母行、METAR、文字、语音、播出行

can-audio 的右栏（`atis/gui.py:406-460`）自上而下六段：预设行（`构型` + 下拉伸缩 1 + 编辑构型）→ 字母行（`情报字母: J` + 撑开 + 推进字母 + 刷新天气）→ METAR 一行等宽换行文案 → `文字通播` 小标题 + 只读框 `setFixedHeight(90)` → `语音稿` 小标题 + 只读框，**不设高度**，占满剩余 → 播出行（主按钮 + 状态文案，状态伸缩 1）。

和 can-audio 的两处不同，都是 §1 说的"can-voice 多出来的东西留着"：

- METAR **保持可编辑**（spec §5 明写），can-audio 那边是只读文案。不上线也要能改一份电码来试算模板。
- 语音稿有中英**两份**，can-audio 只有一份。两份都塞进那个占满剩余高度的区域。

还有一处是本计划的裁决，记在这里：can-audio 字母行上的「刷新天气」只有一个动作，can-voice 有两个——在播时是 `refresh`（让服务端重取），不在播时是 `fetch_metar`（取一份真报文来试算）。**合成一个钮**，按 `onAir` 走哪条、显示哪个文案，两条行为都留着。

**Files:**
- Modify: `apps/atis/src/App.vue`

**Interfaces:**
- Consumes: 任务 3 建的 `PresetDialog`（`asking = "preset"` 打开）、任务 4 建的 `Splitter` 的 `#right` 插槽。

- [ ] **步骤 1：先把现在那一栏读完**

```bash
sed -n '494,592p' apps/atis/src/App.vue
```

现有的控制行、告警、METAR 块、`shown` 那三段都在这里。下面是**目标形状**，每一段的 `:disabled`、`v-if`、`@click` 和 `t()` 键**以现有代码为准**，照搬过去；形状变，行为不变。

- [ ] **步骤 2：加一个计算属性**

```ts
/** 字母行那句话。席位一定有字母，但在播那一份可能还没报上来。 */
const letterText = computed(() => {
  const letter = current.value?.letter ?? station.value?.letter;
  return letter ? t("draft.letter", { letter }) : t("draft.letter_none");
});
```

- [ ] **步骤 3：重画 `#right` 插槽**

```vue
<template #right>
  <p v-if="!station" class="text-xs opacity-60">{{ t("station.pick") }}</p>
  <div v-else class="flex min-h-0 flex-1 flex-col gap-2">
    <!-- 1. 预设行 -->
    <div class="flex shrink-0 items-center gap-2">
      <span class="text-xs opacity-60">{{ t("preset.label") }}</span>
      <select
        v-model="presetName"
        class="min-w-0 flex-1 rounded border px-2 py-1 text-xs"
        :title="t('draft.preset_tip')"
      >
        <option v-for="p in station.presets" :key="p.name" :value="p.name">{{ p.name }}</option>
      </select>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="!editedPreset"
        @click="asking = 'preset'"
      >
        {{ t("preset.edit") }}
      </button>
    </div>

    <!-- 2. 字母行 -->
    <div class="flex shrink-0 items-center gap-2">
      <span class="text-sm font-semibold">{{ letterText }}</span>
      <span class="ml-auto"></span>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="!onAir"
        :title="t('draft.bump_tip')"
        @click="bumpLetter"
      >
        {{ t("draft.bump") }}
      </button>
      <button
        class="rounded border px-2 py-1 text-xs"
        :disabled="busy !== null"
        :title="onAir ? undefined : t('draft.fetch_metar_tip')"
        @click="onAir ? refresh() : fetchMetar()"
      >
        {{ busy === "metar" ? t("busy.fetching") : onAir ? t("draft.refresh") : t("draft.fetch_metar") }}
      </button>
    </div>

    <!-- 3. METAR：在播看服务端那一份（只读），不在播是可以改的试算电码 -->
    <label class="flex shrink-0 flex-col gap-1">
      <span class="text-xs opacity-60">{{ onAir ? t("draft.metar_live") : t("draft.metar_sample") }}</span>
      <textarea
        v-if="!onAir"
        v-model="sampleMetar"
        rows="2"
        class="w-full rounded border px-2 py-1 font-mono text-xs"
      />
      <pre v-else class="w-full whitespace-pre-wrap rounded border px-2 py-1 font-mono text-xs">{{
        current?.metar || t("draft.no_metar")
      }}</pre>
    </label>

    <!-- 4. 文字通播：固定 90px，can-audio 的 setFixedHeight(90) -->
    <div class="flex shrink-0 flex-col gap-1">
      <span class="text-xs font-semibold">{{ t("draft.text") }}</span>
      <pre
        class="h-[90px] w-full overflow-auto whitespace-pre-wrap rounded border px-2 py-1 font-mono text-xs"
        >{{ shown?.text ?? "" }}</pre
      >
    </div>

    <!-- 5. 语音稿：占满剩下的高度，装中英两份 -->
    <div class="flex min-h-0 flex-1 flex-col gap-1">
      <span class="shrink-0 text-xs font-semibold">{{ t("draft.voice") }}</span>
      <div class="flex min-h-0 flex-1 flex-col gap-1 overflow-auto rounded border p-2">
        <span class="text-xs opacity-60">{{ t("draft.voice_en") }}</span>
        <p class="whitespace-pre-wrap text-xs">{{ shown?.voice_en ?? "" }}</p>
        <template v-if="station.voice_language !== 'en'">
          <span class="mt-2 text-xs opacity-60">{{ t("draft.voice_zh") }}</span>
          <p class="whitespace-pre-wrap text-xs">{{ shown?.voice_zh ?? "" }}</p>
        </template>
      </div>
    </div>

    <!-- 6. 播出行 -->
    <div class="flex shrink-0 items-center gap-2">
      <button
        class="rounded px-3 py-1 text-xs text-white"
        :style="{ background: onAir ? 'var(--can-muted)' : 'var(--can-on)' }"
        @click="onAir ? stop() : start()"
      >
        {{ onAir ? t("draft.stop") : t("draft.start") }}
      </button>
      <span class="min-w-0 flex-1 truncate text-xs opacity-70">{{ stateText(current) }}</span>
    </div>

    <!-- 告警和 wire 转储：can-audio 没有，留在播出行下面 -->
    <p v-if="problems.length" class="shrink-0 text-xs" :style="{ color: 'var(--can-active)' }">
      {{ t("draft.unknown_variables", { list: problems.join(t("common.separator.list")) }) }}
    </p>
    <details v-if="shown" class="shrink-0 text-xs">
      <summary class="cursor-pointer opacity-60">{{ t("draft.wire") }}</summary>
      <pre class="mt-1 whitespace-pre-wrap font-mono text-xs">{{ shown.wire }}</pre>
    </details>
  </div>
</template>
```

`draft.unknown_variables` 的参数名和分隔符**以现有代码为准**（`App.vue:535-540`），照搬，别按上面这段猜。

- [ ] **步骤 4：门**

```bash
(cd apps/atis && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

- [ ] **步骤 5：人工要看什么**

`cd apps/atis && bun run tauri dev`，挑一个席位：

1. 六段从上到下就是上面的顺序。文字通播那个框**高度不变**——稿子再长也是框里滚，不是把语音稿挤下去。
2. 把窗口拉高：**只有语音稿那个框变高**，别的都不动。
3. 不在播：METAR 是可以改的输入框；改一个字 → 下面三份稿子跟着变。
4. 不在播时那个天气钮写着「取真实报文」；点一下取到真报文，稿子跟着变，**没有上线**。
5. 上线之后：METAR 变成只读，天气钮写着「取报文」，推字母钮从灰变亮。
6. 上线之后按钮变成红色的「停止」，右边状态文案写「在播」。
7. 席位的播报语言设成「英文」：中文语音稿那一段**不出现**。设成中英双语：两段都在。
8. 模板里故意写一个 `[RUNWAY]`：橙色的未知变量告警出现在播出行下面。

- [ ] **步骤 6：提交**

```bash
git add apps/atis/src/App.vue
git commit -m "$(cat <<'MSG'
feat(atis): rebuild the right pane the way can-audio arranges it

文字通播固定 90px,语音稿占满剩余高度。天气钮合成一个:在播走
refresh,不在播走 fetch_metar,两条行为都留着。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 7: 顶栏收成一行，播出设置和日志并进设置对话框

can-audio 的顶栏（`atis/gui.py:301-327`）：`账号` 标签 + 用户名框（固定 110px）+ 密码框（固定 160px）→ 撑开 → 检查更新 → 设置。整条 `setSizePolicy(Preferred, Fixed)`，否则它会往下长、把分割器压扁。

播出设置（报文周期、登录等级）在 can-audio 里本来就在设置对话框（`atis/settings.py:206-236`），日志开关也是（`:log_row`）。所以左栏那两个折叠块并进 `SettingsDialog`——和管制端把 `SettingsPanel` 塞进同一个插槽是同一个动作。

**两处裁决：**

- **不加「检查更新」钮。** can-audio 需要它是因为它没有更新横幅；can-voice 有 `UpdateBanner`，一直挂在顶栏下面自己报。再加一个钮是**加功能**，§10 把本轮限定在排布上。
- **报文周期保持数字输入框**，不改成 can-audio 那个七档下拉。`set_metar_refresh` 会夹值并把夹过的数回传，输入框已经在回填了；换成下拉是砍掉能填的值。
- 顶栏那个 `<h1>{{ t("app.title") }}</h1>` **去掉**，理由和 §4 给管制端的一样：can-audio 的主窗口没有标题，标题在窗口装饰上。`app.title` 键留着——`tauri.conf.json` 之外还有别处用。落地后 `grep -rn 'app.title' apps/atis/src` 确认没有悬空引用；只剩字典里那一条是对的。

**Files:**
- Create: `apps/atis/src/components/AiringPanel.vue`
- Modify: `apps/atis/src/App.vue`

**Interfaces:**
- Produces: `AiringPanel`，`{ cid?: string }`。自己 `invoke("settings")` 读值、自己 `invoke("set_metar_refresh")` / `invoke("set_rating")` 写值，不经过 `App.vue`。
- Consumes: `SettingsDialog` 的默认插槽（现成的，不改那个文件）。

- [ ] **步骤 1：照着管制端那一份写**

```bash
sed -n '1,12p;170,180p;255,261p' apps/controller/src/components/SettingsPanel.vue
sed -n '432,477p' apps/atis/src/App.vue
```

第一条给你外壳的形状（`<section class="flex flex-col gap-4 text-xs">`，结尾 `<LogPanel :cid="props.cid" />`），第二条给你要搬的两块表单。**表单标记原样搬**，包括那一串写死的等级选项和 `airing.rating_note`。

- [ ] **步骤 2：写 `AiringPanel.vue`**

```vue
<script setup lang="ts">
import { onMounted, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import LogPanel from "./LogPanel.vue";
import { t } from "../i18n";

/// 已经存下来的 CAN 号，寄日志时预填。
const props = defineProps<{ cid?: string }>();

const refreshSecs = ref(300);
const rating = ref(0);

// 这个组件挂在 `SettingsDialog` 的 `v-if="open"` 里面，**每次打开都是新挂一次**,
// 所以 `onMounted` 就是"打开的时候读一遍"。不要改成 watch——外层 `v-if` 已经决定了
// 生命周期，再套一层只会多一条走不到的路（`SettingsCommon` 那条 `{ immediate: true }`
// 是因为它读的是自己的属性，不是自己的挂载）。
onMounted(async () => {
  const s = await invoke<{ metar_refresh_secs: number; rating: number }>("settings");
  refreshSecs.value = s.metar_refresh_secs;
  rating.value = s.rating;
});

/** 夹过的那个数要回填：填 5 之后界面上该看到 60。 */
async function applyRefresh() {
  refreshSecs.value = await invoke<number>("set_metar_refresh", {
    secs: Math.round(refreshSecs.value),
  });
}

const applyRating = () => invoke("set_rating", { rating: rating.value });
</script>

<template>
  <section class="flex flex-col gap-4 text-xs">
    <h3 class="font-semibold">{{ t("airing.title") }}</h3>
    … 把 App.vue:435-468 那两块（报文周期输入框、登录等级下拉、rating_note）
      原样搬进来，`refreshSecs` / `rating` / `applyRefresh` / `applyRating` 名字不变 …
    <LogPanel :cid="props.cid" />
  </section>
</template>
```

- [ ] **步骤 3：`App.vue` 顶栏**

`<header>` 整块换成：

```vue
<header class="flex shrink-0 items-center gap-2">
  <template v-if="!compact">
    … 配置档下拉和新建 / 改名 / 删除三个钮，原样搬（App.vue:306-329）…
    <span class="ml-auto text-xs opacity-60">{{ t("login.account") }}</span>
    <input v-model="cid" :placeholder="t('login.cid')" class="w-[110px] rounded border px-2 py-1 text-xs" />
    <input
      v-model="password"
      type="password"
      :placeholder="t('login.password')"
      class="w-[160px] rounded border px-2 py-1 text-xs"
    />
  </template>
  <!-- 精简时也在：藏掉的话精简之后就切不回来了。此处只剩设置——置顶和精简在左栏标题行。 -->
  <WindowToggles class="ml-auto" :only="['settings']" @settings="showPrefs = true" />
</header>
```

`110px` / `160px` 就是 can-audio 的 `setFixedWidth(110)` / `setFixedWidth(160)`（`atis/gui.py:309,313`）。

- [ ] **步骤 4：`App.vue` 拆掉两个折叠块**

1. 删掉左栏里 airing 和 log 两个 `<details>`（任务 5 之后它们还挂在 `#left` 插槽里）。
2. 删掉 `refreshSecs`、`rating`、`applyRefresh`、`applyRating` 四个声明，和 `LogPanel` 的 import。
3. `loadSettings` 只留 CID：

```ts
async function loadSettings() {
  const s = await invoke<{ cid: string }>("settings");
  cid.value = s.cid;
}
```

4. 设置对话框那一行塞进插槽：

```vue
<SettingsDialog :open="showPrefs" @close="showPrefs = false">
  <AiringPanel :cid="cid" />
</SettingsDialog>
```

- [ ] **步骤 5：门**

```bash
(cd apps/atis && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
grep -rn '"app.title"\|app\.title' apps/atis/src || echo "app.title 只剩字典——对的"
```

- [ ] **步骤 6：人工要看什么**

`cd apps/atis && bun run tauri dev`：

1. 顶栏一行：配置档那几个钮在左，`账号` + 两个输入框在右，最右是设置钮，**没有标题、没有置顶、没有精简**。
2. 两个输入框宽度固定，拉宽窗口它们不跟着长。
3. 开设置 → 底下多了「播出」一节：报文周期、登录等级、日志。**改报文周期填 5 → 失焦后变成 60**（夹值回填生效）。
4. **关掉设置、再打开**：报文周期和等级显示的是刚才存的值，不是默认的 300 / 0。这条是这个任务最容易坏的一处——读值走的是挂载，关掉对话框组件就销毁了。
5. 「打开日志文件」「寄日志」还能用，CID 预填着。
6. 左栏**没有**折叠块了。

- [ ] **步骤 7：提交**

```bash
git add apps/atis/src/components/AiringPanel.vue apps/atis/src/App.vue
git commit -m "$(cat <<'MSG'
feat(atis): fold the top bar to one row and move airing into settings

账号表单取 can-audio 的固定宽度。播出设置和日志并进设置对话框,
和管制端同一个插槽。不加检查更新钮——UpdateBanner 已经在报。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 8: 窗口几何、手工测试清单、spec 补记

can-audio 的通播端：正常最小 `setMinimumSize(900, 600)`（`atis/gui.py:246`），精简 `setMinimumSize(300, 220)` + `resize(300, 320)`（`:508-509`），精简藏三样——顶栏、席位按钮区、整个右栏（`:503-505`）。

**Files:**
- Modify: `apps/atis/src-tauri/tauri.conf.json`
- Modify: `apps/atis/src-tauri/src/lib.rs`
- Modify: `docs/manual-test.md`
- Modify: `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md`

- [ ] **步骤 1：最小尺寸**

`apps/atis/src-tauri/tauri.conf.json` 的 `app.windows[0]`：`minWidth` `720` → `900`，`minHeight` `480` → `600`。`width` / `height`（940 × 680）**不动**——它们已经比新的最小值大。

- [ ] **步骤 2：两个常量**

`apps/atis/src-tauri/src/lib.rs:1098,1101`：

```rust
const COMPACT_MIN: (f64, f64) = (300.0, 220.0);
const COMPACT_SIZE: (f64, f64) = (300.0, 320.0);
```

两条文档注释原样留着，只改数字。`apply_window` 一个字不动——正常模式的最小尺寸是从 `tauri.conf.json` 读的，所以步骤 1 改完它自己就跟上了。

- [ ] **步骤 3：确认精简藏对了三样**

不需要改代码，只需要核对前面几个任务的结果：

```bash
grep -n 'v-if="!compact"\|:collapsed="compact"\|compact' apps/atis/src/App.vue
```

要能看到且只看到这三处：顶栏 `<template v-if="!compact">`（任务 7，藏账号表单和配置档钮）、左栏按钮区 `v-if="!compact"`（任务 5）、`<Splitter :collapsed="compact">`（任务 4，藏分割条和整个右栏）。多出来的第四处说明有东西被多藏了，少一处说明有东西在精简里露着。

- [ ] **步骤 4：Rust 的门**

```bash
(cd apps/atis/src-tauri && cargo check --all-targets)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo test -p can-voice-i18n
(cd apps/atis && bun run build)
```

- [ ] **步骤 5：手工测试清单**

`docs/manual-test.md` §4：

- **4.5 桌面稿子** 这一条里的操作路径按新排布改：取真实报文在**字母行**那个钮上（不在播时写着「取真实报文」）；三份稿子在右栏；导入 vATIS 和并入全网配置在**左栏按钮区**（不再是「导入」折叠块）。
- 新增 **4.6 精简模式和两栏**，写这几条：
  - 拖分割条，左栏在 180 和 520 处停住。
  - 点精简：窗口缩到 300×320，只剩标题行和席位列表；置顶和精简两个钮**还在**（这是这个排布存在的理由）。
  - 退出精简：左栏回到 260px，窗口最小 900×600 拖不小。
  - 席位行的单行摘要：不在播 `ZSPD  J`，在播 `● ZSPD  J  09004MPS  Q1013`，圆点绿色。
  - 中英两种界面语言各看一遍：`账号` 那一行和左栏三个钮不能挤成两行。
- §6 那张「哪个界面在哪」的表里，如果有指向通播端折叠块的行，改成新位置。

- [ ] **步骤 6：spec 补记**

`docs/superpowers/specs/2026-09-22-can-audio-layout-design.md` §5 末尾加一段，记下本计划三处刻意的偏离——照 §4 里「**落地时高度取的是 `min-h-[116px]`…这是刻意的偏离**」那个写法：

1. 两个模态底部只有**关闭**，没有 can-audio 的 `确定 / 取消`：can-voice 的编辑器改的是活对象、改一下存一下，改成拷贝加提交是重写行为而不是搬排布。
2. 字母行上的天气钮是**一个钮两条路**：在播走 `refresh`，不在播走 `fetch_metar`。can-audio 只有一条。
3. 顶栏**不放**「检查更新」：`UpdateBanner` 已经在报，再加一个入口是加功能。

- [ ] **步骤 7：全量门**

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
for app in controller atis xpc msfs; do (cd "apps/$app" && bun run build) || echo "$app FAILED"; done
```

- [ ] **步骤 8：人工要看什么**

`cd apps/atis && bun run tauri dev`：

1. 拖窗口到最小：**停在 900×600**，不是 720×480。
2. 点精简：窗口缩到 300×320，再拖也小不过 300×220。
3. 退出精简：最小值回到 900×600，左栏回到 260。
4. 切成英文界面再走一遍第 1–3 条。

- [ ] **步骤 9：提交**

```bash
git add apps/atis/src-tauri/tauri.conf.json apps/atis/src-tauri/src/lib.rs docs/manual-test.md docs/superpowers/specs/2026-09-22-can-audio-layout-design.md
git commit -m "$(cat <<'MSG'
chore(atis): take can-audio's window geometry, and record the divergences

最小 900x600,精简 300x220 / 300x320。spec §5 补记三处刻意的偏离。

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Self-Review

写完之后对着 spec §5 逐条核了一遍，记下三类东西。

**1. spec 覆盖**

| spec §5 的要求 | 落在哪个任务 |
|---|---|
| 两栏加可拖分割条，默认 260 / 其余 | 任务 4 |
| `StationEditor` 变对话框 | 任务 3 |
| `PresetEditor` 变对话框 | 任务 3 |
| 顶栏取 `账号` + 110px + 160px 的形状 | 任务 7 |
| 配置档那几个钮留在顶栏左端 | 任务 7 |
| 置顶 / 精简移进左栏标题行 | 任务 1（拆开）+ 任务 5（放进去） |
| 左栏标题 `通播席位` | 任务 5 |
| 左栏固定高度按钮区，一行三个 + 竖排三个 | 任务 5 |
| 列表行改单行摘要 | 任务 5 |
| Airing 并进 `SettingsDialog` | 任务 7 |
| `LogPanel` 一起并进去 | 任务 7 |
| 右栏六段的顺序 | 任务 6 |
| METAR 保持可编辑 | 任务 6 |
| 文字通播固定 90px | 任务 6 |
| 语音稿占满剩余，装中英两份 | 任务 6 |
| 未知变量告警和 `wire` 留在下面 | 任务 6 |
| 最小尺寸 900×600 | 任务 8 |
| `COMPACT_MIN` / `COMPACT_SIZE` 300×220 / 300×320 | 任务 8 |
| 精简藏顶栏 / 按钮区 / 右栏 | 任务 4、5、7 分头做，任务 8 步骤 3 核 |
| 退出精简恢复 260 分割 | 任务 4 |
| 分割宽度不持久化 | 任务 4 |
| §7：`atis/gui.py:373` 三个硬编码中文按键搬 | 任务 2（`station.new/edit/delete`） |
| §7：两条长中文提示按键搬 | 任务 2（`import.network_tip`）+ 现有 `import.online_tip` |

没有未覆盖的条目。

**2. 改了 spec 一处、补了 spec 一处**

- **改：spec §5 说左栏三个导入钮叫 `从网络更新配置` / `取在线席位` / `导入 vATIS 配置…`**，那是 can-audio 的 `main.net_config` / `main.net_stations` / `main.import_vatis` 的文案。本计划**沿用 can-voice 现有的 `import.network` / `import.online` / `import.vatis`**（「全网通播配置…」「此刻在线的通播席位」「vATIS 配置文件…」）——它们已经是成对翻好的键，说的是同一件事，为了对齐一个 Python 键名去改文案，换来的只是两份字典多一次改动的风险。§7 点名要沿用的键里没有这三个。
- **补：spec §5 没说席位列表里那个圆点的颜色。** can-audio 的圆点不上色（`atis/script.py:105` 就是一个字符）。本计划按状态上色（在播绿 / 连接中橙 / 出错红），理由是 §1 的「can-voice 多出来的东西全留」——状态是 can-voice 有而 can-audio 没有的信息，扔掉它才是丢功能。

**3. 上一轮的教训，这一轮怎么写的**

计划 1 的四个 Critical 全部出自**我抄进计划里的代码**，由实施者忠实照抄，靠评审才拦下来。所以这一份里：

- 每个要改现有文件的任务，第一步都是 `cat` / `sed -n` **把现在那个文件读一遍**，而且明写「形状以下面为准，`:disabled` / `v-if` / `t()` 键以现有代码为准」。
- 两处最容易踩的生命周期坑**写了为什么**，而不只是写怎么做：`Modal` 用普通 watch（`v-if` 在自己根节点上），`AiringPanel` 用 `onMounted`（外层 `v-if` 已经决定了生命周期）——并各自点名 `SettingsCommon` 那条 `{ immediate: true }` **不适用**，免得被照抄。
- 每个任务的「人工要看什么」都点名了**这个任务最容易坏的那一条**（任务 3 是删席位后对话框不关，任务 7 是关掉设置再打开值没了）。仓库没有前端测试框架，这一段就是唯一的行为覆盖。

**4. 类型一致性**

`Splitter` 的 `v-model:width` 对应 `update:width: [value: number]`；`StationList` 的 `pick: [callsign: string]` 对应 `@pick="selected = $event"`（`selected` 是 `Ref<string>`）；`StationDialog` / `PresetDialog` 转发的四个事件名和 `StationEditor` / `PresetEditor` 现有的 `change` / `pick` / `remove` 一致。`stationSummary` 的第二个参数是 `Live | undefined`，`live[callsignOf(s)]` 正好是这个类型（`Record` 的索引结果在 `noUncheckedIndexedAccess` 关掉时是 `Live`——所以签名写成可空是**放宽**，不会红）。

**5. 一条留给计划 3 的账**

`Modal.vue` 只有通播端有，所以不进 `SHARED_FRONTEND`。**计划 3（xpc）如果也要一个模态外壳，那时候要么复用这一份并登记进清单，要么明确说清楚为什么两份不一样**——两份不登记的副本会无声地漂开，而这正是那张清单存在的理由。

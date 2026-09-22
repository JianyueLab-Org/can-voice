# can-audio Layout, Plan 1 of 4: Shared Chrome and the Controller

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `audio-for-can` opens with can-audio's window layout — a centred login page, can-audio's top-bar order, a 232×116 card grid, a one-row online-frequency strip and can-audio's bottom bar — and the three components the other three apps will reuse exist, guarded against drift.

**Architecture:** Shared frontend files stay byte-identical copies per app, the convention this repo already uses; a manifest in the Rust i18n test declares which file belongs to which apps and fails CI when a copy drifts. The can-audio palette moves out of component-level hex literals into CSS custom properties in `style.css`, the file that is already copied four ways and is therefore the equivalent of can-audio's single `theme.py`.

**Tech Stack:** Vue 3.5 SFC + Vite 7 + TypeScript, Tailwind CSS v4 (CSS-configured, no config file), Tauri 2, Rust (the i18n guard tests).

**Spec:** `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md` (§3 and §4)

**Plan series:** Plan 2 is atis (§5), plan 3 is xpc (§6), plan 4 is msfs plus docs. They are written after their predecessor lands, so they describe the chrome as it actually exists rather than as predicted.

## Global Constraints

- **There is no frontend test runner in this repo.** No vitest, no jest, in any of the four `apps/*/package.json`. So the executable gates for frontend work are `bun run build` (which is `vue-tsc --noEmit && vite build`) and the Rust tests in `crates/can-voice-i18n`. Tasks that change layout carry a named **manual check** instead of a unit test; tasks that change a guard or a dictionary carry a real failing-test-first cycle. Do not add a test runner — the spec rules out new build machinery (§2).
- Per-app frontend gate: `cd apps/<app> && bun install && bun run build`. There is **no `lint` script** in these apps; do not invoke one.
- Repo-root Rust gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Tauri-side gate when Rust in an app changes: `cd apps/<app>/src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets`.
- **Code comments in these files are Chinese**, matching every file you will touch (`apps/*/src/**/*.vue`, `apps/*/src-tauri/src/lib.rs`). Commit messages are English.
- **Every new UI string needs a zh and an en entry.** `crates/can-voice-i18n/tests/dictionaries.rs` checks parity and placeholders and rejects hardcoded Chinese in `apps/*/src`. Strings in `common.*.json` must be byte-identical across all four apps; strings in `app.*.json` are per app.
- **An `app.*.json` namespace must not collide with a `common.*.json` namespace** (`an_app_dictionary_does_not_reuse_a_common_namespace`). Controller `common` namespaces: `common`, `error`, `language`, `log`, `notice`, `settings`, `update`, `window`. Controller `app` namespaces: `app`, `audio`, `duty`, `ended`, `freq`, `health`, `link`, `login`, `online`, `panel`, `ptt`, `radio`.
- Palette values are fixed and come from can-audio's `controller/theme.py`: off `#436384`, on `#28a745`, active `#c7861d`, muted `#dc3545`, theme `#5eb1bf`, idle `#8b90a4`, surface `#252839`, window `#2c2f45`.
- The Rust bridge command surface and the 200 ms polling loop in `App.vue` are out of scope (spec §10). No `invoke` name changes, no new Tauri commands except where a task says so — and no task here says so.
- Scratch files go in `can-voice/.temp/`. Never `/tmp`, `/private/tmp` or `$TMPDIR`. Delete it when the task is done.
- Commit messages end with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- Work on branch `design/can-audio-layout`, which already exists and holds the spec commit.

---

## File Structure

**Created (each a byte-identical copy in the apps listed):**

| File | Apps in this plan | Responsibility |
|---|---|---|
| `src/components/SettingsCommon.vue` | all four | The three settings sections every app shares: Appearance, Endpoints, Troubleshooting. Extracted so each app's `SettingsDialog.vue` can become app-local without losing the shared half. |
| `src/components/StateToggle.vue` | controller | can-audio's hand-painted tri-state toggle. Later reused by xpc and msfs. |
| `src/components/StatusBar.vue` | controller | can-audio's bottom bar: PTT lamp, status caption, duty caption, plus a slot for app extras. Later reused by xpc and msfs. |
| `src/components/LoginCard.vue` | controller only | can-audio's page-0 login card. Not shared — atis and the pilot clients put credentials in a bar and a card respectively. |
| `src/components/Toast.vue` | controller only | Replaces can-audio's `InfoBar.warning`: top-right, 4 s, auto-dismiss. |

**Modified:**

| File | Change |
|---|---|
| `apps/{controller,atis,xpc,msfs}/src/style.css` | Palette tokens. Stays byte-identical four ways. |
| `apps/{controller,atis,xpc,msfs}/src/components/SettingsDialog.vue` | Composes `SettingsCommon.vue`. Leaves the shared manifest — it becomes app-local from here on. |
| `apps/{controller,atis,xpc,msfs}/src/components/WindowToggles.vue` | Short labels in compact. Stays byte-identical four ways. |
| `apps/{controller,atis,xpc,msfs}/src/locales/common.{zh,en}.json` | Two `window.*` short labels. Stays byte-identical four ways. |
| `apps/controller/src/App.vue` | Login page split, top-bar order, add-bar proportions, settings panel moved into the dialog, toast, status bar, compact padding. |
| `apps/controller/src/components/RadioRow.vue` | Uses `StateToggle`; card gets a minimum height; gain slider reads 0–100. |
| `apps/controller/src/components/OnlineList.vue` | One row of pills, callsign only. |
| `apps/controller/src/locales/app.{zh,en}.json` | `status.ready`, `login.idle`. |
| `apps/controller/src-tauri/src/lib.rs` | `COMPACT_MIN` / `COMPACT_SIZE` become can-audio's 248×186. |
| `apps/controller/src-tauri/tauri.conf.json` | Minimum becomes 620×480. |
| `crates/can-voice-i18n/tests/dictionaries.rs` | Palette assertion; shared-file manifest assertion. |
| `docs/manual-test.md` | Controller layout steps. |

**Deleted:** nothing. `SettingsPanel.vue` survives unchanged — task 6 renders it inside the dialog rather than transcribing it.

---

### Task 1: Palette tokens

**Files:**
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs` (append at end of file)
- Modify: `apps/controller/src/style.css:37` (after the existing `@layer base` block)
- Modify: `apps/atis/src/style.css`, `apps/xpc/src/style.css`, `apps/msfs/src/style.css` (copies of the controller's)

**Interfaces:**
- Consumes: nothing.
- Produces: CSS custom properties on `:root`, readable from any component as `var(--can-off)`, `var(--can-on)`, `var(--can-active)`, `var(--can-muted)`, `var(--can-theme)`, `var(--can-idle)`, `var(--can-surface)`, `var(--can-window)`. Tasks 3, 4, 5 and 6 read them.

- [ ] **Step 1: Write the failing test**

Append to `crates/can-voice-i18n/tests/dictionaries.rs`:

```rust
// ——— 界面的颜色只有一处声明 ———

/// can-audio 那套语义色，对应它四个客户端逐字节相同的 `controller/theme.py`。
///
/// **不在组件里写十六进制字面量**：旧版只有一个 theme.py，而这边同一个颜色
/// 散在四个客户端的组件里，改一处就漏三处。
#[test]
fn every_app_declares_the_can_audio_palette() {
    const TOKENS: &[(&str, &str)] = &[
        ("--can-off", "#436384"),
        ("--can-on", "#28a745"),
        ("--can-active", "#c7861d"),
        ("--can-muted", "#dc3545"),
        ("--can-theme", "#5eb1bf"),
        ("--can-idle", "#8b90a4"),
        ("--can-surface", "#252839"),
        ("--can-window", "#2c2f45"),
    ];
    for app in APPS {
        let css = std::fs::read_to_string(repo().join(format!("apps/{app}/src/style.css")))
            .unwrap_or_else(|e| panic!("{app}: style.css: {e}"));
        for (name, value) in TOKENS {
            let decl = format!("{name}: {value}");
            assert!(css.contains(&decl), "{app}: style.css does not declare `{decl}`");
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p can-voice-i18n every_app_declares_the_can_audio_palette`
Expected: FAIL — `controller: style.css does not declare `--can-off: #436384``

- [ ] **Step 3: Add the tokens to the controller's style.css**

Insert immediately after the closing `}` of the existing `@layer base` block (currently `apps/controller/src/style.css:37`):

```css
@layer base {
  /*
   * can-audio 那套语义色，对应它逐字节相同的 controller/theme.py。
   *
   * **明暗两套主题下相同**：三态开关的"关 / 开 / 正在响"在旧版里也不随主题变，
   * 它讲的是电台的状态，不是背景的深浅。界面上再要一个颜色的人在这里加一行,
   * 不要在组件里写字面量——`every_app_declares_the_can_audio_palette` 看着这张表。
   */
  :root {
    --can-off: #436384;
    --can-on: #28a745;
    --can-active: #c7861d;
    --can-muted: #dc3545;
    --can-theme: #5eb1bf;
    --can-idle: #8b90a4;
    --can-surface: #252839;
    --can-window: #2c2f45;
  }
}
```

- [ ] **Step 4: Copy to the other three apps**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
for a in atis xpc msfs; do cp apps/controller/src/style.css "apps/$a/src/style.css"; done
md5 -q apps/*/src/style.css | sort -u | wc -l   # must print 1
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p can-voice-i18n`
Expected: PASS, including `the_common_dictionaries_are_identical_in_every_app`

- [ ] **Step 6: Build one app to prove the CSS still compiles**

Run: `cd apps/controller && bun install && bun run build`
Expected: exit 0. Tailwind 4 parses a bare `@layer base` block; a stray brace shows up here.

- [ ] **Step 7: Commit**

```bash
git add crates/can-voice-i18n/tests/dictionaries.rs apps/*/src/style.css
git commit -m "$(cat <<'MSG'
feat(apps): declare the can-audio palette as CSS custom properties

The four style.css copies stay byte-identical; a test asserts every app
declares all eight tokens.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 2: Shared-file manifest, and `SettingsCommon.vue`

**Why this task exists:** the spec (§4, §5, §6) has each app's settings dialog grow app-specific sections — Audio/PTT/Log on the controller, Airing on atis, a 音频/网络/他机 pivot on the pilot clients. `SettingsDialog.vue` is byte-identical across all four today, so it cannot stay shared. The shared half is extracted first so nothing is lost when the dialogs diverge.

**Files:**
- Create: `apps/controller/src/components/SettingsCommon.vue`, then copies in `atis`, `xpc`, `msfs`
- Modify: `apps/{controller,atis,xpc,msfs}/src/components/SettingsDialog.vue`
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs`

**Interfaces:**
- Consumes: the palette from task 1 (not directly used here, but present).
- Produces: `SettingsCommon.vue` taking one prop, `open: boolean`, and rendering the Appearance, Endpoints and Troubleshooting sections. It owns the `watch(() => props.open)` that loads `endpoint_fields` and `settings` from Rust. Plans 2–4 mount it the same way. Also produces the `SHARED_FRONTEND` manifest constant, which plans 2–4 extend.

- [ ] **Step 1: Write the failing test**

Append to `crates/can-voice-i18n/tests/dictionaries.rs`:

```rust
// ——— 共用的前端文件是逐字节相同的副本 ———

/// 哪些前端文件是共用的，以及每个该出现在哪几个客户端里。
///
/// **这张表就是"共用"这件事的声明处。** 仓库里共用代码靠复制而不是抽包
/// （见 README 里桌面端不进 workspace 那一段），所以唯一的防线是断言副本相同：
/// 改了一个客户端而忘了其余三个，在这里失败；某个客户端多出一份没登记的同名
/// 文件，也在这里失败。没登记的文件按定义就是那个客户端自己的。
const SHARED_FRONTEND: &[(&str, &[&str])] = &[
    ("appearance.ts", &["controller", "atis", "xpc", "msfs"]),
    ("i18n.ts", &["controller", "atis", "xpc", "msfs"]),
    ("main.ts", &["controller", "atis", "xpc", "msfs"]),
    ("style.css", &["controller", "atis", "xpc", "msfs"]),
    ("components/LogPanel.vue", &["controller", "atis", "xpc", "msfs"]),
    ("components/SettingsCommon.vue", &["controller", "atis", "xpc", "msfs"]),
    ("components/StartupGate.vue", &["controller", "atis", "xpc", "msfs"]),
    ("components/UpdateBanner.vue", &["controller", "atis", "xpc", "msfs"]),
    ("components/WindowToggles.vue", &["controller", "atis", "xpc", "msfs"]),
];

#[test]
fn shared_frontend_files_are_identical_in_every_app_that_carries_them() {
    for (file, owners) in SHARED_FRONTEND {
        let path = |app: &str| repo().join(format!("apps/{app}/src/{file}"));
        let first =
            std::fs::read(path(owners[0])).unwrap_or_else(|e| panic!("{}: {file}: {e}", owners[0]));
        for app in &owners[1..] {
            let other = std::fs::read(path(app)).unwrap_or_else(|e| panic!("{app}: {file}: {e}"));
            assert!(
                other == first,
                "apps/{app}/src/{file} differs from apps/{}'s",
                owners[0]
            );
        }
        for app in APPS {
            assert!(
                owners.contains(app) || !path(app).exists(),
                "apps/{app}/src/{file} exists but is not registered for {app} in SHARED_FRONTEND"
            );
        }
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p can-voice-i18n shared_frontend_files_are_identical`
Expected: FAIL — `controller: components/SettingsCommon.vue: No such file or directory`

- [ ] **Step 3: Create `SettingsCommon.vue`**

Create `apps/controller/src/components/SettingsCommon.vue`. Its `<script setup>` is `SettingsDialog.vue:1-101` with the dialog-only parts dropped — remove `nextTick` from the vue import, remove `const emit = defineEmits<{ close: [] }>()`, remove `const box = ref<HTMLElement | null>(null)` and the two lines `await nextTick(); box.value?.focus();` from the watch. Its `<template>` is `SettingsDialog.vue:117-194` verbatim — the three `<section>` elements — wrapped in a fragment. The header comment becomes:

```vue
<script setup lang="ts">
/**
 * 设置对话框里四个客户端共有的那三段：外观、地址、故障排查。
 *
 * **从 SettingsDialog 里抽出来的**，因为那个对话框从此每个客户端都不一样——
 * 管制端要加音频 / PTT / 日志，通播端要加播出，两个飞行员端换成枢轴分页。
 * 共有的这三段仍然只有一份，逐字节相同，`SHARED_FRONTEND` 看着它。
 *
 * 语音服务器、FSD、can-api 这几个地址此前只能靠环境变量改——一个装了 msi 的人
 * 没有地方改它们。**每个客户端用哪几格由 Rust 侧报**（`endpoint_fields`），
 * 默认值也从那里来：这里再抄一份默认地址，迟早和真正生效的那个对不上。
 */
```

- [ ] **Step 4: Rewrite `SettingsDialog.vue` to compose it**

Replace the whole file with:

```vue
<script setup lang="ts">
import SettingsCommon from "./SettingsCommon.vue";
import { t } from "../i18n";

/**
 * 设置对话框的外壳（#45）。
 *
 * **这个文件每个客户端一份，不再共用**：共有的三段在 SettingsCommon 里，
 * 各客户端自己那几段直接写在这里。
 */
const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ close: [] }>();

import { nextTick, ref, watch } from "vue";
const box = ref<HTMLElement | null>(null);
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
      class="flex max-h-full w-[30rem] flex-col gap-4 overflow-auto rounded border bg-white p-4 text-sm outline-none"
      @keyup.escape="emit('close')"
    >
      <h2 class="font-semibold">{{ t("settings.title") }}</h2>

      <SettingsCommon :open="open" />
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

Move the two `import` statements to the top of the script block — the split above is for readability only, and `vue-tsc` is indifferent, but keep the file tidy.

- [ ] **Step 5: Copy both files to the other three apps**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
for a in atis xpc msfs; do
  cp apps/controller/src/components/SettingsCommon.vue "apps/$a/src/components/SettingsCommon.vue"
  cp apps/controller/src/components/SettingsDialog.vue "apps/$a/src/components/SettingsDialog.vue"
done
```

- [ ] **Step 6: Run the test to verify it passes**

Run: `cargo test -p can-voice-i18n`
Expected: PASS. Note `SettingsDialog.vue` is deliberately **absent** from `SHARED_FRONTEND` — it is app-local from now on, and an unlisted file is never checked.

- [ ] **Step 7: Build all four apps**

```bash
for a in controller atis xpc msfs; do (cd "apps/$a" && bun install && bun run build) || break; done
```
Expected: four clean builds.

- [ ] **Step 8: Manual check**

Run `cd apps/controller && bun run dev`, open `http://localhost:4330`, click 设置. Expected: the dialog shows Appearance, Endpoints and Troubleshooting exactly as before; the language select still switches language immediately; Save on an endpoint still reports 已保存.

- [ ] **Step 9: Commit**

```bash
git add crates/can-voice-i18n/tests/dictionaries.rs apps/*/src/components/SettingsCommon.vue apps/*/src/components/SettingsDialog.vue
git commit -m "$(cat <<'MSG'
refactor(apps): split the shared settings sections out of the dialog

The dialog shell becomes app-local because each app grows its own
sections; the three common sections stay one byte-identical copy.
SHARED_FRONTEND is the manifest that keeps the copies honest.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 3: `StateToggle.vue`, and the radio card's remaining deltas

**Note on scope:** the spec §4 lists four card deltas. Two of them are **already implemented** — `RadioRow.vue:93-94` already renders the frequency in `font-mono text-lg font-semibold` with a `▸ ` prefix when selected, and `:71-77` already produces can-audio's `最后通话: {who}  {stamp}` / `最后通话: --` wording via existing keys. What remains is the card's height, the gain scale, and moving the four buttons onto the shared component.

**Files:**
- Create: `apps/controller/src/components/StateToggle.vue`
- Modify: `apps/controller/src/components/RadioRow.vue`
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs` (one manifest line)

**Interfaces:**
- Consumes: palette tokens from task 1.
- Produces: `StateToggle.vue` with props `{ label: string; state: "off" | "on" | "active" | "muted"; width: number; height: number; disabled?: boolean }` and one emit, `press: []`. xpc and msfs mount it in plans 3 and 4 for their TX/RX chips at 46×26.

- [ ] **Step 1: Add the manifest line and watch it fail**

In `SHARED_FRONTEND`, insert in alphabetical position:

```rust
    ("components/StateToggle.vue", &["controller"]),
```

Run: `cargo test -p can-voice-i18n shared_frontend_files_are_identical`
Expected: FAIL — `controller: components/StateToggle.vue: No such file or directory`

- [ ] **Step 2: Create `StateToggle.vue`**

```vue
<script setup lang="ts">
/**
 * 三态开关。can-audio `controller/gui.py:202-238` 那个手绘的 StateToggle。
 *
 * 旧版刻意不用 qfluentwidgets 的按钮：那个库会把主题色重新刷上去，三种状态就
 * 扁成一种。这边的等价问题是 Tailwind 的颜色工具类会被 style.css 的深色覆盖
 * 改写，所以颜色走 `--can-*`，尺寸走行内 style——**这颗按钮讲的是电台的状态，
 * 不跟主题走**。
 *
 * 边框是填充色压暗到 71%（旧版 `fill.darker(140)`）。
 *
 * 用到它的客户端：controller。xpc / msfs 的 TX / RX 色块在后面的计划里接上。
 * 逐字节相同的一份。
 */
const props = defineProps<{
  label: string;
  state: "off" | "on" | "active" | "muted";
  /** 宽高（px）。can-audio 的尺寸：RX / TX 52×26，XC 36×22，静音 46×22。 */
  width: number;
  height: number;
  disabled?: boolean;
}>();

defineEmits<{ press: [] }>();

const fill = () => `var(--can-${props.state})`;
</script>

<template>
  <button
    type="button"
    class="shrink-0 rounded border text-xs font-bold text-white"
    :class="disabled ? 'opacity-40' : ''"
    :style="{
      background: fill(),
      borderColor: `color-mix(in srgb, ${fill()} 71%, black)`,
      width: `${width}px`,
      height: `${height}px`,
    }"
    :disabled="disabled"
    @click.stop="$emit('press')"
  >
    {{ label }}
  </button>
</template>
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p can-voice-i18n shared_frontend_files_are_identical`
Expected: PASS

- [ ] **Step 4: Rewire `RadioRow.vue`**

Four edits.

(a) Import it and drop the four colour literals. Replace `RadioRow.vue:41-45` with:

```ts
import StateToggle from "./StateToggle.vue";
```

placed with the other import at the top, and replace the `fill` function at `:58-64` with a state function:

```ts
/** TrackAudio 三态：关 / 开 / 正在响。静音时 RX 整颗变红。 */
function state(s: "rx" | "tx" | "xc"): "off" | "on" | "active" | "muted" {
  if (s === "rx" && props.radio.muted) return "muted";
  if (!props.radio[s]) return "off";
  if (s === "rx" && props.receiving) return "active";
  if (s === "tx" && props.transmitting) return "active";
  return "on";
}
```

(b) Replace the four buttons. `RadioRow.vue:104-124` (the RX and TX buttons) becomes:

```vue
        <StateToggle
          label="RX"
          :state="state('rx')"
          :width="52"
          :height="26"
          :title="switchTitle('rx')"
          @press="toggle('rx')"
        />
        <StateToggle
          label="TX"
          :state="state('tx')"
          :width="52"
          :height="26"
          :disabled="!transmitAllowed"
          :title="switchTitle('tx')"
          @press="toggle('tx')"
        />
```

and `:128-147` (the XC and mute buttons) becomes:

```vue
      <StateToggle
        label="XC"
        :state="state('xc')"
        :width="36"
        :height="22"
        :disabled="!transmitAllowed"
        :title="switchTitle('xc')"
        @press="toggle('xc')"
      />
      <StateToggle
        :label="t('radio.mute')"
        :state="radio.muted ? 'muted' : 'off'"
        :width="46"
        :height="22"
        :title="radio.muted ? t('radio.unmute_tip') : t('radio.mute_tip')"
        @press="$emit('mute', !radio.muted)"
      />
```

`title` is not a declared prop; it falls through to the root `<button>` as a plain attribute, which is what we want.

(c) The card gets can-audio's height. In the root `div`'s class binding at `:85`, change both branches:

```
      compact ? 'min-h-[116px] w-[200px] gap-1 p-2' : 'min-h-[116px] w-[232px] gap-1.5 p-2.5',
```

Use `min-h-`, not `h-`: can-audio's card is a flat 232×116, but can-voice appends 发射被拒 / 接收被拒 notes that can-audio has no equivalent of, and a hard height would clip them.

(d) The gain slider reads 0–100. Replace `:150-159`:

```vue
      <input
        v-if="!compact"
        type="range"
        min="0"
        max="100"
        step="1"
        :value="Math.round(radio.gain * 50)"
        class="h-[18px] min-w-0 flex-1"
        :class="radio.muted ? 'opacity-40' : ''"
        @click.stop
        @input="
          $emit('volume', Number(($event.target as HTMLInputElement).value) / 50)
        "
      />
```

can-audio's slider is 0–100 while the bridge takes 0–2, so the factor is 50. The conversion stays a single expression on purpose — there is no frontend test runner to cover a helper.

- [ ] **Step 5: Build**

Run: `cd apps/controller && bun run build`
Expected: exit 0.

- [ ] **Step 6: Manual check**

`bun run dev`, connect, add 121.800. Expected: the card is 232 wide and at least 116 tall; RX/TX are 52×26 and XC is 36×22 with a visibly darker 1px border; RX turns green when on and amber while someone talks; muting turns RX red; the gain slider sits mid-travel at the default gain of 1.0 and dragging it changes the received volume.

- [ ] **Step 7: Commit**

```bash
git add crates/can-voice-i18n/tests/dictionaries.rs apps/controller/src/components/StateToggle.vue apps/controller/src/components/RadioRow.vue
git commit -m "$(cat <<'MSG'
feat(controller): take can-audio's tri-state toggle and card metrics

The toggle becomes a shared component xpc and msfs will reuse. The card
keeps a 116px floor rather than a fixed height, because can-voice appends
denial notes can-audio never had. The gain slider reads 0-100 as
can-audio's does and converts to the bridge's 0-2.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 4: `StatusBar.vue` and the controller's bottom bar

**Files:**
- Create: `apps/controller/src/components/StatusBar.vue`
- Modify: `apps/controller/src/App.vue` (template, replacing the `<footer>` at `:489-526`)
- Modify: `apps/controller/src/locales/app.zh.json`, `apps/controller/src/locales/app.en.json`
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs` (one manifest line)

**Interfaces:**
- Consumes: palette tokens (task 1).
- Produces: `StatusBar.vue` with props `{ talking: boolean; status: string; duty: string; dutyOn: boolean }`, emits `down: [e: PointerEvent]` and `up: []`, and a default slot rendered after the duty caption. xpc and msfs mount it in plans 3 and 4.

- [ ] **Step 1: Add the two dictionary keys and the manifest line, and watch them fail**

Add to `apps/controller/src/locales/app.zh.json` a new top-level namespace (`status` collides with nothing — see Global Constraints):

```json
  "status": {
    "ready": "就绪"
  },
```

and to `app.en.json`:

```json
  "status": {
    "ready": "Ready"
  },
```

In `SHARED_FRONTEND`, insert in alphabetical position:

```rust
    ("components/StatusBar.vue", &["controller"]),
```

Run: `cargo test -p can-voice-i18n`
Expected: FAIL — `controller: components/StatusBar.vue: No such file or directory`. The dictionary tests pass: parity holds because both files gained the key, and an unused key is not an error.

- [ ] **Step 2: Create `StatusBar.vue`**

```vue
<script setup lang="ts">
/**
 * 底栏。can-audio `controller/gui.py:650-668`：PTT 灯、占满剩余宽度的状态文案、
 * 值守文案。
 *
 * **PTT 在这里是按钮，旧版是纯指示灯。** Linux 上全局按键监听走 Xlib，Wayland
 * 不给一个普通程序监听全局按键（见 README），所以屏幕上这一颗是那种情况下唯一
 * 能发话的路径，不能退化成一个只会亮的灯。
 *
 * 插槽在值守文案之后，给各客户端自己那一格——管制端放链路健康统计。
 *
 * 用到它的客户端：controller。xpc / msfs 的状态栏在后面的计划里接上。
 * 逐字节相同的一份。
 */
defineProps<{
  /** 正在发话。灯转成 active 色。 */
  talking: boolean;
  /** 中间那句话。can-audio 空闲时是"就绪"。 */
  status: string;
  /** 右边那句话。 */
  duty: string;
  /** 在席位上。着绿，旧版如此。 */
  dutyOn: boolean;
}>();

defineEmits<{ down: [e: PointerEvent]; up: [] }>();
</script>

<template>
  <footer class="flex items-center gap-3 border-t pt-3 text-xs">
    <button
      class="flex shrink-0 items-center gap-2"
      @pointerdown="$emit('down', $event)"
      @pointerup="$emit('up')"
      @pointercancel="$emit('up')"
    >
      <span
        class="inline-block h-3 w-3 rounded-full"
        :style="{ background: talking ? 'var(--can-active)' : 'var(--can-idle)' }"
      />
      <span
        class="text-xs"
        :class="talking ? 'font-bold' : 'opacity-60'"
        :style="talking ? { color: 'var(--can-active)' } : {}"
      >
        PTT
      </span>
    </button>
    <span class="min-w-0 flex-1 truncate opacity-70">{{ status }}</span>
    <span class="shrink-0" :class="dutyOn ? 'text-green-700' : 'opacity-70'">{{ duty }}</span>
    <slot />
  </footer>
</template>
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p can-voice-i18n`
Expected: PASS

- [ ] **Step 4: Mount it in `App.vue`**

Add to the imports:

```ts
import StatusBar from "./components/StatusBar.vue";
```

Add to the script, after the `statusText` computed:

```ts
/**
 * 底栏中间那句话。can-audio 的状态栏空闲时说"就绪"，有事说那件事。
 *
 * 和顶栏那句 `statusText` 分开：那一句讲链路，这一句讲刚刚发生了什么。
 */
const barStatus = computed(() => (error.value ? problemText(error.value) : t("status.ready")));

/** 右边那句话。can-audio 值守时着绿，不值守说"只收不发"。 */
const dutyText = computed(() =>
  onDuty.value
    ? t("duty.staffing", { callsign: feed.value?.duty.callsign ?? "" })
    : t("duty.observer"),
);
```

Replace the whole `<footer>` block at `App.vue:489-526` with:

```vue
      <StatusBar
        v-if="!compact"
        :talking="talking"
        :status="barStatus"
        :duty="dutyText"
        :duty-on="connected && onDuty"
        @down="pttDown"
        @up="pttUp"
      >
        <span v-if="snap?.health" class="shrink-0 opacity-60">
          {{
            t("health.summary", {
              rtt: snap.health.rtt_ms,
              received: snap.health.received,
              lost: snap.health.lost,
            })
          }}
          <span v-if="snap.health.unparsable" class="text-amber-700">
            {{ t("health.unparsable", { count: snap.health.unparsable }) }}
          </span>
        </span>
      </StatusBar>
```

Read the exact placeholder names for `health.summary` and `health.unparsable` out of `apps/controller/src/locales/app.zh.json` before writing this — the dictionary test checks placeholders, and the old footer is the source of truth for them.

- [ ] **Step 5: Build**

Run: `cd apps/controller && bun run build`
Expected: exit 0

- [ ] **Step 6: Manual check**

`bun run dev`. Expected, disconnected: the bottom bar reads 就绪 in the middle and 未上席位 · 只收不发 on the right. Type `999` into the frequency box and press Add: the middle caption turns into the out-of-band message. Hold the PTT button: the lamp turns amber and `PTT` goes bold.

- [ ] **Step 7: Commit**

```bash
git add crates/can-voice-i18n/tests/dictionaries.rs apps/controller/src/components/StatusBar.vue apps/controller/src/App.vue apps/controller/src/locales/app.zh.json apps/controller/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(controller): adopt can-audio's bottom bar

PTT lamp, a status caption that takes the slack, the duty caption, and a
slot the health stats keep. The lamp stays a real hold-to-talk button:
on Wayland it is the only PTT that works.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 5: Login becomes its own page

**Files:**
- Create: `apps/controller/src/components/LoginCard.vue`
- Modify: `apps/controller/src/App.vue`
- Modify: `apps/controller/src/locales/app.zh.json`, `apps/controller/src/locales/app.en.json`

**Interfaces:**
- Consumes: palette tokens (task 1).
- Produces: `LoginCard.vue` with props `{ cid: string; status: string; failed: boolean; busy: boolean; version: string }` and one emit, `connect: [cid: string, password: string]`. Controller-only — not added to `SHARED_FRONTEND`.

- [ ] **Step 1: Add the dictionary key and watch the key test fail**

`app.zh.json`, inside the existing `login` namespace:

```json
    "idle": "未连接",
```

`app.en.json`:

```json
    "idle": "Not connected",
```

Run: `cargo test -p can-voice-i18n`
Expected: PASS — an unused key is not an error, so this step cannot fail on its own. It is here so the key exists before step 3 references it; step 3 is what would fail `every_key_the_interface_asks_for_exists` if the key were missing. To see that guard work, temporarily write `t("login.idle_typo")` in step 3, run the test, watch it fail, then fix it.

- [ ] **Step 2: Create `LoginCard.vue`**

```vue
<script setup lang="ts">
import { ref } from "vue";
import { t } from "../i18n";

/**
 * 登录页。can-audio `controller/gui.py:470-524` 的第 0 页：居中一张 340px 的卡片。
 *
 * **独立成页，不挤在顶栏里。** 换客户端的那一天，人第一眼看到的东西要和旧版
 * 一样——旧版是两页 QStackedWidget，连上之后才换成台面。
 *
 * 卡片下面那行字既是状态也是报错，旧版就是这么用的一个 CaptionLabel。
 *
 * 只有管制端用：通播端把账号放在顶栏一行里，两个飞行员端放在"连接"卡片里。
 */
const props = defineProps<{
  /** 预填的 CAN 号。设置文件里存着上次那个。 */
  cid: string;
  status: string;
  /** 报错时着红。 */
  failed: boolean;
  busy: boolean;
  /** 卡片外面那行版本号。不翻译——要和日志里那一行对得上。 */
  version: string;
}>();

const emit = defineEmits<{ connect: [cid: string, password: string] }>();

const cid = ref(props.cid);
const password = ref("");

function submit() {
  if (props.busy) return;
  emit("connect", cid.value, password.value);
  // 密码用过就丢：它只换一张 60 秒的票。
  password.value = "";
}
</script>

<template>
  <div class="flex h-full flex-col items-center justify-center gap-2">
    <div class="flex w-[340px] flex-col gap-3 rounded-lg border px-6 py-5">
      <h1 class="text-center text-base font-semibold">{{ t("app.title") }}</h1>
      <input
        v-model="cid"
        :placeholder="t('login.cid')"
        class="rounded border px-2 py-1"
        @keyup.enter="submit"
      />
      <input
        v-model="password"
        type="password"
        :placeholder="t('login.password')"
        class="rounded border px-2 py-1"
        @keyup.enter="submit"
      />
      <button
        class="rounded border px-3 py-1 font-semibold text-white"
        :style="{ background: 'var(--can-theme)' }"
        :class="busy ? 'opacity-60' : ''"
        :disabled="busy"
        @click="submit"
      >
        {{ t("login.connect") }}
      </button>
      <p class="text-center text-xs" :class="failed ? 'text-red-600' : 'opacity-60'">
        {{ status }}
      </p>
    </div>
    <p class="text-xs opacity-50">{{ version }}</p>
  </div>
</template>
```

- [ ] **Step 3: Wire the page swap in `App.vue`**

Add imports:

```ts
import LoginCard from "./components/LoginCard.vue";
import { getVersion } from "@tauri-apps/api/app";
```

Add to the script:

```ts
/** 版本号，登录页那行。Tauri 从 tauri.conf.json 读，不用再开一个命令。 */
const version = ref("");

/**
 * 登录页那行字。没连上时说链路状态，连接失败时说失败的原因。
 *
 * `statusText` 在 Offline 时已经会把 `ended` 说成人话，所以这里只需要在
 * 命令本身失败时盖掉它。
 */
const loginStatus = computed(() => {
  if (error.value?.kind === "command") return problemText(error.value);
  return snap.value ? statusText.value : t("login.idle");
});
const loginFailed = computed(() => error.value?.kind === "command");
```

In `onMounted`, after the existing `cid.value = ...` line:

```ts
  version.value = await getVersion();
```

Change `connect()` to take the pair from the card, since the card owns those two inputs now:

```ts
async function connect(enteredCid: string, enteredPassword: string) {
  error.value = null;
  busy.value = true;
  try {
    await invoke("connect", { cid: enteredCid, password: enteredPassword });
    cid.value = enteredCid;
    await refresh();
  } catch (e) {
    error.value = { kind: "command", error: e };
  } finally {
    busy.value = false;
  }
}
```

Delete the now-unused `password` ref.

In the template, wrap the existing `<main>` contents so the login page replaces them. Immediately inside `<StartupGate>`, before `<main>`:

```vue
    <main v-if="!connected" class="flex h-screen w-full flex-col gap-3 p-5 text-sm">
      <UpdateBanner />
      <LoginCard
        :cid="cid"
        :status="loginStatus"
        :failed="loginFailed"
        :busy="busy"
        :version="version"
        @connect="connect"
      />
    </main>

    <main v-else class="flex h-screen w-full flex-col text-sm" :class="compact ? 'gap-2 p-2' : 'gap-4 p-5'">
```

and delete the credential inputs and the Connect button from the header — `App.vue:351-363` — leaving the `v-else-if="!compact"` Disconnect button, which task 6 moves.

`UpdateBanner` appears on the login page because that is when an update matters; the main page keeps its own copy at `:370`.

- [ ] **Step 4: Build**

Run: `cd apps/controller && bun run build`
Expected: exit 0. `vue-tsc` catches a stale reference to the deleted `password` ref here.

- [ ] **Step 5: Manual check**

`bun run dev`. Expected: a centred 340px card on an otherwise empty window, app name at the top, 未连接 beneath, the version below the card. Enter a wrong password and press enter: the caption turns red with the refusal reason. Enter a right one: the window switches to the radio stack. Quit and reopen: the CAN number is prefilled, the password box is empty.

- [ ] **Step 6: Commit**

```bash
git add apps/controller/src/components/LoginCard.vue apps/controller/src/App.vue apps/controller/src/locales/app.zh.json apps/controller/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(controller): give login its own page, as can-audio has

A centred 340px card replaces the credentials wedged into the header.
The caption under the card doubles as the failure message, which is what
can-audio does with it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 6: Top bar, add bar, one settings surface, and the toast

**Files:**
- Create: `apps/controller/src/components/Toast.vue`
- Modify: `apps/controller/src/App.vue`
- Modify: `apps/controller/src/components/SettingsDialog.vue` (controller's copy only — it is app-local after task 2)

**Interfaces:**
- Consumes: `SettingsCommon.vue` (task 2) via the dialog's slot; `SettingsPanel.vue` unchanged.
- Produces: `Toast.vue` with props `{ message: string | null }`, no emits; it self-dismisses after 4000 ms.

- [ ] **Step 1: Create `Toast.vue`**

```vue
<script setup lang="ts">
import { ref, watch } from "vue";

/**
 * 右上角一条，4 秒后自己消失。can-audio 用 `qfluentwidgets.InfoBar.warning`
 * 报校验错误（`controller/gui.py:799-806`），不弹模态。
 *
 * **不用模态**：值班的人手上有飞机，一个要点确定才能继续的框比那条错误本身更碍事。
 */
const props = defineProps<{ message: string | null }>();

const shown = ref<string | null>(null);
let timer: number | undefined;

watch(
  () => props.message,
  (m) => {
    window.clearTimeout(timer);
    shown.value = m;
    if (m) timer = window.setTimeout(() => (shown.value = null), 4000);
  },
);
</script>

<template>
  <div
    v-if="shown"
    class="fixed right-3 top-3 z-40 max-w-80 rounded border border-amber-400 bg-white px-3 py-2 text-xs text-amber-700 shadow"
  >
    {{ shown }}
  </div>
</template>
```

- [ ] **Step 2: Reorder the header**

Replace the `<header>` block at `App.vue:338-368` with:

```vue
      <!-- can-audio 的顺序（controller/gui.py:536-583）：灯、链路、会话、撑开、
           置顶、精简、设置、断开。标题不在这里——旧版主页面没有标题，它在窗口
           装饰和登录卡片上。 -->
      <header class="flex flex-wrap items-center gap-2">
        <span
          class="inline-block h-2.5 w-2.5 shrink-0 rounded-full"
          :style="{ background: connected ? 'var(--can-on)' : 'var(--can-muted)' }"
        />
        <p class="truncate text-xs opacity-70">{{ statusText }}</p>
        <p class="truncate text-xs font-semibold">{{ dutyText }}</p>
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
        <button v-if="!compact" class="rounded border px-3 py-1 text-xs" @click="disconnect">
          {{ t("login.disconnect") }}
        </button>
      </header>
```

- [ ] **Step 3: Give the add bar can-audio's proportions**

Replace `App.vue:431-450` with:

```vue
      <!-- can-audio 的比例（controller/gui.py:586-617）：频率 1、呼号 2、按钮定宽。
           设置的入口不在这一行——旧版只有一个设置对话框。 -->
      <section v-if="!compact" class="flex items-center gap-2">
        <input
          v-model="freqInput"
          :placeholder="t('freq.hint')"
          class="min-w-[120px] max-w-[200px] flex-1 rounded border px-2 py-1"
          @keyup.enter="addFrequency"
        />
        <input
          v-model="callsignInput"
          :placeholder="t('freq.callsign_hint')"
          class="min-w-[160px] max-w-[340px] flex-[2] rounded border px-2 py-1 font-mono uppercase"
          @keyup.enter="addFrequency"
        />
        <button
          class="shrink-0 rounded border px-3 py-1 font-semibold text-white"
          :style="{ background: 'var(--can-theme)' }"
          @click="addFrequency"
        >
          {{ t("freq.add") }}
        </button>
        <span class="flex-1" />
      </section>
```

- [ ] **Step 4: Move the settings panel into the dialog**

In `App.vue`: delete the `<SettingsPanel v-if="showSettings && !compact" :cid="cid" />` line at `:452`, delete the `showSettings` ref, and delete the `SettingsPanel` import.

Pass the panel through the dialog's slot instead — replace `<SettingsDialog :open="showPrefs" @close="showPrefs = false" />` at `:527` with:

```vue
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <SettingsPanel :cid="cid" />
      </SettingsDialog>
```

and keep the `SettingsPanel` import after all — it moves from the page into the dialog slot rather than being deleted.

In `apps/controller/src/components/SettingsPanel.vue`, drop the outer frame now that it sits inside a dialog: change the root `<section class="flex flex-col gap-4 rounded border p-3 text-xs">` to `<section class="flex flex-col gap-4 text-xs">`. Nothing else in that file changes; its 261 lines are not transcribed anywhere.

The `panel.open` / `panel.close` dictionary keys become unused. Leave them — `an_unused_key` is not a failure, and plan 4 sweeps the dictionaries once every app has landed.

- [ ] **Step 5: Route validation errors to the toast**

Add the import and mount it inside the connected `<main>`, as the last child before `</main>`:

```vue
      <Toast :message="error?.kind === 'frequency' ? problemText(error) : null" />
```

The frequency error no longer needs the inline bar. In the error bar at `:372-374`, narrow it to command errors:

```vue
      <p
        v-if="error?.kind === 'command'"
        class="rounded border border-red-400 px-3 py-2 text-xs text-red-600"
      >
        {{ problemText(error) }}
      </p>
```

- [ ] **Step 6: Build**

Run: `cd apps/controller && bun run build`
Expected: exit 0

- [ ] **Step 7: Manual check**

`bun run dev`, connect. Expected: the header reads lamp → link state → duty, then Pin/Compact/Settings/Disconnect pushed right. The add row has no settings button. Click 设置: the dialog shows Appearance, Endpoints, Troubleshooting **and** Audio, PTT, the log block, all in one scrollable card, with only one close button. Type `999` and press Add: a bordered strip appears top-right and disappears on its own after four seconds, and no inline bar appears.

- [ ] **Step 8: Commit**

```bash
git add apps/controller/src/components/Toast.vue apps/controller/src/App.vue apps/controller/src/components/SettingsPanel.vue
git commit -m "$(cat <<'MSG'
feat(controller): can-audio's top bar, add bar and single settings surface

The inline settings panel moves into the dialog through a slot, so the
app has one settings surface as can-audio does, without moving the
panel's contents. Frequency validation becomes a 4s toast.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 7: The online strip becomes one row of pills

**Files:**
- Modify: `apps/controller/src/components/OnlineList.vue`
- Modify: `apps/controller/src/App.vue:483-486`

**Interfaces:**
- Consumes: nothing new.
- Produces: `OnlineList.vue` keeps its existing props `{ online: Position[]; tuned: number[] }` and its `add: [freqKhz: number, callsign: string]` emit. Only the rendering changes.

- [ ] **Step 1: Make the buttons pills, callsign only**

Replace the template of `OnlineList.vue:33-53` with:

```vue
<template>
  <div class="flex min-w-0 flex-wrap items-center gap-1">
    <!-- can-audio 的 PillPushButton（controller/gui.py:889-934）：只显示呼号，
         高 24px，频率进 tooltip。已经加过的画灰而不是藏起来——藏起来的话，
         "没人在线"和"都已经加过了"在界面上长得一模一样。 -->
    <button
      v-for="p in rows"
      :key="`${p.callsign}-${p.freq_khz}`"
      class="h-6 shrink-0 rounded-full border px-3 font-mono text-xs disabled:opacity-40"
      :disabled="p.already"
      :title="
        p.already
          ? t('online.tuned', { frequency: mhz(p.freq_khz) })
          : t('online.add', { frequency: mhz(p.freq_khz) })
      "
      @click="$emit('add', p.freq_khz, p.callsign)"
    >
      {{ p.callsign }}
    </button>
    <p v-if="!rows.length" class="text-xs opacity-50">{{ t("online.none") }}</p>
  </div>
</template>
```

- [ ] **Step 2: Put the caption and the pills on one line**

Replace `App.vue:483-486` with:

```vue
        <!-- can-audio 把标题和药丸放在同一行（controller/gui.py:636-647）。 -->
        <div v-if="connected && !compact" class="mt-2 flex items-center gap-2 border-t pt-2">
          <p class="shrink-0 text-xs opacity-60">{{ t("online.title") }}</p>
          <OnlineList :online="feed?.online ?? []" :tuned="tuned" @add="addOnline" />
        </div>
```

- [ ] **Step 3: Build**

Run: `cd apps/controller && bun run build`
Expected: exit 0

- [ ] **Step 4: Manual check**

`bun run dev`, connect while at least one controller is online on the datafeed. Expected: 在线席位 and the pills share one line; each pill shows only a callsign; hovering one shows the frequency; adding that frequency greys the pill instead of removing it.

- [ ] **Step 5: Commit**

```bash
git add apps/controller/src/components/OnlineList.vue apps/controller/src/App.vue
git commit -m "$(cat <<'MSG'
feat(controller): one-line online strip with can-audio's pills

Callsign only on the button, frequency in the tooltip, caption on the
same row.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

### Task 8: Compact mode, window geometry, and the manual-test doc

**Files:**
- Modify: `apps/{controller,atis,xpc,msfs}/src/components/WindowToggles.vue`
- Modify: `apps/{controller,atis,xpc,msfs}/src/locales/common.zh.json`, `common.en.json`
- Modify: `apps/controller/src/App.vue` (compact padding)
- Modify: `apps/controller/src-tauri/src/lib.rs:800,803`
- Modify: `apps/controller/src-tauri/tauri.conf.json`
- Modify: `docs/manual-test.md`

**Interfaces:**
- Consumes: everything above.
- Produces: nothing new for later plans beyond the two `window.*` short-label keys, which plans 2–4 inherit through the shared `common.*.json`.

- [ ] **Step 1: Add the two short labels to the shared dictionaries and watch the identity test fail**

Add to `apps/controller/src/locales/common.zh.json`, inside the existing `window` namespace:

```json
    "on_top_short": "顶",
    "compact_short": "简",
```

and to `common.en.json`:

```json
    "on_top_short": "Top",
    "compact_short": "Min",
```

Run: `cargo test -p can-voice-i18n the_common_dictionaries_are_identical_in_every_app`
Expected: FAIL — `apps/atis/src/locales/common.zh.json differs from apps/controller's`

can-audio goes icon-only here (`FluentIcon.PIN`, `FluentIcon.MINIMIZE`). can-voice ships no icon set, and adding one is new dependency surface the spec rules out, so the compact labels are one character in Chinese and three in English.

- [ ] **Step 2: Copy the dictionaries to the other three apps**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
for a in atis xpc msfs; do
  cp apps/controller/src/locales/common.zh.json "apps/$a/src/locales/common.zh.json"
  cp apps/controller/src/locales/common.en.json "apps/$a/src/locales/common.en.json"
done
cargo test -p can-voice-i18n the_common_dictionaries_are_identical_in_every_app
```
Expected: PASS

- [ ] **Step 3: Make the toggles icon-sized in compact**

In `WindowToggles.vue`, replace the two buttons' class and label bindings so compact gives them can-audio's 30×26 footprint:

```vue
    <button
      class="rounded border"
      :class="[
        appearance.compact ? 'h-[26px] w-[30px]' : 'px-2 py-0.5',
        appearance.always_on_top ? 'border-sky-500' : 'opacity-60',
      ]"
      :aria-pressed="appearance.always_on_top"
      :title="t('window.on_top_tip')"
      @click="setAppearance({ always_on_top: !appearance.always_on_top })"
    >
      {{ appearance.compact ? t("window.on_top_short") : t("window.on_top") }}
    </button>
    <button
      class="rounded border"
      :class="[
        appearance.compact ? 'h-[26px] w-[30px]' : 'px-2 py-0.5',
        appearance.compact ? 'border-sky-500' : 'opacity-60',
      ]"
      :aria-pressed="appearance.compact"
      :title="t('window.compact_tip')"
      @click="setAppearance({ compact: !appearance.compact })"
    >
      {{ appearance.compact ? t("window.compact_short") : t("window.compact") }}
    </button>
```

The Settings button already hides itself in compact at `:37-38`, which is what can-audio does — leave it.

Copy the file to the other three and confirm the manifest test still passes:

```bash
for a in atis xpc msfs; do cp apps/controller/src/components/WindowToggles.vue "apps/$a/src/components/WindowToggles.vue"; done
cargo test -p can-voice-i18n
```

- [ ] **Step 4: Tighten compact padding**

In `App.vue`, the connected `<main>`'s class binding becomes:

```
:class="compact ? 'gap-1 p-1.5' : 'gap-4 p-5'"
```

can-audio uses margins `(6,4,6,4)` and spacing 4 in compact; `p-1.5` is 6px and `gap-1` is 4px.

- [ ] **Step 5: Move the Rust geometry constants**

In `apps/controller/src-tauri/src/lib.rs`, replace lines 800 and 803:

```rust
/// 精简模式下窗口最小能缩到多小。can-audio 的 `CARD_WIDTH + 2*6 + 4` × `CARD_HEIGHT + 70`
/// （`controller/gui.py:672-718`）：正好一列卡片，加上顶栏。
const COMPACT_MIN: (f64, f64) = (248.0, 186.0);
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
/// 旧版缩到的就是最小尺寸本身，这里跟着它。
const COMPACT_SIZE: (f64, f64) = (248.0, 186.0);
```

- [ ] **Step 6: Move the window minimum**

In `apps/controller/src-tauri/tauri.conf.json`, the single window entry becomes:

```json
      "title": "audio-for-can",
      "width": 720,
      "height": 560,
      "minWidth": 620,
      "minHeight": 480
```

can-audio's controller minimum is 620×480 (`controller/gui.py:413`). `apply_window` reads these back when leaving compact, so this is the only place the normal minimum is written.

- [ ] **Step 7: Run every gate**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
(cd apps/controller/src-tauri && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo check --all-targets)
for a in controller atis xpc msfs; do (cd "apps/$a" && bun run build) || break; done
```
Expected: all green. The other three apps are built because tasks 1, 2 and 8 edited their shared files.

- [ ] **Step 8: Manual check**

Build and run the real window: `cd apps/controller && bun run tauri dev`. Expected: the window will not be dragged narrower than 620×480. Click 精简: it collapses to a single column of cards about 248×186, the add row, online strip, bottom bar and Settings button are gone, and Pin and Compact are two small square buttons reading 顶 and 简. Click 简 again: the window returns to at least 620×480 with everything back.

- [ ] **Step 9: Update `docs/manual-test.md`**

Rewrite the controller section to match what now ships: the login page, the header order, the single settings dialog, the pill strip, the toast, and the compact figures above. Keep the file's existing structure and heading style; do not touch its atis, xpc or msfs sections — plans 2 to 4 own those.

- [ ] **Step 10: Commit**

```bash
git add apps/*/src/components/WindowToggles.vue apps/*/src/locales/common.zh.json apps/*/src/locales/common.en.json apps/controller/src/App.vue apps/controller/src-tauri/src/lib.rs apps/controller/src-tauri/tauri.conf.json docs/manual-test.md
git commit -m "$(cat <<'MSG'
feat(controller): can-audio's compact footprint and window minimum

Compact collapses to one column of cards at 248x186 and the normal
minimum becomes 620x480, both can-audio's figures. The window toggles
take short labels in compact, since can-voice ships no icon set.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Self-Review

**Spec coverage (§3, §4):**

| Spec requirement | Task |
|---|---|
| §3 palette as CSS custom properties | 1 |
| §3 `StateToggle.vue` shared | 3 |
| §3 `Panel.vue` shared | — **deferred to plan 3**, where xpc first needs it. Nothing in the controller uses a titled card. |
| §3 `StatusBar.vue` shared | 4 |
| §3 app-local `LoginCard`, `Pill`, toast | 5 (card), 7 (pill rendering, inside `OnlineList` rather than a separate component — a 6-line button needs no file of its own), 6 (toast) |
| §3 widened drift assertion | 2 |
| §4 login as its own page | 5 |
| §4 top-bar order | 6 |
| §4 add-bar proportions | 6 |
| §4 `SettingsPanel` merged into the dialog | 6 |
| §4 card: fixed metrics, mono frequency, `▸ `, last-talk wording | 3 — and two of the four were already implemented; the task says so |
| §4 gain slider 0–100 | 3 |
| §4 one-row pill strip | 7 |
| §4 bottom bar | 4 |
| §4 notice / XC-denied / TX-budget bars stay put | none needed — they are untouched at `App.vue:377-406` |
| §4 validation errors as a toast | 6 |
| §4 compact hides, padding, icon-only toggles | 8 |
| §4 `COMPACT_MIN` / `COMPACT_SIZE` 248×186, minimum 620×480 | 8 |

**One spec correction this plan makes:** §4 says `SettingsPanel.vue`'s three blocks "merge into `SettingsDialog.vue`". Reading the two files shows that is unnecessary — `SettingsPanel.vue`'s template is already a self-contained `<section>` of three blocks, so rendering it through a slot in the dialog achieves the same single settings surface without moving 261 lines. Task 6 does that. The spec's §4 wording should be amended when this plan lands.

**One spec omission this plan fixes:** §3 lists `SettingsDialog.vue` among the files that stay byte-identical, but §4, §5 and §6 each give that dialog app-specific sections, so it cannot stay shared. Task 2 extracts `SettingsCommon.vue` for the shared half and makes the shell app-local. Plans 2–4 depend on this having happened.

**Placeholder scan:** no TBD, no "add error handling", no "similar to task N". Every code step carries its code. The one step that reads as description rather than code is task 2 step 3, which is an extraction defined by exact source line ranges in the file being extracted from.

**Type consistency:** `StateToggle` is `{ label, state, width, height, disabled? }` + `press` in tasks 1 and 3 and nowhere else. `StatusBar` is `{ talking, status, duty, dutyOn }` + `down`/`up` + slot, consistent between task 4's creation and its mount. `LoginCard` is `{ cid, status, failed, busy, version }` + `connect: [cid, password]`; task 5's `connect(enteredCid, enteredPassword)` matches that emit's two positional arguments. `SettingsCommon` takes `{ open }` in task 2 and is mounted with `:open="open"` in the same task. `OnlineList`'s props and emit are unchanged by task 7, and `App.vue`'s `addOnline(khz, callsign)` still matches.

**Known weakness:** the `state()` helper in task 3 and the gain factor of 50 are untested, because the repo has no frontend test runner. Both are visible in the task 3 manual check. If you want them covered, adding vitest to the four apps is a separate decision — the spec currently rules out new build machinery.

# xpc 换上 can-audio 的窗口布局 — 实施计划（4 之 3）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `apps/xpc` 的单列页面改成 can-audio 的四张带标题卡片加原生菜单栏，`PilotPanel.vue` 解体成一个飞行计划对话框和设置对话框里的三页。

**Architecture:** 前端排布、窗口几何、**加一条原生菜单**。菜单是 Rust 侧的东西，所以这一份比前两份多一个 Rust 任务；但**不引入事件 API，也不放宽 ACL**——菜单要传给前端的三件事走已有的 250ms 轮询，见下面的 Global Constraints。

**Tech Stack:** Vue 3.5 SFC + Vite 7 + TypeScript + Tailwind CSS v4（CSS 配置，没有配置文件）；Tauri v2（`tauri::menu`，桌面端自带，不用加 feature）；`crates/can-voice-i18n` 的字典测试是唯一的自动化护栏。

**Spec:** `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md`，本计划落地 §6 的 xpc 那一半（§3 的共用件、§7 的 i18n 规矩同样适用）。msfs 是计划 4，**写在这一份落地之后**——spec 把它定义成"xpc 的差量"，提前写等于猜。

**Prior plans:** 计划 1（管制端）和计划 2（通播端）已经合进 `main`（PR #112）。调色板、`StateToggle.vue`、`StatusBar.vue`、`WindowToggles.vue` 的 `only` 属性、`SHARED_FRONTEND` 清单都在树上了。**读文件，不要凭计划 1、2 的文字回忆它们长什么样。**

## Global Constraints

每个任务的要求都隐含这一节。

- **不碰事件 API，不放宽 ACL。** `apps/xpc/src-tauri/capabilities/default.json` 只给了 `["updater:default"]`，而全仓库**没有一处**用 `@tauri-apps/api/event`。菜单点一下要让前端开个对话框，走的是**已有的 250ms 轮询**：Rust 存一个待办，`view()` 取走并清掉，前端下一拍看见。计划 1 有过一次教训——`getVersion()` 是插件命令，ACL 没给，它在 `onMounted` 里静默 reject，把后面的 `setInterval` 和 PTT 绑定全带走了，界面连上之后画一帧就不动了。而且仓库自己写明了为什么轮询：`update_state` 的注释说 `.setup()` 跑的时候 webview 还没加载完自己的包，监听器一个都不存在，**tauri 不会为还没起来的页面补发事件**。
- **交给操作系统的路径不能来自前端。** `open_in_browser` 只放行 https，注释点名理由是"把外部数据交给操作系统"。所以"打开日志目录"那个命令**不收参数**，路径自己从 `can_voice_log::path()` 算。收一个 `String` 再 spawn，等于把任意路径执行权交给网页那一侧。
- **逐字节复制，不抽包。** 共用前端文件在各端一份逐字节相同的副本，清单在 `crates/can-voice-i18n/tests/dictionaries.rs` 的 `SHARED_FRONTEND`。改一份要改所有拥有它的端，否则 `shared_frontend_files_are_identical_in_every_app_that_carries_them` 红。
- **界面代码里不许出现硬编码中文。** `the_interface_code_has_no_hardcoded_chinese` 扫 `apps/*/src/**`。每一句人看得见的话都走 `t("…")`，键名只能是字面量。
- **每个新键中英成对**，占位符一致，英文那份里不许有中文。
- **调色板只从 CSS 自定义属性取**：`--can-off / --can-on / --can-active / --can-muted / --can-theme / --can-idle / --can-surface / --can-window`，写法 `var(--can-on)`。不写十六进制，也不新加 Tailwind 颜色类。
- **版本号不动**，四个端都在 `27.0.4`。
- **提交签名不能绕过。** `commit.gpgsign=true` 且是硬件密钥，每次提交都要等实体按键，可能等很久。不要 `--no-gpg-sign`，不要改 git 配置，不要杀掉卡住的提交重来。
- **不要 `git add -A` 或 `git add .`**，只 `git add` 点名的路径。
- `Co-Authored-By:` 那一行是**仓库的固定约定**，逐字照抄，不是"哪个模型干的活"的描述。
- 临时文件放 `<项目>/.temp/`（已存在，已 gitignore），不许用 `/tmp` 或 `$TMPDIR`。别动不是自己建的文件，自己建的做完删掉。
- **每个任务的门**：`cd apps/xpc && bun run build`（= `vue-tsc --noEmit && vite build`）、`cargo test -p can-voice-i18n`、`cargo fmt --all --check`。动过共用文件的任务，四个端都要 `bun run build`。动过 `src-tauri` 的另加 `cd apps/xpc/src-tauri && cargo check --all-targets` 和 `cargo clippy --workspace --all-targets -- -D warnings`。
- **没有前端测试框架**，也没有 `lint` 脚本。`vue-tsc` 和 Rust 字典测试就是全部的自动化覆盖，所以每个任务的验证步骤里要写清楚**人工要看什么**。

---
## 菜单的两条线，先讲清楚

这一份和前两份不一样的地方全在这里。原生菜单是操作系统画的，而**词在 webview
里**——`crates/can-voice-i18n/src/lib.rs` 开头写明了为什么：Rust 侧拼好一句中文交出去，
切到英文之后那一句还是中文。所以菜单有两条线，方向相反：

**标签从前端来。** Rust 不持有任何一句菜单文案。前端在挂载时、以及每次 `language`
变化时调一次 `set_menu(labels)`，Rust 拿这批字符串**重建**整条菜单。菜单跟着语言当场
变，字典仍然只有一份，在 webview 里。菜单在最初几帧是没有的——这和仓库里别处一样，是
轮询式启动的正常样子，不是 bug。

**点击往前端去，走轮询。** `on_menu_event` 只把"点了哪一项"存进 `App`，前端下一拍
（250ms）调 `view` 时**取走并清掉**。不用 `emit`/`listen`：ACL 没给事件权限，而且
`update_state` 的注释已经写明 tauri 不会为还没起来的页面补发事件。

两项不往前端去，在 Rust 里就地做完：**退出**（关窗口）和**打开日志目录**。后者的路径
自己从 `can_voice_log::path()` 算，**命令不收参数**——`open_in_browser` 只放行 https，
注释点名的理由是不把外部数据交给操作系统，收一个 `String` 再 spawn 等于把这条理由作废。

## File Structure

| 文件 | 动作 | 说明 |
|---|---|---|
| `apps/xpc/src/components/StateToggle.vue` | 新增（复制 controller 的） | TX / RX 色块。登记进 `SHARED_FRONTEND`，`["controller", "xpc"]` |
| `apps/xpc/src/components/StatusBar.vue` | 新增（复制 controller 的） | 底栏。xpc 只传 `talking` + `status`，不传 `duty`/`pttTitle` |
| `apps/xpc/src/components/Modal.vue` | 新增（复制 atis 的） | 通用模态外壳。登记进 `SHARED_FRONTEND`，`["atis", "xpc"]` |
| `apps/xpc/src/components/Panel.vue` | 新增 | 带标题的卡片。can-audio 的 `Card(HeaderCardWidget)`。xpc-only，先不登记 |
| `apps/xpc/src/components/FlightPlanDialog.vue` | 新增 | `PilotPanel` 的飞行计划页，装进 `Modal`，从菜单进 |
| `apps/xpc/src/components/PilotSettings.vue` | 新增 | `PilotPanel` 的本机页，拆成 音频 / 网络 / 他机 三页，装进 `SettingsDialog` 的 `<slot />` |
| `apps/xpc/src/components/PilotPanel.vue` | **删除** | 两页都搬走之后它是空壳 |
| `apps/xpc/src/App.vue` | 大改 | 四张 `Panel`、无线电那一行、`StatusBar`、菜单两条线 |
| `apps/xpc/src/locales/app.zh.json` / `app.en.json` | 改 | 新增 `menu` 命名空间；`radio`、`panel`、`about` 几段 |
| `apps/xpc/src-tauri/src/lib.rs` | 改 | `MenuRequest`、`set_menu`、`open_log_dir`、`View.menu`、`on_menu_event` |
| `apps/xpc/src-tauri/tauri.conf.json` | 改 | 980×660，最小 900×600 |
| `apps/controller/src/components/StateToggle.vue` / `StatusBar.vue` | 改注释 | 末尾那句"用到它的客户端"要跟着改，两份仍逐字节相同 |
| `apps/atis/src/components/Modal.vue` | 改注释 | 开头那句"只有通播端有这个文件"不再成立 |
| `crates/can-voice-i18n/tests/dictionaries.rs` | 改 | `SHARED_FRONTEND` 三行 |
| `docs/manual-test.md` | 改 | xpc 那一节按新布局重写，菜单和几何要能人工核 |

**`SettingsDialog.vue` 的 `w-[30rem]` 不动。** 480px，和 can-audio 的
`setMinimumWidth(500)` 是一回事；三页分页塞得进去。改它会连带动 controller 和 atis 两份，
而那不是这一份计划要做的事。

---
### Task 1: 把三个共用件搬进 xpc，加一个 `Panel`

**Files:**
- Modify: `apps/controller/src/components/StateToggle.vue`（只改文档注释末尾一行）
- Modify: `apps/controller/src/components/StatusBar.vue`（只改文档注释末尾一行）
- Modify: `apps/atis/src/components/Modal.vue`（只改文档注释开头那一段）
- Create: `apps/xpc/src/components/StateToggle.vue`（`cp` 来的）
- Create: `apps/xpc/src/components/StatusBar.vue`（`cp` 来的）
- Create: `apps/xpc/src/components/Modal.vue`（`cp` 来的）
- Create: `apps/xpc/src/components/Panel.vue`
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs`（`SHARED_FRONTEND` 三行）

**Interfaces:**
- Consumes: 无。
- Produces: 后面的任务用 `<StateToggle :label :state :width :height>`、
  `<StatusBar :talking :status>`、`<Modal :open :title :width @close>`、`<Panel :title>`。
  `Panel` 的插槽内容自己撑高，`Panel` 自己是 `flex flex-col`，内容区 `flex-1 min-h-0`。

**先改源件、再 `cp`，不要两边手打。** 三个文件都要在两个（或更多）客户端之间逐字节相同，
而手打一段中文注释会把全角标点敲成半角——这在计划 2 的执行里真发生过，而且仓库里
**没有任何一条测试看标点**。`cp` 是这一条的唯一保证。

- [ ] **Step 1: 改 controller 两个共用件末尾那句"用到它的客户端"**

两个文件的文档注释里各有一行，现在写的是：

```
 * 用到它的客户端：controller。xpc / msfs 的 TX / RX 色块在后面的计划里接上。
```
```
 * 用到它的客户端：controller。xpc / msfs 的状态栏在后面的计划里接上。
```

分别改成：

```
 * 用到它的客户端：controller、xpc。msfs 的 TX / RX 色块在计划 4 里接上。
```
```
 * 用到它的客户端：controller、xpc。msfs 的状态栏在计划 4 里接上。
```

- [ ] **Step 2: 改 atis `Modal.vue` 开头那一段**

现在写的是：

```
 * 只有通播端有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 xpc 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
```

改成：

```
 * 用到它的客户端：atis、xpc。**登记在 `SHARED_FRONTEND` 上**——两份不登记的副本
 * 会无声地漂开。msfs 的飞行计划对话框在计划 4 里接上。
```

- [ ] **Step 3: 复制三个文件**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cp apps/controller/src/components/StateToggle.vue apps/xpc/src/components/StateToggle.vue
cp apps/controller/src/components/StatusBar.vue   apps/xpc/src/components/StatusBar.vue
cp apps/atis/src/components/Modal.vue             apps/xpc/src/components/Modal.vue
```

- [ ] **Step 4: 写 `apps/xpc/src/components/Panel.vue`**

```vue
<script setup lang="ts">
/**
 * 带标题的卡片。can-audio 的 `Card(HeaderCardWidget)`（`xpc/gui.py:111`）：
 * 一行标题，一条分隔线，下面是内容。
 *
 * **底色写 `bg-white` 而不是 `var(--can-surface)`。** `style.css` 里
 * `.dark .bg-white` 就映到 `--can-surface`，所以这一个类在两套主题下都对；
 * 直接写那个变量的话，浅色主题下卡片会是深的。
 *
 * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
 */
defineProps<{ title: string }>();
</script>

<template>
  <section class="flex min-w-0 flex-col rounded border bg-white">
    <h2 class="border-b px-3 py-2 text-xs font-semibold opacity-80">{{ title }}</h2>
    <div class="flex min-h-0 min-w-0 flex-1 flex-col gap-2 p-3">
      <slot />
    </div>
  </section>
</template>
```

- [ ] **Step 5: 登记进 `SHARED_FRONTEND`**

`crates/can-voice-i18n/tests/dictionaries.rs`。三处改动，表是按文件名排序的，保持有序：

```rust
    ("components/Modal.vue", &["atis", "xpc"]),
```
插在 `components/LogPanel.vue` 那一项之后、`components/SettingsCommon.vue` 之前。

```rust
    ("components/StateToggle.vue", &["controller", "xpc"]),
    ("components/StatusBar.vue", &["controller", "xpc"]),
```
替换现有那两行的 `&["controller"]`。

**`Panel.vue` 不登记。** 只有 xpc 有它，而"存在却没登记"那一条只对清单上**已有**的
路径生效（`owners.contains(app) || !path(app).exists()` 在 `for (file, owners)` 循环里）。

- [ ] **Step 6: 跑门**

```bash
cargo test -p can-voice-i18n
cargo fmt --all --check
for a in controller atis xpc msfs; do (cd apps/$a && bun run build) || echo "$a FAILED"; done
```
四个端都要过：这一步动了 controller 和 atis 的文件。

- [ ] **Step 7: 提交**

```bash
git add apps/controller/src/components/StateToggle.vue \
        apps/controller/src/components/StatusBar.vue \
        apps/atis/src/components/Modal.vue \
        apps/xpc/src/components/StateToggle.vue \
        apps/xpc/src/components/StatusBar.vue \
        apps/xpc/src/components/Modal.vue \
        apps/xpc/src/components/Panel.vue \
        crates/can-voice-i18n/tests/dictionaries.rs
git commit -m "$(cat <<'MSG'
feat(xpc): adopt the shared chrome and add a titled panel

StateToggle, StatusBar and Modal gain an xpc copy and are registered in
SHARED_FRONTEND. Panel is xpc-only and stays unregistered.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：什么都还没接上，界面不变。这一任务只证明四个端仍然构建、
字典测试仍然绿。

---
### Task 2: 一次把新词条加完

**Files:**
- Modify: `apps/xpc/src/locales/app.zh.json`
- Modify: `apps/xpc/src/locales/app.en.json`

**Interfaces:**
- Consumes: 无。
- Produces: 后面每个任务的 `t("…")` 大部分都从这里取键。**默认不再加键**——
  分散着加，`Key` 类型的报错会在四个提交里各出现一次。

  **三处例外，都是有意的，各自在自己的任务里当作「修订任务 2」来做**，不要以为是
  别人漏了：任务 5 加枢轴三页的 `local.audio` / `local.network` / `local.traffic`
  （页名此前不存在，而它们只有到拆页那一刻才知道叫什么）、任务 7 把
  `controllers.hint` 的措辞对齐成单击、任务 9 往**共用**字典加 `update.current`
  （那是 `common.*.json`，四个端八个文件，和这里改的 app 字典不是一回事）。
  反过来，任务 5 和任务 8 各删一个因为这次改版没人用了的键
  （`local.tab`、`observer.frequency`），删之前都要 grep 到零命中。

**能沿用的一律沿用**（spec §7：「can-audio 已有的键名能用就沿用」，但 can-voice
自己已经有的同义键**优先于**新造一个 can-audio 名字，同一句话两个键比键名不一致更糟）：

| 要用的地方 | 用已有的键，不要新造 |
|---|---|
| IDENT 按钮 | `session.ident` |
| PTT 按钮和它的提示 | `chat.push_to_talk`、`chat.push_to_talk_tip` |
| 观察员手输频率的占位符和提示 | `observer.frequency_placeholder`、`observer.frequency_tip` |
| 无线电那一行的座舱读数 | `cockpit.transponder / altitude / groundspeed / heading / pressure_delta` |
| 三张列表卡片里的空态 | `lists.no_traffic / no_controllers / no_messages` |
| 发送行 | `chat.recipient`、`chat.message`、`chat.send` |
| 连接网格里的每一格 | `login.*`、`session.*` |
| 日志没写成文件（`open_log_dir` 的失败） | `log.no_file`，共用字典里已经有了 |
| 打不开目录 / 这个平台没有 opener | `error.update.no_opener`、`error.update.open_failed`，共用字典里已经有了 |

**三张列表卡片的标题用带计数的那两个已有键**：附近管制用 `lists.controllers`
（「在线席位（{count}）」），他机用 `lists.traffic`（「附近的飞机（{count}）」）。
所以**不加** `controllers.title` / `traffic.title`——计数是 can-voice 比 can-audio 多出来
的东西，这次移植的规矩是一个不丢，而 can-audio 的卡片标题本来就只是个名词。
消息那张没有计数可显示，用新加的 `messages.title`。

`controllers.hint` 是新加的：spec §6 点名「下面一行换行提示」，共用件
`ControllerList.vue` 里没有这句话，而它和 msfs 逐字节相同，**不许改它**——提示那一行
写在 `App.vue` 的卡片里。

**`radio.position` 没有对应键，也不需要。** can-audio 的 `position_label` 装的是
**本机的位置和姿态**（`xpc/gui.py:532-535`：经纬度、高度、地速、航向、应答机），
不是管制席位。spec §6 把它写成「席位」，那是对 `位置` 的误译。所以无线电那一行的这一格
**就是 can-voice 现成的座舱读数**，沿用 `cockpit.*` 即可——不用新造一个本机呼号读数。

- [ ] **Step 1: 往 `app.zh.json` 里加**

`status` 是已有的命名空间，只加一个键；其余六个是新的顶层命名空间。
新命名空间和 `common.*.json` 的 `common / language / settings / window / update /
notice / log / error` 都不重名（`an_app_dictionary_does_not_reuse_a_common_namespace`）。

```json
  "menu": {
    "file": "文件",
    "flight_plan": "飞行计划…",
    "settings": "设置…",
    "quit": "退出",
    "help": "帮助",
    "open_log": "打开日志目录",
    "update": "检查更新",
    "about": "关于"
  },
  "connect": { "title": "连接" },
  "messages": { "title": "消息" },
  "controllers": { "hint": "双击一个席位，把它填进收件人。" },
  "radio": {
    "title": "无线电",
    "com1": "COM1  {frequency}",
    "com1_none": "COM1  ---.---",
    "traffic": "他机 {count}"
  },
  "about": {
    "title": "关于",
    "body": "{name} {version}\n\nCerulean Aviation Network 的 X-Plane 飞行员客户端。\n语音走 Mumble，网络走 FSD，飞行数据从 X-Plane 的 UDP 取。\n\n日志：{log}",
    "no_log": "（未写入文件）"
  },
```

`status` 里加一行：

```json
    "ready": "就绪",
```

`problem` 里加一行——这一条是 **Rust 侧发出来的**（`set_menu` 失败），
所以它要通过 `every_key_rust_sends_to_the_interface_exists`：

```json
    "menu": "菜单建不起来：{detail}",
```

**助记符去掉了。** can-audio 写的是 `"文件(&F)"`，那是 Qt 的写法；Tauri 在 macOS 上
不吃 `&`，会把它原样画出来，菜单上就是「文件(&F)」。

- [ ] **Step 2: 往 `app.en.json` 里加同样的结构**

```json
  "menu": {
    "file": "File",
    "flight_plan": "Flight plan…",
    "settings": "Settings…",
    "quit": "Quit",
    "help": "Help",
    "open_log": "Open the log folder",
    "update": "Check for updates",
    "about": "About"
  },
  "connect": { "title": "Connection" },
  "messages": { "title": "Messages" },
  "controllers": { "hint": "Double-click a position to put it in the recipient box." },
  "radio": {
    "title": "Radio",
    "com1": "COM1  {frequency}",
    "com1_none": "COM1  ---.---",
    "traffic": "Traffic {count}"
  },
  "about": {
    "title": "About",
    "body": "{name} {version}\n\nThe X-Plane pilot client for the Cerulean Aviation Network.\nVoice over Mumble, network over FSD, flight data over X-Plane's UDP link.\n\nLog: {log}",
    "no_log": "(not written to a file)"
  },
```

`status` 里加一行：

```json
    "ready": "Ready",
```

`problem` 里加一行：

```json
    "menu": "Could not build the menu: {detail}",
```

- [ ] **Step 3: 跑门**

```bash
cargo test -p can-voice-i18n
(cd apps/xpc && bun run build)
```
`both_languages_have_the_same_keys_and_none_is_empty`、
`placeholders_agree_between_the_languages`、`english_has_no_chinese_in_it` 三条是这一步的
真正门。**没有"键加了却没人用"的测试**，所以先加后用是安全的。

- [ ] **Step 4: 提交**

```bash
git add apps/xpc/src/locales/app.zh.json apps/xpc/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(xpc): add the dictionary keys the new layout needs

menu, the five panel titles, the radio row and the about box.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：界面不变。

---
### Task 3: 原生菜单（Rust 侧）

**Files:**
- Modify: `crates/can-voice-update/src/lib.rs`（加一个 `open_folder`）
- Modify: `apps/xpc/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: 任务 2 的 `problem.menu`。
- Produces: 三个命令和一个字段，任务 4 以后的前端都靠它们——
  - `set_menu(labels: MenuLabels) -> Result<(), Message>`，`labels` 八个字段：
    `file`、`flightPlan`、`settings`、`quit`、`help`、`openLog`、`update`、`about`
    （serde 默认按 Rust 字段名收，而 tauri 的命令参数**是驼峰**——见下面 Step 4 的注）。
  - `open_log_dir() -> Result<(), Message>`，**不收参数**。
  - `app_version() -> &'static str`。
  - `View.menu: Option<MenuRequest>`，取值 `"flight_plan" | "settings" | "update" | "about"`，
    **每次 `view` 调用取走并清掉**。

**下面用到的 tauri API 都已经对着 `tauri-2.11.6` / `muda-0.19.3` 的源码核过了**，
照写即可，不用再查：`MenuBuilder::new(manager)`、`SubmenuBuilder::new(manager, text)`、
`.text(id, text)`、`.separator()`、`.items(&[&dyn IsMenuItem])`、`.build()`、
`AppHandle::set_menu(menu) -> Result<Option<Menu>>`、
`Builder::on_menu_event(|handle: &AppHandle, event: MenuEvent|)`，
以及 `MenuId: AsRef<str>`（所以 `event.id().as_ref()` 能和 `&str` 常量比）。
`tauri` 在 `Cargo.toml` 里是 `features = []`——**菜单不需要额外 feature**，它是按平台
（desktop）编进去的，不是按 feature。

- [ ] **Step 1: 在 `can-voice-update` 里加 `open_folder`**

紧跟在 `open_in_browser` 之后：

```rust
/// 用系统默认的文件管理器打开一个目录。
///
/// **不收字符串，只收 `&Path`。** `open_in_browser` 只放行 https，注释写明的
/// 理由是这一步在把外部数据交给操作系统；一个能从前端传任意路径进来的命令会把
/// 那条理由作废。调用方自己算路径，这个签名让"从网页那一侧传一个路径进来"
/// 写不出来。
pub fn open_folder(path: &std::path::Path) -> Result<(), Message> {
    let (program, args) =
        opener(std::env::consts::OS).ok_or_else(|| Message::new("error.update.no_opener"))?;
    std::process::Command::new(program)
        .args(args)
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|e| Message::new("error.update.open_failed").with("detail", e))
}
```

两个 key 共用字典里都已经有了，不用加。

- [ ] **Step 2: `App` 上加一个待办字段**

`apps/xpc/src-tauri/src/lib.rs` 的 `pub struct App`（:250）末尾加：

```rust
    /// 菜单点了哪一项，还没被前端取走。
    ///
    /// **不走事件。** 理由和 `update_state` 那条注释一样：`.setup()` 跑的时候
    /// webview 还没加载完自己的包，监听器一个都不存在，而 tauri 不会为还没起来的
    /// 页面补发事件。而且这四个端的 capabilities 只给了 `updater:default`，
    /// 事件那一套要另外放权限。
    menu: Mutex<Option<MenuRequest>>,
```

`App` 的构造处跟着加 `menu: Mutex::new(None)`。

- [ ] **Step 3: `MenuRequest` 和 `View.menu`**

`View` 结构体（:468）末尾加：

```rust
    /// 菜单上刚点的那一项。**读一次就没了**——见 `build_view`。
    pub menu: Option<MenuRequest>,
```

在 `View` 前面加：

```rust
/// 菜单上那几项里，要前端去做的那几项。退出和打开日志目录在 Rust 里就地做完，
/// 不从这里走。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuRequest {
    FlightPlan,
    Settings,
    Update,
    About,
}
```

`build_view`（:923）里填上，**取走并清掉**：

```rust
        menu: app.menu.lock().expect("menu").take(),
```

放在 `build_view` 里而不是 `view` 命令里，是为了它可测：`view` 是 `#[tauri::command]`，
参数是 `tauri::State`，测不动。

- [ ] **Step 4: `set_menu`**

```rust
// ——— 原生菜单 ———

/// 菜单上的字。**从前端来。**
///
/// 字典在 webview 里（`can_voice_i18n` 的模块注释写明了为什么：Rust 侧拼好一句
/// 中文交出去，切到英文之后那一句还是中文）。所以这里一句文案都不持有：前端挂载时
/// 和每次切语言时各调一次，整条菜单重建，菜单跟着语言当场变。
///
/// 代价是最初几帧没有菜单。和这个程序里别处一样，是轮询式启动的正常样子。
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MenuLabels {
    file: String,
    flight_plan: String,
    settings: String,
    quit: String,
    help: String,
    open_log: String,
    update: String,
    about: String,
}

/// 建（或重建）整条菜单。
#[tauri::command]
fn set_menu(handle: tauri::AppHandle, labels: MenuLabels) -> Result<(), Message> {
    use tauri::menu::{MenuBuilder, SubmenuBuilder};

    let fail = |e: tauri::Error| Message::new("problem.menu").with("detail", e);

    let file = SubmenuBuilder::new(&handle, labels.file)
        .text(MENU_FLIGHT_PLAN, labels.flight_plan)
        .text(MENU_SETTINGS, labels.settings)
        .separator()
        .text(MENU_QUIT, labels.quit)
        .build()
        .map_err(fail)?;
    let help = SubmenuBuilder::new(&handle, labels.help)
        .text(MENU_OPEN_LOG, labels.open_log)
        .text(MENU_UPDATE, labels.update)
        .text(MENU_ABOUT, labels.about)
        .build()
        .map_err(fail)?;
    let menu = MenuBuilder::new(&handle)
        .items(&[&file, &help])
        .build()
        .map_err(fail)?;
    handle.set_menu(menu).map(|_| ()).map_err(fail)
}
```

上面那几个 id 常量放在 `MenuRequest` 旁边：

```rust
const MENU_FLIGHT_PLAN: &str = "flight_plan";
const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";
const MENU_OPEN_LOG: &str = "open_log";
const MENU_UPDATE: &str = "update";
const MENU_ABOUT: &str = "about";
```

**`#[serde(rename_all = "camelCase")]` 不能漏。** tauri 把命令参数按驼峰交给 serde，
前端传的是 `flightPlan`；不写这一行，`flight_plan` 收不到，而 serde 的报错会是
"missing field"，看起来像前端漏传了一个字段。

- [ ] **Step 5: `on_menu_event`，在 `.setup()` 之前挂上**

`tauri::Builder` 链上（`.setup(` 那一行之前）加：

```rust
        .on_menu_event(|handle, event| {
            let request = match event.id().as_ref() {
                MENU_FLIGHT_PLAN => Some(MenuRequest::FlightPlan),
                MENU_SETTINGS => Some(MenuRequest::Settings),
                MENU_UPDATE => Some(MenuRequest::Update),
                MENU_ABOUT => Some(MenuRequest::About),
                // 这两项不用惊动前端。
                MENU_QUIT => {
                    if let Some(window) = handle.get_webview_window("main") {
                        let _ = window.close();
                    }
                    None
                }
                MENU_OPEN_LOG => {
                    // 失败只记日志：菜单事件没有回程，这里 `?` 不出去。
                    if let Err(e) = open_log_dir_inner() {
                        tracing::warn!(error = %e, "could not open the log folder");
                    }
                    None
                }
                _ => None,
            };
            if let Some(request) = request {
                *handle.state::<App>().menu.lock().expect("menu") = Some(request);
            }
        })
```

**「打开日志目录」在这里就地做完，同时还留一个命令**（下一步），因为菜单在最初
几帧还不存在，而设置里的日志那一段也该能打开目录。两条路径调同一个 `open_log_dir_inner`。

- [ ] **Step 6: `open_log_dir` 和 `app_version`**

放在 `log_file`（:1569）旁边：

```rust
/// 日志目录。**不收参数**：路径自己从 `can_voice_log::path()` 算。
///
/// 收一个 `String` 再 spawn，等于把任意路径的执行权交给网页那一侧，而
/// `can_voice_update::open_in_browser` 只放行 https 就是为了不做这件事。
fn open_log_dir_inner() -> Result<(), Message> {
    let file = can_voice_log::path().ok_or_else(|| Message::new("log.no_file"))?;
    let dir = file
        .parent()
        .ok_or_else(|| Message::new("log.no_file"))?
        .to_path_buf();
    can_voice_update::open_folder(&dir)
}

#[tauri::command]
fn open_log_dir() -> Result<(), Message> {
    open_log_dir_inner()
}

/// 当前版本号。关于框那一行用它。
///
/// **不用 `@tauri-apps/api/app` 的 getVersion**：那是插件命令，要 `core:app` 权限，
/// 而这四个端的 capabilities 只给了 `updater:default`。自定义命令不走 ACL，
/// 而且这里读的是和日志、User-Agent 同一个常量，版本号对得上。
#[tauri::command]
fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

- [ ] **Step 7: 登记三个命令**

`tauri::generate_handler![`（:1726）那张表里加 `set_menu,`、`open_log_dir,`、
`app_version,`。**别动 `capabilities/default.json`**——自定义命令不走 ACL。

- [ ] **Step 8: 写一条测试，钉住"取走就没了"**

`apps/xpc/src-tauri/src/lib.rs` 的 `#[cfg(test)] mod tests` 里加。仿照那个模块里
已有的构造方式建一个 `App`（照抄同模块里现成的 helper，不要新发明一个）：

```rust
    /// 菜单那一项是**取走就没了**。留着的话，前端每一拍都会重新开一次对话框——
    /// 250 ms 一次，关都关不掉。
    #[test]
    fn a_menu_request_is_taken_once() {
        let app = test_app();
        *app.menu.lock().expect("menu") = Some(MenuRequest::Settings);
        assert_eq!(build_view(&app).menu, Some(MenuRequest::Settings));
        assert_eq!(build_view(&app).menu, None);
    }
```

**如果那个模块里没有现成的建 `App` 的 helper**，就不要为这条测试造一个——
改成直接测那一格的语义：

```rust
    #[test]
    fn a_menu_request_is_taken_once() {
        let slot: Mutex<Option<MenuRequest>> = Mutex::new(Some(MenuRequest::Settings));
        assert_eq!(slot.lock().expect("menu").take(), Some(MenuRequest::Settings));
        assert_eq!(slot.lock().expect("menu").take(), None);
    }
```
第二种弱一些，但它仍然钉住了会出事的那一条，而且不会为了一条测试把 `App` 的构造
撬开。**两种选一种，报告里说清楚选了哪一种、为什么。**

- [ ] **Step 9: 跑门**

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
(cd apps/xpc/src-tauri && cargo check --all-targets)
cargo test -p can-voice-i18n
```
`can-voice-update` 是共用 crate，所以这一步要跑整个 workspace，不只 xpc。

- [ ] **Step 10: 提交**

```bash
git add crates/can-voice-update/src/lib.rs apps/xpc/src-tauri/src/lib.rs
git commit -m "$(cat <<'MSG'
feat(xpc): add the native menu, fed by the frontend's dictionary

Labels come from the webview via set_menu and the menu is rebuilt on every
language change. Clicks reach the frontend through the existing 250ms view
poll, taken and cleared. Quit and open-the-log-folder are handled in Rust;
open_log_dir takes no argument so no path can arrive from the page.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：还没有前端去调 `set_menu`，所以窗口上**仍然没有菜单**。这一任务
只证明它编译、测试绿。菜单第一次出现是在任务 4。

---
### Task 4: 飞行计划搬进对话框，菜单接上前端

**Files:**
- Create: `apps/xpc/src/components/FlightPlanDialog.vue`（`PilotPanel` 的飞行计划页装进 `Modal`）
- Modify: `apps/xpc/src/components/PilotPanel.vue`（只剩本机那一页，页签行去掉）
- Modify: `apps/xpc/src/types.ts`（`MenuRequest` 类型，`View.menu` 字段）
- Modify: `apps/xpc/src/App.vue`（`showPlan`、`pushMenu`、`watch(language)`、`refresh` 里分发菜单）

**Interfaces:**
- Consumes: 任务 1 的 `<Modal :open :title :width @close>`；任务 2 的 `menu.file/flight_plan/settings/quit/help/open_log/update/about`、`problem.menu` 和已有的 `plan.*`；任务 3 的 `set_menu(labels)`（参数驼峰）、`View.menu`、`open_log_dir()`、`app_version()`。
- Produces:
  - `FlightPlanDialog.vue` — `defineProps<{ open: boolean; observer?: boolean }>()`、`defineEmits<{ close: [] }>()`。
  - `apps/xpc/src/types.ts` 里 `export type MenuRequest = "flight_plan" | "settings" | "update" | "about";`，`View` 多一个 `menu: MenuRequest | null;`。
  - App.vue 里 `showPlan`（ref）、`pushMenu()`、`refresh()` 里那一段分发。任务 5 往 `SettingsDialog` 的插槽里塞东西时，`showPrefs` 的开法不变。
  - **改过之后的 `PilotPanel.vue`**：script 1–188 行，模板 190–322 行，本机那一页的内容在 193–319 行。任务 5 就是按这几个行号抽的。

**四件会写错的事，先讲清楚。**

**一、机型预填自己读一次 `settings`。** `PilotPanel` 的 `onMounted` 读一次 `settings`，同一份既喂音频设备那一页、又顺手 `plan.value.aircraft = s.aircraft`。拆开之后 `FlightPlanDialog` **保留自己的那一小段 `onMounted`，只取 `s.aircraft`**，行为和今天逐字节一样。不要把 App.vue 登录框里那个 `aircraft`（`App.vue:26`）用 v-model 传进来：那是跟着用户打字变的活值，而这一格此前取的是**启动时存着的那一份**，两者在"改了登录框但没连接"的时候不同。启动时多调一次 `settings` 可以忽略不计（它是本地读一个结构体），换成活值则是行为变化，而这一份计划不改行为。

**二、`FlightPlanDialog` 自己一直挂着，`Modal` 的 `v-if` 在 `Modal` 的根节点上。** 所以 `plan` 和 `filed` 跨开关保留——今天切页签也是保留的，一样。真正每次重建的是插槽里的那些 `<input>`，它们从 `plan` 重新渲染，v-model 自己填回去。**这一点和任务 5 正好相反**，那边的 `PilotSettings` 整个挂在 `SettingsDialog` 的 `v-if="open"` 里面，每次开都是新挂一次。两处写法不同不是不一致，是两处的 `v-if` 位置不同。

**三、`Modal` 的盒子是 `text-sm`，而这一页此前长在 `PilotPanel` 的 `text-xs` 里。** 搬过去的时候外面那一层要写 `class="grid grid-cols-4 gap-2 text-xs"`，不然整张表单会比今天大一号。宽度用 `Modal` 的默认 `w-[44rem]`，不传 `width`。

**四、菜单的两条线在这一步才第一次连通。** 标签从前端去（`set_menu`），点击从 Rust 回来（`View.menu`，**取走就没了**）。`{ immediate: true }` 那一下就是"挂载时那一次"：不写它的话，不切语言就永远没有菜单。而 `view` 每调一次就把 `menu` 清掉，所以 `refresh` 这一拍读到非 null 必须当场处理，存起来等于永远不处理。

**「检查更新」和「关于」这一任务不接，故意留空。** 现成的 `UpdateBanner`（`apps/xpc/src/components/UpdateBanner.vue`）**自己持有**查到的那一版（它的 `latest` 是组件内的 ref），从 App.vue 再调一次 `check_update` 它看不见——那才是"看起来接上了其实没有"。关于框今天整个不存在。这两项由**任务 9（窗口几何、精简模式、最后两个菜单项、文档）**接上；如果后面的任务编号挪了，认的是那个接关于框的任务，不是编号本身。

- [ ] **Step 1: 从还没动过的 `PilotPanel.vue` 里抽出飞行计划那一页，拼成 `FlightPlanDialog.vue`**

**先抽再删**，顺序不能反：下一步的 `sed` 会把这些行删掉。下面这些行号对的是**现在树上的 `PilotPanel.vue`（399 行）**，先核一遍再跑：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
sed -n '22p;27p;83p;87p;218p;266p' apps/xpc/src/components/PilotPanel.vue
```

该打印出：

```
const plan = ref<FlightPlan>(emptyFlightPlan());
const filed = ref<"filed" | "offline" | null>(null);
async function file() {
}
      <label class="flex flex-col gap-1">
      </div>
```

对上了再跑组装。**中文注释一律从文件里抽，不手打**——手打会把全角标点敲成半角，而仓库里没有任何一条测试看标点：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
P=apps/xpc/src/components/PilotPanel.vue
{
  cat <<'HEAD'
<script setup lang="ts">
/**
 * 飞行计划对话框（spec §6）。`PilotPanel` 的飞行计划页搬进 `Modal`，
 * 从原生菜单「文件 → 飞行计划…」进，不再是主界面上的一个页签。
 *
 * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
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

HEAD
  sed -n '22,27p' "$P"
  cat <<'MID'

/**
 * 机型从设置里预填。**这里自己读一次 `settings`**，不把 App.vue 登录框里那个
 * `aircraft` 传进来：那一个是跟着用户打字变的活值，而这一格此前取的是启动时
 * 存着的那一份。启动时多调一次 `settings` 可以忽略不计，换成活值则是行为变化。
 */
onMounted(async () => {
  const s = await invoke<Settings>("settings");
  plan.value.aircraft = s.aircraft;
});

MID
  sed -n '83,87p' "$P"
  cat <<'BODY'
</script>

<template>
  <!-- 宽度用 `Modal` 的默认 44rem，四列表单在 980 宽的窗口里正好。外面补一个
       `text-xs`：`Modal` 的盒子是 `text-sm`，而这一页此前长在 `PilotPanel` 的
       `text-xs` 里，不补的话整张表单比今天大一号。 -->
  <Modal :open="props.open" :title="t('plan.tab')" @close="emit('close')">
    <div class="grid grid-cols-4 gap-2 text-xs">
BODY
  sed -n '218,266p' "$P"
  cat <<'TAIL'
    </div>
  </Modal>
</template>
TAIL
} > apps/xpc/src/components/FlightPlanDialog.vue
```

跑完看一眼首尾对不对：

```bash
head -20 apps/xpc/src/components/FlightPlanDialog.vue
tail -8 apps/xpc/src/components/FlightPlanDialog.vue
```

- [ ] **Step 2: `PilotPanel.vue` 只留本机那一页**

一条 `sed`，所有地址都是**原文件**的行号（sed 的地址永远按输入行算，所以删除和替换写在同一条里不会互相挪位）：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
sed -i '' \
  -e '208,268d' \
  -e '83,88d' \
  -e '73d' \
  -e '21,27d' \
  -e '13d' \
  -e '10d' \
  -e '9s/；$/。/' \
  -e '11s|.*|const props = defineProps<{ cid?: string; csl?: CslView }>();|' \
  -e '12s|.*|import type { CslView, Settings } from "../types";|' \
  -e '269s|<div v-else class=|<div class=|' \
  apps/xpc/src/components/PilotPanel.vue
```

一条一条是什么：

| 地址 | 删/改的是 |
|---|---|
| `208,268d` | 页签那一行（`<div class="flex gap-2">` 两个按钮）连着飞行计划那一页整块，以及两者之间和之后的空行 |
| `83,88d` | `file()` 和它后面那个空行 |
| `73d` | `plan.value.aircraft = s.aircraft;` |
| `21,27d` | `tab`、`plan`、`filed`（连 `filed` 那段文档注释） |
| `13d` | `import { emptyFlightPlan } from "../types";` |
| `10d` | 属性注释里 `observer` 那一行 |
| `9s` | 上一行的 `；` 收尾改成 `。`——两条的列表变成一条 |
| `11s` | 属性去掉 `observer?: boolean` |
| `12s` | 类型导入去掉 `FlightPlan` |
| `269s` | **本机那一页外层 div 的 `v-else` 必须去掉**。前面那个 `v-if` 已经删了，孤立的 `v-else` 是 Vue 编译错误（“v-else has no adjacent v-if”），`vite build` 当场红 |

**外层 `<section class="flex flex-col gap-3 rounded border p-3 text-xs">` 不动**，里面那个 `<div class="flex flex-col gap-3">` 也留着不拆。拆了要把 127 行重新缩进，而任务 5 正是按行号从这个文件里抽内容的——为了省一层 div 让那边的行号全部作废，不划算。这一层 div 在任务 5 里随文件一起消失。

跑完核一遍：

```bash
sed -n '188p;190,192p;320,322p' apps/xpc/src/components/PilotPanel.vue
wc -l apps/xpc/src/components/PilotPanel.vue
```

该看到 322 行，以及：

```
</script>

<template>
  <section class="flex flex-col gap-3 rounded border p-3 text-xs">
    <div class="flex flex-col gap-3">
    </div>
  </section>
</template>
```

- [ ] **Step 3: `types.ts` 加 `MenuRequest` 和 `View.menu`**

`apps/xpc/src/types.ts`，`export interface View {`（:46）**之前**插：

```ts
/**
 * 菜单上刚点的那一项。和 Rust 侧 `MenuRequest` 一一对应。
 *
 * **读一次就没了**：`view` 每调一次就把它取走并清掉。所以读到非 null 要当场
 * 处理，不要存进别的地方等以后——不会有以后。
 */
export type MenuRequest = "flight_plan" | "settings" | "update" | "about";
```

`View` 里 `observer: ObserverView | null;`（:61）之后加一行：

```ts
  /** 菜单上刚点的那一项，没点过是 `null`。见 `MenuRequest`。 */
  menu: MenuRequest | null;
```

- [ ] **Step 4: App.vue 的导入和 `showPlan`**

`apps/xpc/src/App.vue`。第 2 行的 vue 导入加 `watch`：

```ts
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
```

第 12 行 `import PilotPanel ...` 之后加一行：

```ts
import FlightPlanDialog from "./components/FlightPlanDialog.vue";
```

第 15 行的 i18n 导入加 `language`：

```ts
import { errorText, language, t } from "./i18n";
```

`const showPrefs = ref(false);`（:19）之后加：

```ts
/** 飞行计划对话框开没开。只有菜单「文件 → 飞行计划…」开它。 */
const showPlan = ref(false);
```

- [ ] **Step 5: `pushMenu` 和那条 watch**

放在 `pttUp()`（:78-81）之后、`refresh()`（:83）之前：

```ts
/**
 * 把菜单上的八个字交给 Rust，整条菜单重建。
 *
 * **字典在这一侧**，Rust 一句文案都不持有（`can_voice_i18n` 的模块注释写明了
 * 为什么：Rust 侧拼好一句中文交出去，切到英文之后那一句还是中文）。所以挂载时
 * 一次、之后每次 `language` 变一次，菜单跟着语言当场换。
 *
 * **参数名是驼峰**：`flightPlan` / `openLog`。tauri 把命令参数按驼峰交给 serde，
 * 写成下划线收不到，而报错长得像"前端漏传了一个字段"。
 */
async function pushMenu() {
  try {
    await invoke("set_menu", {
      labels: {
        file: t("menu.file"),
        flightPlan: t("menu.flight_plan"),
        settings: t("menu.settings"),
        quit: t("menu.quit"),
        help: t("menu.help"),
        openLog: t("menu.open_log"),
        update: t("menu.update"),
        about: t("menu.about"),
      },
    });
  } catch (e) {
    error.value = e;
  }
}

// `{ immediate: true }` 这一下就是"挂载时那一次"：setup 跑完紧接着就挂载，而
// `invoke` 不要求组件已经挂上。不写 immediate 的话，不切语言就永远没有菜单。
watch(language, () => void pushMenu(), { immediate: true });
```

失败走 `error`（:38），界面上那条红横幅（:232-234）用 `errorText` 翻 `problem.menu`——那个键是任务 2 加的，Rust 侧任务 3 发的。

- [ ] **Step 6: `refresh()` 里分发菜单**

`refresh()`（:83-86）整个换成：

```ts
async function refresh() {
  view.value = await invoke<View>("view");
  pressed.value = await invoke<boolean>("ptt_pressed");
  // 菜单那一项是**取走就没了**：上面这一次 `view` 已经把它从 Rust 侧清掉，
  // 这一拍不处理就没有下一拍。
  switch (view.value.menu) {
    case "flight_plan":
      showPlan.value = true;
      break;
    case "settings":
      showPrefs.value = true;
      break;
    case "update":
    case "about":
      // 任务 9（窗口几何、精简模式、最后两个菜单项、文档）接上。今天故意什么
      // 都不做：`UpdateBanner` 自己持有查到的那一版，从这里再调一次
      // `check_update` 它也看不见，那才是"看起来接上了其实没有"；关于框
      // 今天整个不存在。
      break;
    default:
      break;
  }
}
```

`refresh` 也从 `guard()` 的 `finally`（:120）里被调。那没问题：取走并清掉这件事谁先做都一样，只是点了菜单又恰好在跑一条命令时，对话框会早那么一拍弹出来。

- [ ] **Step 7: App.vue 模板挂上对话框，`PilotPanel` 去掉 `observer`**

第 374 行：

```html
      <PilotPanel v-if="!compact" :cid="cid" :csl="view?.csl" />
```

第 441 行那一行之前加一行（**不要包在 `v-if="!compact"` 里**：菜单在精简模式下照样在，对话框也该开得出来。今天飞行计划整页在精简模式下根本不存在，这是一处顺带的改善）：

```html
      <FlightPlanDialog :open="showPlan" :observer="observer" @close="showPlan = false" />
      <SettingsDialog :open="showPrefs" @close="showPrefs = false" />
```

- [ ] **Step 8: 跑门**

```bash
(cd apps/xpc && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

只动了 `apps/xpc/src`，没动共用文件，也没动 `src-tauri`，所以只 build xpc。`cargo test -p can-voice-i18n` 仍然要跑：它扫 `apps/*/src/**` 的硬编码中文，也查 `t("…")` 用到的键在不在字典里。

- [ ] **Step 9: 提交**

```bash
git add apps/xpc/src/components/FlightPlanDialog.vue \
        apps/xpc/src/components/PilotPanel.vue \
        apps/xpc/src/types.ts \
        apps/xpc/src/App.vue
git commit -m "$(cat <<'MSG'
feat(xpc): move the flight plan into a dialog opened from the menu

FlightPlanDialog carries the flight-plan page in a Modal and seeds the
aircraft type from settings on mount. App.vue pushes the eight menu labels on
startup and on every language change, and dispatches View.menu from the
existing 250ms poll. Quit and open-the-log-folder stay in Rust; check-for-
updates and about are wired in a later task. PilotPanel keeps only the local
setup page and loses its tab row.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：

1. 启动之后**菜单出现了**（macOS 在屏幕顶栏，Windows / Linux 在窗口里）。最初一两帧没有是正常的。
2. 「文件 → 飞行计划…」开出对话框，四列表单和今天那一页逐字一样，机型已经预填；遮罩、「关闭」、Esc 三条路都关得掉；关掉再开，刚才填的字还在。
3. 观察员模式下「拍发」是灰的，旁边写着 `plan.observer` 那句话。
4. 「文件 → 设置…」开出设置对话框（和右上角那个齿轮开的是同一个）。
5. 在设置里把语言切成 English，**菜单上的字当场变**，不用重开窗口。
6. 「帮助 → 打开日志目录」打开日志所在的目录；「帮助 → 检查更新」和「帮助 → 关于」**现在点了什么都不发生，这是这一任务的预期**（任务 9「几何、精简模式、关于框与检查更新、文档」接）。「文件 → 退出」关窗口。
7. 主界面上 `PilotPanel` 只剩本机那一页，**页签行没有了**，飞行计划不再占着主界面。
8. 精简模式下也能从菜单开飞行计划对话框。

---
### Task 5: 本机那一页拆成三页枢轴，`PilotPanel` 删掉

**Files:**
- Create: `apps/xpc/src/components/PilotSettings.vue`（音频 / 网络 / 他机三页，装进 `SettingsDialog` 的 `<slot />`）
- Delete: `apps/xpc/src/components/PilotPanel.vue`
- Modify: `apps/xpc/src/App.vue`（换导入，`SettingsDialog` 带插槽）
- Modify: `apps/xpc/src/locales/app.zh.json` / `app.en.json`（**任务 2 的修订**：三个页签键、删掉没人用的 `local.tab`、两条插件横幅的指路更正）

**Interfaces:**
- Consumes: 任务 4 改完之后的 `PilotPanel.vue`（script 1–188，本机那一页的内容在 193–319）；树上现成的 `SettingsDialog.vue`（`defineProps<{ open: boolean }>()`，`SettingsCommon` 之后一个 `<slot />`）。
- Produces: `PilotSettings.vue` — `defineProps<{ cid?: string; csl?: CslView }>()`，无 emits。**后面那个重排 App.vue 的任务要把 `<SettingsDialog>…<PilotSettings/></SettingsDialog>` 这一对原样带过去**，不要在重排时退回自闭合写法。
- Produces: 字典里 `local.audio`、`local.network`、`local.traffic` 三个新键；`local.tab` 在零引用时删掉。

**`SettingsDialog.vue` 一个字都不改。** 它在四个端逐字节相同，登记在 `SHARED_FRONTEND` 上，改一份要改四份，而那不是这一份计划要做的事。它的盒子是 `w-[30rem]`（480px，对应 can-audio 的 `setMinimumWidth(500)`），三页分页塞得进去，**不要加宽**。

#### 三页怎么分，每一条为什么

can-audio 的分法在 `can-audio/xpc/gui.py:1077-1110`：`_audio_tab` 是设备 / 音量 / 提示音 / PTT / 日志，`_network_tab` 是主机地址 / 端口 / 真实姓名 / 两个连接开关，`_traffic_tab` 是注入开关 / CSL 路径 / 范围 / 插件安装。can-voice 的控件按下面这样落位：

| 控件（任务 4 之后的 `PilotPanel.vue` 行号） | 落在哪一页 | 一句话理由 |
|---|---|---|
| 麦克风 / 耳机下拉 + `local.devices_note`（199–215） | 音频 | can-audio `_audio_tab` 的头两行就是它 |
| 试听喇叭 / 试听麦克风 + 错误行（216–224） | 音频 | 试的就是上面那两个设备，隔开就没法"选一个试一下" |
| 麦克风音量 / 喇叭音量（225–232） | 音频 | can-audio `_audio_tab` 的两条 slider |
| PTT 绑定列表 / 录制 / Wayland、鼠标提示（298–315） | 音频 | can-audio 把 `PttBindingList` 放在 `_audio_tab`，PTT 决定的是"麦克风什么时候开" |
| 寄日志 `LogPanel`（319） | 网络 | 见下面那一段 |
| 把他机注入模拟器（193–197） | 他机 | can-audio `_traffic_tab` 第一个控件就是 `render_traffic`，同一件事 |
| 提示音开关 / 每条都响 / 提示音量 / 试听 / 说明（234–260） | 他机 | **spec §6 点名**：「提示音开关和范围放到「他机」和他机设置一起」。can-audio 把它放在音频页，这一条是 spec 刻意的偏离，照 spec |
| 显示距离 `local.range`（262–275） | 他机 | 同上，spec 点名；而且它本来就是"画几架他机"的参数 |
| CSL 目录 + 重扫 + 扫描结果（277–296） | 他机 | can-audio `_traffic_tab` 的 `csl_path`。没有模型就画不出他机 |
| `<InstallWizard />`（317） | 他机 | can-audio `_traffic_tab` 末尾整段就是插件安装（`plugin.*`）。没有这个插件，天上一架别人的飞机都不会有 |

**「网络」那一页只装 `LogPanel`，这是 can-voice 和 can-audio 的真实差别，不是漏了。** can-audio 的网络页装的是 Mumble / FSD 地址端口、真实姓名和两个连接开关；can-voice 里**这些格子已经在同一个对话框里了**——它们是 `SettingsCommon.vue` 的「服务器地址」那一段（格子由 Rust 的 `endpoint_fields` 报），就摆在枢轴的正上方；真实姓名和连不连在 App.vue 的「连接」卡片里（spec §6 明确让它们留在那张卡片的网格里）。剩下唯一一件"这个客户端自己对外做的网络动作"就是把日志寄回 can-api（`LogPanel` 拿 CAN 号加网络密码打 `/api/v1/logs`），can-audio 把日志按钮放在音频页只是因为它和 debug 开关挨着，而 can-voice 的 debug 开关在 `SettingsCommon` 的「故障排查」里。所以日志落在网络页。

**「网络」这一页薄是对的，不要去"补满"它。** 后来的人看见一页只装一段，第一反应会是把服务器地址搬下来——**那会让同一组输入框在同一个对话框里出现两次**，一次在枢轴上方的 `SettingsCommon` 里、一次在枢轴的第二页上，而真正生效的是哪一份没人说得清。地址那几格由 Rust 的 `endpoint_fields` 报、默认值也从那里来，只能有一份；真实姓名按 spec §6 留在连接卡片的网格里。所以「寄日志」确实就是这一页剩下的全部内容，薄是 can-voice 和 can-audio 的真实差别，不是缺口。

#### 页签的字：两个新键，第三个沿用

**三个页签是三个新键，一个都没得沿用。** `local.tab` 是「本机」，`plan.tab` 是「飞行计划」，都不是页名；`lists.traffic`（附近的飞机（{count}））和 `lists.controllers`（在线席位（{count}））带 `{count}` 占位符，**当不了页签**——而它们正是任务 2 里那两张卡片标题用的键：带实时计数是 can-voice 的东西，这次移植要求保住，所以任务 2 没有另造 `traffic.title` / `controllers.title`。于是「音频」「网络」「他机」三个字一个现成的都没有，**这一任务要往任务 2 的字典里补三个键**，按它们所在的那一页命名，都放在 `local` 下：Step 1 就是它。

`local.tab`（本机 / Local setup）从此没人渲染。Step 2 先 grep 再删，**零引用才删**。

#### 生命周期：这一页每开一次就新挂一次

`PilotSettings` 挂在 `SettingsDialog` 的 `v-if="open"` **里面**，所以**每次打开对话框都是新挂一次，关掉就销毁**。今天的 `PilotPanel` 是 `v-if="!compact"`，开着程序就一直挂着。这是真实的行为变化，逐条落实：

- **`onMounted` 就是"打开的时候读一遍"，不要改成 watch。** 外层 `v-if` 已经决定了生命周期，再套一层只会多一条走不到的路。通播端的 `AiringPanel.vue:13-16` 写着同样的话，管制端的 `SettingsPanel.vue:82-87` 也是这个形状（它连那个 2000 ms 的 `deviceTimer` 都是这么挂的）。`SettingsCommon.vue` 那条 `{ immediate: true }` 是另一回事：它读的是自己的 `open` 属性，不是自己的挂载。
- **`deviceTimer`（2000 ms 扫设备）原样搬，不改 interval，不改清理。** 效果是它只在对话框开着的时候跑——而它存在的理由正是"设备下拉框开着的时候有人拔了耳机"，下拉框不在屏幕上时每两秒枚举一次音频设备是纯浪费。
- **每次打开重读一次 `settings` 是好事**，不是重复劳动：夹过的范围、Rust 侧改过的 CSL 目录会在重开时回到界面上。也不会有"写回旧值"的风险——每个控件都是改一下就 `invoke` 一次，界面里没有攒着等保存的状态。
- **`captureTimer`（150 ms 抓按键）原样搬，但 `onUnmounted` 要多一行。** 今天录制到一半没法关掉这个面板；从此可以（关对话框）。清掉那个 150 ms 轮询**并不会**让 Rust 侧退出录制模式，于是对话框关着的时候按下的键被留在 Rust 那里，下次一点「录制」立刻抓到它。所以 `onUnmounted` 里加一句 `if (capturing.value) void invoke("cancel_ptt_capture");`（`cancel_ptt_capture` 是现成的命令，`capture()` 的 10 秒超时分支已经在调它）。**这一行是这一任务唯一一处不是纯搬运的改动**，是刻意的：它修的是一个真实的缺陷，不是顺手美化。管制端的 `SettingsPanel.vue` 今天有一模一样的洞（同样挂在对话框的 `v-if` 里，同样只清定时器不取消录制），**这一份计划刻意不去动它**——那一处另开条目，不夹带在这次移植里。

顺带：本机设置从此在精简模式下也打得开（齿轮和菜单在精简模式下都在），今天 `PilotPanel` 是 `v-if="!compact"`，精简时整页不存在。

- [ ] **Step 1: 修订任务 2 的字典——三个页签键**

`apps/xpc/src/locales/app.zh.json`，`local` 命名空间里 `"tab": "本机",` 之后（`"tab"` 在 `plan` 里也有一个，认准值是「本机」的那一个）：

```json
    "tab": "本机",
    "audio": "音频",
    "network": "网络",
    "traffic": "他机",
```

`apps/xpc/src/locales/app.en.json`，同一处：

```json
    "tab": "Local setup",
    "audio": "Audio",
    "network": "Network",
    "traffic": "Traffic",
```

三个都放在 `local` 下、按页名命名：它们是同一个枢轴上的三兄弟，分散到三个命名空间去取会让下一个读代码的人以为它们各自还有别的用处。**不要去取 `lists.traffic`**——那一条是「附近的飞机（{count}）」，带占位符，也是卡片标题在用的那一个。

- [ ] **Step 2: `local.tab` 没人用了就删掉**

`PilotPanel` 一删，「本机」这个页签不再存在于任何界面上。先查再删，**零引用才删**：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -rn 'local\.tab' apps/xpc/src/
```

没有任何输出，就把 `"tab": "本机",` / `"tab": "Local setup",` 这两行从两份字典的 `local` 里删掉（Step 1 加的三行留着）。**有输出就不要删**，把还在用它的文件和行号记在完成报告里——说明这一次解体漏了一处，那一处要先处理。

`plan.tab` 不动：飞行计划对话框的标题还在用它。没删成的话，Step 10 提交正文里 `local.tab` 那半句也要去掉——正文说的必须是真做了的事。

- [ ] **Step 3: 修订两条插件横幅的指路**

`plugin.version_mismatch` 和 `plugin.not_heard` 都写着「到下面「本机 → X-Plane 他机插件」」。这一任务之后**下面没有「本机」这一页了**，插件向导在设置对话框的「他机」页上，所以这两句话要改，否则程序在教人去一个不存在的地方。只替换那一个片段，别重打整句（中文标点）：

**不要用 `perl -CSD -pi -e`。** `-CSD` 只改 I/O 的编码层，`-e` 里那段字面量在没有
`use utf8` 时仍按字节算，于是「文件已经解码成字符、模式还是字节」，非 ASCII 的替换
**一次都不匹配，而且悄悄地成功退出**。这一条是写这份计划时真踩到的：同样的写法改本
文件里两处交叉引用时报了 0 命中，换成 python 立刻 2 命中。用 python，并且**带断言**：

把下面这段存成 `.temp/fix_plugin_hint.py`（用编辑器写，不要用 shell 的 heredoc——
里面有反引号和中文引号，套在 heredoc 里会被 shell 吃掉），然后
`python3 .temp/fix_plugin_hint.py && rm .temp/fix_plugin_hint.py`：

```python
import pathlib

edits = [
    ("apps/xpc/src/locales/app.zh.json",
     "\u5230\u4e0b\u9762\u300c\u672c\u673a \u2192 X-Plane \u4ed6\u673a\u63d2\u4ef6\u300d",
     "\u6253\u5f00\u8bbe\u7f6e\uff0c\u5230\u300c\u4ed6\u673a\u300d\u9875\u4e0a\u7684"
     "\u300cX-Plane \u4ed6\u673a\u63d2\u4ef6\u300d"),
    ("apps/xpc/src/locales/app.en.json",
     "under \u201cLocal setup \u2192 X-Plane traffic plugin\u201d below",
     "under \u201cSettings \u2192 Traffic \u2192 X-Plane traffic plugin\u201d"),
]

for path, old, new in edits:
    f = pathlib.Path(path)
    t = f.read_text(encoding="utf-8")
    n = t.count(old)
    assert n == 2, f"{path}: expected 2 hits, got {n}"
    f.write_text(t.replace(old, new), encoding="utf-8")
    print(f"{path}: {n} replaced")
```

`assert n == 2` 是真正的门：两句话（`version_mismatch` 和 `not_heard`）里各有一处，
替换不到就当场炸，不会安静地跳过。**两处字面量都写成 `\uXXXX` 转义**，免得再摔在同一个
编码坑里——`\u300c` / `\u300d` 是 `「` `」`，`\u201c` / `\u201d` 是 `“` `”`，`\u2192` 是 `→`。

跑完核一遍，两个都该是 `0`：

```bash
python3 -c "import json;d=json.load(open('apps/xpc/src/locales/app.zh.json'))['plugin'];print(sum('\u672c\u673a \u2192 X-Plane' in v for v in d.values()))"
python3 -c "import json;d=json.load(open('apps/xpc/src/locales/app.en.json'))['plugin'];print(sum('Local setup \u2192 X-Plane' in v for v in d.values()))"
```**msfs 的同名字典不动**——它那一份现在仍然是对的，msfs 的布局在计划 4 里才改。

- [ ] **Step 4: 核对任务 4 之后的 `PilotPanel.vue` 行号**

下面组装用的行号全是**任务 4 改完之后**的。先核：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
sed -n '188p;192p;194p;200p;235p;298p;317p;319p;320p' apps/xpc/src/components/PilotPanel.vue
```

该打印出，一行不差：

```
</script>
    <div class="flex flex-col gap-3">
        <input v-model="inject" type="checkbox" @change="applyInject" />
        <span class="w-16 opacity-70">{{ t("local.microphone") }}</span>
        <input v-model="chime" type="checkbox" @change="applyChime" />
      <div class="flex flex-col gap-2">
      <InstallWizard />
      <LogPanel :cid="props.cid" />
    </div>
```

对不上就**停下来**：说明任务 4 的那条 `sed` 没有按原样跑，后面每一个范围都会切错地方。

- [ ] **Step 5: 组装 `PilotSettings.vue`**

script 整段原样搬（`1,188p`，里面那一堆中文注释一个字都不重打），模板按上面的表重新拼：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
P=apps/xpc/src/components/PilotPanel.vue
{
  sed -n '1,188p' "$P"
  cat <<'HEAD'

<template>
  <section class="flex flex-col gap-3 text-xs">
    <!-- 枢轴就是 `PilotPanel` 那个页签的写法：一个 ref、几个按钮、v-if/v-else。
         三页值不上一个分页组件。 -->
    <div class="flex gap-2">
      <button class="rounded border px-2 py-1" :class="page === 'audio' ? 'border-sky-500' : ''" @click="page = 'audio'">
        {{ t("local.audio") }}
      </button>
      <button class="rounded border px-2 py-1" :class="page === 'network' ? 'border-sky-500' : ''" @click="page = 'network'">
        {{ t("local.network") }}
      </button>
      <button class="rounded border px-2 py-1" :class="page === 'traffic' ? 'border-sky-500' : ''" @click="page = 'traffic'">
        {{ t("local.traffic") }}
      </button>
    </div>

    <div v-if="page === 'audio'" class="flex flex-col gap-3">
HEAD
  sed -n '199,232p' "$P"
  echo
  sed -n '298,315p' "$P"
  cat <<'NETWORK'
    </div>

    <div v-else-if="page === 'network'" class="flex flex-col gap-3">
NETWORK
  sed -n '319p' "$P"
  cat <<'TRAFFIC'
    </div>

    <div v-else class="flex flex-col gap-3">
TRAFFIC
  sed -n '193,197p' "$P"
  echo
  sed -n '234,296p' "$P"
  echo
  sed -n '317p' "$P"
  cat <<'TAIL'
    </div>
  </section>
</template>
TAIL
} > apps/xpc/src/components/PilotSettings.vue
```

缩进对得上：搬过来的那些行在 `PilotPanel` 里是 template → section → div 的六格缩进，在这里是 template → section → 分页 div 的六格缩进，一格不差，所以一行都不用重排。

根节点是 `<section class="flex flex-col gap-3 text-xs">`：**去掉了 `rounded border p-3`**——对话框自己已经是个盒子，再套一圈边框就是盒中盒；`text-xs` 留着，`SettingsDialog` 的盒子是 `text-sm`，不留的话这几页会比今天大一号（管制端的 `SettingsPanel.vue` 根节点也是这个形状）。

- [ ] **Step 6: `PilotSettings.vue` 的三处手改**

**（一）文件头的文档注释。** 把

```
import { t } from "../i18n";

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
```

改成

```
import { t } from "../i18n";

/**
 * 设置对话框里 xpc 自己那几段：音频 / 网络 / 他机三页（spec §6）。
 *
 * **它整个挂在 `SettingsDialog` 的 `v-if="open"` 里面**，每次打开都是新挂一次，
 * 关掉就销毁。所以 `onMounted` 就是「打开的时候读一遍」，`onUnmounted` 就是
 * 「关掉的时候停掉」——不要改成 watch，外层 `v-if` 已经决定了生命周期，再套一层
 * 只会多一条走不到的路（`SettingsCommon` 那条 `{ immediate: true }` 是因为它读的
 * 是自己的属性，不是自己的挂载）。管制端的 `SettingsPanel.vue` 是同一个形状。
 *
 * 「网络」那一页只有寄日志：服务器地址那几格在 `SettingsCommon` 里，就在这个枢轴
 * 的正上方；真实姓名和连不连在主界面的连接卡片上。
 *
 * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
 */

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
```

**（二）枢轴那个 ref。** 把

```
import type { CslView, Settings } from "../types";

interface BindingView {
```

改成

```
import type { CslView, Settings } from "../types";

/** 枢轴停在哪一页。默认音频，和 can-audio 的 `setCurrentItem("audio")` 一样。 */
const page = ref<"audio" | "network" | "traffic">("audio");

interface BindingView {
```

**（三）`onUnmounted` 多一行。** 把

```ts
onUnmounted(() => {
  window.clearInterval(captureTimer);
  window.clearInterval(deviceTimer);
});
```

改成

```ts
onUnmounted(() => {
  window.clearInterval(captureTimer);
  window.clearInterval(deviceTimer);
  // 关掉对话框就销毁这个组件，所以"录到一半"是关得掉的——而清掉那个 150 ms
  // 轮询并不会让 Rust 侧退出录制。不取消的话，对话框关着的时候按下的键会留在
  // 那里，下次一点「录制」立刻抓到它。
  if (capturing.value) void invoke("cancel_ptt_capture");
});
```

- [ ] **Step 7: App.vue 换掉 `PilotPanel`**

第 12 行

```ts
import PilotPanel from "./components/PilotPanel.vue";
```

换成

```ts
import PilotSettings from "./components/PilotSettings.vue";
```

第 374 行那一行（任务 4 之后是 `<PilotPanel v-if="!compact" :cid="cid" :csl="view?.csl" />`）**整行删掉**，连同它上一行的空行。

`<SettingsDialog :open="showPrefs" @close="showPrefs = false" />` 换成带插槽的写法：

```html
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <PilotSettings :cid="cid" :csl="view?.csl" />
      </SettingsDialog>
```

`csl` 还是从那份 250 ms 快照来，对话框开着的时候扫描进度照样在动。

- [ ] **Step 8: 删掉 `PilotPanel.vue`**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git rm apps/xpc/src/components/PilotPanel.vue
grep -rn "PilotPanel" apps/xpc/src || echo "xpc 里没有残留引用"
```

`apps/msfs/src/components/PilotPanel.vue` **不动**：那是另一个客户端自己的副本（381 行，和这一份本来就不同），计划 4 才轮到它。

- [ ] **Step 9: 跑门**

```bash
(cd apps/xpc && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

只动了 `apps/xpc`，共用文件一个没动（`SettingsDialog.vue`、`SettingsCommon.vue`、`LogPanel.vue` 原样），所以只 build xpc。字典测试这一步是真的门：它查 `local.audio` / `local.network` / `local.traffic` 中英成对、英文里没有汉字、`t("…")` 用到的键都在（删掉的 `local.tab` 要是还有人用，`Key` 那一侧 `vue-tsc` 就先红了）。

- [ ] **Step 10: 提交**

```bash
git add apps/xpc/src/components/PilotSettings.vue \
        apps/xpc/src/components/PilotPanel.vue \
        apps/xpc/src/App.vue \
        apps/xpc/src/locales/app.zh.json \
        apps/xpc/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(xpc): split local setup into a three-page pivot in the settings dialog

PilotSettings holds the audio, network and traffic pages inside
SettingsDialog's slot, so it is constructed on every open and destroyed on
close; the device poll and the PTT capture poll stop with it, and a capture
left running is cancelled. PilotPanel is deleted and local.tab goes with it.
The three page labels are new keys under local. The two plugin banners now
name the settings dialog instead of the local page they used to point at.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：

1. 齿轮或者「文件 → 设置…」开设置：最上面仍然是外观 / 服务器地址 / 故障排查三段（`SettingsCommon`，没动过），**下面多了三个页签：音频 / 网络 / 他机**，默认停在音频。对话框宽度没变（30rem），三页都不横向溢出。
2. **音频**页：麦克风、耳机、两个试听按钮、两条音量条、PTT 绑定列表和「录制一个绑定」。选设备、拉音量立刻生效（换耳机马上听得到）。
3. **网络**页：只有寄日志那一段（路径、CAN 号、密码、发送）。
4. **他机**页：注入开关、三条提示音设置加「试听」、显示距离、CSL 目录加「重扫」加扫描结果、X-Plane 插件向导。
5. **对话框开着**的时候拔掉 USB 耳机：两秒内它从下拉框里消失，选中项回落到「跟随系统默认」。
6. 关掉对话框再打开：页签回到「音频」，设备列表和各项设置都重新读了一遍（在别处改过的值会体现出来）。
7. 点「录制一个绑定」，**不按键直接关掉对话框**；再打开、再点「录制」，它应该还在等你按键，**不该立刻抓到一个你在对话框关着时按过的键**。
8. 主界面上原来 `PilotPanel` 那一整块没有了，页面短了一截；精简模式下现在也能从齿轮/菜单打开这三页。
9. 不装插件时的那条黄色横幅，现在写的是「打开设置，到「他机」页上的「X-Plane 他机插件」…」，照着做找得到。

---
### Task 6: 连接卡片，外加它上面那一叠横幅

**Files:**
- Modify: `apps/xpc/src/App.vue`（拆掉页头，把登录区和会话区并成一张 `Panel`，横幅留在卡片上方）

**Interfaces:**
- Consumes: `Panel.vue`（Task 1）——`<Panel :title="string">`，默认插槽；词条 `connect.title`（Task 2）。
  App.vue 里已有的这些一个都不改签名：`compact`、`online`、`observing`、`connected`、`busy`、
  `view`、`error`、`mouseSupported`、`connect()`、`disconnect()`、`toggleObserver()`、
  `cid` / `password` / `callsign` / `aircraft` / `realName` / `follow`。
  **`ident()`（`App.vue:170`）这一任务做完之后没有调用方**，因为 IDENT 归 Task 8。
  **函数不要删**，也不要加 `// eslint-disable` 之类的东西——`vue-tsc --noEmit` 不报没人用的
  顶层常量，Task 8 接上就有人用了。
  Task 4–5 产出的 `showPlan`、`pushMenu()`、`refresh()` 里那段 `view.menu` 处理**不碰**。
- Produces:
  - `<header>` 只剩 `WindowToggles`（`class="ml-auto"`），页头那一排 pill 不再存在。
    **Task 8 不要再往无线电那一行加模拟器灯或语音文案**——它们在连接卡片的三格状态文案里。
  - 卡片上方一叠横幅，从上到下：`UpdateBanner`、错误、语音通知、**插件那一行灯**、
    插件两条横幅、鼠标侧键提示。Task 7、8、9 往下面插卡片，不往这一叠里插东西。
  - 第一张卡片 `<Panel :title="t('connect.title')">`，**没有 `compact` 门**——Task 9 才加。
  - 连接卡片里**只有连接/断开一个按钮**（连着的时候翻成断开），外加观察员的跟随读数。
    **IDENT 不在这张卡片里**，归 Task 8 的无线电那一行。见下面「IDENT 只有一个」。

**这一任务不动的三块。** 观察员频率那一栏（`v-if="observer"` 的 `<section>`）、座舱读数那一栏
（`v-if="!compact"` 的六格 `<section>`）、底栏 `<footer>`——三块都原地不动，Task 8 才把它们
合进无线电那一行。这一任务只处理页头、横幅、登录区、会话区。

**IDENT 只有一个，而且不在这张卡片里。** spec §6 把 IDENT 写了两遍：连接卡片那一段说
「独立的会话区并进来，连接按钮翻成断开，Ident 在旁边」，无线电那一行又说
「…他机计数 → 席位 → IDENT → PTT 按钮」。**spec 自己重复的地方，由 can-audio 定**——
整份 spec 就是照着它写的，而 can-audio 只有一个 IDENT 按钮，在无线电栏里
（`can-audio/xpc/gui.py:540-544`，`self.ident_button` 建在 `_build_radio_bar` 里），
它的连接卡片一个都没有。**所以 IDENT 归 Task 8**，这一任务不渲染它，也不引用 `session.ident`。
连接卡片里那一格只有连接/断开那一个按钮。

留给 Task 8 的形状（can-audio 也是这个形状，`gui.py:380`、`717`）：IDENT **常驻**在无线电那一行，
用 `:disabled="!connected || busy"` 控制能不能点，**不要用 `v-if` 让它时有时无**——
那一行会随着上线下线横向跳动，而 PTT 按钮就在它旁边。

**`记住密码` 那一行不移植，因为 can-voice 根本没有这个设置。** can-audio 的
`xpc/gui.py:244-246、291-295` 有 `settings.remember_password`，而 can-voice 的
`Settings`（`apps/xpc/src-tauri/src/lib.rs:65-138`、`apps/xpc/src/types.ts:155-183`）里
没有任何一个密码字段，`set_endpoints` 也不收密码。更要紧的是 can-voice 是**反着设计**的：
`App.vue` 的 `connect()` 在成功之后立刻 `password.value = ""`，注释写明「密码用过就丢：
它只需要换一张短期票，之后重连带的是票不是密码」。移植这一行等于新增一个要落盘的凭据设置，
那是一件安全决策，不是一次排版。**所以这张卡片只有三行：网格、三格状态文案、观察员那一行。**

**页头那几个 pill 的去处，一个一个对。** `h1`（`app.title`）**删掉**——can-audio 的主窗口
没有标题，标题在窗口装饰上；`app.title` 不会变成没人用的键，Task 3 的「关于」对话框
拿它当 `{name}`。X-Plane 灯进状态文案的第一格。`observing` / `linkText` 那两句进第二格。
`voiceText` 进第三格。**三格不用加标签**：`linkText` 返回的字本来就以 `FSD` 开头，
`voiceText` 返回的以「语音」开头，第一格里 `X-Plane` 是拉丁字面量——
所以三格自带名字，一个新键都不要。

**插件那盏灯不进卡片，跟着它那两条横幅上去。** spec §6 的两句话各说一半，合起来是完整的：
「然后三行状态文案——模拟器、FSD、语音，也就是现在页头那几个 pill 的去处」只点了三个 pill；
「xpc 的插件 pill 和它那两条横幅（版本不符、没听到）留在卡片上方当横幅」把第四个排除在外。
**两盏灯说的不是同一件事**，这也是它们今天分成两个 pill 的原因（`App.vue` 原处那条注释写明了）：
X-Plane 那盏只代表 UDP 数据源通不通，而没装插件的人连得上、说得了话，天上却一架飞机都没有。
插件的灯、版本不符、没听到是同一件事的三个程度，摆在一起才读得懂。**不要把它们再并回去。**

**灯的「灭」用 `var(--can-idle)`，不用 `var(--can-muted)`。** 计划 1 的
`apps/controller/src/App.vue` 写的是 `connected ? 'var(--can-on)' : 'var(--can-muted)'`，
那是链路断了该报警。X-Plane 没开是飞行员启动客户端时的**正常状态**，红灯只会吓人；
can-audio 那三格未连接时也是 `theme.IDLE_COLOR`。文案本身继续用 `opacity-60/70`，
和这个文件其余部分、和计划 1 的 `App.vue` 一致。

**中文注释一律搬，不要手打。** 下面的代码块里凡是写着 `<!-- XXX-COMMENT -->` 的那一行，
都要替换成原文件里对应的注释行，**从 `.temp/xpc-header.txt` 或 App.vue 原处复制**。
手打会把全角标点敲成半角，而仓库里没有任何一条测试看标点。

- [ ] **Step 1: 先把页头整块抄进 `.temp/`，再动它**

```bash
mkdir -p .temp
sed -n '/<header class="flex flex-wrap/,/<\/header>/p' apps/xpc/src/App.vue > .temp/xpc-header.txt
grep -n -B2 'v-model="observer"' apps/xpc/src/App.vue
grep -n -B1 'v-model="follow"' apps/xpc/src/App.vue
cat .temp/xpc-header.txt
```

这三条打印出来的注释就是下一步要搬的全部中文。`.temp/xpc-header.txt` 在本任务结束时删掉。

**`session.ident` 上面那一行注释（「识别是 FSD 的事，观察员没有那条连接。」）跟着会话区
一起删掉**，因为 IDENT 归 Task 8。它讲的是 Task 8 要重建的那个按钮，所以 Task 8 需要它时
从这一任务的父提交里取回原文，不要手打：

```bash
git show HEAD:apps/xpc/src/App.vue | grep -n -B1 'session\.ident'
```

- [ ] **Step 2: 引入 `Panel`**

`apps/xpc/src/App.vue` 的 `<script setup>` 顶上，和别的组件 import 放一起：

```ts
import Panel from "./components/Panel.vue";
```

- [ ] **Step 3: 页头缩成一行**

把 `<header class="flex flex-wrap items-center gap-3">` 到 `</header>` 整块换成：

```html
      <header class="flex items-center gap-2">
        <!-- TOGGLES-COMMENT -->
        <WindowToggles class="ml-auto" @settings="showPrefs = true" />
      </header>
```

`TOGGLES-COMMENT` 是 `.temp/xpc-header.txt` 里 `<WindowToggles` 上面那一行
（「精简时也在……」），原样复制。这和计划 1 里 `apps/controller/src/App.vue` 登录页的
页头是同一个写法：只剩置顶/精简/设置，`ml-auto` 靠右。

**`UpdateBanner` 不动**，仍然紧跟在 `</header>` 之后、仍然是 `v-if="!compact"`——
计划 1 的 controller 也是这么摆的。

- [ ] **Step 4: 插件那盏灯和鼠标侧键那一句都变成横幅**

插件灯原来在页头里，现在挪到它那两条横幅**正上方**，三条连成一组。
`PLUGIN-COMMENT` 是 `.temp/xpc-header.txt` 里插件那个 `<span>` 上面那两行注释，原样复制：

```html
      <!-- PLUGIN-COMMENT -->
      <p class="flex items-center gap-1 text-xs opacity-70">
        <span
          class="h-2 w-2 shrink-0 rounded-full"
          :style="{ background: view?.plugin ? 'var(--can-on)' : 'var(--can-idle)' }"
        />
        {{ t("status.plugin") }}
        <span v-if="view?.plugin" class="opacity-60">{{ view.plugin.drawn }}</span>
      </p>
```

**它不加 `observer` 门**，和今天一样——那两条横幅才是 `!observer` 的。灯只是陈述插件在不在，
横幅才是「你该去装一下」，别把门搬到灯上。

鼠标侧键那一句（`!mouseSupported && !compact`）插在插件两条横幅**之后**，
也就是整叠横幅的最后一条：

```html
      <p v-if="!mouseSupported && !compact" class="rounded border px-3 py-2 text-xs opacity-60">
        {{ t("status.no_mouse_ptt") }}
      </p>
```

横幅这一叠到此定型，顺序是：`UpdateBanner` → 错误（`error !== null`）→ 语音通知（`v-for`）
→ 插件灯 → 插件版本不符 / 没听到（`v-if` / `v-else-if`）→ 鼠标侧键。
**原有的四块横幅内容原地不动**，只是现在它们下面跟的是卡片。
鼠标侧键这一条摆在这里是为了先有个去处；Task 8 要是觉得它属于无线电那一行，
那是 Task 8 的事，这一任务不替它决定。

- [ ] **Step 5: 登录区和会话区并成连接卡片**

把 `<section v-if="!online" class="grid grid-cols-5 gap-2">` 到它 `v-else` 那一段
（IDENT / 下线 / 跟随）的 `</section>` 为止**整块**换成下面这一张卡片。
四个 `-COMMENT` 标记按 Step 1 打印出来的原文替换（`PLUGIN-COMMENT` 在 Step 4 用掉了）。
**IDENT 那个按钮和它上面那行注释一起删掉，不要搬进来**——理由见上面「IDENT 只有一个」。

```html
      <Panel :title="t('connect.title')">
        <!-- can-audio 的网格顺序（xpc/gui.py:251-263）：呼号 · CID · 密码 · 机型 · 连接。
             follow 和姓名是 can-voice 多出来的两格，留在同一行。 -->
        <div class="grid grid-cols-7 gap-2">
          <!-- FOLLOW-COMMENT -->
          <input
            v-if="observer"
            v-model="follow"
            :disabled="online"
            :placeholder="t('login.follow')"
            :title="t('login.follow_tip')"
            class="rounded border px-2 py-1 font-mono text-xs uppercase"
          />
          <input
            v-else
            v-model="callsign"
            :disabled="online"
            :placeholder="t('login.callsign')"
            class="rounded border px-2 py-1 font-mono text-xs uppercase"
          />
          <input
            v-model="cid"
            :disabled="online"
            :placeholder="t('login.cid')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="password"
            type="password"
            :disabled="online"
            :placeholder="t('login.password')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="aircraft"
            :disabled="observer || online"
            :placeholder="t('login.aircraft')"
            class="rounded border px-2 py-1 text-xs"
          />
          <input
            v-model="realName"
            :disabled="observer || online"
            :placeholder="t('login.real_name')"
            class="rounded border px-2 py-1 text-xs"
          />
          <div class="col-span-2 flex flex-wrap items-center gap-2">
            <button
              v-if="!online"
              :disabled="busy"
              class="min-w-[110px] rounded border px-3 py-1 text-xs"
              @click="connect"
            >
              {{ t("login.connect") }}
            </button>
            <template v-else>
              <button
                :disabled="busy"
                class="min-w-[110px] rounded border px-3 py-1 text-xs"
                @click="disconnect"
              >
                {{ t("session.disconnect") }}
              </button>
              <span v-if="observing" class="text-xs opacity-70">
                {{ t("session.following") }}
                <span class="font-mono">{{ view?.observer?.follow }}</span>
              </span>
            </template>
          </div>
        </div>

        <!-- can-audio 把三格状态文案排在网格第二行（xpc/gui.py:265-270）：模拟器、FSD、语音。
             页头那三个 pill 就是搬到这里来的。三格自带名字：FSD 和语音那两句译文本来就以
             它们的名字开头，第一格里 `X-Plane` 是拉丁字面量。
             **插件那盏灯不在这三格里**：它跟着版本不符和没听到那两条横幅留在卡片上方，
             因为这一格说的是 UDP 数据源通不通，而插件说的是天上画不画得出他机——
             没装插件的人这一格是绿的。别把两盏灯并回一格。 -->
        <div class="grid grid-cols-3 gap-2 text-xs">
          <span class="flex items-center gap-1 opacity-70">
            <!-- SIM-COMMENT -->
            <span
              class="h-2 w-2 shrink-0 rounded-full"
              :style="{ background: view?.sim_connected ? 'var(--can-on)' : 'var(--can-idle)' }"
            />
            X-Plane
          </span>
          <span class="truncate opacity-70">
            {{ observing ? t("status.observing") : linkText(view?.link) }}
          </span>
          <!-- VOICE-COMMENT -->
          <span class="truncate opacity-70">{{ voiceText(view?.voice) }}</span>
        </div>

        <!-- 观察员开关排在状态文案下面，和 can-audio 一样（xpc/gui.py:277-287）：
             它决定这一次连接会不会在网络上多出一架飞机，是每次点「连接」之前该看一眼的事。 -->
        <div class="flex flex-wrap items-start gap-2 text-xs">
          <!-- OBSERVER-COMMENT -->
          <label class="flex shrink-0 items-center gap-2">
            <input v-model="observer" type="checkbox" :disabled="online" @change="toggleObserver" />
            {{ t("login.observer_mode") }}
          </label>
          <p class="min-w-0 flex-1 opacity-60">{{ t("login.observer_note") }}</p>
        </div>
      </Panel>
```

三处和原来不一样的地方，都是「并进来」带出来的，不是顺手改的：

1. **输入框不再随上线消失，改成 `:disabled="online"`。** 原来整块 `v-if="!online"`，
   上线之后呼号、CID、机型从屏幕上没了。卡片是常驻的，藏掉半张卡片会留下一个空洞；
   can-audio 也是禁用而不是隐藏。
2. **观察员勾选框加 `:disabled="online"`。** 原来靠 Rust 侧拒绝再把勾扳回去
   （`toggleObserver` 的 catch）。那条兜底留着，但连着的时候本来就不该能点。
3. **`session.following` 那一句留在按钮旁边。** 跟随的呼号在 `follow` 输入框里也有，
   但那一格是本地 `ref`，这一句读的是 `view?.observer?.follow`，也就是服务端认下来的那个。
   两者不一致时，要看得见的是后者。

- [ ] **Step 6: 查没手打坏中文标点，再删掉 `.temp/`**

```bash
perl -ne 'print "$.: $_" if /\p{Han}[,:;?!]/' apps/xpc/src/App.vue
rm .temp/xpc-header.txt
```

第一条命令**什么都不该打印**。打印出来的行就是把全角标点敲成半角的地方——
仓库里没有任何一条测试看这个，所以这一步是唯一的门。

- [ ] **Step 7: 跑门**

```bash
(cd apps/xpc && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

只动了 `apps/xpc/src/App.vue`，没动共用文件，也没动 `src-tauri`，所以别的端不用重建。

- [ ] **Step 8: 提交**

```bash
git add apps/xpc/src/App.vue
git commit -m "$(cat <<'MSG'
feat(xpc): fold the header and the session row into a connection panel

The login fields, the connect/disconnect button and the sim/FSD/voice captions
now live in one full-width titled panel. The header keeps only the window
toggles. The update, error, voice-notice and mouse-PTT lines stay above the
panel as banners, and so does the plugin indicator, next to the two plugin
banners it belongs with.

Ident is not rendered here. can-audio has one Ident button and it sits in the
radio bar, so it lands with the radio row instead; ident() is left in place
with no caller until then.

The remember-password row of can-audio is not ported: can-voice has no such
setting, and connect() drops the password as soon as it has a ticket.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：开起来是一张标题写着「连接」的整宽卡片，上面是置顶/精简/设置那一排，
再往下是插件那盏灯和它的横幅。卡片里第一行五个框加「上线」，第二行三格状态：
X-Plane 那盏灯、FSD 那句、语音那句；第三行是观察员勾选框加那段说明。
**拔掉 X-Plane**：卡片里第一格的灯变灰，卡片上方插件那盏灯和它的横幅各自照旧——
两者不联动就是对的。
**上线**：五个框变灰但字还在，按钮变成「下线」，第二格变成「FSD 已连接」。
**那一格里只有这一个按钮**——「识别（8 秒）」这一任务里整个界面上都不该出现，
Task 8 才把它建在无线电那一行。**勾观察员再上线**：呼号框换成「跟随的呼号」，
「下线」旁边多一句「跟随 <呼号>」。**连一次故意打错密码**：错误横幅在卡片上方，不在卡片里。
**切英文**：三格状态文案跟着变。**开精简**：这一任务里连接卡片仍然在（Task 9 才收起来），
置顶/精简两个钮仍然点得到。

---
### Task 7: 中间一行三张卡片 — 消息（伸缩 3）‖ 附近管制（1）‖ 他机（1）

**Files:**
- Modify: `apps/xpc/src/App.vue`（中间那一段 grid 换成三张 `Panel`；底栏的发送那一半提上来）
- Modify: `apps/xpc/src/locales/app.zh.json`（`controllers.hint` 的措辞改成单击）
- Modify: `apps/xpc/src/locales/app.en.json`（同上）

**Interfaces:**
- Consumes: `Panel.vue`（Task 1）；词条 `messages.title`（Task 2）、已有的 `lists.traffic`、
  `lists.controllers`、`chat.recipient` / `chat.message` / `chat.send` / `chat.observer_no_text`；
  `controllers.hint`（Task 2，措辞在 Step 2 里改）；App.vue 里已有的 `view`、`compact`、
  `connected`、`observing`、`recipient`、`message`、`send()`、`setRecipient()`。
  Task 6 引进来的 `Panel` import 直接用，不要再写一遍。
- Produces:
  - 中间一行 `<section class="flex min-h-0 flex-1 gap-3">`，三张 `Panel`，
    `flex-[3]` / `flex-1` / `flex-1`，都带 `min-h-0`。
  - `<footer>` 只剩 TX 色块、RX 色块、PTT 按钮三样。**Task 8 接手的就是这三样**，
    它要把 `<footer>` 整块换成无线电 `Panel` 加 `StatusBar`。
  - 收件人框、正文框、发送钮搬进消息卡片，`recipient` / `message` / `send()` 的语义一个不变。

**三个列表组件一个都不改。** `ChatLog.vue`、`ControllerList.vue`、`TrafficList.vue` 和 msfs 那三份
**逐字节相同**，登记在 `SHARED_FRONTEND` 里（`crates/can-voice-i18n/tests/dictionaries.rs:743-744、765`）。
这一任务做得到的事全部在 App.vue 里：卡片标题、`flex-1`、空态都是组件自带的。
真要改其中一个，必须**同一个提交里把 msfs 的那一份一起改**，并把两个路径都写进 `git add`，
否则 `shared_frontend_files_are_identical_in_every_app_that_carries_them` 会红。
**这一任务的预期是一份都不改。**

**附近管制保持单击，不改成双击。** can-audio 是 `itemDoubleClicked`（`xpc/gui.py:327`），
can-voice 的 `ControllerList` 每一行是个 `<button>`，单击就把呼号填进收件人框。改成双击
要动一个共用文件外加 msfs 的副本，换来的是**更难**发现的同一个功能。保持现状。

**卡片标题用哪个键。** 三张卡片各一句：

| 卡片 | 标题键 | 一句话理由 |
|---|---|---|
| 消息 | `messages.title`（消息） | can-voice 的 `lists.messages`（文字消息）不带计数，换成 can-audio 的名字不丢任何东西。 |
| 附近管制 | `lists.controllers`（在线席位（{count}）） | 它带**实时计数**，而 can-audio 那个纯名词标题不带；计数是 can-voice 现有的功能，标题栏正是它该在的地方。 |
| 他机 | `lists.traffic`（附近的飞机（{count}）） | 同上，且和邻座那张卡片对称——两张列表卡片一个带计数一个不带才是怪的。 |

`controllers.title` 和 `traffic.title` 因此没有人用，**Task 2 已经不再加它们**；
`controllers` 这个命名空间留着，里面只有下面这条 `hint`。

**附近管制列表下面那条换行提示要移植**，spec §6 点名了它（「下面一行换行提示」），
键是 `controllers.hint`（对应 `can-audio/xpc/i18n.py:127`），Task 2 已经带上。
**但它的原话要改**：can-audio 那一句写的是双击，而这里保持单击（见上一段）。
**文案和处理器必须说同一件事**——界面上写着双击而单击就生效，比没有提示更让人不信这个界面。

| | Task 2 现在的值 | 改成 |
|---|---|---|
| zh | `双击一个席位，把它填进收件人。` | `点一个席位，把它填进收件人。` |
| en | `Double-click a position to put it in the recipient box.` | `Click a position to put it in the recipient box.` |

**这是对 Task 2 的一处修订**，所以这一任务要动两个字典文件，见下面的 Step 2 和 `git add`。
哪天有人把 `ControllerList` 改成双击（那要连 msfs 的副本一起改），这两句要跟着改回去。

**伸缩和最小高度要写全，三张卡片才不会塌。** `Panel` 的根是
`<section class="flex min-w-0 flex-col rounded border bg-white">`——它**不自带 `flex-1`**，
所以伸缩比例写在 `<Panel>` 标签的 `class` 上（`Panel` 只有一个根元素，class 会落到那个
`<section>` 上）。行本身要 `min-h-0 flex-1`，卡片各自要 `min-h-0`，
里面三个列表要 `flex-1`：它们的根都带 `overflow-auto`，按 flexbox 规范那已经给了
`min-height: 0`，所以只缺一个「长满」。少任何一环，列表会把页面顶高，
`h-screen` 之下的结果是底栏被挤出窗口。

- [ ] **Step 1: 先把要搬的两块抄进 `.temp/`**

```bash
mkdir -p .temp
sed -n '/grid min-h-0 flex-1 gap-3/,/<\/section>/p' apps/xpc/src/App.vue > .temp/xpc-lists.txt
sed -n '/v-model="recipient"/,/<\/footer>/p' apps/xpc/src/App.vue | sed '$d' > .temp/xpc-send.txt
cat .temp/xpc-lists.txt .temp/xpc-send.txt
```

`.temp/xpc-send.txt` 里是收件人框、`.wallop` 那两行注释、正文框、发送钮——
**底栏要留给 Task 8 的 TX / RX / PTT 三样不在里面**，那三样不要动。

- [ ] **Step 2: 把 `controllers.hint` 的措辞改成单击**

Task 2 照 can-audio 的原话加的是双击，而这里的列表是单击。两个字典各改一个值：

`apps/xpc/src/locales/app.zh.json`

```json
    "hint": "点一个席位，把它填进收件人。"
```

`apps/xpc/src/locales/app.en.json`

```json
    "hint": "Click a position to put it in the recipient box."
```

只动这一个键的值，`controllers` 命名空间里别的不碰。两句都没有占位符，
所以 `placeholders_agree_between_the_languages` 不受影响；
`english_has_no_chinese_in_it` 看的是英文那一份，别把中文粘错文件。

- [ ] **Step 3: 中间那一段换成三张卡片**

把 `<section class="grid min-h-0 flex-1 gap-3" ...>` 到它的 `</section>` 整块换成：

```html
      <!-- LISTS-COMMENT -->
      <section class="flex min-h-0 flex-1 gap-3">
        <Panel :title="t('messages.title')" class="min-h-0 flex-[3]">
          <ChatLog class="min-h-0 flex-1" :messages="view?.messages ?? []" @reply="setRecipient" />
          <!-- can-audio 的发送行（xpc/gui.py:307-320）：收件人最大 240、正文撑开、发送。 -->
          <div class="flex items-center gap-2">
            <!-- SEND-ROW -->
          </div>
        </Panel>

        <Panel
          v-if="!compact"
          :title="t('lists.controllers', { count: view?.controllers.length ?? 0 })"
          class="min-h-0 flex-1"
        >
          <ControllerList
            class="min-h-0 flex-1"
            :controllers="view?.controllers ?? []"
            @reply="setRecipient"
          />
          <!-- can-audio 在列表下面放一条换行提示（xpc/gui.py:329-332）：
               单击就填收件人这件事，界面上不说没人会去试。 -->
          <p class="text-xs opacity-60">{{ t("controllers.hint") }}</p>
        </Panel>

        <Panel
          v-if="!compact"
          :title="t('lists.traffic', { count: view?.traffic.length ?? 0 })"
          class="min-h-0 flex-1"
        >
          <TrafficList class="min-h-0 flex-1" :traffic="view?.traffic ?? []" />
        </Panel>
      </section>
```

`LISTS-COMMENT` 从 `.temp/xpc-lists.txt` 里原样复制：是那一段上面两条注释
（「左边是天上的……」和「精简时只留文字消息……」）。**不要手打。**
原来 `<ControllerList` 上面那一行注释（「点一行就把那个席位填进收件人框。」）**不搬**——
它下面那条 `controllers.hint` 现在把同一句话说给用户听了，留着就是两处要一起维护的同一句。

`ControllerList` 原来带的 `max-h-28 shrink-0` 去掉了：它当时是为了在同一列里给下面的
消息让位，现在它自己占一整张卡片。

**精简模式原样保住了**：附近管制和他机两张卡片是 `v-if="!compact"`，消息卡片常驻——
这正是今天的行为（今天精简时只有 `ChatLog` 和正文框在）。消息卡片的标题栏在精简时也会画出来，
这是**故意的**：spec §6 说精简时「只留消息卡片」，那张卡片是带标题的。

- [ ] **Step 4: 把发送那一半填进 `SEND-ROW`**

`SEND-ROW` 那一行换成 `.temp/xpc-send.txt` 的内容（注释一起搬），然后改两个 `class`，
别的属性一个不动：

- 收件人框：`class="w-40 rounded border px-2 py-1 text-xs"`
  → `class="min-w-0 max-w-[240px] flex-1 rounded border px-2 py-1 text-xs"`
  （can-audio `setMaximumWidth(240)`，`xpc/gui.py:310`）
- 正文框：`class="min-w-0 flex-1 rounded border px-2 py-1 text-xs"`
  → `class="min-w-0 flex-[2] rounded border px-2 py-1 text-xs"`
  （收件人和正文在同一行里抢宽度时，正文该赢）

收件人框上的 `v-if="!compact"` **留着**：精简时那一行只剩正文和发送，和今天一样。
`:disabled="!connected"`、`@keyup.enter="send"`、观察员那个占位符三样原样保留。

- [ ] **Step 5: 底栏删掉搬走的那四行**

`<footer>` 里 `v-model="recipient"` 那个 `<input>` 起、到 `{{ t("chat.send") }}` 那个
`</button>` 为止删掉。删完底栏应该正好剩三样：

```bash
sed -n '/<footer/,/<\/footer>/p' apps/xpc/src/App.vue
```

打印出来的里面应该有且只有 `TX`、`RX`、`chat.push_to_talk` 三处，
不再有 `chat.recipient`、`chat.message`、`chat.send`。**底栏本身不要删**——
Task 8 才把它换成无线电 `Panel` 加 `StatusBar`。

- [ ] **Step 6: 查标点，删 `.temp/`**

```bash
perl -ne 'print "$.: $_" if /\p{Han}[,:;?!]/' apps/xpc/src/App.vue
git diff --stat
rm .temp/xpc-lists.txt .temp/xpc-send.txt
```

第一条什么都不该打印；`git diff --stat` 里**只该有三个文件**：`apps/xpc/src/App.vue`
和两个 `locales/app.*.json`——出现 `components/ChatLog.vue`、`ControllerList.vue` 或
`TrafficList.vue` 就是动了共用文件，回退掉，或者把 msfs 的对应文件一起改、一起 `git add`。

- [ ] **Step 7: 跑门**

```bash
(cd apps/xpc && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

`shared_frontend_files_are_identical_in_every_app_that_carries_them` 是这一步的真门：
它红了就说明三个列表组件里有一个被改了而 msfs 没跟上。

- [ ] **Step 8: 提交**

```bash
git add apps/xpc/src/App.vue \
        apps/xpc/src/locales/app.zh.json \
        apps/xpc/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(xpc): lay the three lists out as titled panels

Messages (flex 3), nearby ATC (flex 1) and traffic (flex 1) sit side by side.
The send row moves out of the footer into the messages panel; the footer keeps
TX, RX and the PTT button. The ATC and traffic titles keep their live counts.
The ATC panel carries the hint line, reworded to say click: the rows are
buttons and a single click fills the recipient box.

ChatLog, ControllerList and TrafficList are unchanged, so the msfs copies stay
byte-identical.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：连接卡片下面是三张并排的卡片，标题分别是「消息」「在线席位（0）」
「附近的飞机（0）」，宽度大致 3:1:1。**上线之后**：两个计数跟着数字走。
**把窗口拉窄到 900px**：三张卡片一起变窄，没有一张塌成零宽，也没有横向滚动条。
**让消息刷满**：消息卡片里面滚，页面**不**跟着长高，底栏还在窗口里——这一条是
`min-h-0` 那一串的唯一验证方式。**读一遍在线席位列表下面那句提示**：它说的是单击，
而单击确实就管用——这两件事对不上就是 Step 2 没做。**单击一个在线席位**：呼号进收件人框。
**点消息里的发件人**：同样进收件人框。**发一条**：正文框清空，新消息自动滚到底。
**开精简**：只剩消息那一张卡片（带标题栏），收件人框消失，正文和发送还在，
底栏 TX / RX / PTT 还在。

---
### Task 8: 无线电那一行和底部的 `StatusBar`

**Files:**
- Modify: `apps/xpc/src/App.vue`（加第五张 `Panel`「无线电」和一条 `StatusBar`；删掉旧的 `<footer>`、观察员频率栏、座舱读数网格）
- Modify: `apps/xpc/src/locales/app.zh.json` / `app.en.json`（删 `observer.frequency`，它在这一步失去最后一个消费者）

**Interfaces:**
- Consumes:
  - 任务 1 的 `Panel.vue`（`defineProps<{ title: string }>()`，一个默认插槽）、
    `StateToggle.vue`（`{ label; state: "off"|"on"|"active"|"muted"; width; height; disabled? }`，
    `emits: { press: [] }`）、`StatusBar.vue`
    （`{ talking; status; duty?; dutyOn?; pttTitle? }`，`emits: { down: [e]; up: [] }`）。
  - 任务 2 的 `radio.title`、`radio.com1`（`COM1  {frequency}`）、`radio.com1_none`、
    `radio.traffic`（`他机 {count}`）、`status.ready`。
  - 任务 6、7 已经把 `<main>` 改成「横幅 → 连接卡片 → 中间三张卡片」。这一任务接在那后面。
- Produces:
  - `transient: Ref<"update" | null>` 和 `barStatus: ComputedRef<string>`——任务 9 的
    「检查更新」把 `transient` 置上，底栏那句话就换过去。
  - 幂等的 `pttDown` / `pttUp`。
  - 无线电那一行本身：任务 9 的手工测试清单按它写。

**三块可见性不同的东西并成一行，这是这一任务单独存在的理由。** 今天的 `App.vue` 里有
三段，各有各的开关：座舱读数网格（`:355-372`，`v-if="!compact"`）、观察员频率栏
（`:327-350`，`v-if="observer"`，里面还套着一对 `v-if` / `v-else`，`:339-349`）、
底栏 TX / RX / PTT（`:403-440`，永远在）。合成一行之后每一格的开关要重新算一遍，
算错的表现是「某个人某种模式下少一格」，而没有任何测试看得见。下面这张表就是算完的结果，
**照着写**：

| 这一格 | 今天的开关 | 并进来之后的开关 | 精简模式 | 观察员 |
|---|---|---|---|---|
| TX 色块 | 永远（底栏） | 永远 | 在 | 在 |
| RX 色块 | 永远（底栏） | 永远 | 在 | 在 |
| COM1（等宽 15 粗） | `!compact`（座舱网格里的第一格） | 永远 | **在（变了，见下）** | 在，没开模拟器时是 `radio.com1_none` |
| 手输频率框 | `observer` | `observer` | 在 | 只有他有 |
| 频道文案（手输 / 跟随 COM1 / 还没有频率） | `observer` 且 `view.observer` 非空 | 同左 | 在 | 只有他有 |
| 他机计数 | 无（新的一格，旧的是列表上方的 `lists.traffic`） | `!compact` | 不在 | 在 |
| 座舱读数（应答机·高度·地速·航向·气压修正） | `!compact` | `!compact` | 不在 | 在，值全是 `—` |
| IDENT | `online && !observing`（会话区） | **永远画，`:disabled="!connected \|\| busy"`** | 在 | 在，但一直是灰的 |
| PTT 按钮 | 永远（底栏） | 永远 | 在 | 在 |

表里有两格的可见性真的变了，都要说清楚：

**COM1 在精简模式下从「没有」变成「有」。** 今天它在座舱读数网格里，网格整块被 `!compact`
关掉，所以精简时飞行员看不到自己调在哪个频率上。合进无线电那一行之后它跟着那一行在，
这是**改好了**不是改坏了：精简模式就是在飞的时候用的，而语音在哪个频率上正是那时候唯一
要盯的数字。座舱读数剩下的五个值仍然按 `!compact` 收起来——那几个数模拟器里都有，
压在模拟器上的窗口不必再显示一遍（这条理由是 `:354` 那行注释原来就写着的）。

**IDENT 从「连上才画」变成「一直画、没连上就是灰的」。** 一行里的格子来回出现会让右边所有
东西横着跳。can-audio 也是这么做的（`xpc/gui.py:541` 建好就 `setEnabled(False)`，连上才亮）。

**精简模式下这张卡片整张留着，不跟着中间那三张一起关。** spec §6 写的是「精简时只留消息
卡片，也就是它现在的行为」——**后半句和前半句对不上**，今天的精简模式留的是消息列表**加**
整条底栏（TX / RX / PTT 都在），而且观察员频率栏那段注释（`:326`）写得很死：
「精简时也在：这是他唯一的调频手段」。按前半句做会一次砍掉两样东西：屏幕上那颗 PTT
（Linux / Wayland 上它是唯一能发话的路径，见 `StatusBar.vue` 的注释）和观察员唯一的调频
入口。所以这里按后半句做：**精简 = 消息卡片 + 无线电那一行（收起他机计数、座舱读数和底栏）**，
等于今天的行为换了个外壳。任务 9 的窗口几何是照这个结论定的。

**IDENT 只画在这一行，连接卡片里不要再画一个。** spec §6 两处都提到它——连接那一段说
「连接按钮翻成断开，Ident 在旁边」，无线电那一行又写着「→ IDENT → PTT 按钮」。can-audio
只有一个，在无线电那一行（`xpc/gui.py:540-544`），它的连接卡片上没有。两个按钮发同一条
`$ID` 是纯粹的重复，**这一条已经定了：IDENT 在无线电那一行，任务 6 的连接卡片只留
连接 / 断开，不画 IDENT。**

**TX / RX 的三态怎么取值。** 和管制端同一套语义（`RadioRow.vue:54-60`）：有频率是 `on`
（绿），此刻真的在收 / 发是 `active`（琥珀），没有频率是 `off`（暗蓝）。**红色不要用**：
`--can-muted` 在管制端只表示静音，两个端各红各的，看代码的人就再也分不清红是什么意思。
这是对今天 xpc（TX 红、RX 绿）和 can-audio（`Indicator("TX", MUTED_COLOR)`）的**刻意偏离**，
换来的是四个端一套颜色语义。

**`StatusBar` 不接 `@down` / `@up`。** xpc 不传 `pttTitle`，那颗 PTT 按钮压根不画
（`StatusBar.vue:59` 的 `v-if="pttTitle"`），但 `onUnmounted(() => emit("up"))`
（`:54`）在 `<script setup>` 里，**不受模板条件约束，照样会 emit**。不绑监听它就落空，
这正是想要的：xpc 的 PTT 在无线电那一行上，不在这条栏里，拆这条栏不该松开麦克风。
顺手把 `pttDown` / `pttUp` 改成幂等的（今天 `:78-81` 不是），理由和管制端
（`App.vue:355-365`）一样：键盘 / 鼠标侧键那一路的按下状态在 Rust 侧
（`ptt_pressed`），一次多余的 `set_transmitting(false)` 会在人还按着键的时候把话切断。

- [ ] **Step 1: 把三段旧的删掉**

**这一步不许用行号。** 到这一任务跑的时候，`App.vue` 已经被任务 4、6、7 各改过一遍
（任务 6 把整个页头拆没了，几十行），**这份计划别处写的 `:327-350` 这类行号全部作废**——
它们说的是没动过的那份文件。按内容找，不按位置找：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -n 'v-if="observer"' apps/xpc/src/App.vue
grep -n 'grid-cols-6' apps/xpc/src/App.vue
grep -n '<footer' apps/xpc/src/App.vue
```

三条各该恰好一个命中。**不是一个命中就停下来查**，不要猜。

删除（都按开头那一行往下找到它自己的收尾标签）：
1. 观察员频率那一段：`<!-- 观察员的频率。… -->` 那条注释起，到 `v-if="observer"`
   那个 `<section>` 的 `</section>` 止。
2. 座舱读数那一段：`<!-- 座舱读数。… -->` 那条注释起，到那个 `grid-cols-6`
   的 `<section>` 的 `</section>` 止。
3. 底栏：`<footer …>` 整块到 `</footer>`。任务 7 已经把收件人框、正文框和「发送」
   搬进消息卡片了，所以这时候它里面通常只剩 TX / RX / PTT 三件；**剩什么都一起删**。

删完之后这两条都必须没有输出：

```bash
grep -n '<footer' apps/xpc/src/App.vue
grep -n 'grid-cols-6' apps/xpc/src/App.vue
```

- [ ] **Step 2: `<script setup>` 里加五样东西**

`StateToggle`、`StatusBar`、`Panel` 的 import 加在已有的 import 里（`Panel` 任务 6 已经引过，
重复引会被 `vue-tsc` 报重复声明，先看一眼）：

```ts
import Panel from "./components/Panel.vue";
import StateToggle from "./components/StateToggle.vue";
import StatusBar from "./components/StatusBar.vue";
```

然后在 `receiving` 那几行下面加：

```ts
/**
 * TX / RX 两块色块的三态。**和管制端同一套语义**（`RadioRow.vue:54-60`）：
 * 有频率就是 `on`，此刻真的在收 / 发是 `active`，一个频率都没有是 `off`。
 *
 * **红色（`muted`）不用。** 管制端那边红色只表示静音；同一个组件在两个端表示两件事，
 * 看代码的人就再也分不清红是什么意思。can-audio 的 `Indicator("TX", MUTED_COLOR)`
 * 和今天这里的 `bg-red-600` 都让出来，换四个端一套颜色。
 */
const txState = computed<"off" | "on" | "active">(() =>
  talking.value ? "active" : voiceKhz.value !== null ? "on" : "off",
);
const rxState = computed<"off" | "on" | "active">(() =>
  receiving.value ? "active" : voiceKhz.value !== null ? "on" : "off",
);

/**
 * 底栏中间那句话此刻在说什么。**存的是状态不是那句话**：存一句翻好的话，
 * 切了语言之后已经显示着的那一句不会跟着变（和 `error` 同一条规矩，见 `:34-38`）。
 *
 * `"update"` 由任务 9 的「帮助 → 检查更新」置上，那一任务还会再给它加一种取值。
 */
const transient = ref<"update" | null>(null);

/** 底栏那句话。can-audio 空闲时说「就绪」，有事说那件事（`xpc/gui.py:200`）。 */
const barStatus = computed(() => {
  if (transient.value === "update") return t("update.checking");
  if (talking.value) return t("chat.transmitting");
  return t("status.ready");
});

/**
 * 无线电那一行右边那串数字。can-audio 的 `position_label`（`xpc/gui.py:532-535`）
 * 就是这一格——它装的是**本机的位置和姿态**，不是管制席位。
 *
 * 不画标签：五个标签摊在这一行上会把它挤散，而 can-audio 也没有标签。标签进
 * `title`（下面那个函数），悬停时仍说得出这串数字各是什么。
 */
function cockpitText(): string {
  const s = view.value?.sim;
  if (!s) return "—";
  const squawk = String(s.squawk).padStart(4, "0");
  const heading = String(Math.round(s.heading)).padStart(3, "0");
  return `A${squawk} ${xpdrText(s.xpdr_mode)}  ${s.altitude} ft  ${s.groundspeed} kt  ${heading}°  ${s.pressure_delta} ft`;
}

/** 上面那串数字的读法，按顺序列出五个标签。分隔符走字典：中文是「、」，英文是「, 」。 */
function cockpitTitle(): string {
  return [
    t("cockpit.transponder"),
    t("cockpit.altitude"),
    t("cockpit.groundspeed"),
    t("cockpit.heading"),
    t("cockpit.pressure_delta"),
  ].join(t("common.separator.list"));
}
```

`computed` 和 `ref` 已经在第一行 import 过了；`mhz`、`khzText`、`xpdrText` 也已经在
`:14` 那行 import 过了，不要重复引。

- [ ] **Step 3: `pttDown` / `pttUp` 改成幂等的**

把今天的 `:73-81` 换成：

```ts
/**
 * 屏幕上那颗 PTT。**两头都写成幂等的**：键盘 / 鼠标侧键那一路的按下状态在 Rust 侧
 * （`ptt_pressed`），而这里每按一下都发一趟命令。多余的一次 `set_transmitting(false)`
 * 会在人还按着键的时候把话切断——那时候他正在说话，而界面上一点异样都没有。
 */
function pttDown(e: PointerEvent) {
  // 指针捕获：按住说话时手是会动的，滑出按钮之后 `pointerup` 就落到别的元素上，
  // 松手事件永远不来，麦克风一直开着。
  (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  if (holding.value) return;
  holding.value = true;
  void invoke("set_transmitting", { on: true });
}
function pttUp() {
  if (!holding.value) return;
  holding.value = false;
  void invoke("set_transmitting", { on: false });
}
```

- [ ] **Step 4: 无线电那张卡片**

放在中间那三张卡片的 `</section>` 之后、`<SettingsDialog …>` 之前：

```html
      <!-- 无线电。can-audio 的 `_build_radio_bar`（`xpc/gui.py:334-380`）：这张卡片
           本来**就是**模拟器状态行，COM1、频道、他机计数和座舱读数都属于这里。 -->
      <!-- 精简时也在，只收起他机计数和座舱读数：屏幕上这颗 PTT 在 Wayland 上是唯一
           能发话的路径，手输频率框是观察员唯一的调频手段，两样都不能跟着精简消失。 -->
      <Panel :title="t('radio.title')">
        <!-- 排不下就换行，不是裁掉：900×600 的最小尺寸只管正常模式，精简时窗口能
             拖到 320px 宽。 -->
        <div class="flex flex-wrap items-center gap-2">
          <StateToggle label="TX" :state="txState" :width="52" :height="26" />
          <StateToggle label="RX" :state="rxState" :width="52" :height="26" />

          <!-- 电门关着时照样显示调在哪：这是座舱里的读数，而「听不见」由左边两块
               色块转暗去说。 -->
          <span class="font-mono text-[15px] font-bold tabular-nums">
            {{
              view?.sim?.com1
                ? t("radio.com1", { frequency: mhz(view.sim.com1) })
                : t("radio.com1_none")
            }}
          </span>

          <!-- 手输频率**只有观察员有**。正常上网络的飞行员要是能把语音频率和座舱 COM1
               分开设，迟早出现「管制以为你在 121.8、你人在别的频道」，那比听不见更糟。 -->
          <input
            v-if="observer"
            v-model="manualFrequency"
            :placeholder="t('observer.frequency_placeholder')"
            :title="t('observer.frequency_tip')"
            class="w-28 rounded border px-2 py-1 font-mono text-xs"
            @change="applyFrequency"
          />
          <!-- 频道文案也只有观察员有：飞行员的频道就是左边那个 COM1，同一个数字写两遍
               就是两个真相（`:352-353` 原来那条注释说的就是这件事）。 -->
          <template v-if="observer && view?.observer">
            <span v-if="view.observer.frequency !== null" class="font-mono text-xs opacity-60">
              {{ khzText(view.observer.frequency) }}
              {{ view.observer.manual ? t("observer.manual") : t("observer.follow_com1") }}
            </span>
            <span
              v-else
              class="min-w-0 truncate text-xs text-amber-700"
              :title="t('observer.no_frequency')"
            >
              {{ t("observer.no_frequency") }}
            </span>
          </template>

          <span class="grow" />

          <span v-if="!compact" class="font-mono text-xs tabular-nums opacity-60">
            {{ t("radio.traffic", { count: view?.traffic.length ?? 0 }) }}
          </span>

          <!-- 座舱读数。精简时收起：这几个数模拟器里都有，压在模拟器上的窗口不必再
               显示一遍。 -->
          <span
            v-if="!compact"
            class="font-mono text-xs tabular-nums opacity-60"
            :title="cockpitTitle()"
          >
            {{ cockpitText() }}
          </span>

          <!-- 一直画着，没连上就是灰的：格子来回出现会让右边所有东西横着跳。
               观察员没有 FSD 链路，`connected` 对他恒为假，所以这颗对他一直是灰的。 -->
          <button
            class="rounded border px-3 py-1 text-xs"
            :disabled="!connected || busy"
            @click="ident"
          >
            {{ t("session.ident") }}
          </button>

          <button
            class="rounded border px-4 py-2 text-xs"
            :class="talking ? 'text-white' : ''"
            :style="talking ? { background: 'var(--can-active)' } : {}"
            :title="t('chat.push_to_talk_tip')"
            @pointerdown="pttDown"
            @pointerup="pttUp"
            @pointercancel="pttUp"
          >
            {{ t("chat.push_to_talk") }}
          </button>
        </div>
      </Panel>
```

- [ ] **Step 5: 底部那条 `StatusBar`**

紧跟在上面那张 `</Panel>` 之后，仍然在 `<main>` 里、`<SettingsDialog>` 之前：

```html
      <!-- can-audio 飞行员端的 `QStatusBar`（`xpc/gui.py:200`）：只说「就绪」和瞬时状态。
           **不传 `pttTitle`**，所以这条栏上不画 PTT——那颗按钮在上面那一行里。
           也**不接 `@down` / `@up`**：`StatusBar` 卸载时会补发一次 `up` 当保险
           （`StatusBar.vue:54`），而 xpc 的 PTT 不在这条栏上，拆这条栏不该松开麦克风。
           `talking` 仍然要传，它是必填属性。 -->
      <StatusBar v-if="!compact" :talking="talking" :status="barStatus" />
```

- [ ] **Step 6: 处理被这一步孤立的那个键**

行里那个手输频率框不再有「语音频率」那句前缀标签——can-audio 也没有，占位符
（`留空跟随 COM1`）已经把话说完了。所以 `observer.frequency` 在这一步失去最后一个消费者。
**规矩是：孤立了一个键的那一步负责 grep，零命中才删，有命中就照实说谁还在用。**

```bash
grep -rn 't("observer.frequency")' apps/xpc/src apps/msfs/src
```

`apps/xpc` 里应当零命中（`view.observer.frequency` 是 `View` 上的字段，不是键，
grep 的写法已经把它排除了）。`apps/msfs` 会有一条——那是 msfs 自己的 `App.vue` 配自己的
`apps/msfs/src/locales/`，**和这里删的不是同一份文件**，不用管它，计划 4 到那一步时一起收。

零命中就把 `apps/xpc/src/locales/app.zh.json` 和 `app.en.json` 的 `observer` 段里那一行
删掉（两份都要删，`both_languages_have_the_same_keys_and_none_is_empty` 会看）：

```json
    "frequency": "语音频率",
```
```json
    "frequency": "Voice frequency",
```

- [ ] **Step 7: 跑门**

```bash
(cd apps/xpc && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```
没动共用文件，所以只用 build xpc。`every_key_the_interface_asks_for_exists` 会看这一步
新引的 `radio.*`、`status.ready`、`chat.transmitting`、`common.separator.list`——
`chat.transmitting` 在这之前是一条没有消费者的键，现在有了。

- [ ] **Step 8: 提交**

```bash
git add apps/xpc/src/App.vue \
        apps/xpc/src/locales/app.zh.json \
        apps/xpc/src/locales/app.en.json
git commit -m "$(cat <<'MSG'
feat(xpc): fold the cockpit, observer and footer rows into one radio card

TX/RX blocks, COM1, the observer-only frequency box, the traffic count, the
cockpit readout, IDENT and PTT become one row in a titled panel, and a
StatusBar carries the idle caption below it.

COM1 now survives compact mode, where the cockpit grid used to hide it, and
IDENT is always rendered and disabled off-line instead of appearing and
disappearing. TX/RX take the controller's tri-state colours so that red keeps
meaning muted in both apps. The PTT handlers are idempotent.

observer.frequency loses its last consumer with the caption: the placeholder
already says what the box is, as can-audio's does.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：

- 连上之后最下面多出一张标题为「无线电」的卡片，一行从左到右是
  TX、RX、`COM1  121.800`、（观察员才有的频率框和频道文案）、撑开、`他机 N`、
  一串座舱读数、IDENT、按住通话。
- 按住 PTT：TX 那块变琥珀，松开变回暗蓝。管制说话时 RX 变琥珀。**两块都不该是红的**。
- COM1 电门关掉：两块都回到暗蓝，但 COM1 那个数字还在。
- 悬停座舱读数那一串：提示里是「应答机、高度、地速、航向、气压修正」五个词。
- 没连上时 IDENT 是灰的，连上之后能按；观察员一直是灰的。
- 窗口最底下一条细线上写着「就绪」；按住 PTT 时变成「发话中」，松开变回来。
- 切精简：无线电那一行还在，`他机 N`、座舱读数和底栏那条不见了；观察员的频率框**还在**。
- 切到 English 再切回来：这一行每一句话都跟着变。

---
### Task 9: 窗口几何、精简模式、最后两个菜单项、文档

**Files:**
- Modify: `apps/xpc/src-tauri/tauri.conf.json`（980×660，最小 900×600）
- Modify: `apps/xpc/src-tauri/src/lib.rs`（只改 `COMPACT_SIZE` 的文档注释，数字不动）
- Modify: `apps/xpc/src/App.vue`（菜单的 `update` / `about` 两个分支、关于对话框、底栏那句话多一种状态）
- Modify: `apps/{controller,atis,xpc,msfs}/src/locales/common.zh.json` / `common.en.json`
  （**八个文件**，加一个 `update.current`）
- Modify: `docs/manual-test.md`（§3 按新排布重写）
- Modify: `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md`（§6 补两条订正）

**Interfaces:**
- Consumes:
  - 任务 3 的 `app_version() -> &'static str`、`View.menu`；仓库原有的 `log_file`、
    `check_update`、`skip_update`、`open_download`。
  - 任务 4 建的菜单派发函数（`flight_plan` / `settings` 两条分支）和 `showPrefs` / `showPlan`。
  - 任务 1 的 `Modal.vue`（`{ open; title; width? }`，`emits: { close: [] }`，自带「关闭」）。
  - 任务 2 的 `about.title` / `about.body` / `about.no_log`；任务 8 的 `transient`。
- Produces: 没有后续任务依赖的东西——这是这一份计划的最后一步。

**`COMPACT_MIN` / `COMPACT_SIZE` 不动，这是想过之后的结论，不是漏了。** 两个常量在
`apps/xpc/src-tauri/src/lib.rs:1598`（320×220）和 `:1601`（460×340）。计划 2 把通播端那
一对改成了 300×220 / 300×320，改的理由是**去取 can-audio 的数字**（spec §5 点名了
`atis/gui.py`）。这里没有这样的数字可取：**can-audio 的两个飞行员端根本没有精简模式**。
所以只能自己算，而算下来今天这一对仍然合适：

- 精简模式留的东西没变（消息卡片 + 无线电那一行的必需件，见任务 8 的表），所以照着它们
  调出来的尺寸仍然对得上。
- 460px 正好排得下那一行不换行的宽度：TX 52 + RX 52 + `COM1  121.800` 约 110 + IDENT 约 70
  + 按住通话 约 80，加 `gap-2` 五道和卡片 `p-3` 两边，合计约 440。
- 320px 的下限也不会裁掉任何东西：那一行是 `flex-wrap` 的，排不下就折成两行，卡片跟着变高。

**改的是注释，不是数字。** spec §6 写着「精简时只留消息卡片」，而代码里留的是两张，
不写下来的话下一个读 spec 的人会来把无线电那张关掉。

**`WindowToggles` 不传 `only`，三颗钮全留着。** can-audio 的 xpc 窗口里确实没有设置钮
（`xpc/gui.py:213-216`，设置只在菜单里），但这里多一颗是**便宜的保险**：这条菜单是前端在
挂载时调 `set_menu` 建起来的，那一趟会失败（失败的话走 `problem.menu`）。菜单建不起来、
而顶栏那颗钮又被拿掉了的话，**设置就一个门都没有了**——连改服务器地址都做不到，
而那正是部署出问题时要改的东西。精简模式不受影响：那颗钮今天就自己藏起来
（`WindowToggles.vue:55` 的 `!appearance.compact`），精简下本来就没有设置的门，
换成菜单之后反而多了一个。组件是四个端逐字节相同的一份，**这一任务一个字都不改它**。

**「检查更新」不许是静悄悄的，所以要加一个键。** 它有两条什么都查不到的路：一是
`check_update` 在连着的时候（含观察员）直接返回 `None`，注释写明了理由——「连着的时候一个
模态框盖在台面上比晚一次更新糟得多」；二是本来就已经是最新版。两条都要有一句回话，
否则点了跟没点一样（can-audio 自己在 `xpc/gui.py:894-918` 就专门为这件事留了
`manual=True` 那条分支）。

措辞是**「没有可用的更新。」**，不是「已经是最新版本」：后者在第一条路上是假话——
那时候可能有新版，只是客户端刻意没去问。前者两条路上都成立。

这个键加在 **`common`** 的 `update` 命名空间里，因为它和 `update.checking` 是一组的。
`common.*.json` 是四个端逐字节相同的（`the_common_dictionaries_are_identical_in_every_app`），
所以**这一步要改八个文件**，`git add` 里四个端都要点名，跑门时四个端都要 build。
`UpdateBanner.vue` 同样是四端共用件（`SHARED_FRONTEND`），**不要为这件事改它**——
新那句话在 `App.vue` 的底栏上说，不进横幅。

- [ ] **Step 1: 窗口几何**

`apps/xpc/src-tauri/tauri.conf.json` 的 `app.windows[0]`，四个数字整段换成：

```json
        "title": "xpc-for-can",
        "width": 980,
        "height": 660,
        "minWidth": 900,
        "minHeight": 600
```

`minWidth` / `minHeight` **是刻意加的**，can-audio 一个都不设（`xpc/gui.py` 里没有
`setMinimumSize`）。没有它中间那三张卡片（消息 3 ‖ 附近管制 1 ‖ 他机 1）一挤就塌成一条。
spec §6 已经写着这一条，这里照做。

- [ ] **Step 2: 把精简模式的结论写进 `COMPACT_SIZE` 的注释**

`apps/xpc/src-tauri/src/lib.rs:1599-1601`，两个常量的**数字不动**，给 `COMPACT_SIZE`
的文档注释续上两句：

```rust
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
///
/// **这一对数字在换 can-audio 布局时没有跟着改，是算过的。** can-audio 的两个飞行员端
/// 没有精简模式，没有数字可取；而精简留下的东西没变——消息卡片，加无线电那一行里的
/// TX / RX、COM1、IDENT、PTT 和观察员的频率框（屏幕上这颗 PTT 在 Wayland 上是唯一能
/// 发话的路径，那个频率框是观察员唯一的调频手段）。460 正好排得下那一行不换行，
/// 320 的下限也不裁东西：那一行是 `flex-wrap` 的，排不下就折成两行。
const COMPACT_SIZE: (f64, f64) = (460.0, 340.0);
```

注意上半段是原样保留的，只在末尾加一段。抽出来对一眼：

```bash
sed -n '1596,1602p' apps/xpc/src-tauri/src/lib.rs
```

- [ ] **Step 3: 加 `update.current`，八个文件**

`common.*.json` 四个端逐字节相同，所以同一行要写进八个文件。`update` 命名空间在每一份的
`:51-61`，把新键插在 `checking` 后面（它俩是一组：一个说正在查，一个说查完了）：

```bash
python3 - <<'PY'
from pathlib import Path

rows = {
    "zh": ('    "checking": "正在检查更新…",\n',
           '    "current": "没有可用的更新。",\n'),
    "en": ('    "checking": "Checking for updates…",\n',
           '    "current": "No update available.",\n'),
}
for app in ("controller", "atis", "xpc", "msfs"):
    for lang, (anchor, added) in rows.items():
        p = Path(f"apps/{app}/src/locales/common.{lang}.json")
        s = p.read_text(encoding="utf-8")
        assert s.count(anchor) == 1, p
        assert added not in s, p
        p.write_text(s.replace(anchor, anchor + added, 1), encoding="utf-8")
        print("ok", p)
PY
```

八份仍然要逐字节相同，核一眼：

```bash
md5 -q apps/*/src/locales/common.zh.json | sort -u | wc -l   # 要是 1
md5 -q apps/*/src/locales/common.en.json | sort -u | wc -l   # 要是 1
```

**措辞不要改成「已经是最新版本」。** 客户端连着网络的时候 `check_update` 直接返回
「没有更新」而根本没去问服务端（`lib.rs:1448-1466`），那种情况下说「已经是最新版本」
是假话；「没有可用的更新」两种情况下都成立。

- [ ] **Step 4: 关于对话框和最后两个菜单分支**

`<script setup>` 里，接在任务 4 那些对话框开关旁边：

```ts
/** 关于对话框开没开。 */
const showAbout = ref(false);
const version = ref("");
const logPath = ref("");

/**
 * 「帮助 → 关于」。版本号和日志路径**打开时才读**，不在 `onMounted` 里读：
 * 计划 1 有过一次教训，`onMounted` 里一条 reject 的 invoke 会把它后面的
 * `setInterval` 和 PTT 绑定一起带走，界面连上之后画一帧就不动了。
 */
async function openAbout() {
  try {
    version.value = await invoke<string>("app_version");
  } catch {
    version.value = "";
  }
  try {
    logPath.value = (await invoke<string | null>("log_file")) ?? "";
  } catch {
    logPath.value = "";
  }
  showAbout.value = true;
}

/**
 * 「帮助 → 检查更新」。`UpdateBanner` 自己在挂载时查一次，所以这里换掉它的 `key`
 * 让它重挂一次；自己这一趟 `check_update` 用来知道「查完了」，以及查到了没有。
 *
 * **查不到也要回一句。** 点了跟没点一样是最糟的形态（can-audio 专门为这件事留了
 * `manual=True` 那条分支，`xpc/gui.py:894-918`）。有新版时的反馈是卡片上方那条横幅，
 * 没有就在底栏说「没有可用的更新。」，停 4 秒回到「就绪」。
 *
 * **正连着的时候一定查不到**，这是 Rust 侧刻意的（`lib.rs:1448-1466`：一个更新框盖在
 * 台面上比晚一次更新糟得多），所以那句话说的是「没有可用的更新」而不是「已经是最新版本」。
 */
const updateNonce = ref(0);
let updateClear: number | undefined;
async function checkUpdate() {
  window.clearTimeout(updateClear);
  transient.value = "update";
  updateNonce.value += 1;
  let found = false;
  try {
    found = (await invoke<unknown>("check_update")) !== null;
  } catch {
    // 查不动就当没有更新——Rust 侧每条错误路径本来也返回「没有更新」。
  }
  transient.value = found ? null : "no_update";
  // 停一会儿就收回去：底栏那句话讲的是此刻，不是一条留着的记录。
  if (!found) updateClear = window.setTimeout(() => (transient.value = null), 4000);
}
```

任务 8 那个 `transient` 只有 `"update"` 一种取值，这一步把它和 `barStatus` 各加一种
（新那句话现在才有键，写早了 `vue-tsc` 过不去）：

```ts
const transient = ref<"update" | "no_update" | null>(null);
```
```ts
const barStatus = computed(() => {
  if (transient.value === "update") return t("update.checking");
  if (transient.value === "no_update") return t("update.current");
  if (talking.value) return t("chat.transmitting");
  return t("status.ready");
});
```

`onUnmounted` 里把那个定时器也清掉，和 `timer`、`detachPtt` 并列——不清的话它会朝着一个
已经拆掉的组件写值：

```ts
  window.clearTimeout(updateClear);
```

任务 4 建的那个菜单派发函数补上后两条分支。补完是这个样子（任务 4 若写成 `switch`，
就按同样的顺序加两个 `case`）：

```ts
/** 菜单点了哪一项。`view.menu` 是取走即清的，所以读到非 null 就立刻处理一次，不要存。 */
function onMenu(m: NonNullable<View["menu"]>) {
  if (m === "flight_plan") showPlan.value = true;
  else if (m === "settings") showPrefs.value = true;
  else if (m === "update") void checkUpdate();
  else if (m === "about") void openAbout();
}
```

模板里给 `UpdateBanner` 加一个 `key`（它今天在横幅那一段，`:230`）：

```html
      <UpdateBanner v-if="!compact" :key="updateNonce" />
```

关于对话框放在 `<SettingsDialog …>` 旁边，`</main>` 之后也行、之前也行——`Modal` 自己是
`fixed` 覆盖层：

```html
    <!-- `about.body` 里有 `\n`。`whitespace-pre-line` 让浏览器照着换行就够了，
         **不要 `v-html`**：那句话里插着版本号和日志文件路径，都是从程序外面来的字符串。 -->
    <Modal
      :open="showAbout"
      :title="t('about.title')"
      width="w-[28rem]"
      @close="showAbout = false"
    >
      <p class="whitespace-pre-line text-xs leading-relaxed">
        {{
          t("about.body", {
            name: t("app.title"),
            version,
            log: logPath || t("about.no_log"),
          })
        }}
      </p>
    </Modal>
```

`Modal` 的 import 任务 4 已经加过了（飞行计划对话框在用），不要重复引。

- [ ] **Step 5: 手工测试清单**

`docs/manual-test.md` 的 §3 是「飞行员」，今天是 `148-177` 行。**保留的三小节原样不动**
（3.1 能说话、3.2 他机、3.4 服务端通知），只改两处、补四小节。

`msfs-for-can` 在这一份计划里没有动，所以新排布那几条只对 `xpc-for-can` 成立，
清单里要写明，否则测 msfs 的人会照着一张对不上的清单打叉。

```bash
python3 - <<'PY'
from pathlib import Path

p = Path("docs/manual-test.md")
s = p.read_text(encoding="utf-8")

# 1) 小节抬头下面补一句：哪几条只对 xpc 成立。
old = "`xpc-for-can` 或 `msfs-for-can`。COM1 调到 121.800，电门打开。\n"
assert s.count(old) == 1
s = s.replace(
    old,
    old
    + "\n**3.5 起只对 `xpc-for-can` 成立**：新排布先落在这一支，`msfs-for-can` "
      "还是老样子。\n",
    1,
)

# 2) 观察员那条补上频率框的新位置。
old = "- 清空手输频率：改跟 COM1。COM1 没开时可以没有频率，不要猜一个。\n"
assert s.count(old) == 1
s = s.replace(
    old,
    "- 清空手输频率：改跟 COM1。COM1 没开时可以没有频率，不要猜一个。"
    "（xpc 上这个框在「无线电」那一行，COM1 右边；别人没有这个框。）\n",
    1,
)

# 3) 在 §4 之前补四小节。
anchor = "\n## 4. 通播\n"
assert s.count(anchor) == 1
added = """
### 3.5 菜单（只有 xpc）

窗口有了一条原生菜单：macOS 在屏幕顶栏，Windows / Linux 在窗口里。六项都点一遍。

| 点 | 通过 |
|---|---|
| 文件 → 飞行计划… | 开出一张飞行计划对话框。主页面上**没有**飞行计划那一页了 |
| 文件 → 设置… | 开出设置对话框，里面有 音频 / 网络 / 他机 三页 |
| 文件 → 退出 | 窗口关掉 |
| 帮助 → 打开日志目录 | 系统的文件管理器打开日志所在目录。日志没写成文件时说「这台机器上没有日志文件」 |
| 帮助 → 检查更新 | 底栏写「正在检查更新…」；有新版时卡片上方出现更新横幅，没有时底栏写「没有可用的更新。」，约 4 秒后回到「就绪」 |
| 帮助 → 关于 | 一个小对话框：程序名、版本号、日志文件路径。**换行要是真的换行**，不是一串 `\\n` |

- **设置 → 语言切成 English，菜单栏应当场变英文**，不用重启；切回中文同理。这是这条菜单
  唯一的失败点：文案一句都不在 Rust 侧，全靠前端每次换语言重建一次菜单。
- 刚启动的头几帧没有菜单，是正常的——菜单要等 webview 起来之后把文案送过去。
- **「检查更新」在正连着网络的时候必定说「没有可用的更新。」**，即使真有新版——客户端刻意
  不在飞行途中弹更新。这不算失败；要测有新版那条路，先下线再点。

### 3.6 四张卡片（只有 xpc）

从上到下：整宽的「连接」，中间一行「消息」「附近管制」「他机」，最下「无线电」。

- 中间三张的宽度大致是 3 : 1 : 1，拉宽窗口时「消息」长得最快。
- 报错、服务端通知、插件那两条横幅（版本不符 / 没听到）在卡片**上方**，不在卡片里。
- 点「附近管制」里的一行，收件人框自动填上那个呼号。
- 「他机」那张此前是页面左半边的一列，现在是中间一行的第三张，内容不变。

### 3.7 无线电那一行（只有 xpc）

从左到右：TX、RX 两块色块 → `COM1  121.800`（等宽粗体）→（观察员才有的频率框和
「手输 / 跟随 COM1」）→ 撑开 → `他机 N` → 一串座舱读数 → IDENT → 按住通话。

- 按住 PTT：TX 变琥珀，松开变回暗蓝。管制说话时 RX 变琥珀。**两块都不该是红的**——
  红色在管制端表示静音。
- COM1 电门关掉：两块色块都回暗，COM1 那个数字**还在**（那是座舱读数，不是「能不能听见」）。
- 悬停那串座舱读数：提示里是「应答机、高度、地速、航向、气压修正」五个词，顺序和数字对得上。
  模拟器没开时那一格是一条 `—`。
- IDENT 没连上 FSD 时是灰的，连上能按；观察员一直是灰的（他没有 FSD 链路）。

### 3.8 状态栏、几何和精简模式（只有 xpc）

- 窗口默认 980×660。往小了拖：停在 900×600，拖不进比这更小。
- 底栏：空闲写「就绪」，按住 PTT 写「发话中」，点了「检查更新」写「正在检查更新…」，
  查完没有新版写「没有可用的更新。」并在约 4 秒后收回去。
- 点顶栏「精简」：窗口缩到约 460×340，只剩「消息」和「无线电」两张卡片；`他机 N`、
  座舱读数、底栏、更新横幅都不见了。**TX / RX、COM1、IDENT、按住通话还在**，观察员的
  频率框也还在——这两样是精简模式留着这张卡片的全部理由。
- 「置顶」「精简」两个钮变成约 30×26 的方块，字变成「顶」「简」；顶栏那颗「设置」钮
  精简时自己藏起来（本来就是这样）。
- **精简模式下设置仍然进得去**：文件 → 设置…。旧排布进不去，这是新排布多出来的一条路。
  顶栏那颗「设置」钮**留着**，是菜单建不起来时的后备。
- 再点一次「简」：窗口退回不小于 900×600，刚才那些东西都回来。
- 中英两种界面语言各看一遍：「无线电」那一行不该挤成三行。
"""
s = s.replace(anchor, added + anchor, 1)

p.write_text(s, encoding="utf-8")
print("ok")
PY
```

- [ ] **Step 6: 给 spec §6 补两条订正**

两条都是**计划 4（msfs）会照着再错一次**的东西，所以订正写在 spec 里而不是只写在这份计划里。

一是 spec 把 can-audio 的 `position_label` 译成了「席位」，而它装的是本机的位置和姿态。
二是 spec 那句「精简时只留消息卡片，也就是它现在的行为」自相矛盾：今天的精简留的是
消息列表**加**整条底栏（TX / RX / PTT 都在），而且观察员频率栏那段注释
（`apps/xpc/src/App.vue:326`）写着「精简时也在：这是他唯一的调频手段」。后半句才是本意。

```bash
python3 - <<'PY'
from pathlib import Path

p = Path("docs/superpowers/specs/2026-09-22-can-audio-layout-design.md")
s = p.read_text(encoding="utf-8")

anchor = "横幅（版本不符、没听到）留在卡片上方当横幅。\n"
assert s.count(anchor) == 1
note = """
**订正一处：上面两段里的「席位」指的是本机座舱读数，不是管制席位。** can-audio 的
`position_label`（`xpc/gui.py:532-535`）装的是经纬度、高度、地速、航向和应答机，
落地时它就是 can-voice 现成的 `cockpit.*` 那几个值，不要新造一个本机呼号读数。
msfs 那一步按同一条读。
"""
s = s.replace(anchor, anchor + note, 1)

anchor = "也就是它现在的行为。\n"
assert s.count(anchor) == 1
note = """
**再订正一处：「只留消息卡片」按「也就是它现在的行为」读。** 今天的精简留的是消息列表
加整条底栏，而屏幕上那颗 PTT 在 Wayland 上是唯一能发话的路径、手输频率框是观察员唯一的
调频手段，两样都不能跟着精简消失。所以精简 = 消息卡片 + 无线电那一行，只收起他机计数、
座舱读数和状态栏；msfs 那一步同此。
"""
s = s.replace(anchor, anchor + note, 1)

p.write_text(s, encoding="utf-8")
print("ok")
PY
```

- [ ] **Step 7: 跑门**

```bash
for a in controller atis xpc msfs; do (cd apps/$a && bun run build) || echo "$a FAILED"; done
cargo test -p can-voice-i18n
cargo fmt --all --check
(cd apps/xpc/src-tauri && cargo check --all-targets)
cargo clippy --workspace --all-targets -- -D warnings
```
四个端都要 build：`common.*.json` 是四端逐字节相同的一份，这一步动了它。
`the_common_dictionaries_are_identical_in_every_app` 会看八份是不是仍然一样。
动过 `src-tauri`（`lib.rs` 的注释、`tauri.conf.json`），所以后两条也要跑。

改过的应当只有这八份字典加 xpc 自己那几个文件，核一眼：

```bash
git status --short apps/controller apps/atis apps/msfs
```
只该看到六份 `common.{zh,en}.json`（controller、atis、msfs 各两份），别的都不该有。

- [ ] **Step 8: 提交**

```bash
git add apps/xpc/src-tauri/tauri.conf.json \
        apps/xpc/src-tauri/src/lib.rs \
        apps/xpc/src/App.vue \
        apps/controller/src/locales/common.zh.json \
        apps/controller/src/locales/common.en.json \
        apps/atis/src/locales/common.zh.json \
        apps/atis/src/locales/common.en.json \
        apps/xpc/src/locales/common.zh.json \
        apps/xpc/src/locales/common.en.json \
        apps/msfs/src/locales/common.zh.json \
        apps/msfs/src/locales/common.en.json \
        docs/manual-test.md \
        docs/superpowers/specs/2026-09-22-can-audio-layout-design.md
git commit -m "$(cat <<'MSG'
chore(xpc): take the new window geometry, and wire the last two menu items

The window becomes 980x660 with a 900x600 minimum. can-audio sets no minimum;
this one is deliberate, because the three-card middle row collapses without it.

COMPACT_MIN and COMPACT_SIZE keep their numbers. can-audio's pilot clients have
no compact mode to copy a figure from, and what compact keeps did not change:
the messages card plus the radio row's PTT, COM1 and the observer's frequency
box. The comment now says so.

Check-for-update remounts the shared banner and reports both the wait and the
empty result in the status bar. The new common key is worded "no update
available" rather than "you are up to date", because the check is declined
outright while the link is up. common.*.json is byte-identical across the four
apps, so it lands in eight files.

About reads app_version and log_file when it opens. WindowToggles keeps all
three buttons: the menu is built by a frontend call that can fail, and with the
settings button gone too there would be no way into settings at all.

The manual test's pilot section covers the menu, the cards, the radio row, the
status bar and the new geometry. Spec section 6 records that its 席位 cell is
the cockpit readout, and that compact keeps the radio row as well as the
messages card.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

**人工要看的**：这一任务做完，`docs/manual-test.md` 的 §3 就是验收单本身，整节走一遍。
最少要核这六条：

1. 窗口起来是 980×660，拖不到比 900×600 更小。
2. 六个菜单项都点得动；设置 → 语言切成 English，菜单栏当场变英文。
3. 顶栏「置顶」「精简」「设置」三颗钮都还在；设置从 文件 → 设置… 也进得去，
   **精简模式下只有菜单那条路，而它是通的**。
4. 帮助 → 检查更新：下线状态下点它，底栏先说「正在检查更新…」，再说「没有可用的更新。」
   或者上面冒出更新横幅。**不许点了什么都不发生。**
5. 帮助 → 关于：版本号和日志路径都在，正文是分行的。
6. 切精简：窗口约 460×340，消息和无线电两张卡片还在，PTT 按得动。

---
## Self-Review

### 1. spec §6 每一条落在哪个任务

| spec §6 的要求 | 任务 |
|---|---|
| 原生菜单：文件 → 飞行计划…、设置…、─、退出 | 3（Rust 建菜单）、4（前端喂标签、接飞行计划和设置） |
| 原生菜单：帮助 → 打开日志目录、检查更新、关于 | 3（打开日志目录在 Rust 就地做完）、9（检查更新、关于） |
| `PilotPanel.vue` 解体 | 4（飞行计划页）、5（本机页 + 删文件） |
| 飞行计划页变成 `FlightPlanDialog`，从菜单进 | 4 |
| 本机页变成设置对话框里的枢轴分页：音频 / 网络 / 他机 | 5 |
| 提示音开关和范围放到「他机」页 | 5 |
| 菜单项派发到已经存在的 Rust 命令 | 3（派发机制）、4 / 9（各项接上） |
| 连接卡片，整宽，网格 + 三行状态文案 + 观察员 + 记住密码 | 6 |
| 页头那几个 pill 变成连接卡片里的三行状态文案 | 6 |
| follow 和真实姓名留在网格里 | 6 |
| 会话区并进来，连接按钮翻成断开，Ident 在旁边 | 6 |
| 消息（3）‖ 附近管制（1）并排 | 7 |
| 消息卡片吃下底栏提上来的发送行 | 7 |
| 附近管制双击发消息 + 换行提示 | 7 |
| 他机表变成中间一行的第三张卡片（1） | 7 |
| 插件 pill 和两条横幅留在卡片上方 | 6 |
| 无线电卡片，最下，一行 | 8 |
| 座舱读数并入无线电那一行 | 8 |
| 底部 `StatusBar`，显示「就绪」和瞬时状态 | 8 |
| 几何 980×660 | 9 |
| 刻意保留 900×600 最小尺寸 | 9 |
| 精简模式只留消息卡片 | 9 |
| §3 的共用件（`StateToggle`、`StatusBar`） | 1 |
| §7 的 i18n 规矩（中英成对、不硬编码中文） | 2（加键）+ 每个任务的门 |

### 2. 占位符扫描

（装配完之后跑：搜 `TBD`、`TODO`、`稍后`、`类似 Task`、`适当`、`etc.`、`…以此类推`。）

### 3. 类型一致性

（装配完之后核：`MenuRequest` 的四个取值 ↔ 前端 `view.menu` 的 switch 分支；
`MenuLabels` 的八个驼峰字段 ↔ 前端 `set_menu` 的实参；`Panel` / `Modal` /
`StateToggle` / `StatusBar` 的 props 名 ↔ 各任务的调用处；新键名 ↔ 各任务的 `t()` 实参。）

# msfs 换上 can-audio 的窗口布局 — 实施计划（4 之 4）

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 `apps/msfs` 换成和 `apps/xpc` 一样的排布——原生菜单、四张带标题的卡片、
底部状态栏——并把 xpc 那边已经写好、和模拟器无关的几个文件**登记成共用件**，而不是
再抄一份会各自漂开的副本。

**Architecture:** 这一份和前三份最大的不同：**目标文件已经存在**。`apps/xpc/src/App.vue`
就是要的样子，而 `apps/msfs/src/App.vue` 在换布局之前和 xpc 换布局之前的那一份只差
**四处**。所以主体任务不是「照着规格重排一遍」，而是**把 xpc 那份拿过来，再把 msfs 的
四处差异重新贴回去**——目标是一个已经过审、已经跑过全套门的文件，不是一段描述。

**Tech Stack:** Vue 3.5 SFC + Vite 7 + TypeScript + Tailwind CSS v4（CSS 配置）；
Tauri v2（`tauri::menu`）；`crates/can-voice-i18n` 的字典测试是唯一的自动化护栏。

**Spec:** `docs/superpowers/specs/2026-09-22-can-audio-layout-design.md` §6——它把两个
飞行员端写成一份设计，「msfs 作为 xpc 的差量落地」。§3 的共用件、§7 的 i18n 规矩同样适用。

**Prior plans:** 计划 1（管制端）、计划 2（通播端）已在 `main`；计划 3（xpc）是 PR #114，
**本计划从它的分支上接着做**。计划 3 的执行记录在
`.superpowers/sdd/2026-09-22-can-audio-layout-3-xpc/progress.md`，里面的 R1–R15 条裁决
和最终复审的四条发现**对这一份同样有效**，不要重新论证一遍。

## 先读这两份，别凭记忆

- `.temp/msfs-map-for-plan-4.md` — msfs 和 xpc 的逐项差异地图。
- `apps/xpc/src/App.vue`、`apps/xpc/src/components/` — 这一份计划的**目标形态**。

## Global Constraints

每个任务的要求都隐含这一节。

- **不碰事件 API，不放宽 ACL。** `apps/msfs/src-tauri/capabilities/default.json` 和另外三个端
  一样只给了 `["updater:default"]`，全仓库没有一处用 `@tauri-apps/api/event`。菜单点一下
  要让前端开对话框，走**已有的 250ms 轮询**：Rust 存一个待办，`view()` 取走并清掉。
- **交给操作系统的路径不能来自前端。** `open_log_dir` **不收参数**，路径自己从
  `can_voice_log::path()` 算。照抄 xpc 的形状。
- **逐字节复制，不抽包。** 共用前端文件在各端一份逐字节相同的副本，清单在
  `crates/can-voice-i18n/tests/dictionaries.rs` 的 `SHARED_FRONTEND`。**先改源件、再 `cp`**，
  不要两边手打——手打会把全角标点敲成半角，而仓库里没有任何一条测试看标点。
- **界面代码里不许出现硬编码中文。** 每一句人看得见的话都走 `t("…")`，键名只能是字面量。
- **每个新键中英成对**，占位符一致，英文那份里不许有中文。
- **`common.{zh,en}.json` 是四个端逐字节相同的一份**（`the_common_dictionaries_are_identical_in_every_app`）。
  动它就是动八个文件，四个端都要 build。这一份计划**预计不需要动它**：`update.current`
  在计划 3 里已经加进四个端了。
- **调色板只从 CSS 自定义属性取**：`var(--can-off|on|active|muted|theme|idle|surface|window)`。
  卡片底色写 `bg-white`，**不写** `var(--can-surface)`——`.dark .bg-white` 已经映到它了，
  直接写变量会让浅色主题下的卡片是深的。
- **版本号不动**，四个端都在 `27.0.4`。
- **提交签名不能绕过。** 硬件密钥，每次提交都要等实体按键。不要 `--no-gpg-sign`，
  不要改 git 配置。**卡住就等**；如果是硬失败（`agent refused operation`），原样重试一次可以。
- **不要 `git add -A` 或 `git add .`**，只 `git add` 点名的路径。
- **提交正文由控制方写在文件里**，实现者用 `git commit -F <文件>`，**自己一个字都不要打**。
  实现者自己的系统提示里那一行 `Co-Authored-By:` 写的是别的模型，对这个仓库是错的。
- 临时文件放 `<项目>/.temp/`，不许用 `/tmp` 或 `$TMPDIR`。
- **每个任务的门**：`cd apps/msfs && bun run build`、`cargo test -p can-voice-i18n`、
  `cargo fmt --all --check`。动过共用文件的**四个端都要 build**。动过 `src-tauri` 的另加
  `cargo check --all-targets` 和 `cargo clippy --workspace --all-targets -- -D warnings`。
- **没有前端测试框架**，也没有 `lint` 脚本。所以每个任务的验证步骤里要写清楚**人工要看什么**。

## 计划 3 留下的、这一份必须照做的几条

不要重新论证，照做：

- **IDENT 只有一颗，在无线电那一行**（can-audio `xpc/gui.py:540-544`），连接卡片上没有。
  一直画着，`:disabled="!connected || busy"`，**不要用 `v-if`**。
- **TX / RX 用管制端那套三态**：有频率 `on`，正在收发 `active`，没有频率 `off`。
  红色（`muted`）只表示静音，不在这里用。
- **精简 = 消息卡片 + 无线电那一行 + 状态栏**，连接卡片收起来。代价是精简时连不上也断不开
  （「简」那颗钮一直在，退出精简就是一下）。状态栏在精简时**也在**，插槽里补上三格状态文案。
- **`SettingsDialog` 关闭前要先 blur**（计划 3 的最终复审发现的 Critical）。那个修复已经
  落在四个端上了，这一份**不要再动那个文件**。
- **`local.tab` 之类被这次改版孤立的键，由制造出孤儿的那个任务 grep 到零命中才删。**

---
## 主体策略：搬 xpc 的 `App.vue`，再贴回 msfs 的四处差异

**不要照着规格重排一遍 msfs 的 `App.vue`。** 换布局之前，msfs 的那一份和 xpc 的那一份
只差四处（`diff <(git show main:apps/xpc/src/App.vue) apps/msfs/src/App.vue` 自己看一眼，
三个 hunk，四十行上下）。xpc 那一份现在已经是想要的样子，而且过了全套门和一次整分支复审。

所以主体任务是：**把 `apps/xpc/src/App.vue` 复制过来，再把下面这四处贴回去。**
这比重排一遍安全得多——目标是一个已知good的文件，不是一段描述。

| # | msfs 和 xpc 的差异 | 在 xpc 新版里落在哪 |
|---|---|---|
| 1 | 模拟器叫 `MSFS`，不叫 `X-Plane` | **两处**：连接卡片那三格状态文案里一处，精简时状态栏插槽里一处。两处都要改 |
| 2 | 没有插件指示灯 | 卡片上方那一叠横幅里，把插件那一行灯整段删掉 |
| 3 | 没有 `plugin.version_mismatch` / `plugin.not_heard` 两条横幅 | 同上，整段删掉 |
| 4 | 有一条 xpc 没有的 **`sim_problem`** 琥珀色文案 | 接在插件那几段原来的位置上，理由见下 |
| 5 | `PilotSettings` 收的是 `:hangar`，不是 `:csl` | 模板最下面那一对 `<SettingsDialog>` 里 |

**`sim_problem` 要当横幅排，不能当胶囊排。** 它装的是 SimConnect 原样的英文 `detail`，
**长度没有上限**；而插件那盏灯是定宽的。把它塞进连接卡片那三格 `grid-cols-3` 里会把
那一行挤散。它接的就是插件那两条横幅腾出来的位置——那里本来就是给会换行的长句子用的。
**在非 Windows 上这是 msfs 屏幕上最要紧的一句话**（那里 `simconnect.unavailable` 恒真，
模拟器那盏灯按设计一直是灰的），所以它不许被精简模式藏掉，和 xpc 的插件横幅一样待遇。

## File Structure

| 文件 | 动作 | 说明 |
|---|---|---|
| `apps/msfs/src/components/{Panel,Modal,StateToggle,StatusBar,FlightPlanDialog}.vue` | 新增（`cp` 自 xpc） | 五个都登记进 `SHARED_FRONTEND` |
| `apps/msfs/src/components/PilotSettings.vue` | 新增 | msfs 自己的：机库不是 CSL，没有显示距离，没有安装向导 |
| `apps/msfs/src/components/PilotPanel.vue` | **删除** | |
| `apps/msfs/src/App.vue` | 整份换掉 | 见上面那张差异表 |
| `apps/msfs/src/types.ts` | 改 | 加 `MenuRequest`、`View.menu` |
| `apps/msfs/src/locales/app.{zh,en}.json` | 改 | 新键；`hangar.*` 顶 `csl.*` |
| `apps/msfs/src-tauri/src/lib.rs` | 改 | 菜单、`set_menu`、`open_log_dir`、`app_version`、`View.menu` |
| `apps/msfs/src-tauri/tauri.conf.json` | 改 | 980×660，最小 900×600。**`devUrl` 是 4333，不要抄成 4332** |
| `crates/can-voice-i18n/tests/dictionaries.rs` | 改 | 五行 |
| `docs/manual-test.md` | 改 | msfs 那一节 |

**`FlightPlanDialog.vue` 和 `Panel.vue` 这一次变成共用件，不是各抄一份。**
已经核过：`FlightPlanDialog.vue` 里没有一处 xpc 专有的东西（没有 `csl` / `plugin` /
`xplane`），它只用 `plan.*` 键和 `FlightPlan` / `Settings` / `emptyFlightPlan`；而
msfs 的 `plan.*` 键集合和 xpc **完全一样**，`FlightPlan` 结构体也**逐字节相同**。
两份会漂开的副本不如一份被测试钉住的共用件。

`PilotSettings.vue` **不能**共用：msfs 那一份是机库（四种状态、不显示路径）、没有显示距离、
没有安装向导。这三条在 `.temp/msfs-map-for-plan-4.md` §7 里有细节。

## 这一份里**不做**的事

- **不动 `SettingsDialog.vue`**。计划 3 的最终复审已经把「关闭前先 blur」修进四个端了。
- **不动 `common.{zh,en}.json`**。`update.current` 计划 3 已经加进四个端。
- **不修 `apps/controller/src/components/SettingsPanel.vue` 那个同款的录制没取消的洞。**
  它是另一条条目，不夹带在这次移植里。
- **不给 msfs 加显示距离**。msfs 根本没有 `set_traffic_range` 这个命令，
  也没有 `Settings.traffic_range_nm`。所以设置里的「他机」页在 msfs 上只有注入开关
  和机库目录——**这是真实差异，不是漏了**，任务 4 要把这句话写进注释里。

---
### Task 1: 把五个共用件搬进 msfs，并登记进 `SHARED_FRONTEND`

**Files:**
- Modify: `apps/controller/src/components/StateToggle.vue`（只改文档注释里那一句"用到它的客户端"）
- Modify: `apps/controller/src/components/StatusBar.vue`（同上）
- Modify: `apps/atis/src/components/Modal.vue`（同上）
- Modify: `apps/xpc/src/components/Panel.vue`（把"只有 xpc 有这个文件"那一段换掉）
- Modify: `apps/xpc/src/components/FlightPlanDialog.vue`（同上）
- Modify: `apps/xpc/src/components/StateToggle.vue`、`StatusBar.vue`、`Modal.vue`（`cp` 覆盖，只跟注释变化）
- Create: `apps/msfs/src/components/StateToggle.vue`（`cp` 来的）
- Create: `apps/msfs/src/components/StatusBar.vue`（`cp` 来的）
- Create: `apps/msfs/src/components/Modal.vue`（`cp` 来的）
- Create: `apps/msfs/src/components/Panel.vue`（`cp` 来的）
- Create: `apps/msfs/src/components/FlightPlanDialog.vue`（`cp` 来的）
- Modify: `crates/can-voice-i18n/tests/dictionaries.rs`（`SHARED_FRONTEND` 五行：三行改、两行新增）

**Interfaces:**
- Consumes: 无。这是这一份计划的第一个任务。
- Produces: msfs 侧可用的
  `<StateToggle :label :state :width :height :disabled @press>`、
  `<StatusBar :talking :status>`（`duty` / `dutyOn` / `pttTitle` 飞行员端不传）、
  `<Modal :open :title :width @close>`、
  `<Panel :title>`、
  `<FlightPlanDialog :open :observer @close>`。
  后面的 `App.vue` 任务和 `PilotSettings.vue` 任务都靠这一批。

**`FlightPlanDialog.vue` 能共用，是已经核过的事，不要再论证一遍。** 三件事都已核实：
它里面没有一处 xpc 专有的东西（`grep -c "csl\|plugin\|xplane\|X-Plane"` 得 0）；
它只读 `plan.*` 这一组键，而 msfs 的 `plan` 命名空间和 xpc 的中英两份都逐字节等价；
`FlightPlan` 接口和 `emptyFlightPlan()` 两边也逐字节相同。复核用这三条：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -c "csl\|plugin\|xplane\|X-Plane" apps/xpc/src/components/FlightPlanDialog.vue   # 0
for l in zh en; do for a in xpc msfs; do python3 -c "import json;d=json.load(open('apps/$a/src/locales/app.$l.json',encoding='utf-8'));print(json.dumps(d['plan'],ensure_ascii=False,sort_keys=True))"; done | uniq -c; done   # 两行都是 "2"
diff <(sed -n '/interface FlightPlan/,/^}/p' apps/xpc/src/types.ts) <(sed -n '/interface FlightPlan/,/^}/p' apps/msfs/src/types.ts)   # 无输出
```

它调的两个命令 `file_flight_plan` 和 `settings` 在 msfs 的 `generate_handler!` 里都已经有了，
所以它一复制过去就能编译，哪怕这一任务里还没有人 import 它——
`apps/msfs/tsconfig.json` 的 `include` 是 `src/**/*.vue`，`vue-tsc --noEmit` 会把它一起看。

**注释里那句"msfs 在计划 4 里接上"在五份里全都过期了。** 而共用件必须逐字节相同，
所以顺序只能是**先改源件、再 `cp` 出去**，绝不能两边手打——手打会把全角标点敲成半角，
仓库里没有任何一条测试看标点。源件是哪一份看 `SHARED_FRONTEND` 里 `owners[0]`：
`StateToggle.vue` / `StatusBar.vue` 是 controller，`Modal.vue` 是 atis，
`Panel.vue` / `FlightPlanDialog.vue` 现在只有 xpc 一份。

- [ ] **Step 1: 用写文件工具写 `.temp/plan4-comments.py`**

**用写文件工具写，不要用 shell heredoc**（里面有反引号和中文引号），
也**不要用 `perl -CSD -pi -e`**（`-e` 里的字面量按字节算，一次都不匹配还安静退出 0）。

```python
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

EDITS = [
    (
        "apps/controller/src/components/StateToggle.vue",
        " * 用到它的客户端：controller、xpc。msfs 的 TX / RX 色块在计划 4 里接上。\n"
        " * 逐字节相同的一份。\n",
        " * 用到它的客户端：controller、xpc、msfs。逐字节相同的一份。\n",
    ),
    (
        "apps/controller/src/components/StatusBar.vue",
        " * 用到它的客户端：controller、xpc。msfs 的状态栏在计划 4 里接上。\n"
        " * 逐字节相同的一份。\n",
        " * 用到它的客户端：controller、xpc、msfs。逐字节相同的一份。\n",
    ),
    (
        "apps/atis/src/components/Modal.vue",
        " * 用到它的客户端：atis、xpc。**登记在 `SHARED_FRONTEND` 上**——两份不登记的副本\n"
        " * 会无声地漂开。msfs 的飞行计划对话框在计划 4 里接上。\n",
        " * 用到它的客户端：atis、xpc、msfs。**登记在 `SHARED_FRONTEND` 上**——不登记的\n"
        " * 副本会无声地漂开。\n",
    ),
    (
        "apps/xpc/src/components/Panel.vue",
        " * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，\n"
        " * 那时候把它登记进去**——两份不登记的副本会无声地漂开。\n",
        " * 用到它的客户端：xpc、msfs。**登记在 `SHARED_FRONTEND` 上**——不登记的副本\n"
        " * 会无声地漂开。\n",
    ),
    (
        "apps/xpc/src/components/FlightPlanDialog.vue",
        " * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，\n"
        " * 那时候把它登记进去**——两份不登记的副本会无声地漂开。\n",
        " * 用到它的客户端：xpc、msfs。**登记在 `SHARED_FRONTEND` 上**——不登记的副本\n"
        " * 会无声地漂开。\n",
    ),
]

for rel, old, new in EDITS:
    p = ROOT / rel
    s = p.read_text(encoding="utf-8")
    hits = s.count(old)
    assert hits == 1, f"{rel}: 命中 {hits} 次，期望 1"
    p.write_text(s.replace(old, new), encoding="utf-8")
    print("ok", rel)
```

- [ ] **Step 2: 跑它，五行都要打出 `ok`**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
python3 .temp/plan4-comments.py
```

任何一行 `AssertionError` 就停下来，按内容重新找锚点，**不要按行号找**。

- [ ] **Step 3: 复制八份出去**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cp apps/controller/src/components/StateToggle.vue apps/xpc/src/components/StateToggle.vue
cp apps/controller/src/components/StateToggle.vue apps/msfs/src/components/StateToggle.vue
cp apps/controller/src/components/StatusBar.vue   apps/xpc/src/components/StatusBar.vue
cp apps/controller/src/components/StatusBar.vue   apps/msfs/src/components/StatusBar.vue
cp apps/atis/src/components/Modal.vue             apps/xpc/src/components/Modal.vue
cp apps/atis/src/components/Modal.vue             apps/msfs/src/components/Modal.vue
cp apps/xpc/src/components/Panel.vue              apps/msfs/src/components/Panel.vue
cp apps/xpc/src/components/FlightPlanDialog.vue   apps/msfs/src/components/FlightPlanDialog.vue
```

- [ ] **Step 4: 改 `SHARED_FRONTEND` 的五行**

`crates/can-voice-i18n/tests/dictionaries.rs` 里那张表**按路径字母序排**，所以两行新增的
位置是定死的：`components/FlightPlanDialog.vue` 排在 `components/ControllerList.vue` 和
`components/LogPanel.vue` 之间，`components/Panel.vue` 排在 `components/Modal.vue` 和
`components/SettingsCommon.vue` 之间。

三行改：

```rust
    ("components/Modal.vue", &["atis", "xpc"]),
    ("components/StateToggle.vue", &["controller", "xpc"]),
    ("components/StatusBar.vue", &["controller", "xpc"]),
```

分别改成：

```rust
    ("components/Modal.vue", &["atis", "xpc", "msfs"]),
    ("components/StateToggle.vue", &["controller", "xpc", "msfs"]),
    ("components/StatusBar.vue", &["controller", "xpc", "msfs"]),
```

两行新增：

```rust
    ("components/FlightPlanDialog.vue", &["xpc", "msfs"]),
```
```rust
    ("components/Panel.vue", &["xpc", "msfs"]),
```

五行都在 100 列以内，`cargo fmt` 不会把它们拆成多行；拆了就是抄错了。

- [ ] **Step 5: 跑门**

动过共用文件，**四个端都要 build**。

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
(cd apps/controller && bun run build)
(cd apps/atis       && bun run build)
(cd apps/xpc        && bun run build)
(cd apps/msfs       && bun run build)
cargo test -p can-voice-i18n
cargo fmt --all --check
```

`shared_frontend_files_are_identical_in_every_app_that_carries_them` 是这一步的主角：
它既比对每一份的字节，也断言"存在但没登记"的文件不存在。

- [ ] **Step 6: 删掉临时脚本**

```bash
rm /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/.temp/plan4-comments.py
```

- [ ] **Step 7: 提交**

```bash
git add apps/controller/src/components/StateToggle.vue \
        apps/controller/src/components/StatusBar.vue \
        apps/atis/src/components/Modal.vue \
        apps/xpc/src/components/StateToggle.vue \
        apps/xpc/src/components/StatusBar.vue \
        apps/xpc/src/components/Modal.vue \
        apps/xpc/src/components/Panel.vue \
        apps/xpc/src/components/FlightPlanDialog.vue \
        apps/msfs/src/components/StateToggle.vue \
        apps/msfs/src/components/StatusBar.vue \
        apps/msfs/src/components/Modal.vue \
        apps/msfs/src/components/Panel.vue \
        apps/msfs/src/components/FlightPlanDialog.vue \
        crates/can-voice-i18n/tests/dictionaries.rs
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-1.txt
```

**人工要看的**：这一任务做完之后，屏幕上**什么都不该变**——五个文件还没有人 import。
四个端各开一次确认没有回归即可：管制端的 TX / RX 色块和底栏还在、通播端的设置对话框还能开，
xpc 的布局和菜单和上一个分支末尾一模一样，msfs 还是老样子。

---

### Task 2: 给 msfs 补上新布局要的字典键

**Files:**
- Modify: `apps/msfs/src/locales/app.zh.json`（六个新命名空间 + `local` 三条 + `problem.menu` + `status.ready`）
- Modify: `apps/msfs/src/locales/app.en.json`（同上，英文）

**Interfaces:**
- Consumes: 无。
- Produces: `menu.{file,flight_plan,settings,quit,help,open_log,update,about}`、
  `connect.title`、`messages.title`、`controllers.hint`、
  `radio.{title,com1,com1_none,traffic}`、`about.{title,body,no_log}`、
  `local.{audio,network,traffic}`、`problem.menu`、`status.ready`——
  一共 23 个键，中英成对。
  后面的 `App.vue` 任务、`PilotSettings.vue` 任务和 Rust 菜单任务都靠这一批；
  `problem.menu` 尤其是 Task 3 的前置：`set_menu` 用 `Message::new("problem.menu")`，
  键不在字典里 `every_key_rust_sends_to_the_interface_exists` 就红。

**这 23 个键是算出来的，不是抄来的。** 口径是「计划 3 给 xpc 加了什么」减去「msfs 已经有了什么」，
算式跑过，结果是 xpc 在计划 3 里加的那 23 个键 msfs **一个都没有**，所以整套照搬，
只有 `about.body` 一条要按 msfs 重写。复核用：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
diff <(git show main:apps/xpc/src/locales/app.zh.json) apps/xpc/src/locales/app.zh.json
```

**`local.traffic` 要，不要犹豫。** 它是设置枢轴第三页的页签名「他机」，
而那一页在 msfs 上**仍然存在**——只是里面只有注入开关（`local.inject`）和机库目录
（`hangar.dir`），没有 xpc 的显示距离（msfs 根本没有 `set_traffic_range`，
也没有 `Settings.traffic_range_nm`）。页在，页签名就要。

**反过来，xpc 有而 msfs 不要的有两个**，别顺手抄进来：`status.plugin`（msfs 没有插件指示灯）
和 `local.{range,range_note}`（msfs 没有显示距离）。抄进去不会让任何一条测试变红
——仓库里没有"没人用的键要删掉"这条测试——但那正是它危险的地方。

**这一任务只加，不删。** `lists.messages`、`local.tab`、`observer.frequency` 三个键
在新布局下会变成孤儿，但**现在还有人在用**，这一任务里删掉会让
`every_key_the_interface_asks_for_exists` 当场红：

| 键 | 现在谁在用 | 谁把它变成孤儿 | 那个任务要做什么 |
|---|---|---|---|
| `lists.messages` | `apps/msfs/src/App.vue`（消息列表上方那行小字） | 换 `App.vue` 的主体任务——xpc 那份用 `Panel` 的标题 `messages.title` 顶掉了它 | 换完之后 `grep -rn '"lists.messages"\|lists\.messages' apps/msfs/src/`，**零命中**再从中英两份里删掉 |
| `observer.frequency` | `apps/msfs/src/App.vue:309`（观察员那一格的 label） | 同上——xpc 那份把 label 去掉了，只留 `observer.frequency_placeholder` / `_tip` | 换完之后 `grep -rn 't("observer.frequency")' apps/msfs/src/`，**零命中**再删。**只删 `observer.frequency` 这一个键**，`frequency_placeholder` / `frequency_tip` 留着，它们还在用 |
| `local.tab` | `apps/msfs/src/components/PilotPanel.vue:203` | 删 `PilotPanel.vue`、建 `PilotSettings.vue` 的那个任务——三页枢轴用 `local.{audio,network,traffic}` 顶掉了它 | 删完之后 `grep -rn 'local\.tab' apps/msfs/src/`，**零命中**再删 |

三条都和 xpc 在计划 3 里的下场一样，不是 msfs 的特例。

**`common.{zh,en}.json` 这一任务不碰。** 新键一条都不落在 common 命名空间里；
`log.no_file` 和 `error.update.{no_opener,open_failed}`（Task 3 的 `open_log_dir` 要用）
msfs 的 common 里**已经有了**，核过。动 common 就是动八个文件、四个端都要 build，
这一份计划不需要。

- [ ] **Step 1: 用写文件工具写 `.temp/plan4-keys.py`**

同样：**用写文件工具写，不要用 shell heredoc，不要用 `perl -CSD -pi -e`。**
脚本做文本插入而不是 `json.load` + `json.dump`——后者会把
`"connect": { "title": "连接" }` 这类单行写法展平，整个文件变成一坨无关的 diff。

三个锚点都在原文件里唯一，脚本每一步都断言命中数。

```python
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

ZH_NAMESPACES = r"""  "menu": {
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
  "controllers": { "hint": "点一个席位，把它填进收件人。" },
  "radio": {
    "title": "无线电",
    "com1": "COM1  {frequency}",
    "com1_none": "COM1  ---.---",
    "traffic": "他机 {count}"
  },
  "about": {
    "title": "关于",
    "body": "{name} {version}\n\nCerulean Aviation Network 的 MSFS 飞行员客户端。\n语音走 Mumble，网络走 FSD，飞行数据从 MSFS 的 SimConnect 取。\n\n日志：{log}",
    "no_log": "（未写入文件）"
  },
"""

EN_NAMESPACES = r"""  "menu": {
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
  "controllers": { "hint": "Click a position to put it in the recipient box." },
  "radio": {
    "title": "Radio",
    "com1": "COM1  {frequency}",
    "com1_none": "COM1  ---.---",
    "traffic": "Traffic {count}"
  },
  "about": {
    "title": "About",
    "body": "{name} {version}\n\nThe MSFS pilot client for the Cerulean Aviation Network.\nVoice over Mumble, network over FSD, flight data over SimConnect.\n\nLog: {log}",
    "no_log": "(not written to a file)"
  },
"""

EDITS = {
    "apps/msfs/src/locales/app.zh.json": [
        # 六个新命名空间，插在 "status" 之前，和 xpc 的排法一致。
        ('  "status": {\n', ZH_NAMESPACES + '  "status": {\n'),
        # status.ready
        (
            '    "no_mouse_ptt": "本系统不支持鼠标侧键作 PTT"\n',
            '    "no_mouse_ptt": "本系统不支持鼠标侧键作 PTT",\n    "ready": "就绪"\n',
        ),
        # problem.menu
        (
            '    "offline": "还没上线"\n  },\n  "app": {\n',
            '    "offline": "还没上线",\n    "menu": "菜单建不起来：{detail}"\n  },\n  "app": {\n',
        ),
        # local 的三个页签名，排在 "tab" 之前。"tab" 由后面那个任务删。
        (
            '  "local": {\n    "tab": "本机",\n',
            '  "local": {\n    "audio": "音频",\n    "network": "网络",\n'
            '    "traffic": "他机",\n    "tab": "本机",\n',
        ),
    ],
    "apps/msfs/src/locales/app.en.json": [
        ('  "status": {\n', EN_NAMESPACES + '  "status": {\n'),
        (
            '    "no_mouse_ptt": "Mouse side buttons cannot be used for PTT on this system"\n',
            '    "no_mouse_ptt": "Mouse side buttons cannot be used for PTT on this system",\n'
            '    "ready": "Ready"\n',
        ),
        (
            '    "offline": "Not connected yet"\n  },\n  "app": {\n',
            '    "offline": "Not connected yet",\n'
            '    "menu": "Could not build the menu: {detail}"\n  },\n  "app": {\n',
        ),
        (
            '  "local": {\n    "tab": "Local setup",\n',
            '  "local": {\n    "audio": "Audio",\n    "network": "Network",\n'
            '    "traffic": "Traffic",\n    "tab": "Local setup",\n',
        ),
    ],
}

for rel, edits in EDITS.items():
    p = ROOT / rel
    s = p.read_text(encoding="utf-8")
    for old, new in edits:
        hits = s.count(old)
        assert hits == 1, f"{rel}: 命中 {hits} 次，期望 1：{old[:40]!r}"
        s = s.replace(old, new)
    p.write_text(s, encoding="utf-8")
    print("ok", rel)
```

注意三处容易抄错的地方：`radio.com1` 里 `COM1` 和 `{frequency}` 之间是**两个空格**；
`about.body` 里的 `\n` 是 JSON 字符串里的字面转义（所以那两段用 `r"""…"""`，
不能写成普通三引号）；中文里的省略号是全角 `…`，不是三个点。

- [ ] **Step 2: 跑它，两行都要打出 `ok`，并确认 JSON 还是合法的**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
python3 .temp/plan4-keys.py
python3 -c "import json;[json.load(open(f'apps/msfs/src/locales/app.{l}.json',encoding='utf-8')) for l in ('zh','en')];print('json ok')"
```

- [ ] **Step 3: 核一遍两边键集一致、23 个键都在**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
python3 - <<'EOF'
import json
def flat(o,p=""):
    out={}
    for k,v in o.items():
        key=f"{p}.{k}" if p else k
        out.update(flat(v,key)) if isinstance(v,dict) else out.update({key:v})
    return out
zh=flat(json.load(open("apps/msfs/src/locales/app.zh.json",encoding="utf-8")))
en=flat(json.load(open("apps/msfs/src/locales/app.en.json",encoding="utf-8")))
assert set(zh)==set(en), set(zh)^set(en)
want="""about.body about.no_log about.title connect.title controllers.hint
local.audio local.network local.traffic menu.about menu.file menu.flight_plan
menu.help menu.open_log menu.quit menu.settings menu.update messages.title
problem.menu radio.com1 radio.com1_none radio.title radio.traffic status.ready""".split()
assert len(want)==23
missing=[k for k in want if k not in zh]
assert not missing, missing
extra=[k for k in ("status.plugin","local.range","local.range_note") if k in zh]
assert not extra, extra
print("23 keys ok")
EOF
```

- [ ] **Step 4: 跑门**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cargo test -p can-voice-i18n
(cd apps/msfs && bun run build)
```

看住这四条：`both_languages_have_the_same_keys_and_none_is_empty`、
`placeholders_agree_between_the_languages`（`about.body` 的
`{name}` `{version}` `{log}` 三个占位符中英必须一致）、`english_has_no_chinese_in_it`、
`an_app_dictionary_does_not_reuse_a_common_namespace`。

- [ ] **Step 5: 删掉临时脚本**

```bash
rm /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/.temp/plan4-keys.py
```

- [ ] **Step 6: 提交**

```bash
git add apps/msfs/src/locales/app.zh.json apps/msfs/src/locales/app.en.json
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-2.txt
```

**人工要看的**：这一任务做完之后，msfs 界面上**仍然什么都不该变**——新键还没有人读。
开一次 msfs，中英各切一遍，确认没有哪一处变成了键名原文（那说明 JSON 插坏了，
`t()` 找不到键就回落成键名）。

---

### Task 3: msfs 的原生菜单（Rust 侧）

**Files:**
- Modify: `apps/msfs/src-tauri/src/lib.rs`（`MenuRequest`、六个 `MENU_*` 常量、
  `App.menu` 字段、`View.menu` 字段、`build_view` 里的 `.take()`、
  `open_log_dir` / `open_log_dir_inner`、`app_version`、`MenuLabels`、`set_menu`、
  `on_menu_event`、`generate_handler!` 三行、一条测试）

**Interfaces:**
- Consumes: Task 2 的 `problem.menu`（`set_menu` 失败时 `Message::new("problem.menu")`）。
  `log.no_file` 和 `error.update.{no_opener,open_failed}` 已经在 msfs 的
  `common.{zh,en}.json` 里，核过，不用加。
- Produces: 三个命令 `set_menu(labels: MenuLabels)`、`open_log_dir()`、`app_version() -> &str`，
  以及 `View.menu: Option<MenuRequest>`（序列化成 `"flight_plan"` / `"settings"` /
  `"update"` / `"about"`，**读一次就没了**）。后面的 `types.ts` 任务和 `App.vue`
  任务靠这三样。

**这一份是 `apps/xpc/src-tauri/src/lib.rs` 的镜像，照抄，不要重新设计。**
计划 3 写的时候，这里每一个 tauri API 都对着 `tauri 2.11.5`（四个端各自的
`src-tauri/Cargo.lock` 锁的版本）和 `muda 0.19.3` 核过——`SubmenuBuilder::new`、
`.text(id, label)`、`.separator()`、`MenuBuilder::items`、`AppHandle::set_menu`、
`Builder::on_menu_event`、`MenuEvent::id()` 的签名都是核过的。**照着写，不要再去查一遍。**

四条约束跟着一起抄过来：

- **不放宽 ACL。** `apps/msfs/src-tauri/capabilities/default.json` 只给
  `["updater:default"]`，这一任务**一个字都不改它**。菜单点一下要让前端开对话框，
  走已有的 250 ms 轮询：Rust 存一个待办，`view()` 取走并清掉。
- **`open_log_dir` 不收参数。** 路径自己从 `can_voice_log::path()` 算。
  收一个 `String` 再 spawn，等于把任意路径的执行权交给网页那一侧，而
  `can_voice_update::open_in_browser` 只放行 https 就是为了不做这件事。
- **`app_version` 是自定义命令，不是 `@tauri-apps/api/app` 的 `getVersion`。**
  后者是插件命令，要 `core:app` 权限，ACL 没给——计划 1 在这上面栽过一次，
  它在 `onMounted` 里静默 reject，把后面的 `setInterval` 和 PTT 绑定全带走了。
- **`can_voice_update::open_folder(&Path) -> Result<(), Message>` 已经有了**，计划 3 加在共用 crate 里的。
  msfs 直接调。`apps/msfs/src-tauri/Cargo.toml` 的 `[dependencies]` 里
  `can-voice-update` 和 `can-voice-log` **两条都已经在了**，核过，这一任务不动 `Cargo.toml`。

`use tauri::Manager;`（`handle.get_webview_window` / `handle.state::<App>()` 要它）
在 `apps/msfs/src-tauri/src/lib.rs` 里**已经有了**，不用再加 `use`。

**行号会过期，按内容找锚点。** 下面每一步都给了唯一的锚点文本，动手前先
`grep -c` 确认是 1。

- [ ] **Step 1: `MenuRequest` 和六个 `MENU_*` 常量**

插在 `pub struct View` 那一段之前。锚点（唯一）：

```rust
/// 界面读的一份快照。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct View {
```

在它上面插入：

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

const MENU_FLIGHT_PLAN: &str = "flight_plan";
const MENU_SETTINGS: &str = "settings";
const MENU_QUIT: &str = "quit";
const MENU_OPEN_LOG: &str = "open_log";
const MENU_UPDATE: &str = "update";
const MENU_ABOUT: &str = "about";

```

- [ ] **Step 2: `View.menu` 字段**

`View` 的最后一个字段是 `pub observer: Option<ObserverView>,`（全文件唯一）。
把它那一行连同后面的 `}` 改成：

```rust
    /// 以观察员身份连着时的状况；没连、或者正常上着网是 `None`。
    pub observer: Option<ObserverView>,
    /// 菜单上刚点的那一项。**读一次就没了**——见 `build_view`。
    pub menu: Option<MenuRequest>,
}
```

（前一行的注释原文就是这一句，一起贴出来是为了让锚点唯一。）

- [ ] **Step 3: `App` 上的待办格子**

`App` 结构体的最后一个字段是（唯一）：

```rust
    pump: Mutex<Option<tokio::task::AbortHandle>>,
```

在它下面加：

```rust
    /// 菜单上刚点的那一项，等着前端下一拍取走。**不走 `emit`**：ACL 没给事件权限，
    /// 而且 `.setup()` 跑的时候 webview 还没加载完自己的包，tauri 不会为还没起来的
    /// 页面补发事件。
    menu: Mutex<Option<MenuRequest>>,
```

`App::new()` 里，锚点（唯一）：

```rust
            pump: Mutex::new(None),
```

改成：

```rust
            pump: Mutex::new(None),
            menu: Mutex::new(None),
```

- [ ] **Step 4: `build_view` 里取走并清掉**

`build_view` 结尾的 `View { … }` 字面量里，最后一项是那个 `observer:` 闭包。
锚点（唯一）：

```rust
        observer: app.observing().map(|follow| {
            let manual = app.manual_frequency();
            ObserverView {
                follow,
                frequency: can_voice_app::observer::frequency_for(manual, com1),
                manual: manual.is_some(),
            }
        }),
    }
}
```

改成：

```rust
        observer: app.observing().map(|follow| {
            let manual = app.manual_frequency();
            ObserverView {
                follow,
                frequency: can_voice_app::observer::frequency_for(manual, com1),
                manual: manual.is_some(),
            }
        }),
        // **取走并清掉。** 留着的话前端每一拍都会重新开一次对话框——250 ms 一次，
        // 关都关不掉。
        menu: app.menu.lock().expect("menu").take(),
    }
}
```

- [ ] **Step 5: `open_log_dir` 和 `app_version`**

接在 `log_file` 之后。锚点（唯一）：

```rust
/// 当前这份日志在哪。界面上显示给用户，让他知道要发的是哪个文件。
#[tauri::command]
fn log_file() -> Option<String> {
    can_voice_log::path().map(|p| p.display().to_string())
}
```

在它下面插入：

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

`open_log_dir_inner` 拆出来是**必要的**，不是风格：`on_menu_event` 的闭包里也要调它，
而那里调不了 `#[tauri::command]` 包出来的那一层。

- [ ] **Step 6: `MenuLabels` 和 `set_menu`**

接在 `send_log` 之后、`// ——— 设置对话框、置顶、精简（#45）———` 之前。
锚点（唯一）：

```rust
// ——— 设置对话框、置顶、精简（#45）———
```

在它上面插入：

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

**`#[serde(rename_all = "camelCase")]` 不能省。** tauri 把 JS 那一侧的参数名按
camelCase 传过来，字段是 `flight_plan` / `open_log`，前端传的是 `flightPlan` / `openLog`；
少了这一行，`set_menu` 每次都以"缺字段"失败，而界面上只会多一条
`problem.menu` 之外的反序列化报错。

- [ ] **Step 7: `on_menu_event`**

`run()` 里，锚点（唯一）：

```rust
    builder
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
```

改成：

```rust
    builder
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
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
```

那条 `tracing::warn!` 是**英文**，和仓库的规矩一致：界面文字中文，日志英文。

- [ ] **Step 8: 注册三个命令**

`generate_handler!` 里第一项是 `log_file,`（唯一）。改成：

```rust
        .invoke_handler(tauri::generate_handler![
            log_file,
            open_log_dir,
            app_version,
            set_menu,
```

- [ ] **Step 9: 「取走就没了」那条测试**

msfs 的测试模块**和 xpc 用同一个套路**：先起一个 tokio runtime 再 `App::new()`
（`App::new()` 会 `SimLink::spawn()`，没有 runtime 就 panic）。
**照这个写，不要用 `App::default()`，也不要省掉 runtime 那两行。**

锚点（唯一）：

```rust
    use can_voice_fsd::pilot::XpdrMode;
```

在它**上面**插入：

```rust
    /// 菜单那一项是**取走就没了**。留着的话，前端每一拍都会重新开一次对话框——
    /// 250 ms 一次，关都关不掉。
    #[test]
    fn a_menu_request_is_taken_once() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let _guard = rt.enter();
        let app = App::new();
        *app.menu.lock().expect("menu") = Some(MenuRequest::Settings);
        assert_eq!(build_view(&app).menu, Some(MenuRequest::Settings));
        assert_eq!(build_view(&app).menu, None);
    }

```

- [ ] **Step 10: 确认没动 ACL**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git diff --name-only | grep -c capabilities/default.json   # 必须是 0
cat apps/msfs/src-tauri/capabilities/default.json          # permissions 仍是 ["updater:default"]
```

- [ ] **Step 11: 跑门**

动过 `src-tauri`，所以比别的任务多两条。

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs/src-tauri
cargo fmt --all --check
cargo check --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cargo test -p can-voice-i18n
cargo fmt --all --check
(cd apps/msfs && bun run build)
```

`apps/*` 不在根 workspace 里（根 `Cargo.toml` 的 `exclude`），所以
`cargo fmt --all --check` 要在 `apps/msfs/src-tauri` 里**再跑一遍**，
只在仓库根跑是看不到这个文件的。

`cargo test` 要看见 `a_menu_request_is_taken_once ... ok`。
`cargo test -p can-voice-i18n` 这一步看的是
`every_key_rust_sends_to_the_interface_exists`——它会扫出新加的
`Message::new("problem.menu")` 和两处 `Message::new("log.no_file")`，
三个键都得在 msfs 的字典里（Task 2 加了前者，后者本来就在 common 里）。

- [ ] **Step 12: 提交**

```bash
git add apps/msfs/src-tauri/src/lib.rs
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-3.txt
```

**人工要看的**：这一任务做完之后 `bun run tauri dev` 起 msfs，**窗口顶上还是没有菜单**
——标签要前端调 `set_menu` 才有，而那是后面 `App.vue` 任务的事。
这一任务能人工核的是它**没有把别的东西弄坏**：窗口照常出来、连得上、
文字消息收发正常、设置对话框能开、PTT 还能按。
菜单本身的人工验收（中英切换当场换词、「文件 → 退出」关窗、
「帮助 → 打开日志目录」在访达/资源管理器里打开日志所在的文件夹）留到接线那一任务。

---
### Task 4: msfs 的 `PilotSettings.vue`，`PilotPanel.vue` 删掉

**Files:**
- Create: `apps/msfs/src/components/PilotSettings.vue`（音频 / 网络 / 他机三页，装进 `SettingsDialog` 的 `<slot />`）
- Delete: `apps/msfs/src/components/PilotPanel.vue`
- Modify: `apps/msfs/src/App.vue`（**只改三处**：换导入、删掉 `<PilotPanel>` 那一行、`SettingsDialog` 带上插槽。任务 5 会把整份换掉，这里只是让这一拍的门是绿的）
- Modify: `apps/msfs/src/locales/app.zh.json` / `app.en.json`（`local.tab` 零引用后删掉）
- Modify: `apps/xpc/src/components/PilotSettings.vue`（**只改头部注释**，代码一行不动）

**Interfaces:**
- Consumes: 任务 1 搬进 msfs 的 `Modal.vue` / `FlightPlanDialog.vue` 等共用件；树上现成的 `SettingsDialog.vue`（`defineProps<{ open: boolean }>()`，`SettingsCommon` 之后一个 `<slot />`）和 `LogPanel.vue`；任务 2 加的 `local.audio` / `local.network` / `local.traffic` 三个页签键。
- Produces: `PilotSettings.vue` — `defineProps<{ cid?: string; hangar?: HangarView }>()`，无 emits。**任务 5 把 `<SettingsDialog>…<PilotSettings :cid="cid" :hangar="view?.hangar" /></SettingsDialog>` 这一对原样带过去**，不要在整份替换时退回自闭合写法。
- Produces（给任务 5 的一条硬要求）：机库扫描的「扫描中」和「扫空了」两种状态要出现在对话框**外面**，具体落点和理由见下面那一段《机库扫描是阻塞的》。**实现在任务 5 的 Step 7**，因为 `App.vue` 整份是任务 5 的。

#### 源件和目标件：读哪两个文件

`apps/xpc/src/components/PilotSettings.vue`（363 行）是**目标形态**，`apps/msfs/src/components/PilotPanel.vue`（381 行）是**现状**。两边的音频页、PTT、试听、设备轮询逐字一样——它们本来就是同一份代码各抄了一遍。所以这一任务是 `cp` 之后贴差异，不是照着 msfs 那份重排。

msfs 那份 `PilotPanel.vue` 的「飞行计划」页（`tab === 'plan'`，197–257 行）**不搬进来**：它在任务 1 里已经以 `FlightPlanDialog.vue` 的形式进了 msfs，由任务 5 接到菜单「文件 → 飞行计划…」上。

**认下这一拍的代价：任务 4 做完到任务 5 做完之间，msfs 拍发不了飞行计划**（`PilotPanel` 的页签没了，而挂 `FlightPlanDialog` 的菜单分派在任务 5 里）。两个任务背靠背，中间不发版。xpc 在计划 3 里是反过来的顺序（先对话框、后拆面板），这里不能照搬：msfs 的菜单整条都要等任务 3 的 Rust 侧，而 `App.vue` 是任务 5 一次换掉的。

#### 和 xpc 的差异，一条一条

| # | xpc 有 | msfs | 落在哪 |
|---|---|---|---|
| 1 | `csl?: CslView` | `hangar?: HangarView` | props、`import type`、模板最下面那一段 |
| 2 | `csl_dir` / `set_csl_dir` | `packages_dir` / `set_packages_dir` | `onMounted`、`applyPackagesDir` |
| 3 | CSL 状态**三种**，而且每一种都印根路径 | 机库状态**四种**，而且**一条都不印路径** | 模板 |
| 4 | `local.range` + `set_traffic_range` | **没有这个东西** | 整段删 |
| 5 | `<InstallWizard />` | **没有这个组件** | 整段删 |

**差异 3 —— 四种状态，而且不显示路径。** `HangarView` 报的是 `{ loading, files, liveries, types, dir }`，界面要分的是四种：

| 状态 | 条件 | 键 | 颜色 |
|---|---|---|---|
| 正在扫 | `loading` | `hangar.loading` | 正常（`opacity-60`） |
| 扫到了 | `types > 0` | `hangar.found{liveries,types,files}` | 正常 |
| 扫到涂装但一个机型码都没有 | `types === 0 && liveries > 0` | `hangar.no_types{liveries}` | **琥珀** |
| 什么都没扫到 | 两个都是 0 | `hangar.empty` | **琥珀** |

第三种是 msfs 独有的失败形态：读到的全是附加件那类没有机型码的配置，`liveries` 有几百个而 `types` 是 0——光看一个总数分不出它和「目录指错了」。`lib.rs` 的 `HangarView` 文档注释已经写明了这件事（「**三个数都要给**」），照它办。

**一条都不印路径**：`hangar.found` / `hangar.empty` / `hangar.no_types` 四句的译文里**没有 `{path}` 占位符**（xpc 的 `csl.*` 三句每一句都有）。路径就在同一行的输入框里，印第二遍只是把那一行撑长。**不要给这四句加占位符**——加了就要动字典，而 `hangar.*` 的中英两份今天是对齐的。

**差异 4 —— 没有显示距离，这是真实差异，不是漏了。** msfs 的 Rust 侧**根本没有** `set_traffic_range` 这个命令，`Settings` 里也没有 `traffic_range_nm`；距离是常量 `MAX_RANGE_NM = 200.0`（`apps/msfs/src-tauri/src/lib.rs:139`），注入上限是 `MAX_TRAFFIC = 64`，两个都不可配。所以 msfs 的「他机」页比 xpc 的少一格。**这句话要写进文件头部的注释里**，否则下一个对着 xpc 那份读代码的人会以为这里漏了一格，然后去 Rust 侧加一个命令。

**差异 5 —— 没有安装向导。** msfs 没有插件这回事（SimConnect 是模拟器自带的），四个插件命令（`xplane_installs` / `plugin_install_status` / `install_plugin` / `bundled_plugin_dir`）在 msfs 的 `lib.rs` 里一个都不存在，`InstallWizard.vue` 这个文件也只在 xpc 有。

**提示音那一组（`local.chime*`）留在「他机」页，和 xpc 一样。** spec §6 点名把它放在这一页；xpc 的 `PilotSettings.vue` 就是这么排的，msfs 逐字照抄那一段。差异地图 §7.3 那句「msfs 的他机页只有注入开关和机库目录」讲的是**和 xpc 比少了哪两格**，不是那一页的全部内容——提示音那一组今天就在 `PilotPanel.vue:302-325`，删掉它是丢功能。这一页删掉的只有显示距离和安装向导两样。

#### 生命周期：每开一次新挂一次

`PilotSettings` 挂在 `SettingsDialog` 的 `v-if="open"` **里面**，所以每次打开对话框都是新挂一次，关掉就销毁。今天的 `PilotPanel` 是 `v-if="!compact"`，开着程序就一直挂着。逐条：

- **`onMounted` 就是「打开的时候读一遍」，不要改成 watch。** 外层 `v-if` 已经决定了生命周期，再套一层只会多一条走不到的路。`SettingsCommon.vue` 那条 `{ immediate: true }` 是另一回事：它读的是自己的属性，不是自己的挂载。
- **`deviceTimer`（2000 ms 扫设备）原样搬。** 效果是它只在对话框开着的时候跑——而它存在的理由正是「设备下拉框开着的时候有人拔了耳机」。
- **`onUnmounted` 里那一行 `cancel_ptt_capture` 要跟着搬过来。** xpc 的 `PilotSettings.vue:87-94` 有，msfs 今天的 `PilotPanel.vue:74-77` **没有**（它当年挂在 `v-if="!compact"` 上，录制到一半根本关不掉这个面板）。从此关得掉了：清掉那个 150 ms 轮询**并不会**让 Rust 侧退出录制模式，于是对话框关着的时候按下的任意一个键会被留在 Rust 那里，下次一点「录制」立刻抓到它。这一行是这一任务唯一一处不是纯搬运的改动，是刻意的。
- 本机设置从此在精简模式下也打得开（齿轮和菜单在精简模式下都在）。

#### 机库扫描是阻塞的：这一份计划里唯一一个新的设计问题

`HANGAR_WAIT = 120s`（`apps/msfs/src-tauri/src/lib.rs:144`）。`wait_for_hangar` 会把**注入**堵到扫完或者堵满两分钟为止；扫描本身在启动时就 spawn 了（`:1775`），跟连不连网无关。

把「包目录」这一格从常驻面板搬进一个**默认关着**的对话框，代价是：包目录填错的人（或者目录在网络盘上的人）连上网络之后，两分钟里天上什么都不会出现，而屏幕上一个字都没有解释。这和 msfs 反复要躲开的那类故障是同一种——「对着一个永远灰着的灯猜」。

**落点分两处，都在 `App.vue`（任务 5 Step 7 实现）：**

1. **「正在扫」进状态栏中间那句话。** `barStatus` 里，在 `talking` 那一条之后、`status.ready` 之前插一条 `if (view.value?.hangar.loading) return t("hangar.loading");`。选它是因为：状态栏**在精简模式下也一直在**（计划 3 定死的），所以这句话总有地方说；`hangar.loading`（「正在扫本机机库…」）本来就是一句短句，而 `StatusBar` 那一格是 `truncate` 的，长句子会被裁掉；而且它说的正是「此刻有一件事正在跑」，和那一格「空闲说就绪，有事说那件事」的定位一致。**不另造新键**，这一条不需要动字典。
2. **「扫空了 / 一个机型码都没有」当横幅排，接在插件那两条横幅腾出来的位置上**，和 `sim_problem` 做邻居。理由：这两句是长句（`hangar.empty` 一整句话带一条建议），进不了状态栏那一格；而它们的修法只有一个——把包目录填对——所以必须在对话框外面把人指过去。**只在 `view.sim_connected` 为真时才说**：没连上模拟器的机器上根本不注入，`sim_problem` 那条横幅已经把话说完了，再叠一条常驻的琥珀横幅只是噪音（非 Windows 上机库永远是空的，那条横幅会一直挂着）。观察员同样不显示，理由和 xpc 那两条插件横幅一样：他不上 FSD，天上本来就不会有他机。

**不做的第三种选择：不要给「正在扫」加进度条，也不要在扫描期间禁用连接。** 扫描和连接是两条独立的路，语音和文字消息在扫描期间完全可用；挡住连接会把一个「他机可能晚两分钟」的问题升级成「两分钟内上不了网」。

- [ ] **Step 1: `cp` 再贴差异**

先复制：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cp apps/xpc/src/components/PilotSettings.vue apps/msfs/src/components/PilotSettings.vue
```

然后把下面这段存成 `.temp/msfs_pilot_settings.py`（**用写文件的工具写，不要用 shell 的 heredoc**——里面有反引号和中文引号，套在 heredoc 里会被 shell 吃掉），跑 `python3 .temp/msfs_pilot_settings.py && rm .temp/msfs_pilot_settings.py`：

```python
import pathlib

p = pathlib.Path("apps/msfs/src/components/PilotSettings.vue")
lines = p.read_text(encoding="utf-8").split("\n")


def find(exact: str) -> int:
    hits = [i for i, l in enumerate(lines) if l == exact]
    assert len(hits) == 1, (exact, len(hits))
    return hits[0]


# ——— 1. 头部注释 + props + import type：从 i18n 那一行之后到 import type 那一行为止，整段换掉 ———
head_a = find('import { t } from "../i18n";')
head_b = find('import type { CslView, Settings } from "../types";')
assert head_a < head_b
HEAD = '''
/**
 * 设置对话框里 msfs 自己那几段：音频 / 网络 / 他机三页（spec §6）。
 *
 * **它整个挂在 `SettingsDialog` 的 `v-if="open"` 里面**，每次打开都是新挂一次，
 * 关掉就销毁。所以 `onMounted` 就是「打开的时候读一遍」，`onUnmounted` 就是
 * 「关掉的时候停掉」——不要改成 watch，外层 `v-if` 已经决定了生命周期，再套一层
 * 只会多一条走不到的路。
 *
 * 「网络」那一页只有寄日志：服务器地址那几格在 `SettingsCommon` 里，就在这个枢轴
 * 的正上方；真实姓名和连不连在主界面的连接卡片上。
 *
 * **和 xpc 的同名文件是两份，不是一份，所以两份都不进 `SHARED_FRONTEND`。**
 * 三处真实差异：这里是机库不是 CSL（四种状态，而且一条都不印路径——路径就在同一行
 * 的输入框里）；**没有显示距离**（msfs 的 Rust 侧根本没有 `set_traffic_range` 这个
 * 命令，`Settings` 里也没有 `traffic_range_nm`，距离是常量 `MAX_RANGE_NM`）；
 * **没有安装向导**（msfs 走 SimConnect，没有插件这回事）。所以「他机」这一页比 xpc
 * 的少两格——**这是真实差异，不是漏了**，别照着 xpc 那份去补。
 */

/// `cid` 是已经存下来的 CAN 号，寄日志时预填，省得再打一遍；
/// `hangar` 是扫机库那一侧的现状，由 App.vue 那份轮询回来的快照带进来。
const props = defineProps<{ cid?: string; hangar?: HangarView }>();
import type { HangarView, Settings } from "../types";'''
lines[head_a + 1 : head_b + 1] = HEAD.split("\n")[1:]

# ——— 2. 安装向导：导入和模板里那一行都删掉 ———
del lines[find('import InstallWizard from "./InstallWizard.vue";')]
i = find("      <InstallWizard />")
assert lines[i - 1] == ""
del lines[i - 1 : i + 1]

# ——— 3. applyRange 整段删（文档注释 1 行 + 函数 3 行 + 空行 1 行） ———
i = find("async function applyRange() {")
assert lines[i - 1].startswith("/**") and lines[i + 2] == "}" and lines[i + 3] == ""
del lines[i - 1 : i + 4]

# ——— 4. 模板里「显示距离」那一格整段删（注释没有，label 14 行 + 空行 1 行） ———
i = find('        <span class="w-16 shrink-0 opacity-70">{{ t("local.range") }}</span>')
assert lines[i - 1] == '      <label class="flex items-center gap-2">'
assert lines[i + 12] == "      </label>" and lines[i + 13] == ""
del lines[i - 1 : i + 14]

# ——— 5. CSL 那一段（注释 2 行 + label 11 行 + 状态 p 9 行）整段换成机库 ———
i = find('        <span class="w-16 shrink-0 opacity-70">{{ t("csl.dir") }}</span>')
assert lines[i - 2].lstrip().startswith("<!--")
assert lines[i + 18] == "      </p>"
HANGAR = '''
      <!-- 扫不到机模的表现是「他机全是同一架第一方飞机」，和机型码对不上长得差不多，
           所以三个数都要显示出来。**四种状态**：正在扫 / 扫到了 / 扫到涂装但一个机型码
           都没有（琥珀）/ 什么都没扫到（琥珀）。第三种是 msfs 独有的：读到的全是附加件
           那类没有机型码的配置，光看一个总数分不出它和「目录指错了」。
           **一条都不印路径**：路径就在上面那个输入框里，印第二遍只会把这一行撑长。 -->
      <label class="flex items-center gap-2">
        <span class="w-16 shrink-0 opacity-70">{{ t("hangar.dir") }}</span>
        <input
          v-model="packagesDir"
          :placeholder="t('hangar.dir_placeholder')"
          class="flex-1 rounded border px-2 py-1 font-mono"
          @change="applyPackagesDir"
          @keyup.enter="applyPackagesDir"
        />
        <button class="rounded border px-2 py-1" @click="applyPackagesDir">{{ t("local.rescan") }}</button>
      </label>
      <p v-if="props.hangar" class="opacity-60">
        <template v-if="props.hangar.loading">{{ t("hangar.loading") }}</template>
        <template v-else-if="props.hangar.types">
          {{
            t("hangar.found", {
              liveries: props.hangar.liveries,
              types: props.hangar.types,
              files: props.hangar.files,
            })
          }}
        </template>
        <span v-else-if="props.hangar.liveries" class="text-amber-700">
          {{ t("hangar.no_types", { liveries: props.hangar.liveries }) }}
        </span>
        <span v-else class="text-amber-700">
          {{ t("hangar.empty") }}
        </span>
      </p>'''
lines[i - 2 : i + 19] = HANGAR.split("\n")[1:]

text = "\n".join(lines)

# ——— 6. 剩下四处纯改名，全是 ASCII，逐条断言命中数 ———
renames = [
    ('const range = ref(200);\nconst cslDir = ref("");\n',
     'const packagesDir = ref("");\n'),
    ("  range.value = s.traffic_range_nm;\n  cslDir.value = s.csl_dir;\n",
     "  packagesDir.value = s.packages_dir;\n"),
    ('const applyCslDir = () => invoke("set_csl_dir", { dir: cslDir.value });',
     'const applyPackagesDir = () => invoke("set_packages_dir", { dir: packagesDir.value });'),
]
for old, new in renames:
    assert text.count(old) == 1, old
    text = text.replace(old, new, 1)

p.write_text(text, encoding="utf-8")
print("ok", p)
```

每一处都有断言，对不上就当场炸；**不要用 `perl -CSD -pi -e` 改这些**（`-CSD` 只改 I/O 编码层，`-e` 里的字面量仍按字节算，非 ASCII 的替换一次都不匹配而且安静地退出 0）。

- [ ] **Step 2: 核一遍新文件里没有留下 xpc 的东西**

下面五条**都要零命中**：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -nE 'csl|CslView|InstallWizard|traffic_range|local\.range' apps/msfs/src/components/PilotSettings.vue
```

再确认该有的都有（每条 1 命中）：

```bash
grep -c 'set_packages_dir\|cancel_ptt_capture\|hangar.no_types\|hangar.empty\|hangar.found\|hangar.loading' apps/msfs/src/components/PilotSettings.vue   # 6 行各一处
grep -n 'local.audio\|local.network\|local.traffic' apps/msfs/src/components/PilotSettings.vue   # 三个页签，各一处
```

- [ ] **Step 3: `PilotPanel.vue` 删掉，`App.vue` 改最小的三处**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git rm apps/msfs/src/components/PilotPanel.vue
```

`apps/msfs/src/App.vue` 里三处，**按内容找，不按行号找**：

导入那一行：

```ts
import PilotPanel from "./components/PilotPanel.vue";
```

换成：

```ts
import PilotSettings from "./components/PilotSettings.vue";
```

模板里这一行整行删掉（连同它上面那一行空行）：

```html
      <PilotPanel v-if="!compact" :cid="cid" :hangar="view?.hangar" :observer="observer" />
```

模板最后那个自闭合的对话框：

```html
      <SettingsDialog :open="showPrefs" @close="showPrefs = false" />
```

换成：

```html
      <SettingsDialog :open="showPrefs" @close="showPrefs = false">
        <PilotSettings :cid="cid" :hangar="view?.hangar" />
      </SettingsDialog>
```

**这三处都是权宜的**：任务 5 会把整份 `App.vue` 换掉。这里改它只为了让这一拍的 `bun run build` 是绿的——一个引用着已删文件的仓库状态不该被提交。

- [ ] **Step 4: `local.tab` 没人用了就删掉**

`PilotPanel` 一删，「本机」这个页签不再存在于任何界面上。先查再删，**零引用才删**：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -rn 'local\.tab' apps/msfs/src/
```

没有任何输出，就把 `"tab": "本机",` / `"tab": "Local setup",` 这两行从两份字典的 `local` 命名空间里删掉（**认准 `local` 里的那一个**，`plan` 里也有一个 `"tab"`，值是「飞行计划」，那一个是 `FlightPlanDialog` 的标题，不要动）。**有输出就不要删**，把还在用它的文件和行号记在完成报告里。

`lists.messages` 和 `observer.frequency` 这一步**不要动**：它们此刻还被 `App.vue` 用着，是任务 5 制造出来的孤儿，由任务 5 删。

- [ ] **Step 5: 改 xpc 那份的头部注释**

`apps/xpc/src/components/PilotSettings.vue` 的文档注释里这两行今天在说假话：

```
 * 只有 xpc 有这个文件，所以不进 `SHARED_FRONTEND`。**哪天 msfs 也要一个，
 * 那时候把它登记进去**——两份不登记的副本会无声地漂开。
```

msfs 现在有一个了，而且**故意不一样**，所以「把它登记进去」是错的指路。换成：

```
 * **msfs 也有一个同名文件，但两份故意不一样，所以两份都不进 `SHARED_FRONTEND`。**
 * msfs 那一份是机库不是 CSL（四种状态、不印路径）、没有显示距离（它的 Rust 侧根本
 * 没有 `set_traffic_range`）、没有安装向导。**不要把这两份合成一份**——合起来就要给
 * 一半的字段写「这个端没有」的分支，而那正是共用件要避开的东西。
```

**代码一行不动**，所以 xpc 只需要过一次 `bun run build`。

- [ ] **Step 6: 跑门**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs && bun run build
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/xpc && bun run build
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo test -p can-voice-i18n
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo fmt --all --check
```

`every_key_the_interface_asks_for_exists` 是这一步真正的门：新文件里每一个 `t("…")` 的键都要在 msfs 的字典里有。`the_interface_code_has_no_hardcoded_chinese` 同样会看这个新文件。

- [ ] **Step 7: 提交**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git add apps/msfs/src/components/PilotSettings.vue apps/msfs/src/components/PilotPanel.vue \
        apps/msfs/src/App.vue \
        apps/msfs/src/locales/app.zh.json apps/msfs/src/locales/app.en.json \
        apps/xpc/src/components/PilotSettings.vue
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-4.txt
```

**不要 `git add -A` / `git add .`**，不要写 `-m`，不要自己写提交正文。

**人工要看的**：`cd apps/msfs && bun run tauri dev`。点顶栏的齿轮开设置对话框，枢轴上是「音频 / 网络 / 他机」三页，默认停在「音频」。

- 音频页：麦克风 / 耳机下拉、两条试听、两根音量条、PTT 绑定列表和「录制」。
- 网络页：只有寄日志那一块。**薄是对的**，服务器地址在这个对话框枢轴的正上方。
- 他机页：注入开关、提示音那一组、**包目录那一行**。**没有「显示距离」，也没有安装向导**——这两样 msfs 本来就没有。
- 包目录那一行下面那句话：刚开程序时是「正在扫本机机库…」，扫完是「扫到 N 个涂装、M 种机型（读了 K 个 aircraft.cfg）」。把包目录改成一个不存在的路径，回车，应当变成琥珀色的「没扫到任何机模…」。**这四句里一条都不印路径**。
- 点「录制」，**录到一半直接关掉对话框**，再打开，再点「录制」——不应当立刻抓到一个你在对话框关着时按过的键。这就是 `cancel_ptt_capture` 那一行修的东西。
- 主界面上「本机」那一整块不见了，飞行计划那一页也不见了（下一个任务把它接到菜单上）。

---

### Task 5: `App.vue` 整份换成 xpc 的，再把 msfs 的五处差异贴回去

**Files:**
- Modify: `apps/msfs/src/App.vue`（**整份换掉**，`cp` 自 xpc，再贴五处差异 + 一处机库落点）
- Modify: `apps/msfs/src/types.ts`（加 `MenuRequest` 和 `View.menu`）
- Modify: `apps/msfs/src/locales/app.zh.json` / `app.en.json`（`lists.messages`、`observer.frequency` 零引用后删掉）

**Interfaces:**
- Consumes: 任务 1 搬进 msfs 的 `Panel.vue` / `Modal.vue` / `StateToggle.vue` / `StatusBar.vue` / `FlightPlanDialog.vue`；任务 2 的全部新键；任务 3 的 `set_menu(labels)`、`app_version()`、`open_log_dir()` 和 Rust 侧的 `View.menu`；任务 4 的 `PilotSettings.vue`。
- Consumes: 任务 4 定下的机库落点（状态栏一条、横幅两条），在 Step 7 实现。
- Produces: 完整的新版 `App.vue`。任务 6 只**核**它，不再写它。

#### 主体策略：不要重排，要搬

换布局之前 msfs 的 `App.vue` 和 xpc 换布局之前的那一份只差**四个 hunk**，自己看一眼：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
diff <(git show main:apps/xpc/src/App.vue) apps/msfs/src/App.vue
```

xpc 那一份现在已经是想要的样子，而且过了全套门和一次整分支复审。所以：**`cp` 过来，再把五处差异贴回去**。目标是一个已知 good 的文件，不是一段描述。

**下面每一处都按内容找，不按行号找。** xpc 那份 700 行出头，任何写进计划的行号在前一处改动落地的那一刻就过期了。这一条计划 3 栽过两次。

| # | 差异 | 在新版 xpc 的 `App.vue` 里落在哪 |
|---|---|---|
| 1 | 模拟器叫 `MSFS` 不叫 `X-Plane` | **两处**：连接卡片那三格状态文案里一处，精简时 `StatusBar` 插槽里一处。另有两处在注释里 |
| 2 | 没有插件指示灯 | 卡片上方那一叠横幅，整段删 |
| 3 | 没有 `plugin.version_mismatch` / `plugin.not_heard` 两条横幅 | 同上，整段删 |
| 4 | 多一条 `sim_problem` 琥珀横幅 | 接在插件那几段腾出来的位置上 |
| 5 | `PilotSettings` 收 `:hangar` 不是 `:csl` | 模板最下面那一对 `<SettingsDialog>` |
| 6 | 机库那两条落点（任务 4 定的） | `barStatus` 一条 + 横幅两条 |

**差异 1 是这一任务最容易做漏的一处**：`X-Plane` 在 xpc 的新版里出现 **4 次**——两次在模板里（连接卡片的三格状态、精简时状态栏的插槽），两次在注释里（其中一处跟着插件那盏灯一起被删）。**只改模板里那一处的，漏掉的正是精简模式下的那一处**，而精简模式是飞起来之后的形态，测试时最不容易翻到。下面的脚本用 `assert n == 2` 把这件事钉死。

**差异 4 —— `sim_problem` 当横幅排，不当胶囊排。** 它装的是 SimConnect 原样的英文 `detail`（`simconnect.not_running` / `simconnect.lost` 都带 `{detail}`），**长度没有上限**；而连接卡片那三格是定宽的 `grid-cols-3`，塞进去会把那一行挤散。它接的就是插件那两条横幅腾出来的位置——那里本来就是给会换行的长句子用的。**在非 Windows 上这是屏幕上最要紧的一句话**（那里 `simconnect.unavailable` 恒真，模拟器那盏灯按设计一直是灰的），所以它**不跟着精简收起**，和 xpc 的插件横幅一样待遇。

- [ ] **Step 1: 复制**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
cp apps/xpc/src/App.vue apps/msfs/src/App.vue
```

这一刻 `bun run build` 是红的（`view?.csl`、`view?.plugin` 在 msfs 的 `View` 里不存在），这是预期的——Step 2 到 Step 7 之后才绿。

- [ ] **Step 2: `types.ts` 加 `MenuRequest` 和 `View.menu`**

两段注释**从 xpc 的 `types.ts` 里抽出来，不要手打**（中文标点）。把下面这段存成 `.temp/msfs_types_menu.py`（用写文件的工具写），跑 `python3 .temp/msfs_types_menu.py && rm .temp/msfs_types_menu.py`：

```python
import pathlib

xl = pathlib.Path("apps/xpc/src/types.ts").read_text(encoding="utf-8").split("\n")

i = [n for n, l in enumerate(xl) if l.startswith("export type MenuRequest")]
assert len(i) == 1, i
i = i[0]
assert xl[i - 6] == "/**", xl[i - 6]
menu_type = "\n".join(xl[i - 6 : i + 1])          # 文档注释 6 行 + 类型 1 行

j = [n for n, l in enumerate(xl) if l.strip() == "menu: MenuRequest | null;"]
assert len(j) == 1, j
menu_field = "\n".join(xl[j[0] - 1 : j[0] + 1])   # 文档注释 1 行 + 字段 1 行

p = pathlib.Path("apps/msfs/src/types.ts")
t = p.read_text(encoding="utf-8")

anchor = '  | "Stopped";\n'
assert t.count(anchor) == 1
t = t.replace(anchor, anchor + "\n" + menu_type + "\n", 1)

anchor = "  observer: ObserverView | null;\n}"
assert t.count(anchor) == 1
t = t.replace(anchor, "  observer: ObserverView | null;\n" + menu_field + "\n}", 1)

p.write_text(t, encoding="utf-8")
print("ok", p)
```

`MenuRequest` 的四个取值（`"flight_plan" | "settings" | "update" | "about"`）和任务 3 的 Rust 侧 `MenuRequest` 一一对应。**不要自己另起一套名字**——那一侧已经按 xpc 的形状写好了。

- [ ] **Step 3–7: `App.vue` 的六处改动，一个脚本做完**

把下面这段存成 `.temp/msfs_app_vue.py`（**用写文件的工具写，不要用 shell heredoc**），跑 `python3 .temp/msfs_app_vue.py && rm .temp/msfs_app_vue.py`：

```python
import pathlib

p = pathlib.Path("apps/msfs/src/App.vue")
lines = p.read_text(encoding="utf-8").split("\n")


def find(exact: str) -> int:
    hits = [i for i, l in enumerate(lines) if l == exact]
    assert len(hits) == 1, (exact, len(hits))
    return hits[0]


# ——— 差异 2 + 3 + 4：插件那盏灯和那两条横幅整段删，换成 sim_problem 和机库两条横幅 ———
a = find('        {{ t("status.plugin") }}')
b = find('        {{ t("plugin.not_heard") }}')
start, end = a - 7, b + 1
assert lines[start].lstrip().startswith("<!--"), lines[start]
assert lines[end] == "      </p>", lines[end]
BANNERS = '''
      <!-- 连不上要说得出原因。非 Windows 上就是「这个系统没有 SimConnect」——
           让人对着一个永远灰着的灯猜，是这个项目反复要躲开的那类故障。
           **当横幅排，不当胶囊排**：它装的是 SimConnect 原样的英文 detail，长度没有
           上限，塞进下面那三格 `grid-cols-3` 会把那一行挤散。
           **精简时也在**：非 Windows 上这是屏幕上最要紧的一句话，和 xpc 的插件横幅
           一样待遇。 -->
      <p
        v-if="!view?.sim_connected && view?.sim_problem"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ errorText(view.sim_problem) }}
      </p>

      <!-- 机库扫空了要在对话框外面说。包目录那一格在设置对话框的「他机」页上，而对话框
           默认是关着的；机库扫一遍要几十秒，注入还要等它扫完（`wait_for_hangar`，最多
           `HANGAR_WAIT` 120 秒）。填错路径的人在此之前只看得见「天上是空的」，而那和
           没连上模拟器、和机型码对不上长得一模一样。
           **只在模拟器连上之后才说**：没连上模拟器的机器上根本不注入，上面那条
           `sim_problem` 已经把话说完了，再叠一条常驻的琥珀横幅只是噪音（非 Windows 上
           机库永远是空的，它会一直挂着）。
           观察员不上 FSD，天上本来就不会有他机，别拿这两条吓他。 -->
      <p
        v-if="!observer && view?.sim_connected && !view.hangar.loading && !view.hangar.types && view.hangar.liveries"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("hangar.no_types", { liveries: view.hangar.liveries }) }}
      </p>
      <p
        v-else-if="!observer && view?.sim_connected && !view.hangar.loading && !view.hangar.liveries"
        class="rounded border border-amber-400 px-3 py-2 text-xs text-amber-700"
      >
        {{ t("hangar.empty") }}
      </p>'''
lines[start : end + 1] = BANNERS.split("\n")[1:]

# ——— 差异 1 第一处的那段注释：它整段在讲插件那盏灯，msfs 没有 ———
i = find('        <div class="grid grid-cols-3 gap-2 text-xs">')
assert lines[i - 6].lstrip().startswith("<!--"), lines[i - 6]
assert lines[i - 1].rstrip().endswith("-->"), lines[i - 1]
COMMENT = '''
        <!-- can-audio 把三格状态文案排在网格第二行（xpc/gui.py:265-270）：模拟器、FSD、语音。
             页头那三个 pill 就是搬到这里来的。三格自带名字：FSD 和语音那两句译文本来就以
             它们的名字开头，第一格里 `MSFS` 是拉丁字面量。
             **`sim_problem` 不在这三格里**：它装的是 SimConnect 原样的英文 detail，长度
             没有上限，塞进这三格会把这一行挤散，所以它在卡片上方当横幅排。 -->'''
lines[i - 6 : i] = COMMENT.split("\n")[1:]

text = "\n".join(lines)

# ——— 差异 1：模板里那两处 X-Plane。两处缩进一样，所以一次替换、断言命中 2 ———
old = "            X-Plane\n"
assert text.count(old) == 2, text.count(old)
text = text.replace(old, "            MSFS\n")

# ——— 差异 5：PilotSettings 收的是 hangar ———
old = '<PilotSettings :cid="cid" :csl="view?.csl" />'
assert text.count(old) == 1
text = text.replace(old, '<PilotSettings :cid="cid" :hangar="view?.hangar" />', 1)

# ——— 差异 6：底栏那句话多一种状态（任务 4 定的落点之一） ———
old = ('  if (talking.value) return t("chat.transmitting");\n'
       '  return t("status.ready");')
new = ('  if (talking.value) return t("chat.transmitting");\n'
       "  // 机库扫一遍要几十秒，而注入要等它扫完（`wait_for_hangar`，最多 120 秒）。\n"
       "  // 包目录填错的人在此之前只看得见「天上是空的」，而那和没连上模拟器、和机型码\n"
       "  // 对不上长得一模一样。这条栏在精简模式下也在，所以这句话总有地方说；\n"
       "  // 那一格是 `truncate` 的，所以这里只能放短句——长的两句在上面当横幅排。\n"
       '  if (view.value?.hangar.loading) return t("hangar.loading");\n'
       '  return t("status.ready");')
assert text.count(old) == 1
text = text.replace(old, new, 1)

p.write_text(text, encoding="utf-8")
print("ok", p)
```

六处改动各带断言，任何一处对不上就当场炸。

- [ ] **Step 8: 核一遍没有留下 msfs 没有的东西**

下面这条**必须零命中**，`bun run build` 抓得到类型层面的漏网，但抓不到模板里对一个可选字段的引用：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -nE 'view\.plugin|view\?\.plugin|status\.plugin|plugin\.|csl|local\.range|InstallWizard|set_traffic_range|X-Plane' apps/msfs/src/App.vue
```

再确认该有的都在（各 1 命中，`MSFS` 是 2）：

```bash
grep -c 'MSFS' apps/msfs/src/App.vue          # 要是 2
grep -n 'sim_problem' apps/msfs/src/App.vue   # 1 处 v-if + 1 处 errorText
grep -n 'hangar' apps/msfs/src/App.vue        # barStatus 1 处 + 两条横幅 + PilotSettings 1 处
grep -n 'view.value.menu\|view.value?.menu\|switch (view.value.menu)' apps/msfs/src/App.vue
```

- [ ] **Step 9: 两个孤儿键删掉**

`lists.messages`（「文字消息」，老版那一行小标题，新版换成了 `Panel` 的标题 `messages.title`）和 `observer.frequency`（「语音频率」，老版观察员那一栏的行首标签，新版那个框在无线电那一行里、旁边就是 COM1，不再需要标签）在这一任务之后没人渲染。先查再删，**零引用才删**：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -rn 'lists\.messages\|observer\.frequency"' apps/msfs/src/
```

注意第二条的引号：`observer.frequency_placeholder` 和 `observer.frequency_tip` **还在用**，不带引号的 `grep observer.frequency` 会把它们也捞进来。没有输出，就把 `lists` 里的 `"messages"` 和 `observer` 里的 `"frequency"` 两个键从两份字典里删掉（各两份，共四处）。**有输出就不要删**。

xpc 在计划 3 里删的正是这两个加一个 `local.tab`（`local.tab` 在任务 4 已经删了）。

- [ ] **Step 10: 跑门**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs && bun run build
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo test -p can-voice-i18n
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo fmt --all --check
```

`bun run build` 里的 vue-tsc 是这一任务真正的门：`view?.csl`、`view?.plugin` 只要漏一处就编不过。**但它抓不到模板里对一个可选字段的拼写错误**（`view?.hangar.typos` 在 `View` 上不存在时 vue-tsc 会报，而 `t("hangar.typo")` 不会——那一条由 `every_key_the_interface_asks_for_exists` 接住）。两道都跑。

- [ ] **Step 11: 提交**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git add apps/msfs/src/App.vue apps/msfs/src/types.ts \
        apps/msfs/src/locales/app.zh.json apps/msfs/src/locales/app.en.json
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-5.txt
```

**人工要看的**：`cd apps/msfs && bun run tauri dev`。

- 从上到下：整宽的「连接」卡片，中间一行「消息」「附近管制」「他机」三张，最下「无线电」，最底一条状态栏。中间三张的宽度大致 3 : 1 : 1。
- 连接卡片第二行三格状态文案：第一格是一盏灯加 **`MSFS`**（不是 `X-Plane`），第二格 FSD，第三格语音。**这一行上没有插件那盏灯**——msfs 没有插件。
- **在一台没开 MSFS 的机器上**（macOS / Linux 直接就是）：卡片上方应当有一条琥珀横幅，写着「这个系统上没有 SimConnect…」或者「连不上 MSFS：…」。这条横幅**在精简模式下也要在**。
- 点顶栏的「简」：窗口缩小，只剩「消息」卡片 + 「无线电」那一行 + 状态栏。**状态栏右边补出三格状态文案，第一格仍然是 `MSFS`**——这一处是最容易漏改的，专门看一眼。
- 底栏中间那句话：刚启动时（机库还在扫）写「正在扫本机机库…」，扫完回到「就绪」，按住 PTT 写「发话中」。
- 菜单「文件 → 飞行计划…」开出飞行计划对话框；「文件 → 设置…」开出设置对话框，里面是任务 4 那三页。

---

### Task 6: 窗口几何、精简模式、最后两个菜单项、文档

**Files:**
- Modify: `apps/msfs/src-tauri/tauri.conf.json`（980×660，最小 900×600。**`devUrl` 不动**）
- Modify: `apps/msfs/src-tauri/src/lib.rs`（**只改两段文档注释，数字一个不动**）
- Modify: `docs/manual-test.md`（§3 按新排布重写，加一节 msfs 的差别）
- **只核不改**：`apps/msfs/src/App.vue`（「检查更新」和「关于」那两条路在任务 5 `cp` 过来的时候就已经在里面了）

**Interfaces:**
- Consumes: 任务 3 的 `app_version()`、`open_log_dir()`、`set_menu(labels)`；任务 5 的完整 `App.vue`（`checkUpdate` / `openAbout` / `transient` / `updateNonce` / 关于 `Modal`）。
- Produces: 没有后续任务依赖的东西——这是这一份计划的最后一步。

#### `devUrl` 是 `4333`，不是 `4332`

这是这一任务最容易抄错的一处，因为改的那个 JSON 对象就在 `devUrl` 的下面，而源文件是 xpc 的。

```
apps/xpc/src-tauri/tauri.conf.json   →  "devUrl": "http://localhost:4332"
apps/msfs/src-tauri/tauri.conf.json  →  "devUrl": "http://localhost:4333"   ← 不动它
```

这一仓库的端口梯子是 controller 4330、atis 4331、xpc 4332、msfs 4333，动手前自己看一眼：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -h devUrl apps/*/src-tauri/tauri.conf.json
```

抄错了的表现是 `tauri dev` 起来一片白，而 `bun run build` 一点问题都没有——它根本不看 `devUrl`。

#### `COMPACT_MIN` / `COMPACT_SIZE` 不动，这是想过之后的结论

两个常量在 `apps/msfs/src-tauri/src/lib.rs`（`COMPACT_MIN` 320×220，`COMPACT_SIZE` 460×340），**和 xpc 今天那一对一模一样**。xpc 在计划 3 里也没有改数字，只给 `COMPACT_SIZE` 的文档注释续了一段，把「为什么不改」写下来。msfs 照做，理由再加一条：

- 精简留下的东西两个端一模一样——消息卡片，加无线电那一行里的 TX / RX、COM1、IDENT、PTT 和观察员的频率框。msfs 少的那盏插件灯**本来就不在精简模式里**（它在卡片上方那一叠横幅上，精简时连接卡片才收，横幅不收）。
- 460px 正好排得下那一行不换行；320px 的下限也不裁东西——那一行是 `flex-wrap` 的，排不下就折成两行。

- [ ] **Step 1: 窗口几何**

`apps/msfs/src-tauri/tauri.conf.json` 的 `app.windows[0]`，五行整段换成：

```json
        "title": "msfs-for-can",
        "width": 980,
        "height": 660,
        "minWidth": 900,
        "minHeight": 600
```

（今天是 940×680 / 720×480。）`minWidth` / `minHeight` **是刻意加的**：没有它中间那三张卡片（消息 3 ‖ 附近管制 1 ‖ 他机 1）一挤就塌成一条。

改完立刻核端口，两行的结果必须是 `1` 和 `0`：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -c '4333' apps/msfs/src-tauri/tauri.conf.json    # 要是 1
grep -c '4332' apps/msfs/src-tauri/tauri.conf.json    # 要是 0
```

- [ ] **Step 2: 把精简模式的结论写进 `COMPACT_SIZE` 的注释**

`apps/msfs/src-tauri/src/lib.rs`，**数字不动**，给 `COMPACT_SIZE` 的文档注释续上后面两段（上半段原样保留）：

```rust
/// 按下"精简"那一刻缩成多大。**不缩的话**，东西藏起来了窗口却还是那么大，
/// 人还得自己去拖——而这个开关存在的全部理由就是一下子压到雷达屏的角落里。
///
/// **这一对数字在换 can-audio 布局时没有跟着改，是算过的。** can-audio 的两个飞行员端
/// 没有精简模式，没有数字可取；而精简留下的东西没变——消息卡片，加无线电那一行里的
/// TX / RX、COM1、IDENT、PTT 和观察员的频率框（屏幕上这颗 PTT 在 Wayland 上是唯一能
/// 发话的路径，那个频率框是观察员唯一的调频手段）。460 正好排得下那一行不换行，
/// 320 的下限也不裁东西：那一行是 `flex-wrap` 的，排不下就折成两行。
///
/// **和 xpc 那一对是同一对数字，要改一起改。** 精简留下的东西两个端一模一样；
/// msfs 少的那盏插件灯本来就不在精简模式里（它在卡片上方那一叠横幅上，精简时
/// 收起来的是连接卡片，横幅不收）。
const COMPACT_SIZE: (f64, f64) = (460.0, 340.0);
```

- [ ] **Step 3: `apply_window` 的文档注释补上最小尺寸那一句**

同一个文件，`apply_window` 的文档注释今天只有前半段。补上 xpc 那一段的等价物——`tauri.conf.json` 里那对 900×600 **不是照抄漏了的数字**，JSON 里搁不下这句话，所以记在这里：

```rust
/// 把外观里和窗口有关的两样落到窗口上。
///
/// 正常模式的最小尺寸**从 `tauri.conf.json` 读**，不在这里再抄一份：两处写同一
/// 对数字，改了一边，退出精简时窗口就还原到一个旧尺寸上。那里今天写的是
/// 900×600，**是刻意加的**：can-audio 那边一个 `setMinimumSize` 都不设，但中间那
/// 三张卡片（消息 3 ‖ 附近管制 1 ‖ 他机 1）一挤就塌成一条。JSON 里搁不下这句话，
/// 所以记在这里——那不是照抄漏了的数字，别去掉。
///
/// `shrink` 为真时顺手把窗口缩到 [`COMPACT_SIZE`]：只在精简**刚打开**的那一刻、
/// 和启动时照着存下来的状态还原时才这么做——不然每改一次主题窗口都跳一下。
```

抽出来对一眼（两段注释都在 `COMPACT_MIN` 上下）：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -n 'COMPACT_MIN' -B 4 -A 32 apps/msfs/src-tauri/src/lib.rs | head -60
```

- [ ] **Step 4: 核「检查更新」和「关于」——这两条路是 `cp` 带过来的，不用写**

任务 5 把 xpc 的 `App.vue` 整份搬了过来，所以 `checkUpdate` / `openAbout` / `transient` / `updateNonce` / 关于那个 `Modal` / 菜单 `switch` 里的 `update` 和 `about` 两个分支**已经在文件里了**。这一步只核，不写。七条都要有命中：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs
grep -n 'async function checkUpdate' src/App.vue
grep -n 'async function openAbout' src/App.vue
grep -n 'const updateNonce' src/App.vue
grep -n 'const transient = ref' src/App.vue
grep -n 'invoke<string>("app_version")' src/App.vue
grep -n 'invoke<string | null>("log_file")' src/App.vue
grep -n 'case "update":' src/App.vue && grep -n 'case "about":' src/App.vue
```

**`update.current` 不需要加。** 它在计划 3 的任务 9 里已经进了四个端的 `common.*.json`（八个文件逐字节相同）。所以**这一份计划一个字都不动 `common.{zh,en}.json`**。核一眼，两行都要输出 `1`：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
grep -c '"current"' apps/msfs/src/locales/common.zh.json     # 要是 1
md5 -q apps/*/src/locales/common.zh.json | sort -u | wc -l   # 要是 1
```

`about.body` 是 msfs 自己那一句（X-Plane → MSFS，UDP → SimConnect），任务 2 已经按 msfs 写好了；这里只核它在 `app.zh.json` / `app.en.json` 里，**不要去照抄 xpc 那一句**：

```bash
grep -n '"body"' apps/msfs/src/locales/app.zh.json
grep -n 'X-Plane\|UDP' apps/msfs/src/locales/app.zh.json     # 要零命中
```

- [ ] **Step 5: 核精简模式的三条门闩**

计划 3 的最终复审定死的三条，任务 5 `cp` 过来就应该都在。**只核，不改**：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs
# 1. 连接卡片精简时收起来
grep -n '<Panel v-if="!compact" :title="t(.connect.title.)">' src/App.vue
# 2. 状态栏在精简模式下也在 —— 这一行上不许有 v-if
grep -n '<StatusBar :talking="talking" :status="barStatus">' src/App.vue
# 3. 插槽里那三格只在精简时画
grep -n 'v-if="compact" class="flex shrink-0 items-center gap-2 opacity-70"' src/App.vue
```

第 2 条是要点：**`<StatusBar>` 那一行上出现任何 `v-if` 都是错的**。连接卡片精简时收了起来，而它把「模拟器通没通、FSD 在不在线、语音是不是好的」三句一并带走了；状态栏的插槽是它们在精简模式下唯一的落点。顺带，「检查更新」在精简模式下也就有地方说话了——藏掉这条栏，那条菜单在精简模式下是空操作。

`sim_problem` 那条横幅**不带 `!compact`**，Step 3 的 `grep` 已经确认过（`v-if="!view?.sim_connected && view?.sim_problem"`，没有第三个条件）。非 Windows 上它是屏幕上最要紧的一句话。

- [ ] **Step 6: `docs/manual-test.md`**

四处改动。把下面这段存成 `.temp/msfs_manual_test.py`（用写文件的工具写），跑 `python3 .temp/msfs_manual_test.py && rm .temp/msfs_manual_test.py`：

```python
import pathlib

p = pathlib.Path("docs/manual-test.md")
t = p.read_text(encoding="utf-8")

# 1. §3 开头那句「只对 xpc 成立」的免责声明，整行换掉。
old = "**3.5 起只对 `xpc-for-can` 成立**：新排布先落在这一支，`msfs-for-can` 还是老样子。\n"
new = ("**3.5 起两个飞行员端都成立**：新排布先落在 `xpc-for-can`，`msfs-for-can` 随后跟上。\n"
       "两边的差别集中在 §3.9，其余各节两个端一样看。\n")
assert t.count(old) == 1
t = t.replace(old, new, 1)

# 2. 四个小节标题上的「（只有 xpc）」。
old = "（只有 xpc）"
assert t.count(old) == 4, t.count(old)
t = t.replace(old, "（xpc / msfs）")

# 3. §3.9 整节，接在 §3.8 的末尾（§4 之前）。
anchor = "\n## 4. 通播\n"
assert t.count(anchor) == 1
t = t.replace(anchor, SECTION + anchor, 1)

p.write_text(t, encoding="utf-8")
print("ok", p)
```

`SECTION` 写在脚本的开头（`import` 之后），内容是：

```python
SECTION = '''
### 3.9 msfs 和 xpc 的差别（只有 msfs）

3.5–3.8 各节两个端一样看，**下面这几条只在 `msfs-for-can` 上看**。

- **第一格是 `MSFS`，不是 `X-Plane`**。连接卡片那三格状态文案里一处，**精简模式下状态栏
  右边那三格里还有一处**——两处都要看。只改了前一处的话，精简之后第一格会写着 `X-Plane`。
- **没有插件那盏灯，也没有插件那两条横幅。** msfs 走 SimConnect，模拟器自带，没有插件
  这回事。卡片上方那一叠横幅里出现「插件」两个字就是错的。
- **多一条 `sim_problem` 琥珀横幅。** 在**一台没有开 MSFS 的机器上**开程序（macOS 或者
  Linux 上直接就是）：卡片上方应当有一条琥珀横幅，写「这个系统上没有 SimConnect：MSFS
  客户端只有在 Windows 上才连得上模拟器」，或者「连不上 MSFS：…」加一段英文原文。
  - 它**不跟着精简收起**：点「简」之后这条横幅还要在。非 Windows 上这是屏幕上最要紧的
    一句话，而那里模拟器那盏灯按设计一直是灰的。
  - 它是**横幅**不是胶囊：那段英文 detail 长度没有上限，排进三格状态文案里会把那一行挤散。
- **设置 → 他机：没有「显示距离」，也没有插件安装向导。** 这一页上只有注入开关、提示音
  那一组、包目录那一行。**这是真实差异，不是漏了**——msfs 的距离是写死的 200 海里。
- **包目录那一行下面那句话有四种。** 依次试出来：
  | 怎么做 | 该看见 |
  |---|---|
  | 刚开程序 | 「正在扫本机机库…」 |
  | 扫完，包目录是对的 | 「扫到 N 个涂装、M 种机型（读了 K 个 aircraft.cfg）」 |
  | 把包目录指到一个只有附加件、没有飞机的目录 | **琥珀**：「扫到 N 个涂装，但一个机型码都没有…」 |
  | 把包目录指到一个空目录，回车 | **琥珀**：「没扫到任何机模…」 |

  四句**一条都不印路径**——路径就在上面那个输入框里。
- **机库扫描在对话框外面也看得见。** 这是新加的一条路，因为机库扫一遍要几十秒，而他机
  注入要等它扫完（最多 120 秒）；包目录填错的人此前只看得见「天上是空的」。
  - 刚开程序、机库还在扫的那几十秒里，**底栏那句话是「正在扫本机机库…」**，扫完回到
    「就绪」。精简模式下也是。
  - **MSFS 连上之后**机库仍然是空的（或者一个机型码都没有）时，卡片上方多一条琥珀横幅，
    把人指回设置里的包目录。**没连上 MSFS 时不该出现这条横幅**——那时候根本不注入，
    上面那条 `sim_problem` 已经把话说完了。
  - 观察员模式下不出现这条横幅：他不上 FSD，天上本来就不会有他机。
- §3.8 里「拔掉 X-Plane 那一端，灯应当转灰」这一条，在 msfs 上是关掉 MSFS。
'''
```

**注意 `SECTION` 里的表格用的是全角括号和全角顿号**，和文件其余部分一致。

- [ ] **Step 7: 跑门**

`src-tauri` 动过，所以多两道：

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice/apps/msfs && bun run build
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo check --all-targets
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo clippy --workspace --all-targets -- -D warnings
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo test -p can-voice-i18n
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice && cargo fmt --all --check
```

`common.{zh,en}.json` 一个字都没动，所以**这一任务不需要四个端都 build**。

- [ ] **Step 8: 提交**

```bash
cd /Users/jhl/Documents/Dev/CeruleanAviationNetwork/can-voice
git add apps/msfs/src-tauri/tauri.conf.json apps/msfs/src-tauri/src/lib.rs docs/manual-test.md
git commit -F .superpowers/sdd/2026-09-23-can-audio-layout-4-msfs/commit-msg-task-6.txt
```

**人工要看的**：`cd apps/msfs && bun run tauri dev`。

- 窗口默认 980×660。往小了拖：停在 900×600，拖不进比这更小。
- 底栏：空闲写「就绪」，按住 PTT 写「发话中」，点「帮助 → 检查更新」写「正在检查更新…」，
  查完没有新版写「没有可用的更新。」并在约 4 秒后收回去。刚启动机库还在扫时写「正在扫本机机库…」。
- 「帮助 → 关于」：一个小对话框，程序名、版本号、日志文件路径。**换行要是真的换行**，不是一串 `\n`。
  那段话里应当写着 MSFS 和 SimConnect，**不该出现 X-Plane 或者 UDP**。
- 点「简」：窗口缩到约 460×340，只剩「消息」和「无线电」两张卡片加最底下那条状态栏。
  **状态栏右边补出三格状态文案，第一格是 `MSFS`**；`sim_problem` 那条横幅**仍然在**。
- 再点一次「简」：窗口退回不小于 900×600。
- 设置 → 语言切成 English，菜单栏当场变英文；切回中文同理。
- 中英两种界面语言各看一遍：「无线电」那一行不该挤成三行。

---

# 自动更新设计：四个桌面端在启动时自己装上新版

日期：2026-09-21
状态：已评审，待实施
范围：P0（定决策、定线格式、定密钥去向）。实施计划另写。

---

## 1. 为什么，以及这次推翻了什么

现在四个桌面端只会**报告**有新版，人自己去下载、自己跑安装包。这条路是从
`can-audio` 搬过来的，`crates/can-voice-update/src/lib.rs` 的模块头把理由写成了
一条规矩：

> **绝不自动更新。** 这里只报告，下不下载是人决定的——一个正在值班的管制员不需要
> 一个自作主张重启自己的程序。

**这条规矩这次被推翻，但它担心的事没有消失**，所以整个设计是围着它转的：更新只在
**启动时**发生（§5），那一刻用户还没有连上语音，也就不存在"把一个正在讲话的人踢
下线"这回事。规矩的结论变了，它保护的东西一寸没让。

推翻它的理由是版本收敛。切换日的机制就是"旧客户端自己提示升级"（can-voice 设计文档
§11.2），而一条要人点四次的升级路径，收敛速度取决于最不爱点的那个人。语音是**双方**
的事：一个人留在旧版上，受影响的是所有和他通话的人。

## 2. 决策摘要

| 决策 | 结论 | 理由 |
|---|---|---|
| 自动到什么程度 | 下载、安装、重启，全自动，不问 | 版本收敛 |
| 什么时候 | **只在启动时**，进主界面之前 | 那一刻没有连语音，绕开值班问题 |
| 用什么 | `tauri-plugin-updater` | 见 §3 |
| Linux | AppImage 自更新；**deb / rpm 维持现状的提示** | 插件**能**装它们，但要提权，和"不问人"矛盾。见 §6 |
| macOS | 不涉及——根本没有构建 | 缺 Developer ID |
| 清单与包体 | 都走 can-api 的新路由，不走 GitHub | 大陆连通性，见 §4 |
| 旧路由 | `/api/v1/clients/*` **不动**，继续指 can-audio | 两代产品名相同，见 §4.3 |
| 签名 | minisign，密钥独立于代码签名 | 见 §7 |

## 3. 为什么用官方插件而不自己写

**因为安装包不签名。**

Windows 和 Linux 的包到今天都没有代码签名（2026-09-17 定的，README 的"平台"一段）。
这意味着一个被掉包的安装包和真的那一份，在操作系统看来没有区别——SmartScreen 对两者
的警告一模一样，而我们还在 release 正文里教人点过去。

`tauri-plugin-updater` 在安装之前验一次 **minisign 签名**，私钥只在 CI 里。那是这条
链路上唯一一道真正的门。自己写更新器就要把这道门自己实现一遍，而它恰恰是最不该自己
实现的那部分——一个写错了的签名校验和没有签名校验，外观上完全一致。

代价接受：插件的行为（静默装 NSIS/MSI、就地替换 AppImage、重启）是它定的，我们只在
外面决定"什么时候调它"。

钉的版本是 **`tauri-plugin-updater = "2.12.0"`**（仓库是 `tauri = "2.11.6"`，插件
声明 `tauri = "2.10"`，兼容）。本设计 §4.2 / §6 / §7 的每个字段名和取值都是从这个
版本的源码里读出来的，换版本要重核一遍。

## 4. 清单与包体从哪来

### 4.1 一条规矩先行

`can-voice-update` 模块头四条规矩之首：**走 can-api，不走 GitHub**。不是偏好，是从
大陆拉一个几十 MB 的 GitHub 资产经常卡死，而 `api.ceruleanavi.net` 本来就通。

自动更新把这条规矩的重要性放大了：一次卡死的手动下载，用户会换个时间再试；一次卡死
的自动更新，用户什么都不知道，只是永远停在旧版上。

所以**清单和包体都走 can-api**。

### 4.2 新路由

```
GET /api/v1/voice/update/{client}/{target}/{arch}?current=<version>
```

回一份 Tauri 更新器要的清单，`url` 指向 can-api 自己的中转而不是 GitHub：

```json
{
  "version": "27.0.9",
  "notes": "…",
  "pub_date": "2026-09-22T00:00:00Z",
  "platforms": {
    "windows-x86_64": {
      "signature": "<minisign>",
      "url": "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/windows-x86_64"
    },
    "linux-x86_64": { "signature": "…", "url": "…" }
  }
}
```

以上字段名已对着 `tauri-plugin-updater 2.12.0` 的源码核过（`src/updater.rs` 的
`RemoteRelease` 及其自定义 `Deserialize`），不是照记忆写的：

- `version` 必填，带 `#[serde(alias = "name")]`，解析时 `trim_start_matches('v')`，
  所以 `"v27.0.9"` 也收。
- `notes` 可选。`pub_date` 可选，但**必须是 RFC 3339**——格式不对是反序列化直接失败，
  不是忽略。
- `platforms` 的键是 `"<os>-<arch>"`，也可以带安装器后缀
  （`"linux-x86_64-deb"`、`"windows-x86_64-nsis"`）；插件先查带后缀的，再退回不带的。
- `os`：`windows` / `linux` / `darwin`（**是 `darwin` 不是 `macos`**）。
  `arch`：`i686` / `x86_64` / `armv7` / `aarch64` / `riscv64`。

**URL 占位符有四个，不是三个**：`{{current_version}}`、`{{target}}`、`{{arch}}`、
以及容易漏掉的 **`{{bundle_type}}`**（取值 `appimage` / `deb` / `rpm` / `app` /
`msi` / `nsis`）。

清单还有一种**扁平**形态：插件的反序列化在没有 `platforms` 时，要求顶层直接带
`url` 和 `signature`。**本设计用的是这一种**——请求路径里已经写明了平台，再建一个
只有一个键的 map 没有意义。上面那份带 `platforms` 的例子是插件也接受的另一种，
留着说明键长什么样。

**"没有更新"回 HTTP 204**，插件见 204 直接 `Ok(None)`。回 200 加一份版本号不大于
当前版本的清单也等效，但 204 是显式约定，用它。非 2xx **不算"没有更新"**——插件会
记日志并试下一个 endpoint，全失败才报错。

仍未核实的只有一处：npm 侧 `@tauri-apps/plugin-updater` 的 TypeScript 签名（发布到
crates.io 的包把 guest-js 排除了）。本设计的调用都在 Rust 侧，用不到它。

### 4.3 旧路由不动，而且分不开

`/api/v1/clients/latest` 和 `/api/v1/clients/download/{client}` **保持指向
can-audio**，直到切换日。

原因不是谨慎，是**办不到**：四个产品名 `audio-for-can` / `atis-for-can` /
`xpc-for-can` / `msfs-for-can` 在两代之间是同一组字符串（can-voice 设计文档 §11.3
明确要求沿用）。can-api 拿到一个请求，**没有任何字段能告诉它这是哪一代的客户端**。
所以只能用路由命名空间分开，新的归新的。

切换日那天把旧路由也指过来，或者下线它——那是切换的动作，不是这个设计的动作。

### 4.4 中转现在就是坏的，这次必须一起修

`can-api/internal/release/release.go:207-213`：

```go
for _, asset := range payload.Assets {
    for _, client := range Clients {
        if strings.HasPrefix(asset.Name, client) {
            out.Assets[client] = Asset{…}   // 后一个覆盖前一个
            break
        }
    }
}
```

按**文件名前缀**匹配，每个产品**只留一个**资产，而且是遍历到的最后一个。

can-audio 每个产品只发一个包，所以这个模型一直成立。can-voice 一个产品发五个
（`.exe` / `.msi` / `.deb` / `.rpm` / `.AppImage`），于是 `audio-for-can` 会匹配五次，
留下的是 GitHub 返回顺序里的最后一个——**Linux 用户可能拿到 `.exe`**。还有一个更
干脆的：`xpc-for-can-xplane-plugin.zip` 也以 `xpc-for-can` 开头，会被当成 xpc 的
客户端包发出去。

这不是这次新引入的缺陷，是**切换日把中转指过来的那一刻就会炸**的缺陷。新的 resolver
按 `(产品, 平台)` 建键，旧的那个一并改掉。

## 5. 客户端：只在启动时，而且绝不挡路

顺序：

```
进程起来 → 检查更新 → 有？下载 → 验签 → 装 → 重启
                    ↓ 没有 / 任何一步失败
                 进主界面
```

三条不可让的：

- **在连语音之前。** 启动那一刻用户还没上线，这是整个设计能成立的前提。
- **失败必须安静。** `can-voice-update` 已经写着的规矩原样带过来：任何一步失败都当
  "没有更新"，记一条 INFO 就算完。**绝不能挡住启动**——一个连不上更新服务的人必须
  还能上线值班。更新服务挂掉不该让全网上不了线。
- **运行期间不再管。** 检查一次就不再检查。一个开着八小时的管制端不会在第七个小时
  突然决定重启自己。

界面上要有的只有一句"正在更新…"，因为下载几十 MB 需要时间，而一个卡在启动画面上不说
话的程序看起来像死了。

## 6. 平台矩阵

| 平台 | 包 | 自动更新 |
|---|---|---|
| Windows | `.exe`（NSIS）/ `.msi` | 是 |
| Linux | `.AppImage` | 是 |
| Linux | `.deb` / `.rpm` | **否，退回提示横幅** |
| macOS | —— | 没有构建 |

**deb / rpm 不做，不是因为做不到——这一点本设计的初稿写错了，写在这里免得下一个人
照着错的前提重新决定一次。**

插件**能**装它们：`install_deb` 走 `dpkg -i`、`install_rpm` 走 `rpm -U`，
2.10.0（PR #2624）加的，CHANGELOG 原话是 "Updater plugin now supports all bundle
types: Deb, Rpm and AppImage for Linux"。`tauri build` 也会给这两种产物签名。

不做的理由换成两条，而且第一条是**这份设计自己的第一个决定**逼出来的：

- **装 deb / rpm 要提权。** 插件的做法是 `pkexec` → 图形 sudo（zenity/kdialog）→
  终端 `sudo` 三级回退。而 §2 定的是"全自动、不问人、启动时装"——在 deb / rpm 上
  那等于**每次启动弹一个系统密码框**。这不是体验差一点，是和那个决定直接矛盾：一个
  要输密码的"不问人"更新不存在。
- **`/usr` 是包管理器的地盘。** 就算用户每次都输了密码，下一次 `apt upgrade` 会把
  版本盖回去，而用户看到的是"版本莫名其妙退回去了"，无从查起。

所以 Linux 上自动更新只走 AppImage。**哪天决定改，要先推翻的是"不问人"那一条**，
不是这一条。

判别不要自己发明，**而且这里第一稿又写错了一次**：说插件读的是 `APPIMAGE`
环境变量。它不是。`install_inner` 分派用的是
`tauri_utils::platform::bundle_type()`，读的是 `__TAURI_BUNDLE_TYPE`——**打包时
写进二进制的一个静态字符串**，`tauri build` 按产物种类改写它。环境变量在别处用
（AppImage 的 `executable_path`），不是分派依据。

我们调同一个函数，理由是两处判据一旦不同，就会出现"我们以为是 AppImage 而插件
以为不是"的那一类分歧。没有这个标记的构建（`cargo run` 出来的）返回 `None`，
两边都当作不自动更新。

判别本身因此不是一个可以单测的纯函数——它读的是链接期的东西。能测的是那一层
之上的决定：`self_replaceable(bundle: Option<&str>)`，六种取值各一条。

deb / rpm 上退回现有的 `can-voice-update` 提示横幅——那条路径**不删**，它现在是
Linux 一半用户唯一的更新通知。

## 7. 签名与密钥

一对 **minisign** 密钥，和代码签名无关（所以"暂不签名"那个决定不挡这件事）。

- 生成：`tauri signer generate -w <路径>`（CI 场景加 `--ci` 跳过交互）
- 私钥 + 口令：can-voice 的 Actions secrets（`TAURI_SIGNING_PRIVATE_KEY`、
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`）。前者既收私钥内容也收私钥文件路径
- 公钥：四份 `tauri.conf.json` 的 **`plugins.updater.pubkey`**，跟着客户端一起发出去
- `.sig` 的命名是**在原文件名后直接追加 `.sig`**（`app_27.0.9_amd64.AppImage.tar.gz`
  → `….tar.gz.sig`），所以中转按名字找签名时不要去掉原扩展名
- 签名的 trusted comment 里带版本号，插件用它防降级

四个 `src-tauri/capabilities/*.json` 还要加权限，否则命令调不通：`"updater:default"`
（等于 `allow-check` / `allow-download` / `allow-install` /
`allow-download-and-install` 四项）。

**丢了私钥的后果要写在这里**：所有已经装出去的客户端都持着那个公钥，换一对新的就意味着
它们会拒绝之后的每一次更新——**所有人只能手动重装一遍**。它和 `VOICE_TOKEN_KEY`、
TLS 证书是同一类东西，备份放在一起。

## 8. 测试

| 层 | 方法 |
|---|---|
| 清单形状 | Go 表驱动：给定一组 release 资产，断言清单里每个平台的 `url` 和 `signature` |
| 按平台建键 | Go：**含 `xpc-for-can-xplane-plugin.zip` 这个陷阱**，断言它不会被当成 xpc 的包 |
| 两代不串 | Go：断言 `/api/v1/clients/*` 仍解析 can-audio，新路由解析 can-voice |
| 缺签名 | Go：资产旁边没有 `.sig` 时，那个平台**不出现在清单里**，而不是给一份没有签名的 |
| 平台判别 | Rust 纯函数：AppImage / deb / rpm / Windows 各一条 |
| 不挡启动 | Rust：更新检查返回错误时，启动流程照常走完 |
| 端到端 | 自动化不了。进 `docs/manual-test.md`：装一个旧版、发一个新版、开一次，确认它自己换好了 |

## 9. 风险与未决

- **第一次真正的验证只能靠手工。** 自动更新这件事，写得对不对要等到有一个真的旧版在
  真的机器上自己换成新版才知道。它和封闭测试（R6）应该排在一起做。
- ~~插件的确切接口没有对着版本核过。~~ **已核**：`tauri-plugin-updater 2.12.0`
  的 vendored 源码，结论进了 §4.2、§6、§7。核的过程推翻了初稿里"deb/rpm 不支持"
  这条事实——**结论没变，理由全换了**，见 §6。
- **Windows 的重启由安装器做，不是我们做。** 插件在 Windows 上安装完直接
  `std::process::exit(0)`，由 NSIS / MSI 把新版本拉起来（`restart_after_install`
  默认为真，当前命令行参数经 `/ARGS` 或 `LAUNCHAPPARGS` 传过去）。**Linux 上插件
  不会自己重启**，要自己调 `tauri::process::restart`。两个平台走两条路，这一处是
  "写一遍两边都对"最容易出错的地方。
- **更新失败会静默。** 这是有意的（§5），代价是"一批人停在旧版上"没有任何声音。
  can-api 侧应当能看出清单被谁取过——但**日志之外不做遥测**，这个网络没有那种东西。
- **`const repo` 的改动落在切换日**，不在本设计内。新路由自己解析 can-voice 的
  release，和旧常量互不影响。
- **整条路现在是哑的，而且哑得没有声音。** `releases/latest` **不返回预发布**，
  而版本号规则把 `YY = 0` 留给了预发布——`v27.0.1` 到 `v27.0.8` 它一个都看不见，
  GitHub 回 404，解析器缓存 nil，清单路由永远回 204。第一个这条路看得见的版本是
  `v27.1.0`。
  从 can-api 那边看，这个 404 和"GitHub 挂了"**长得一模一样**，两者都变成 204——
  这是 §5"失败要安静"要的，代价是没有任何东西会说出"这条路是哑的"。
  **所以端到端那一条在有正式版之前不能测**：拿预发布去测，看起来会像通过了。

## 10. 不做的

- 增量 / 差分更新。包才几 MB（AppImage 77 MB 是个例外），不值得一套 patch 机制。
- 回滚到旧版。装错了就重装，这个网络没有需要"降级"的规模。
- 运行期间检查更新。见 §5 第三条。
- deb / rpm 的自动更新。见 §6。
- macOS。没有构建，签名和公证那一套还没走。

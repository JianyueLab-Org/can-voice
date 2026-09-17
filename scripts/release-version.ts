// 发版的版本号只从 tag 来。
//
//     bun scripts/release-version.ts v27.0.4            # 校验，打印 version= / prerelease=
//     bun scripts/release-version.ts v27.0.4 --write    # 另外把版本写进四个客户端
//
// # 为什么要注入
//
// 一个客户端的版本写在三个地方：`Cargo.toml`（`env!("CARGO_PKG_VERSION")`，
// 更新检查、日志头和回传报的都是它）、`tauri.conf.json`（安装包文件名、MSI 的
// ProductVersion）、`package.json`（运行时没人读，一起改是为了不留一处对不上的）。
// 而 release 页面和 can-api 的 `/api/v1/clients/latest` 报的是 **tag**
// （`can-api/internal/release/release.go` 里 `TrimPrefix(tag_name, "v")`）。
//
// 这两边此前没有任何联系：tag 打成 v27.0.4 而仓库里还是 27.0.3，客户端就永远
// 觉得自己落后一版，天天弹更新，装完还弹。所以打包前由工作流按 tag 改写，
// 仓库里那个数只是本地构建用的占位。
//
// # 预发布怎么定
//
// 版本号是 `vXX.YY.ZZ`，**`YY = 0` 保留给预发布**，正式版从 1 数起。所以
// `YY = 0` 发 prerelease，否则发正式版。这一条不是装饰：can-api 取的是
// GitHub 的 `/releases/latest`，而它跳过 prerelease——一律 prerelease 的话，
// can-api 切到这个仓库那天下载和更新检查都会 503。

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const APPS = ["controller", "atis", "xpc", "msfs"] as const;

export interface Parsed {
  version: string;
  prerelease: boolean;
}

/**
 * 只认 `v主.次.补丁`，不认 `-beta.1` 这类后缀，也不认前导零——`v27.01.0` 会让
 * tag 和写进包里的 `27.1.0` 字面上对不上，而这个脚本存在就是为了让两者一致。
 *
 * 后缀不认是 MSI 的限制：WiX 的 ProductVersion 只能是数字，Tauri 遇到非数字的
 * 预发布标识会在打 MSI 那一步才失败——也就是八个构建跑完一半之后。上限同理：
 * 主、次版本不超过 255，补丁不超过 65535。
 */
export function parseTag(tag: string): Parsed {
  const m = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(tag);
  if (!m) {
    throw new Error(`tag must look like v27.1.0, got ${JSON.stringify(tag)}`);
  }
  const [major, minor, patch] = m.slice(1).map(Number);
  if (major > 255 || minor > 255 || patch > 65535) {
    throw new Error(`${tag} is out of range for an MSI version (255.255.65535)`);
  }
  return { version: `${major}.${minor}.${patch}`, prerelease: minor === 0 };
}

/** 把 `pattern` 唯一命中的那一处换成 `version`。命中数不是 1 就是文件形状变了，停。 */
function replaceOnce(path: string, pattern: RegExp, version: string) {
  const text = readFileSync(path, "utf8");
  const hits = [...text.matchAll(new RegExp(pattern.source, pattern.flags + "g"))];
  if (hits.length !== 1) {
    throw new Error(`${path}: expected exactly one version field, found ${hits.length}`);
  }
  writeFileSync(path, text.replace(pattern, `$1${version}$2`));
}

export function writeVersion(root: string, version: string) {
  for (const app of APPS) {
    const dir = join(root, "apps", app);
    // 顶层键两格缩进。嵌套对象里的 "version" 不会是两格，所以不会误伤。
    replaceOnce(join(dir, "src-tauri", "tauri.conf.json"), /^(  "version": ")[^"]*(")/m, version);
    replaceOnce(join(dir, "package.json"), /^(  "version": ")[^"]*(")/m, version);
    // 只动 [package] 里那一行。依赖写成 `x = { version = "2" }` 不在行首，
    // `[dependencies.x]` 表里的 version 在另一个表头之后，都匹配不到。
    //
    // `\r?`：Windows 的 runner 默认 `core.autocrlf=true`，检出来是 CRLF。
    replaceOnce(
      join(dir, "src-tauri", "Cargo.toml"),
      /^(\[package\]\r?\n(?:(?!\[)[^\n]*\n)*?version = ")[^"]*(")/m,
      version,
    );
  }
}

if (import.meta.main) {
  const [tag, flag] = process.argv.slice(2);
  if (!tag || (flag !== undefined && flag !== "--write")) {
    console.error("usage: bun scripts/release-version.ts <vX.Y.Z> [--write]");
    process.exit(2);
  }
  const { version, prerelease } = parseTag(tag);
  if (flag === "--write") {
    writeVersion(join(import.meta.dir, ".."), version);
  }
  // 形状就是 $GITHUB_OUTPUT 要的 key=value，工作流直接 >> 进去。
  console.log(`version=${version}`);
  console.log(`prerelease=${prerelease}`);
}

import { describe, expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { parseTag, writeVersion } from "./release-version";

describe("parseTag", () => {
  test("YY = 0 是预发布，其余是正式版", () => {
    expect(parseTag("v27.0.4")).toEqual({ version: "27.0.4", prerelease: true });
    expect(parseTag("v27.1.0")).toEqual({ version: "27.1.0", prerelease: false });
    expect(parseTag("v28.12.3")).toEqual({ version: "28.12.3", prerelease: false });
  });

  test.each(["27.1.0", "v27.1", "v27.1.0-beta.1", "v27.1.0 ", "v27.01.0", "release/v27.1.0", ""])(
    "拒绝 %p",
    (tag) => {
      expect(() => parseTag(tag)).toThrow();
    },
  );

  test("拒绝 MSI 装不下的版本", () => {
    expect(() => parseTag("v256.1.0")).toThrow(/MSI/);
    expect(() => parseTag("v27.1.65536")).toThrow(/MSI/);
    expect(parseTag("v255.255.65535").version).toBe("255.255.65535");
  });
});

describe("writeVersion", () => {
  const repo = join(import.meta.dir, "..");

  function sandbox(): string {
    // 项目内的 .temp/，不是系统临时目录。CI 上它还不存在。
    mkdirSync(join(repo, ".temp"), { recursive: true });
    const root = mkdtempSync(join(repo, ".temp", "release-version-"));
    for (const app of ["controller", "atis", "xpc", "msfs"]) {
      for (const file of ["package.json", "src-tauri/tauri.conf.json", "src-tauri/Cargo.toml"]) {
        cpSync(join(repo, "apps", app, file), join(root, "apps", app, file));
      }
    }
    return root;
  }

  test("四个客户端的三处都改，别的 version 一个不动", () => {
    const root = sandbox();
    try {
      writeVersion(root, "27.1.0");
      for (const app of ["controller", "atis", "xpc", "msfs"]) {
        const cargo = readFileSync(join(root, "apps", app, "src-tauri/Cargo.toml"), "utf8");
        const before = readFileSync(join(repo, "apps", app, "src-tauri/Cargo.toml"), "utf8");
        expect(cargo).toContain('\nversion = "27.1.0"\n');
        // 依赖里的 `version = "2"` 之类原样保留：只有 [package] 那一行变了。
        expect(cargo.split("\n").filter((l, i) => l !== before.split("\n")[i])).toEqual([
          'version = "27.1.0"',
        ]);
        for (const file of ["package.json", "src-tauri/tauri.conf.json"]) {
          const json = JSON.parse(readFileSync(join(root, "apps", app, file), "utf8"));
          expect(json.version).toBe("27.1.0");
        }
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("CRLF 检出（Windows runner）也改得动", () => {
    const root = sandbox();
    try {
      const path = join(root, "apps", "xpc", "src-tauri/Cargo.toml");
      writeFileSync(path, readFileSync(path, "utf8").replace(/\n/g, "\r\n"));
      writeVersion(root, "27.1.0");
      expect(readFileSync(path, "utf8")).toContain('\r\nversion = "27.1.0"\r\n');
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("文件形状变了就停，而不是悄悄改错地方", () => {
    const root = sandbox();
    try {
      const path = join(root, "apps", "atis", "package.json");
      writeFileSync(path, readFileSync(path, "utf8").replace('  "version"', '  "ver"'));
      expect(() => writeVersion(root, "27.1.0")).toThrow(/package\.json.*found 0/);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});

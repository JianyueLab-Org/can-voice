# Auto-Update Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The four can-voice desktop clients install a newer version of themselves at startup, before the user is on voice, with no prompt.

**Architecture:** `tauri-plugin-updater` does the download, signature check and install. can-api gains one route family that answers a Tauri update manifest and relays the payload, so nothing touches GitHub from a user's machine. On Linux only AppImage self-installs; deb and rpm fall back to the banner that exists today, pointed at the same new route.

**Tech Stack:** Go 1.26.5 (can-api), Rust 2021 / rust-version 1.80 (can-voice), Tauri 2.11.5, `tauri-plugin-updater` 2.12.0, Vue 3 + Vite, minisign.

**Spec:** `can-voice/docs/superpowers/specs/2026-09-21-auto-update-design.md`

**Repos:** This plan spans two repositories. Tasks 1-6 land in `can-api` (`github.com/JianyueLab/can-api`); tasks 7-11 land in `can-voice` (`JianyueLab-Org/can-voice`). Each is committed and pushed in its own repo; the monorepo pointer moves afterwards.

## Global Constraints

- Plugin version is pinned: `tauri-plugin-updater = "2.12.0"`. Every field name and behaviour below was read out of that version's source. Changing the version means re-reading it.
- Scratch files go in `<repo>/.temp/`. Never `/tmp`, `/private/tmp` or `$TMPDIR`. Delete the directory when the task is done.
- Code comments and commit messages in English. UI strings follow the app's existing JSON dictionaries (zh + en).
- Commit messages end with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- can-api gate: `gofmt -l .` must be empty, `go vet ./...`, `go test ./...`, `go test -race ./...`.
- can-voice gate: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`; plus `cd apps/<app>/src-tauri && cargo check --all-targets && cargo clippy --all-targets -- -D warnings` for each of the four apps, and `bun run build` in each app.
- `/api/v1/clients/latest` and `/api/v1/clients/download/{client}` keep resolving `JianyueLab-Org/can-audio`. Nothing in this plan changes them beyond adding a regression test.
- The four product names are fixed strings shared with can-audio: `audio-for-can`, `atis-for-can`, `xpc-for-can`, `msfs-for-can`. `audio-for-can` is the ATC client.
- Failure is silent. Every error path in the client logs at INFO and proceeds to the main interface. Nothing about updating may prevent a controller from signing on.
- The update check runs once, at startup, before the user connects. Never during a session.

---

## File Structure

### can-api

| File | Responsibility |
|---|---|
| `internal/release/release.go` | Unchanged except a doc note. Legacy can-audio resolver; dies at cutover. |
| `internal/release/testdata/can-audio-latest.json` | **Create.** Recorded GitHub payload pinning today's legacy behaviour. |
| `internal/release/voice_asset.go` | **Create.** `Platform`, `classify()` — asset filename to platform key. Pure, no I/O. |
| `internal/release/voice_asset_test.go` | **Create.** The filename table, including the `xpc-for-can-xplane-plugin.zip` trap. |
| `internal/release/voice.go` | **Create.** `VoiceResolver`: fetches the can-voice release, pairs each payload with its `.sig`, caches. |
| `internal/release/voice_test.go` | **Create.** Resolver behaviour against a stubbed fetch. |
| `internal/api/voiceupdate.go` | **Create.** `handleVoiceUpdate` (manifest) and `handleVoiceDownload` (relay). |
| `internal/api/voiceupdate_test.go` | **Create.** Manifest shape, the 204 rules, the unknown-bundle case. |
| `internal/api/clients.go` | Modify: extract the streaming half of `handleClientDownload` into `relayAsset`. |
| `internal/api/server.go` | Modify: `voiceReleases` field, construction, four route registrations. |
| `internal/ratelimit/ratelimit.go` | Modify: `VoiceUpdate`, `VoiceDownload` rules. |
| `internal/api/routes_test.go` | Modify: rows for the new routes. |
| `Readme.md` | Modify: the public wire contracts list. |

### can-voice

| File | Responsibility |
|---|---|
| `crates/can-voice-update/src/lib.rs` | Modify: `self_replaceable()`, `check()` reads the Tauri manifest shape, `Latest` gains `url`. |
| `crates/can-voice-autoupdate/` | **Create.** The Tauri glue: plugin wiring, the startup sequence, the state events. Excluded from the workspace because it pulls in `tauri`. |
| `apps/*/src-tauri/Cargo.toml` | Modify: `tauri-plugin-updater`, `can-voice-autoupdate`. |
| `apps/*/src-tauri/tauri.conf.json` | Modify: `bundle.createUpdaterArtifacts`, `plugins.updater`. |
| `apps/*/src-tauri/capabilities/default.json` | **Create.** `updater:default`. |
| `apps/*/src-tauri/src/lib.rs` | Modify: register the plugin, start the sequence in `.setup()`. |
| `apps/*/src/App.vue` | Modify: gate the interface on the startup state. |
| `apps/*/src/components/StartupGate.vue` | **Create.** The "正在更新…" screen. Byte-identical across the four apps. |
| `apps/*/src/locales/common.{zh,en}.json` | Modify: the `update.*` namespace. Byte-identical across the four apps. |
| `.github/workflows/release.yml` | Modify: signing env, harvest `.sig`. |
| `docs/manual-test.md` | Modify: the end-to-end update run. |
| `docs/superpowers/specs/2026-09-21-auto-update-design.md` | Modify: §6's detection claim is wrong. |

---

## Wire Contract

Fixed here once; every task below refers back to it.

**Manifest request** — what the plugin sends, with its own placeholders filled in:

```
GET /api/v1/voice/update/{client}/{target}/{arch}/{bundle}?current={version}
```

- `{client}` is a literal in each app's config, one of the four product names.
- `{target}` is `windows` | `linux` | `darwin`. The plugin writes `darwin`, not `macos`.
- `{arch}` is `i686` | `x86_64` | `armv7` | `aarch64` | `riscv64`.
- `{bundle}` is `nsis` | `msi` | `appimage` | `deb` | `rpm` | `app`, or the literal `unknown` when the binary carries no bundle stamp (an unbundled `cargo run` build).
- `current` is the running version.

**Manifest response** — the flat form of `RemoteRelease`, chosen because the request already names the platform:

```json
{
  "version": "27.0.9",
  "notes": "…",
  "pub_date": "2026-09-22T00:00:00Z",
  "url": "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/windows-x86_64-nsis",
  "signature": "dW50cnVzdGVkIGNvbW1lbnQ6…"
}
```

`platforms` is deliberately absent. The plugin's deserializer takes `platforms` when present and otherwise requires top-level `url` and `signature`; with one platform per request there is nothing to key.

`pub_date` must be RFC 3339 or the plugin fails deserialization outright. GitHub's `published_at` already is. Omit the field rather than send an empty string.

**204 No Content** is the only "nothing to do" answer. The plugin logs it and returns `Ok(None)`. Any other non-2xx makes the plugin try the next endpoint and then error, so every ordinary miss must be 204:

- the release cannot be resolved
- `{bundle}` is `unknown` or not one we ship
- that `(client, target, arch, bundle)` has no asset in the release
- the asset has no `.sig` beside it
- the release version is not newer than `current`

**Payload request:**

```
GET|HEAD /api/v1/voice/download/{client}/{platform}
```

`{platform}` is the key `classify()` produces: `"<os>-<arch>-<bundle>"`, e.g. `linux-x86_64-appimage`.

---

## Task 1: Pin the legacy relay before touching anything near it

The can-audio resolver keeps working unchanged. This task only builds the net: a recorded payload proving the four products still resolve, so later tasks in this package cannot break a live download route silently.

**Files:**
- Create: `internal/release/testdata/can-audio-latest.json`
- Create: `internal/release/legacy_test.go`

**Interfaces:**
- Consumes: `release.Resolver`, `release.Release`, `release.Asset`, `release.Clients` (all existing).
- Produces: nothing. Test-only.

- [ ] **Step 1: Record the live payload**

```bash
mkdir -p .temp
gh api repos/JianyueLab-Org/can-audio/releases/latest > .temp/can-audio-latest.json
jq '{tag_name, published_at, html_url, assets: [.assets[] | {name, size, browser_download_url}]}' \
  .temp/can-audio-latest.json > internal/release/testdata/can-audio-latest.json
cat internal/release/testdata/can-audio-latest.json
```

Read the asset names that come back. They are the ground truth for the next step.

- [ ] **Step 2: Write the test**

`fetchUpstream` is unexported and takes no payload, so the test decodes the fixture the same way it does and runs the same matching loop through the public surface. Use the `fetch` seam.

```go
package release

import (
	"context"
	"encoding/json"
	"os"
	"testing"
)

// The can-audio release is what `/api/v1/clients/*` still resolves, and will
// until the cutover. The fixture is a recorded GitHub payload: if a change in
// this package stops one of the four products resolving, that is a broken
// download for every member on the current clients, and nothing else here
// would fail.
func TestTheLegacyReleaseStillResolvesAllFourProducts(t *testing.T) {
	raw, err := os.ReadFile("testdata/can-audio-latest.json")
	if err != nil {
		t.Fatalf("reading the recorded release: %v", err)
	}

	var payload githubRelease
	if err := json.Unmarshal(raw, &payload); err != nil {
		t.Fatalf("the recorded release no longer decodes: %v", err)
	}

	r, _ := stubbed(func(context.Context) (*Release, error) {
		return releaseFromPayload(payload), nil
	})

	latest := r.Latest(context.Background())
	if latest == nil {
		t.Fatal("Latest() = nil for a release that decoded")
	}

	for _, client := range Clients {
		asset, ok := latest.Assets[client]
		if !ok {
			t.Errorf("%s has no asset — that product is a 404 at the download relay", client)
			continue
		}
		if asset.Href == "" || asset.Size == 0 {
			t.Errorf("%s resolved to %+v — an asset with no href or no size is not downloadable", client, asset)
		}
	}
}
```

- [ ] **Step 3: Run it and watch it fail**

Run: `go test ./internal/release/ -run TestTheLegacyReleaseStillResolvesAllFourProducts -v`

Expected: FAIL — `undefined: releaseFromPayload`. The matching loop is inline in `fetchUpstream` and cannot be called on a decoded payload.

- [ ] **Step 4: Extract the loop, no behaviour change**

In `internal/release/release.go`, cut the body of `fetchUpstream` from the construction of `out` through the asset loop into a new function, and call it. The loop itself is copied verbatim — this is a move, not a fix.

```go
// releaseFromPayload turns a decoded GitHub release into ours. Split out of
// fetchUpstream so a recorded payload can be run through the same matching the
// live path uses.
func releaseFromPayload(payload githubRelease) *Release {
	out := &Release{
		Version:     strings.TrimPrefix(payload.TagName, "v"),
		PublishedAt: payload.PublishedAt,
		Href:        payload.HTMLURL,
		Assets:      make(map[string]Asset, len(Clients)),
	}

	// Assets are matched by name *prefix*, because the published filename
	// carries the version and the platform (`atis-for-can-2.0.1-macos.zip`).
	for _, asset := range payload.Assets {
		for _, client := range Clients {
			if strings.HasPrefix(asset.Name, client) {
				out.Assets[client] = Asset{Name: asset.Name, Size: asset.Size, Href: asset.BrowserDownloadURL}
				break
			}
		}
	}

	return out
}
```

`fetchUpstream` now ends with `return releaseFromPayload(payload), nil`.

- [ ] **Step 5: Run it and watch it pass**

Run: `go test ./internal/release/ -v`

Expected: PASS, all ten existing tests included.

- [ ] **Step 6: Commit**

```bash
rm -rf .temp
gofmt -l .
git add internal/release/testdata/can-audio-latest.json internal/release/legacy_test.go internal/release/release.go
git commit -m "$(cat <<'MSG'
test: pin the can-audio relay against a recorded release

Splits releaseFromPayload out of fetchUpstream so a fixture can run
through the same asset matching the live path uses.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 2: Classify a can-voice asset filename

can-voice publishes five bundles per product where can-audio published one, so the prefix match the legacy resolver uses cannot be reused: `audio-for-can` matches all five and the last one wins. It also matches `xpc-for-can-xplane-plugin.zip`, which is an X-Plane plugin and not a client at all.

This task is the pure function. No network, no resolver.

**Files:**
- Create: `internal/release/voice_asset.go`
- Create: `internal/release/voice_asset_test.go`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `type Platform struct { OS, Arch, Bundle string }`
  - `func (p Platform) Key() string` — `"linux-x86_64-appimage"`
  - `func ParsePlatform(key string) (Platform, bool)` — the inverse, for the download route
  - `func classify(client, name string) (Platform, bool)` — unexported; used by Task 3

- [ ] **Step 1: Write the failing test**

The names are Tauri's defaults, derived from `productName` and `version`. Note rpm uses `-` where the others use `_`, and that its arch token is `x86_64` where deb's is `amd64`.

```go
package release

import "testing"

// Every name here is one tauri-bundler actually produces for productName
// `audio-for-can` at version 27.0.9, plus the two traps.
func TestClassifyReadsTheBundleOutOfTheFilename(t *testing.T) {
	cases := []struct {
		name string
		want string // Platform.Key(), or "" for "not a bundle of this client"
	}{
		{"audio-for-can_27.0.9_x64-setup.exe", "windows-x86_64-nsis"},
		{"audio-for-can_27.0.9_x64_en-US.msi", "windows-x86_64-msi"},
		{"audio-for-can_27.0.9_amd64.AppImage", "linux-x86_64-appimage"},
		{"audio-for-can_27.0.9_amd64.deb", "linux-x86_64-deb"},
		{"audio-for-can-27.0.9-1.x86_64.rpm", "linux-x86_64-rpm"},
		{"audio-for-can_27.0.9_arm64-setup.exe", "windows-aarch64-nsis"},
		{"audio-for-can_27.0.9_aarch64.AppImage", "linux-aarch64-appimage"},

		// A signature is not a payload. The resolver pairs them by name; if
		// classify claimed them too they would overwrite the thing they sign.
		{"audio-for-can_27.0.9_amd64.AppImage.sig", ""},

		// The v1-compatible wrapper, in case createUpdaterArtifacts is ever
		// set to "v1Compatible". install_appimage unwraps gzip itself.
		{"audio-for-can_27.0.9_amd64.AppImage.tar.gz", "linux-x86_64-appimage"},

		// Another product's bundle.
		{"atis-for-can_27.0.9_amd64.deb", ""},

		// Belongs to no product: it is the X-Plane plugin, and it starts with
		// a product name. Prefix matching alone hands this to a user as the
		// xpc client.
		{"xpc-for-can-xplane-plugin.zip", ""},

		// No version where a version must be.
		{"audio-for-cannon_27.0.9_amd64.deb", ""},
		{"audio-for-can-readme.txt", ""},
	}

	for _, tc := range cases {
		got, ok := classify("audio-for-can", tc.name)
		key := ""
		if ok {
			key = got.Key()
		}
		if key != tc.want {
			t.Errorf("classify(%q) = %q, want %q", tc.name, key, tc.want)
		}
	}
}

func TestClassifyRejectsAProductThatIsAPrefixOfAnother(t *testing.T) {
	// `xpc-for-can` is a prefix of `xpc-for-can-xplane-plugin`, which is why
	// the separator rule exists. Assert it from the other side too.
	if _, ok := classify("xpc-for-can", "xpc-for-can-xplane-plugin.zip"); ok {
		t.Error("the X-Plane plugin classified as an xpc client bundle")
	}
	if _, ok := classify("xpc-for-can", "xpc-for-can_27.0.9_amd64.deb"); !ok {
		t.Error("the real xpc deb did not classify")
	}
}

func TestPlatformKeyRoundTrips(t *testing.T) {
	for _, key := range []string{
		"windows-x86_64-nsis",
		"linux-x86_64-appimage",
		"linux-aarch64-rpm",
	} {
		p, ok := ParsePlatform(key)
		if !ok {
			t.Fatalf("ParsePlatform(%q) failed", key)
		}
		if p.Key() != key {
			t.Errorf("ParsePlatform(%q).Key() = %q", key, p.Key())
		}
	}

	for _, bad := range []string{"", "linux", "linux-x86_64", "linux-x86_64-appimage-extra", "linux-x86_64-tarball"} {
		if _, ok := ParsePlatform(bad); ok {
			t.Errorf("ParsePlatform(%q) = ok — the download route would accept a key nothing produces", bad)
		}
	}
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `go test ./internal/release/ -run 'TestClassify|TestPlatformKey' -v`

Expected: FAIL — `undefined: classify`, `undefined: ParsePlatform`, `undefined: Platform`.

- [ ] **Step 3: Write the implementation**

```go
package release

import (
	"strings"
)

// Platform is one downloadable build: an operating system, an architecture and
// the kind of installer. can-voice ships five bundles per product where
// can-audio shipped one, so a product name alone no longer names a file.
type Platform struct {
	OS     string // windows | linux | darwin — the plugin writes darwin, not macos
	Arch   string // i686 | x86_64 | armv7 | aarch64 | riscv64
	Bundle string // nsis | msi | appimage | deb | rpm | app
}

// Key is the form used in the download route and in the platforms map of a
// Tauri manifest.
func (p Platform) Key() string { return p.OS + "-" + p.Arch + "-" + p.Bundle }

// bundleSuffix maps a filename ending to its bundle kind and operating system.
// Order matters: `.AppImage.tar.gz` has to be tried before `.tar.gz` would be,
// and `-setup.exe` is the NSIS installer while a bare `.exe` is not a bundle.
var bundleSuffix = []struct {
	suffix string
	os     string
	bundle string
}{
	{".AppImage.tar.gz", "linux", "appimage"},
	{".AppImage", "linux", "appimage"},
	{".deb", "linux", "deb"},
	{".rpm", "linux", "rpm"},
	{"-setup.exe", "windows", "nsis"},
	{"-setup.nsis.zip", "windows", "nsis"},
	{".msi", "windows", "msi"},
	{".msi.zip", "windows", "msi"},
	{".app.tar.gz", "darwin", "app"},
}

// archToken maps every spelling a bundler uses to the spelling the updater
// plugin asks for. deb says amd64, rpm says x86_64 and the NSIS filename says
// x64 — all three are the same machine.
var archToken = map[string]string{
	"x64":     "x86_64",
	"amd64":   "x86_64",
	"x86_64":  "x86_64",
	"x86":     "i686",
	"i686":    "i686",
	"arm64":   "aarch64",
	"aarch64": "aarch64",
	"armv7":   "armv7",
	"armhf":   "armv7",
	"riscv64": "riscv64",
}

// classify decides whether name is a bundle of client, and of which platform.
//
// Matching is not a plain prefix test. `xpc-for-can-xplane-plugin.zip` starts
// with a product name and is not a client build, so what follows the product
// name must be a separator and then a digit — the version tauri puts there.
func classify(client, name string) (Platform, bool) {
	rest, ok := strings.CutPrefix(name, client)
	if !ok || len(rest) < 2 {
		return Platform{}, false
	}
	if rest[0] != '_' && rest[0] != '-' {
		return Platform{}, false
	}
	if rest[1] < '0' || rest[1] > '9' {
		return Platform{}, false
	}

	for _, s := range bundleSuffix {
		if !strings.HasSuffix(rest, s.suffix) {
			continue
		}
		arch, ok := archIn(strings.TrimSuffix(rest, s.suffix))
		if !ok {
			return Platform{}, false
		}
		return Platform{OS: s.os, Arch: arch, Bundle: s.bundle}, true
	}

	return Platform{}, false
}

// archIn finds the architecture token in the part of the filename between the
// product name and the extension. Tokens are separated by `_`, `-` or `.`,
// which is why the version itself never matches: it carries no letters.
func archIn(stem string) (string, bool) {
	for _, field := range strings.FieldsFunc(stem, func(r rune) bool {
		return r == '_' || r == '-' || r == '.'
	}) {
		if arch, ok := archToken[strings.ToLower(field)]; ok {
			return arch, true
		}
	}
	return "", false
}

// ParsePlatform reads back what Key wrote. The download route takes the key
// from the caller, so it validates rather than trusting it.
func ParsePlatform(key string) (Platform, bool) {
	parts := strings.Split(key, "-")
	if len(parts) != 3 {
		return Platform{}, false
	}
	p := Platform{OS: parts[0], Arch: parts[1], Bundle: parts[2]}

	okOS := p.OS == "windows" || p.OS == "linux" || p.OS == "darwin"
	_, okArch := archToken[p.Arch]
	okBundle := false
	for _, s := range bundleSuffix {
		if s.bundle == p.Bundle {
			okBundle = true
			break
		}
	}
	if !okOS || !okArch || !okBundle {
		return Platform{}, false
	}
	return p, true
}
```

- [ ] **Step 4: Run it and watch it pass**

Run: `go test ./internal/release/ -v`

Expected: PASS. `archToken` maps `x86_64` to itself, so `ParsePlatform` accepts exactly what `Key` emits.

- [ ] **Step 5: Commit**

```bash
gofmt -l .
git add internal/release/voice_asset.go internal/release/voice_asset_test.go
git commit -m "$(cat <<'MSG'
feat(release): classify a can-voice asset filename into a platform

A product name is no longer enough to name a file: can-voice publishes
five bundles per product. The separator-then-digit rule is what keeps
xpc-for-can-xplane-plugin.zip out of the xpc client's bundles.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 3: The can-voice release resolver

Fetches `JianyueLab-Org/can-voice`'s latest release, classifies every asset, pairs each payload with its `.sig` sibling, and fetches the signature bodies. A payload whose signature is missing or unreadable is dropped: an unsigned entry in the manifest is one the plugin will refuse anyway, and dropping it turns a hard error into a quiet 204.

**Files:**
- Create: `internal/release/voice.go`
- Create: `internal/release/voice_test.go`

**Interfaces:**
- Consumes: `Platform`, `classify` (Task 2); `githubRelease`, `successTTL`, `failureTTL`, `fetchLimit` (existing, unexported in the same package).
- Produces:
  - `type VoiceBuild struct { Name string; Size int64; Href string; Signature string }`
  - `type VoiceRelease struct { Version, PublishedAt, Notes string; Builds map[string]VoiceBuild }` — keyed by `Platform.Key()`
  - `func NewVoiceResolver(token string) *VoiceResolver`
  - `func (r *VoiceResolver) Latest(ctx context.Context) *VoiceRelease`

- [ ] **Step 1: Write the failing test**

```go
package release

import (
	"context"
	"testing"
	"time"
)

func voiceStub(fn func(context.Context) (*VoiceRelease, error)) (*VoiceResolver, *int) {
	calls := 0
	r := NewVoiceResolver("")
	r.fetch = func(ctx context.Context) (*VoiceRelease, error) {
		calls++
		return fn(ctx)
	}
	return r, &calls
}

func sampleAssets() []githubAsset {
	return []githubAsset{
		{Name: "audio-for-can_27.0.9_amd64.AppImage", Size: 80 << 20, BrowserDownloadURL: "https://example.invalid/a"},
		{Name: "audio-for-can_27.0.9_amd64.AppImage.sig", Size: 200, BrowserDownloadURL: "https://example.invalid/a.sig"},
		{Name: "audio-for-can_27.0.9_x64-setup.exe", Size: 9 << 20, BrowserDownloadURL: "https://example.invalid/w"},
		{Name: "audio-for-can_27.0.9_x64-setup.exe.sig", Size: 200, BrowserDownloadURL: "https://example.invalid/w.sig"},
		// deb ships, but nothing signed it. It must not reach the manifest.
		{Name: "audio-for-can_27.0.9_amd64.deb", Size: 7 << 20, BrowserDownloadURL: "https://example.invalid/d"},
		{Name: "xpc-for-can-xplane-plugin.zip", Size: 1 << 20, BrowserDownloadURL: "https://example.invalid/p"},
	}
}

func TestAPayloadWithoutASignatureIsNotOffered(t *testing.T) {
	sigs := map[string]string{
		"https://example.invalid/a.sig": "sig-appimage",
		"https://example.invalid/w.sig": "sig-nsis",
	}
	rel := voiceReleaseFrom(githubRelease{
		TagName: "v27.0.9", PublishedAt: "2026-09-22T00:00:00Z", Body: "notes",
		Assets: sampleAssets(),
	}, func(_ context.Context, href string) (string, bool) {
		body, ok := sigs[href]
		return body, ok
	})

	if got := rel.Builds["linux-x86_64-appimage"].Signature; got != "sig-appimage" {
		t.Errorf("appimage signature = %q, want the fetched body", got)
	}
	if _, ok := rel.Builds["linux-x86_64-deb"]; ok {
		t.Error("the unsigned deb reached the manifest — the plugin would reject it as a hard error, not a miss")
	}
	if len(rel.Builds) != 2 {
		t.Errorf("Builds = %v, want exactly the two signed bundles", rel.Builds)
	}
}

func TestTheVersionLosesItsLeadingV(t *testing.T) {
	rel := voiceReleaseFrom(githubRelease{TagName: "v27.0.9"}, noSignatures)
	if rel.Version != "27.0.9" {
		t.Errorf("Version = %q, want 27.0.9 — semver parsing in the plugin is strict", rel.Version)
	}
}

func TestAnUnreadableSignatureDropsOnlyThatBundle(t *testing.T) {
	rel := voiceReleaseFrom(githubRelease{
		TagName: "v27.0.9", Assets: sampleAssets(),
	}, func(_ context.Context, href string) (string, bool) {
		return "sig", href != "https://example.invalid/w.sig"
	})

	if _, ok := rel.Builds["windows-x86_64-nsis"]; ok {
		t.Error("a bundle whose signature could not be fetched was still offered")
	}
	if _, ok := rel.Builds["linux-x86_64-appimage"]; !ok {
		t.Error("one unreadable signature took down a bundle it had nothing to do with")
	}
}

func TestVoiceLatestSurvivesTheCallerHangingUp(t *testing.T) {
	// Same contract as the legacy resolver: a member closing the app mid-check
	// must not poison the cache for everyone else.
	r, calls := voiceStub(func(ctx context.Context) (*VoiceRelease, error) {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		return &VoiceRelease{Version: "27.0.9"}, nil
	})

	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if got := r.Latest(ctx); got == nil {
		t.Fatal("Latest() = nil for a cancelled caller — the fetch inherited the cancellation")
	}
	if *calls != 1 {
		t.Errorf("fetch called %d times, want 1", *calls)
	}
}

func TestVoiceSuccessIsCachedForTheWholeTTL(t *testing.T) {
	at := time.Unix(1_700_000_000, 0)
	r, calls := voiceStub(func(context.Context) (*VoiceRelease, error) {
		return &VoiceRelease{Version: "27.0.9"}, nil
	})
	r.now = func() time.Time { return at }

	r.Latest(context.Background())
	at = at.Add(successTTL - time.Second)
	r.Latest(context.Background())
	if *calls != 1 {
		t.Errorf("fetch called %d times inside the TTL, want 1 — every client start would hit GitHub", *calls)
	}

	at = at.Add(2 * time.Second)
	r.Latest(context.Background())
	if *calls != 2 {
		t.Errorf("fetch called %d times after the TTL, want 2", *calls)
	}
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `go test ./internal/release/ -run 'Voice|Signature|LeadingV' -v`

Expected: FAIL — `undefined: NewVoiceResolver`, `undefined: voiceReleaseFrom`, `undefined: githubAsset`, `undefined: noSignatures`, and `githubRelease` has no `Body` field.

- [ ] **Step 3: Name the asset type and add the body**

`internal/release/release.go` declares the GitHub payload with an anonymous asset struct and no body. Both are needed here. Change the declaration only:

```go
type githubAsset struct {
	Name               string `json:"name"`
	Size               int64  `json:"size"`
	BrowserDownloadURL string `json:"browser_download_url"`
}

type githubRelease struct {
	TagName     string        `json:"tag_name"`
	PublishedAt string        `json:"published_at"`
	HTMLURL     string        `json:"html_url"`
	Body        string        `json:"body"`
	Assets      []githubAsset `json:"assets"`
}
```

- [ ] **Step 4: Write the resolver**

```go
package release

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"strings"
	"sync"
	"time"
)

// voiceRepo is can-voice's own repository. It is a second constant rather than
// a parameter on the existing Resolver because the two families resolve
// different shapes: can-audio publishes one asset per product, can-voice five
// plus a signature for each. The legacy constant stays where it is and goes
// away at cutover.
const voiceRepo = "JianyueLab-Org/can-voice"

// sigBodyLimit caps a signature read. A minisign signature is two short lines;
// anything larger is not one, and reading it would be the only unbounded read
// on this path.
const sigBodyLimit = 4 << 10

// VoiceBuild is one signed, downloadable bundle.
type VoiceBuild struct {
	Name      string
	Size      int64
	Href      string
	Signature string
}

// VoiceRelease is can-voice's latest release, keyed by Platform.Key().
type VoiceRelease struct {
	Version     string
	PublishedAt string
	Notes       string
	Builds      map[string]VoiceBuild
}

// VoiceResolver caches can-voice's latest release. Same cache shape and the
// same TTLs as the legacy Resolver; it is a separate type because the release
// it holds is a different one.
type VoiceResolver struct {
	token string

	mu       sync.Mutex
	cached   *VoiceRelease
	cachedAt time.Time
	failedAt time.Time

	now   func() time.Time
	fetch func(context.Context) (*VoiceRelease, error)
}

func NewVoiceResolver(token string) *VoiceResolver {
	return &VoiceResolver{token: token, now: time.Now}
}

// Latest answers from cache when it can and never returns an error: a failure
// to reach GitHub is a client that does not update, not a client that breaks.
func (r *VoiceResolver) Latest(ctx context.Context) *VoiceRelease {
	r.mu.Lock()
	defer r.mu.Unlock()

	now := r.now()
	if r.cached != nil && now.Sub(r.cachedAt) < successTTL {
		return r.cached
	}
	if !r.failedAt.IsZero() && now.Sub(r.failedAt) < failureTTL {
		return r.cached
	}

	fetch := r.fetch
	if fetch == nil {
		fetch = r.fetchUpstream
	}

	// The caller hanging up must not mark the upstream as failing, and must
	// not abandon a fetch other callers are waiting behind this mutex for.
	fetched, err := fetch(context.WithoutCancel(ctx))
	if err != nil {
		if !errors.Is(err, context.Canceled) {
			r.failedAt = now
		}
		return r.cached
	}

	r.cached, r.cachedAt, r.failedAt = fetched, now, time.Time{}
	return r.cached
}

// signatureReader fetches the body of a `.sig` asset. Returns false when the
// signature cannot be read, which drops the bundle it belongs to.
type signatureReader func(ctx context.Context, href string) (string, bool)

func noSignatures(context.Context, string) (string, bool) { return "", false }

func (r *VoiceResolver) fetchUpstream(ctx context.Context) (*VoiceRelease, error) {
	ctx, cancel := context.WithTimeout(ctx, fetchLimit)
	defer cancel()

	payload, err := r.getRelease(ctx)
	if err != nil {
		return nil, err
	}
	return voiceReleaseFrom(payload, r.readSignature), nil
}

func (r *VoiceResolver) getRelease(ctx context.Context) (githubRelease, error) {
	var payload githubRelease

	req, err := http.NewRequestWithContext(ctx, http.MethodGet,
		"https://api.github.com/repos/"+voiceRepo+"/releases/latest", nil)
	if err != nil {
		return payload, err
	}
	r.authorize(req, "application/vnd.github+json")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return payload, err
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return payload, fmt.Errorf("release: GitHub answered %d for %s", resp.StatusCode, voiceRepo)
	}
	if err := json.NewDecoder(resp.Body).Decode(&payload); err != nil {
		return payload, err
	}
	return payload, nil
}

func (r *VoiceResolver) readSignature(ctx context.Context, href string) (string, bool) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, href, nil)
	if err != nil {
		return "", false
	}
	r.authorize(req, "application/octet-stream")

	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return "", false
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", false
	}
	body, err := io.ReadAll(io.LimitReader(resp.Body, sigBodyLimit))
	if err != nil || len(body) == 0 {
		return "", false
	}
	return strings.TrimSpace(string(body)), true
}

func (r *VoiceResolver) authorize(req *http.Request, accept string) {
	req.Header.Set("Accept", accept)
	req.Header.Set("User-Agent", "can-api")
	if r.token != "" {
		req.Header.Set("Authorization", "Bearer "+r.token)
	}
}

// voiceReleaseFrom pairs every classified bundle with the `.sig` published
// beside it. The signature's name is the payload's name with `.sig` appended
// whole — the original extension is not replaced.
func voiceReleaseFrom(payload githubRelease, signature signatureReader) *VoiceRelease {
	out := &VoiceRelease{
		Version:     strings.TrimPrefix(payload.TagName, "v"),
		PublishedAt: payload.PublishedAt,
		Notes:       payload.Body,
		Builds:      make(map[string]VoiceBuild),
	}

	sigHref := make(map[string]string, len(payload.Assets))
	for _, asset := range payload.Assets {
		if name, ok := strings.CutSuffix(asset.Name, ".sig"); ok {
			sigHref[name] = asset.BrowserDownloadURL
		}
	}

	// Signatures are read once per bundle and held for the release's whole
	// TTL, so the cost is a handful of small requests every fifteen minutes.
	bodies := make(map[string]string, len(sigHref))
	for _, asset := range payload.Assets {
		for _, client := range Clients {
			platform, ok := classify(client, asset.Name)
			if !ok {
				continue
			}
			href, ok := sigHref[asset.Name]
			if !ok {
				break
			}
			body, cached := bodies[href]
			if !cached {
				body, ok = signature(context.Background(), href)
				if !ok {
					break
				}
				bodies[href] = body
			}
			out.Builds[client+"/"+platform.Key()] = VoiceBuild{
				Name:      asset.Name,
				Size:      asset.Size,
				Href:      asset.BrowserDownloadURL,
				Signature: body,
			}
			break
		}
	}

	return out
}
```

Note the key: `client + "/" + platform.Key()`. The test above indexes `rel.Builds["linux-x86_64-appimage"]` — fix the test to `rel.Builds["audio-for-can/linux-x86_64-appimage"]` in the two places it appears, and the `len(rel.Builds) != 2` assertion stays correct.

- [ ] **Step 5: Run it and watch it pass**

Run: `go test ./internal/release/ -v && go test -race ./internal/release/`

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
gofmt -l .
git add internal/release/voice.go internal/release/voice_test.go internal/release/release.go
git commit -m "$(cat <<'MSG'
feat(release): resolve can-voice releases keyed by product and platform

Each bundle is paired with the `.sig` published beside it. A bundle with
no readable signature is dropped rather than offered: the plugin would
refuse it as an error, and a miss has to look like a miss.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 4: The manifest route

**Files:**
- Create: `internal/api/voiceupdate.go`
- Create: `internal/api/voiceupdate_test.go`
- Modify: `internal/api/server.go`
- Modify: `internal/ratelimit/ratelimit.go`

**Interfaces:**
- Consumes: `release.VoiceResolver`, `release.VoiceRelease`, `release.VoiceBuild`, `release.ParsePlatform`, `release.IsClient`, `release.CompareVersions`; `httpx.JSON`, `httpx.Error`, `httpx.PublicCORS`, `httpx.TooManyRequests`; `ratelimit.Check`, `ratelimit.Limits`.
- Produces:
  - `func (s *Server) handleVoiceUpdate(w http.ResponseWriter, r *http.Request)`
  - `Server.voiceReleases *release.VoiceResolver`
  - `ratelimit.Limits.VoiceUpdate`, `ratelimit.Limits.VoiceDownload`

- [ ] **Step 1: Write the failing test**

```go
package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"

	"github.com/JianyueLab/can-api/internal/config"
	"github.com/JianyueLab/can-api/internal/ratelimit"
	"github.com/JianyueLab/can-api/internal/release"
)

func voiceServer(rel *release.VoiceRelease) *Server {
	r := release.NewVoiceResolver("")
	release.SetVoiceCacheForTest(r, rel)
	return &Server{
		cfg:           &config.Config{Issuer: "https://api.ceruleanavi.net"},
		voiceReleases: r,
		limiter:       ratelimit.New(),
	}
}

func sampleVoiceRelease() *release.VoiceRelease {
	return &release.VoiceRelease{
		Version:     "27.0.9",
		PublishedAt: "2026-09-22T00:00:00Z",
		Notes:       "notes",
		Builds: map[string]release.VoiceBuild{
			"audio-for-can/windows-x86_64-nsis": {Name: "audio-for-can_27.0.9_x64-setup.exe", Size: 1, Href: "https://example.invalid/w", Signature: "sig-nsis"},
			"audio-for-can/linux-x86_64-deb":    {Name: "audio-for-can_27.0.9_amd64.deb", Size: 1, Href: "https://example.invalid/d", Signature: "sig-deb"},
		},
	}
}

func voiceGET(t *testing.T, s *Server, path string) *httptest.ResponseRecorder {
	t.Helper()
	mux := http.NewServeMux()
	s.routes(mux)
	w := httptest.NewRecorder()
	mux.ServeHTTP(w, httptest.NewRequest(http.MethodGet, path, nil))
	return w
}

// The plugin's deserializer takes `platforms` when it is present and otherwise
// requires top-level `url` and `signature`. The request already names the
// platform, so we answer the flat form and there is nothing to key.
func TestTheManifestIsTheFlatFormTheRequestAlreadyNarrowedTo(t *testing.T) {
	w := voiceGET(t, voiceServer(sampleVoiceRelease()),
		"/api/v1/voice/update/audio-for-can/windows/x86_64/nsis?current=27.0.3")

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200", w.Code)
	}

	var got map[string]any
	if err := json.Unmarshal(w.Body.Bytes(), &got); err != nil {
		t.Fatalf("the manifest is not JSON: %v", err)
	}
	if _, ok := got["platforms"]; ok {
		t.Error("the manifest carries a platforms map — the request named one platform")
	}
	for _, key := range []string{"version", "url", "signature", "pub_date"} {
		if got[key] == nil || got[key] == "" {
			t.Errorf("%q is missing — the plugin needs it", key)
		}
	}
	if got["version"] != "27.0.9" {
		t.Errorf("version = %v, want 27.0.9", got["version"])
	}
	if got["signature"] != "sig-nsis" {
		t.Errorf("signature = %v, want the one belonging to this platform", got["signature"])
	}
	want := "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/windows-x86_64-nsis"
	if got["url"] != want {
		t.Errorf("url = %v, want %q — it must point at the relay, never at GitHub", got["url"], want)
	}
}

// Every ordinary miss is 204. Anything else makes the plugin log an error and
// try the next endpoint, and there is no next endpoint.
func TestEveryOrdinaryMissIsTwoOhFour(t *testing.T) {
	s := voiceServer(sampleVoiceRelease())

	cases := []struct {
		why  string
		path string
	}{
		{"already current", "/api/v1/voice/update/audio-for-can/windows/x86_64/nsis?current=27.0.9"},
		{"newer than the release", "/api/v1/voice/update/audio-for-can/windows/x86_64/nsis?current=27.1.0"},
		{"bundle not stamped into the binary", "/api/v1/voice/update/audio-for-can/linux/x86_64/unknown?current=27.0.3"},
		{"platform not published", "/api/v1/voice/update/audio-for-can/linux/aarch64/appimage?current=27.0.3"},
		{"product published nothing for this platform", "/api/v1/voice/update/atis-for-can/windows/x86_64/nsis?current=27.0.3"},
	}

	for _, tc := range cases {
		w := voiceGET(t, s, tc.path)
		if w.Code != http.StatusNoContent {
			t.Errorf("%s: status = %d, want 204 (body %q)", tc.why, w.Code, w.Body.String())
		}
		if w.Body.Len() != 0 {
			t.Errorf("%s: 204 carried a body %q", tc.why, w.Body.String())
		}
	}
}

func TestAnUnresolvableReleaseIsAlsoTwoOhFour(t *testing.T) {
	// The update service being down must look exactly like "no update". A 503
	// here is a client that logs an error on every start for no gain.
	w := voiceGET(t, voiceServer(nil),
		"/api/v1/voice/update/audio-for-can/windows/x86_64/nsis?current=27.0.3")
	if w.Code != http.StatusNoContent {
		t.Errorf("status = %d, want 204", w.Code)
	}
}

func TestAnUnknownProductIsStillARefusal(t *testing.T) {
	// The whitelist is what stops this being an open proxy, so this one is a
	// 404 rather than a 204.
	w := voiceGET(t, voiceServer(sampleVoiceRelease()),
		"/api/v1/voice/update/not-a-client/windows/x86_64/nsis?current=27.0.3")
	if w.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404", w.Code)
	}
}

func TestAMissingCurrentVersionStillGetsTheManifest(t *testing.T) {
	// The plugin compares versions itself; `current` is an optimisation. A
	// client that omits it must not be cut off from updating.
	w := voiceGET(t, voiceServer(sampleVoiceRelease()),
		"/api/v1/voice/update/audio-for-can/windows/x86_64/nsis")
	if w.Code != http.StatusOK {
		t.Errorf("status = %d, want 200", w.Code)
	}
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `go test ./internal/api/ -run Voice -v`

Expected: FAIL — `undefined: release.SetVoiceCacheForTest`, `Server has no field voiceReleases`, and the routes are unregistered.

- [ ] **Step 3: Add the test seam, the field and the rate limits**

`internal/release/voice.go`, at the end:

```go
// SetVoiceCacheForTest installs a release directly, so a handler test does not
// need a fake GitHub. Test-only: nothing in the serving path calls it.
func SetVoiceCacheForTest(r *VoiceResolver, rel *VoiceRelease) {
	r.mu.Lock()
	defer r.mu.Unlock()
	r.cached = rel
	r.cachedAt = r.now()
	r.fetch = func(context.Context) (*VoiceRelease, error) { return rel, nil }
}
```

`internal/api/server.go`, beside `releases`:

```go
	// releases resolves can-audio for the legacy client routes; voiceReleases
	// resolves can-voice for the update routes. Two resolvers because they
	// read two repositories and two asset shapes.
	voiceReleases *release.VoiceResolver
```

and in `New`, beside `releases: release.NewResolver(cfg.GitHubToken)`:

```go
		voiceReleases: release.NewVoiceResolver(cfg.GitHubToken),
```

`internal/ratelimit/ratelimit.go`, in `limitTable` beside `ClientUpdate, ClientDownload Rule`:

```go
	// The voice clients check once per start, so the update limit only has to
	// survive a member restarting the app repeatedly. The download limit is
	// the expensive one: each hit streams a whole bundle.
	VoiceUpdate, VoiceDownload Rule
```

and in `var Limits = limitTable{…}`, beside the client rules:

```go
	VoiceUpdate:   Rule{120, hour},
	VoiceDownload: Rule{40, hour},
```

- [ ] **Step 4: Write the handler**

`internal/api/voiceupdate.go`:

```go
package api

import (
	"net/http"
	"strings"

	"github.com/JianyueLab/can-api/internal/httpx"
	"github.com/JianyueLab/can-api/internal/ratelimit"
	"github.com/JianyueLab/can-api/internal/release"
)

// The update manifest tauri-plugin-updater reads. The flat form: the request
// path already names one platform, so there is no platforms map to key.
//
// pub_date must be RFC 3339 or the plugin fails deserialization outright, not
// silently — GitHub's published_at already is, and an empty one is omitted.
type voiceManifest struct {
	Version   string `json:"version"`
	Notes     string `json:"notes,omitempty"`
	PubDate   string `json:"pub_date,omitempty"`
	URL       string `json:"url"`
	Signature string `json:"signature"`
}

// handleVoiceUpdate answers GET /api/v1/voice/update/{client}/{target}/{arch}/{bundle}.
//
// Everything that is merely "nothing to install" answers 204. The plugin turns
// 204 into Ok(None) and anything else non-2xx into an error it retries against
// the next endpoint, and there is no next endpoint.
func (s *Server) handleVoiceUpdate(w http.ResponseWriter, r *http.Request) {
	client := r.PathValue("client")
	if !release.IsClient(client) {
		httpx.Error(w, http.StatusNotFound, "unknown_client", "no such client")
		return
	}

	if wait := s.limiter.Enforce(ratelimit.Check{
		Key:  "voiceUpdate:ip:" + s.clientIP(r),
		Rule: ratelimit.Limits.VoiceUpdate,
	}); wait > 0 {
		httpx.TooManyRequests(w, wait)
		return
	}

	httpx.PublicCORS(w)
	// The manifest names a specific build; the relay behind it is immutable
	// per version, but this document changes the moment a release lands.
	w.Header().Set("Cache-Control", "public, max-age=60")

	platform := release.Platform{
		OS:     r.PathValue("target"),
		Arch:   r.PathValue("arch"),
		Bundle: r.PathValue("bundle"),
	}
	// `unknown` is what the plugin substitutes when the running binary carries
	// no bundle stamp — an unbundled build. Nothing we publish matches it.
	if _, ok := release.ParsePlatform(platform.Key()); !ok {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	latest := s.voiceReleases.Latest(r.Context())
	if latest == nil {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	current := strings.TrimSpace(r.URL.Query().Get("current"))
	if current != "" && release.CompareVersions(latest.Version, current) <= 0 {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	build, ok := latest.Builds[client+"/"+platform.Key()]
	if !ok {
		w.WriteHeader(http.StatusNoContent)
		return
	}

	httpx.JSON(w, http.StatusOK, voiceManifest{
		Version:   latest.Version,
		Notes:     latest.Notes,
		PubDate:   latest.PublishedAt,
		URL:       strings.TrimRight(s.cfg.Issuer, "/") + "/api/v1/voice/download/" + client + "/" + platform.Key(),
		Signature: build.Signature,
	})
}
```

- [ ] **Step 5: Register the routes**

`internal/api/server.go`, in `routes`, next to the existing voice token block:

```go
	// The voice clients' update check and download relay. Unconditional, and
	// separate from /api/v1/clients/* because the four product names are the
	// same strings in both generations: nothing in a request says which
	// generation is asking, so the namespace has to.
	mux.HandleFunc("GET /api/v1/voice/update/{client}/{target}/{arch}/{bundle}", s.handleVoiceUpdate)
	mux.HandleFunc("GET /api/v1/voice/download/{client}/{platform}", s.handleVoiceDownload)
	mux.HandleFunc("HEAD /api/v1/voice/download/{client}/{platform}", s.handleVoiceDownload)
```

`handleVoiceDownload` does not exist yet, so add a stub returning 501 and replace it in Task 5:

```go
// Filled in by the relay task; registered now so the manifest's url is not a 404.
func (s *Server) handleVoiceDownload(w http.ResponseWriter, r *http.Request) {
	httpx.Error(w, http.StatusNotImplemented, "not_implemented", "not yet")
}
```

- [ ] **Step 6: Run it and watch it pass**

Run: `go test ./internal/api/ ./internal/release/ ./internal/ratelimit/ -v`

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
gofmt -l . && go vet ./...
git add internal/api/voiceupdate.go internal/api/voiceupdate_test.go internal/api/server.go internal/ratelimit/ratelimit.go internal/release/voice.go
git commit -m "$(cat <<'MSG'
feat(api): serve the can-voice update manifest

GET /api/v1/voice/update/{client}/{target}/{arch}/{bundle}. The flat
manifest form, because the path already names one platform. Every
ordinary miss is 204; anything else makes the plugin retry and error.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 5: The payload relay

The manifest's `url` has to answer. The streaming half of `handleClientDownload` already does everything needed — range forwarding, header copying, the 502 on an upstream failure — so it is extracted rather than written again.

**Files:**
- Modify: `internal/api/clients.go`
- Modify: `internal/api/voiceupdate.go`
- Modify: `internal/api/voiceupdate_test.go`

**Interfaces:**
- Consumes: `forwardToUpstream`, `copyFromUpstream`, `relayRequestMethod` (existing, unexported in `internal/api`).
- Produces: `func (s *Server) relayAsset(w http.ResponseWriter, r *http.Request, href, filename, version, contentType string)`

- [ ] **Step 1: Write the failing test**

```go
func TestTheRelayNamesTheFileTheBundlerNamedIt(t *testing.T) {
	// The client saves whatever we call it, and an AppImage saved as a .zip is
	// not executable. The legacy route hardcodes application/zip because
	// can-audio only ever published zips; can-voice publishes five kinds.
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Length", "4")
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("body"))
	}))
	defer upstream.Close()

	rel := sampleVoiceRelease()
	build := rel.Builds["audio-for-can/windows-x86_64-nsis"]
	build.Href = upstream.URL
	rel.Builds["audio-for-can/windows-x86_64-nsis"] = build

	w := voiceGET(t, voiceServer(rel), "/api/v1/voice/download/audio-for-can/windows-x86_64-nsis")

	if w.Code != http.StatusOK {
		t.Fatalf("status = %d, want 200", w.Code)
	}
	if got := w.Header().Get("Content-Type"); got != "application/octet-stream" {
		t.Errorf("Content-Type = %q, want application/octet-stream", got)
	}
	if got := w.Header().Get("Content-Disposition"); !strings.Contains(got, "audio-for-can_27.0.9_x64-setup.exe") {
		t.Errorf("Content-Disposition = %q, want the bundler's own filename", got)
	}
	if got := w.Header().Get("X-Client-Version"); got != "27.0.9" {
		t.Errorf("X-Client-Version = %q", got)
	}
	if w.Body.String() != "body" {
		t.Errorf("body = %q", w.Body.String())
	}
}

func TestTheRelayRefusesAPlatformKeyNothingProduces(t *testing.T) {
	w := voiceGET(t, voiceServer(sampleVoiceRelease()), "/api/v1/voice/download/audio-for-can/linux-x86_64-tarball")
	if w.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404", w.Code)
	}
}

func TestTheRelayIsA404ForAPlatformThisReleaseDoesNotCarry(t *testing.T) {
	w := voiceGET(t, voiceServer(sampleVoiceRelease()), "/api/v1/voice/download/atis-for-can/windows-x86_64-nsis")
	if w.Code != http.StatusNotFound {
		t.Errorf("status = %d, want 404", w.Code)
	}
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `go test ./internal/api/ -run TestTheRelay -v`

Expected: FAIL with 501 — the stub from Task 4.

- [ ] **Step 3: Extract the shared relay**

In `internal/api/clients.go`, replace everything in `handleClientDownload` from the upstream request through `io.Copy` with a call, and move that code into the new method. The legacy call keeps `application/zip` and its own `<name>-<version>.zip` filename, so its behaviour does not change.

```go
// relayAsset streams an upstream release asset to the caller. Split out of
// handleClientDownload so the voice routes do not grow a second copy of the
// range forwarding and the header plumbing.
//
// contentType and filename are the caller's: the legacy route answers
// application/zip because can-audio only published zips, while the voice route
// publishes five kinds and has to keep the bundler's own name.
func (s *Server) relayAsset(w http.ResponseWriter, r *http.Request, href, filename, version, contentType string) {
	req, err := http.NewRequestWithContext(r.Context(), r.Method, href, nil)
	if err != nil {
		httpx.Error(w, http.StatusBadGateway, "upstream_error", "could not reach the release host")
		return
	}
	req.Header.Set("Accept", "application/octet-stream")
	req.Header.Set("User-Agent", "can-api")
	for _, name := range forwardToUpstream {
		if v := r.Header.Get(name); v != "" {
			req.Header.Set(name, v)
		}
	}

	upstream, err := http.DefaultClient.Do(req)
	if err != nil {
		slog.Error("release relay failed", "href", href, "err", err)
		httpx.Error(w, http.StatusBadGateway, "upstream_error", "could not reach the release host")
		return
	}
	defer upstream.Body.Close()

	switch upstream.StatusCode {
	case http.StatusOK, http.StatusPartialContent:
	case http.StatusRequestedRangeNotSatisfiable, http.StatusNotImplemented:
		httpx.Error(w, upstream.StatusCode, "bad_range", "the release host refused that range")
		return
	default:
		slog.Error("release relay answered", "href", href, "status", upstream.StatusCode)
		httpx.Error(w, http.StatusBadGateway, "upstream_error", "the release host answered "+strconv.Itoa(upstream.StatusCode))
		return
	}

	w.Header().Set("Content-Type", contentType)
	w.Header().Set("Content-Disposition", `attachment; filename="`+filename+`"`)
	w.Header().Set("X-Client-Version", version)
	httpx.PublicCORS(w)
	for _, name := range copyFromUpstream {
		if v := upstream.Header.Get(name); v != "" {
			w.Header().Set(name, v)
		}
	}

	w.WriteHeader(upstream.StatusCode)
	if r.Method == http.MethodHead {
		return
	}
	if _, err := io.Copy(w, upstream.Body); err != nil {
		// The response is already in flight, so there is no status left to change.
		slog.Debug("release relay copy interrupted", "href", href, "err", err)
	}
}
```

`handleClientDownload` keeps its whitelist, rate limit, 503, 404 and `?v=` redirect, then ends with:

```go
	s.relayAsset(w, r, asset.Href, name+"-"+latest.Version+".zip", latest.Version, "application/zip")
```

- [ ] **Step 4: Write the voice handler**

Replace the stub in `internal/api/voiceupdate.go`:

```go
// handleVoiceDownload answers GET|HEAD /api/v1/voice/download/{client}/{platform}.
//
// The bundler's own filename is preserved: the plugin dispatches on what the
// running binary is, not on what it is handed, but a member who downloads this
// by hand gets a file their system can run.
func (s *Server) handleVoiceDownload(w http.ResponseWriter, r *http.Request) {
	if !relayRequestMethod[r.Method] {
		httpx.Error(w, http.StatusMethodNotAllowed, "method_not_allowed", "GET or HEAD")
		return
	}

	client := r.PathValue("client")
	if !release.IsClient(client) {
		httpx.Error(w, http.StatusNotFound, "unknown_client", "no such client")
		return
	}
	platform, ok := release.ParsePlatform(r.PathValue("platform"))
	if !ok {
		httpx.Error(w, http.StatusNotFound, "unknown_platform", "no such platform")
		return
	}

	if wait := s.limiter.Enforce(ratelimit.Check{
		Key:  "voiceDownload:ip:" + s.clientIP(r),
		Rule: ratelimit.Limits.VoiceDownload,
	}); wait > 0 {
		httpx.TooManyRequests(w, wait)
		return
	}

	latest := s.voiceReleases.Latest(r.Context())
	if latest == nil {
		httpx.Error(w, http.StatusServiceUnavailable, "unavailable", "the release could not be resolved")
		return
	}
	build, ok := latest.Builds[client+"/"+platform.Key()]
	if !ok {
		httpx.Error(w, http.StatusNotFound, "no_asset", "this release publishes nothing for that platform")
		return
	}

	// A bundle is immutable once published, and the manifest that named it is
	// only cached for a minute, so this can be cached hard.
	w.Header().Set("Cache-Control", "public, max-age=604800, immutable")
	s.relayAsset(w, r, build.Href, build.Name, latest.Version, "application/octet-stream")
}
```

- [ ] **Step 5: Run it and watch it pass**

Run: `go test ./internal/api/ -v && go test -race ./...`

Expected: PASS, including every pre-existing test in `internal/api`.

- [ ] **Step 6: Commit**

```bash
gofmt -l . && go vet ./...
git add internal/api/clients.go internal/api/voiceupdate.go internal/api/voiceupdate_test.go
git commit -m "$(cat <<'MSG'
feat(api): relay can-voice bundles

Extracts relayAsset out of handleClientDownload. The voice route keeps
the bundler's own filename and answers application/octet-stream; the
legacy route's application/zip is unchanged.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 6: Route table and documentation

**Files:**
- Modify: `internal/api/routes_test.go`
- Modify: `Readme.md`

**Interfaces:** none.

- [ ] **Step 1: Add the rows and watch them fail first**

Add beside the two existing client rows in the method/path table:

```go
		{"GET", "/api/v1/voice/update/audio-for-can/windows/x86_64/nsis"},
		{"GET", "/api/v1/voice/download/audio-for-can/windows-x86_64-nsis"},
		{"HEAD", "/api/v1/voice/download/audio-for-can/windows-x86_64-nsis"},
```

Run: `go test ./internal/api/ -run TestRoutes -v`

Expected: PASS, because Task 4 registered them. If a row fails, the pattern in `routes` does not match what a client would send — fix the pattern, not the row.

To confirm the rows are load-bearing rather than decorative, comment out one `mux.HandleFunc` line, re-run, watch that row fail, then restore it.

- [ ] **Step 2: Document the routes**

In `Readme.md`'s public wire contracts list, after the two `/api/v1/clients/*` entries:

```markdown
- `GET /api/v1/voice/update/{client}/{target}/{arch}/{bundle}` — the can-voice
  clients' update manifest, in the form `tauri-plugin-updater` reads. 204 when
  there is nothing to install, including when the release cannot be resolved.
- `GET|HEAD /api/v1/voice/download/{client}/{platform}` — relays the can-voice
  release asset. `{platform}` is `<os>-<arch>-<bundle>`.

`/api/v1/clients/*` resolves `JianyueLab-Org/can-audio` and `/api/v1/voice/*`
resolves `JianyueLab-Org/can-voice`. The four product names are the same
strings in both generations, so nothing in a request says which generation is
asking; the namespace is what separates them.
```

- [ ] **Step 3: Full gate**

Run:
```bash
gofmt -l .
go vet ./...
go test ./...
go test -race ./...
```

Expected: all clean.

- [ ] **Step 4: Commit, push, open the PR**

```bash
git add internal/api/routes_test.go Readme.md
git commit -m "$(cat <<'MSG'
docs: record the voice update routes

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
git push -u origin HEAD
gh pr create --base main --title "feat: can-voice update manifest and bundle relay" --body "$(cat <<'MSG'
Adds `/api/v1/voice/update/...` and `/api/v1/voice/download/...`.

`/api/v1/clients/*` is unchanged and still resolves can-audio. A recorded
release fixture now pins that behaviour.

What to check:
- The manifest is the flat form (`url` + `signature`, no `platforms`).
- Every ordinary miss is 204, including an unresolvable release.
- A bundle with no `.sig` beside it never reaches the manifest.
- `xpc-for-can-xplane-plugin.zip` is not classified as an xpc bundle.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
MSG
)"
```

---

## Task 7: The client's decision and the manifest shape

`can-voice-update` today asks `/api/v1/clients/latest`, which resolves can-audio: a can-voice client compares `27.0.3` against can-audio's `2.x` and always concludes there is no update. This task points it at the new route and gives it the one decision the Tauri glue needs.

**Files:**
- Modify: `crates/can-voice-update/src/lib.rs`
- Modify: `crates/can-voice-update/Cargo.toml` (no change expected; confirm `serde_json` is present)

**Interfaces:**
- Consumes: nothing new.
- Produces:
  - `pub fn self_replaceable(bundle: Option<&str>) -> bool`
  - `pub fn manifest_url(api_origin: &str, client: &str, target: &str, arch: &str, bundle: &str, current: &str) -> String`
  - `pub fn parse(body: &Value) -> Option<Latest>` — signature changes: the client is no longer a key into the body
  - `pub struct Latest { pub version: String, pub download: String, pub notes: String, pub size: u64 }` — `size` becomes `0` because the manifest does not carry one; the field stays so `UpdateBanner.vue` keeps compiling
  - `pub async fn check(http, api_origin, client, target, arch, bundle, version) -> Option<Latest>` — signature changes

- [ ] **Step 1: Write the failing tests**

Append to `mod tests`:

```rust
    #[test]
    fn only_a_bundle_that_can_replace_itself_updates_itself() {
        // deb and rpm can be installed by the plugin, but installing them
        // needs pkexec or a graphical sudo, and the whole design is "no
        // prompt". `/usr` also belongs to the package manager, so the next
        // `apt upgrade` would put the old version back.
        assert!(self_replaceable(Some("appimage")));
        assert!(self_replaceable(Some("nsis")));
        assert!(self_replaceable(Some("msi")));

        assert!(!self_replaceable(Some("deb")));
        assert!(!self_replaceable(Some("rpm")));
        assert!(!self_replaceable(Some("app")));

        // An unbundled build — `cargo run`. There is nothing to replace.
        assert!(!self_replaceable(None));
        assert!(!self_replaceable(Some("unknown")));
    }

    #[test]
    fn the_manifest_url_carries_every_part_the_route_matches_on() {
        let url = manifest_url(
            "https://api.ceruleanavi.net/",
            "audio-for-can",
            "linux",
            "x86_64",
            "deb",
            "27.0.3",
        );
        assert_eq!(
            url,
            "https://api.ceruleanavi.net/api/v1/voice/update/audio-for-can/linux/x86_64/deb?current=27.0.3"
        );
    }

    #[test]
    fn the_manifest_is_flat_not_keyed_by_client() {
        // The old reply nested every build under `clients[<name>]`. The new
        // one is one platform, because the request named it.
        let body = serde_json::json!({
            "version": "27.0.9",
            "notes": "有新版",
            "pub_date": "2026-09-22T00:00:00Z",
            "url": "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/linux-x86_64-deb",
            "signature": "sig",
        });
        let latest = parse(&body).expect("a manifest is an update");
        assert_eq!(latest.version, "27.0.9");
        assert_eq!(
            latest.download,
            "https://api.ceruleanavi.net/api/v1/voice/download/audio-for-can/linux-x86_64-deb"
        );
        assert_eq!(latest.notes, "有新版");
    }

    #[test]
    fn a_manifest_without_a_url_is_not_an_update() {
        let body = serde_json::json!({ "version": "27.0.9" });
        assert!(parse(&body).is_none());
    }

    #[tokio::test]
    async fn two_oh_four_is_not_an_update() {
        // can-api answers 204 for every ordinary miss. Reading it as an error
        // would be harmless here but would hide a real one.
        let addr = serve_once("HTTP/1.1 204 No Content", "").await;
        let http = reqwest::Client::new();
        let got = check(
            &http,
            &format!("http://{addr}"),
            "audio-for-can",
            "linux",
            "x86_64",
            "deb",
            "27.0.3",
        )
        .await;
        assert!(got.is_none());
    }
```

The existing tests that build the old nested reply (`reply()`, `the_verdict_lives_under_update_not_at_the_top`, `the_build_lives_under_clients_keyed_by_name`, `no_update_available_means_nothing_to_offer`, `a_missing_build_is_not_an_update`) describe a response shape that no longer exists. Delete them and the `reply` helper; the three new parse tests replace them. Keep every `is_newer` / `should_prompt` test — that logic is unchanged.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p can-voice-update`

Expected: FAIL — `cannot find function self_replaceable`, `cannot find function manifest_url`, and `parse` takes two arguments.

- [ ] **Step 3: Implement**

Replace the module header's third rule, which this change reverses for two of the five bundles:

```rust
//! - **只有能就地替换自己的包才自动更新。** AppImage、NSIS、MSI 可以；deb 和 rpm
//!   要提权，而这套更新不问人，所以它们退回横幅提示。判据是
//!   `self_replaceable`，取值来自 `tauri_utils::platform::bundle_type()`——
//!   和插件 `install_inner` 用的是同一个，两处判据不同就会出现「我们以为是
//!   AppImage 而插件以为不是」。
```

Then:

```rust
/// Whether a build of this bundle kind may replace itself without asking.
///
/// The argument is `tauri_utils::platform::bundle_type()`'s name, which the
/// plugin also dispatches on. deb and rpm are installable by the plugin and
/// deliberately excluded: `dpkg -i` and `rpm -U` escalate through pkexec or a
/// graphical sudo, which contradicts "no prompt", and `/usr` belongs to the
/// package manager, so the next `apt upgrade` reverts the version.
pub fn self_replaceable(bundle: Option<&str>) -> bool {
    matches!(bundle, Some("appimage") | Some("nsis") | Some("msi"))
}

/// The manifest address for one running build.
pub fn manifest_url(
    api_origin: &str,
    client: &str,
    target: &str,
    arch: &str,
    bundle: &str,
    current: &str,
) -> String {
    format!(
        "{}/api/v1/voice/update/{client}/{target}/{arch}/{bundle}?current={current}",
        api_origin.trim_end_matches('/')
    )
}

/// Reads the manifest can-api serves. Flat, because the request named one
/// platform: `{version, notes, pub_date, url, signature}`.
pub fn parse(body: &Value) -> Option<Latest> {
    let version = body.get("version")?.as_str()?.trim_start_matches('v');
    let download = body.get("url")?.as_str()?;
    if version.is_empty() || download.is_empty() {
        return None;
    }
    Some(Latest {
        version: version.to_string(),
        download: download.to_string(),
        notes: body
            .get("notes")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        // The manifest carries no size. The banner formats 0 as "0 MB", so
        // the caller uses the unsized string when this is zero.
        size: 0,
    })
}
```

`check` gains the platform arguments, and 204 short-circuits before the body is read:

```rust
pub async fn check(
    http: &reqwest::Client,
    api_origin: &str,
    client: &str,
    target: &str,
    arch: &str,
    bundle: &str,
    version: &str,
) -> Option<Latest> {
    let url = manifest_url(api_origin, client, target, arch, bundle, version);
    let resp = match http
        .get(&url)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(TIMEOUT)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(err) => {
            tracing::info!(%err, "update check: could not reach can-api");
            return None;
        }
    };

    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return None;
    }
    if !resp.status().is_success() {
        tracing::info!(status = %resp.status(), "update check: can-api refused");
        return None;
    }

    let body: Value = match resp.json().await {
        Ok(body) => body,
        Err(err) => {
            tracing::info!(%err, "update check: the manifest did not parse");
            return None;
        }
    };
    parse(&body)
}
```

`check_once` takes the same new arguments and forwards them.

- [ ] **Step 4: Run and watch pass**

Run: `cargo test -p can-voice-update && cargo fmt --all --check && cargo clippy -p can-voice-update --all-targets -- -D warnings`

Expected: PASS. The four apps will not compile yet — Task 9 updates their call sites.

- [ ] **Step 5: Commit**

```bash
git add crates/can-voice-update
git commit -m "$(cat <<'MSG'
feat(update): read the can-api voice manifest

The old check asked /api/v1/clients/latest, which resolves can-audio: a
can-voice client compared 27.x against 2.x and always concluded there was
nothing. Adds self_replaceable, the one decision the updater glue needs.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 8: The updater glue crate

`tauri` pulls in wry and webkit, which is why `apps` is excluded from the workspace. This crate is excluded for the same reason and is compiled by the four per-app `cargo check` jobs that already run in CI.

**Files:**
- Create: `crates/can-voice-autoupdate/Cargo.toml`
- Create: `crates/can-voice-autoupdate/src/lib.rs`
- Modify: `Cargo.toml` (workspace `exclude`)

**Interfaces:**
- Consumes: `can_voice_update::self_replaceable`; `tauri_plugin_updater::UpdaterExt`; `tauri::utils::platform::bundle_type`.
- Produces:
  - `pub fn plugin<R: tauri::Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config>`
  - `pub fn bundle_name() -> Option<&'static str>`
  - `pub fn start<R: tauri::Runtime>(handle: &tauri::AppHandle<R>)`
  - Event `update://state`, payload `{ "phase": "checking" | "downloading" | "installing" | "done", "received": u64, "total": u64 }`

- [ ] **Step 1: Write the crate manifest**

`crates/can-voice-autoupdate/Cargo.toml`:

```toml
[package]
name = "can-voice-autoupdate"
version = "0.1.0"
edition = "2021"
rust-version = "1.80"

# **Not a workspace member.** It depends on `tauri`, which drags wry and
# webkit in; `cargo test --workspace` would then need a GUI toolchain. The
# four `cargo check --all-targets` jobs in apps/*/src-tauri compile it.

[dependencies]
can-voice-update = { path = "../can-voice-update" }
tauri = { version = "2", features = [] }
tauri-plugin-updater = "2.12.0"
serde = { version = "1", features = ["derive"] }
tracing = "0.1"
```

Workspace `Cargo.toml`:

```toml
exclude = ["apps", "crates/can-voice-autoupdate"]
```

- [ ] **Step 2: Write the glue**

There is no unit test here: every function either talks to a live Tauri handle or to the network, and the one decision worth testing (`self_replaceable`) is tested in `can-voice-update`. Keep it thin enough that this stays true.

`crates/can-voice-autoupdate/src/lib.rs`:

```rust
//! 启动时的自动更新。
//!
//! 两条不可让的规矩：
//!
//! - **只在启动时。** 检查发生在用户连上语音之前；一个开着八小时的管制端不会
//!   在第七个小时突然决定重启自己。
//! - **失败要安静，而且绝不挡路。** 每一条错误路径都发一次 `done` 然后让界面
//!   照常进去。更新服务挂掉不该让全网上不了线。

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_updater::UpdaterExt;

/// 界面等的就是这个事件。**每一条路径最后都要发一次 `Done`**——界面在
/// 收到它之前是挡着的。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum State {
    Checking,
    Downloading { received: u64, total: u64 },
    Installing,
    Done,
}

const EVENT: &str = "update://state";

/// 插件本体。端点和公钥在 `tauri.conf.json` 的 `plugins.updater` 里。
pub fn plugin<R: Runtime>() -> tauri::plugin::TauriPlugin<R, tauri_plugin_updater::Config> {
    tauri_plugin_updater::Builder::new().build()
}

/// 当前这份二进制是哪种包。
///
/// 读的是 `tauri_utils::platform::bundle_type()`——**打包时写进二进制的一个静态
/// 字符串**，不是环境变量。插件的 `install_inner` 读的是同一个函数，所以两边
/// 不会对「这是不是 AppImage」产生分歧。
pub fn bundle_name() -> Option<&'static str> {
    use tauri::utils::config::BundleType;
    match tauri::utils::platform::bundle_type()? {
        BundleType::Deb => Some("deb"),
        BundleType::Rpm => Some("rpm"),
        BundleType::AppImage => Some("appimage"),
        BundleType::Msi => Some("msi"),
        BundleType::Nsis => Some("nsis"),
        BundleType::App => Some("app"),
        _ => None,
    }
}

/// 在 `.setup()` 里调一次。立刻返回，活在后台跑。
pub fn start<R: Runtime>(handle: &AppHandle<R>) {
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        run(&handle).await;
        // 走到哪一步都要放行。
        emit(&handle, State::Done);
    });
}

async fn run<R: Runtime>(handle: &AppHandle<R>) {
    if !can_voice_update::self_replaceable(bundle_name()) {
        tracing::info!(bundle = ?bundle_name(), "auto-update: this bundle does not replace itself");
        return;
    }

    emit(handle, State::Checking);

    let updater = match handle.updater() {
        Ok(updater) => updater,
        Err(err) => {
            tracing::info!(%err, "auto-update: the updater is not configured");
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(update)) => update,
        Ok(None) => {
            tracing::info!("auto-update: already current");
            return;
        }
        Err(err) => {
            tracing::info!(%err, "auto-update: the check failed");
            return;
        }
    };

    tracing::info!(version = %update.version, "auto-update: installing");

    let mut received: u64 = 0;
    let installing = handle.clone();
    let progress = handle.clone();

    // download_and_install 在 Windows 上装完直接 exit(0)，由安装器把新版本拉
    // 起来；Linux 上它会返回，要自己重启。
    let outcome = update
        .download_and_install(
            move |chunk, total| {
                received += chunk as u64;
                emit(
                    &progress,
                    State::Downloading {
                        received,
                        total: total.unwrap_or(0),
                    },
                );
            },
            move || emit(&installing, State::Installing),
        )
        .await;

    match outcome {
        Ok(()) => {
            tracing::info!("auto-update: installed, restarting");
            handle.restart();
        }
        Err(err) => tracing::info!(%err, "auto-update: install failed"),
    }
}

fn emit<R: Runtime>(handle: &AppHandle<R>, state: State) {
    if let Err(err) = handle.emit(EVENT, state) {
        tracing::info!(%err, "auto-update: could not tell the interface");
    }
}
```

- [ ] **Step 3: Confirm it compiles**

It has no dependents yet, so check it directly. This is the one place it is built outside an app.

Run: `cd crates/can-voice-autoupdate && cargo check --all-targets && cargo clippy --all-targets -- -D warnings && cargo fmt --check`

Expected: clean. On Linux the Tauri system dependencies must be installed first — the list is in `.github/workflows/ci.yml`.

- [ ] **Step 4: Confirm the workspace still ignores it**

Run: `cargo test --workspace 2>&1 | grep -c can-voice-autoupdate`

Expected: `0`. If it is non-zero, the `exclude` entry did not take and `cargo test --workspace` now needs webkit.

- [ ] **Step 5: Commit**

```bash
git add crates/can-voice-autoupdate Cargo.toml
git commit -m "$(cat <<'MSG'
feat(autoupdate): the startup updater glue

Excluded from the workspace because it depends on tauri; the four
per-app cargo check jobs compile it. Bundle detection calls the same
tauri_utils function the plugin dispatches on, so the two cannot
disagree about whether a build is an AppImage.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 9: Wire the four apps

Do all four in one commit: they are the same edit four times, and a half-wired set is a set where three apps still call a function whose signature changed.

**Files:** for each of `controller`, `atis`, `xpc`, `msfs`:
- Modify: `apps/<app>/src-tauri/Cargo.toml`
- Modify: `apps/<app>/src-tauri/tauri.conf.json`
- Create: `apps/<app>/src-tauri/capabilities/default.json`
- Modify: `apps/<app>/src-tauri/src/lib.rs`

**Interfaces:**
- Consumes: `can_voice_autoupdate::{plugin, start, bundle_name}`; `can_voice_update::check`.
- Produces: nothing new to Rust callers. The `check_update` command keeps its signature.

- [ ] **Step 1: Generate the signing keys**

Done once, by hand, before anything else — the public key goes into all four configs.

```bash
mkdir -p .temp
bunx @tauri-apps/cli signer generate -w .temp/can-voice-updater.key
```

It prints the public key and writes the private key plus a `.pub`. Put both halves where `VOICE_TOKEN_KEY` and the TLS certificate live: losing the private key means every installed client refuses every future update and everyone reinstalls by hand.

Then set the repository secrets:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY < .temp/can-voice-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD   # paste the passphrase
```

Keep the public key string for the next step, then `rm -rf .temp`.

- [ ] **Step 2: Add the dependencies**

In each `apps/<app>/src-tauri/Cargo.toml`, after the existing `can-voice-update` line:

```toml
can-voice-autoupdate = { path = "../../../crates/can-voice-autoupdate" }
tauri-plugin-updater = "2.12.0"
```

- [ ] **Step 3: Configure the plugin**

In each `apps/<app>/src-tauri/tauri.conf.json`, add `createUpdaterArtifacts` inside `bundle` and a top-level `plugins` block. `<APP>` is the product name; `<PUBKEY>` is the string from Step 1, identical in all four.

```json
  "bundle": {
    "active": true,
    "targets": "all",
    "createUpdaterArtifacts": true,
```

```json
  "plugins": {
    "updater": {
      "endpoints": [
        "https://api.ceruleanavi.net/api/v1/voice/update/<APP>/{{target}}/{{arch}}/{{bundle_type}}?current={{current_version}}"
      ],
      "pubkey": "<PUBKEY>",
      "requireSignedVersion": true,
      "windows": {
        "installMode": "passive"
      }
    }
  }
```

Three things about that block:

- `{{bundle_type}}` is the placeholder that is easy to miss. Without it the same manifest would be served to a deb and to an AppImage.
- `requireSignedVersion` makes the plugin reject an update whose signed trusted comment names a different version than the manifest announced, which is the only guard against a crafted manifest pairing an inflated `version` with an older genuine artifact's `url` and `signature`. Enabling it rejects releases signed before the CLI recorded a version — can-voice has never shipped a signed release, so there is nothing to reject.
- `installMode: "passive"` is the plugin's own default; it is written out because the alternative (`"quiet"`) is the one that looks like what "no prompt" means and is not — it needs admin rights if the installer does. Passive shows a progress bar and asks nothing.

Do not add a `"version"` key anywhere at two-space indent: `scripts/release-version.ts` rewrites `/^(  "version": ")[^"]*(")/m` and throws unless it matches exactly once. The block above nests deeper and is safe.

- [ ] **Step 4: Add the capability**

None of the four apps has a `capabilities/` directory today, because every command is app-local and needs no ACL entry. Plugin commands do. Create `apps/<app>/src-tauri/capabilities/default.json`:

```json
{
  "$schema": "../gen/schemas/desktop-schema.json",
  "identifier": "default",
  "description": "Permissions the app's own windows hold.",
  "windows": ["main"],
  "permissions": ["updater:default"]
}
```

`tauri.conf.json` names no capability list, so everything in `capabilities/` is included.

- [ ] **Step 5: Register the plugin and start the sequence**

In each `apps/<app>/src-tauri/src/lib.rs`, `run()`. Add the `.plugin` call before `.setup`, and the `start` call at the end of the existing setup closure — after the window work, so the "正在更新…" screen has a window to appear in.

controller, in full; the other three differ only in what their setup body already does:

```rust
    tauri::Builder::default()
        .manage(App::new())
        .plugin(can_voice_autoupdate::plugin())
        // 置顶和精简在窗口一出来就还原。压在雷达屏上用的人不该每次启动都再点一遍。
        .setup(|handle| {
            let app = handle.state::<App>();
            app.install_ptt(app.settings().ptt);
            let appearance = app.settings().appearance;
            if let Some(window) = handle.get_webview_window("main") {
                apply_window(&window, &appearance, appearance.compact);
            }
            // 检查更新。立刻返回；结果通过 `update://state` 发给界面。
            can_voice_autoupdate::start(handle.handle());
            Ok(())
        })
```

- [ ] **Step 6: Fix the `check_update` call sites**

`can_voice_update::check` took four arguments and now takes seven. In each app's `check_update`:

```rust
#[tauri::command]
async fn check_update(
    app: tauri::State<'_, App>,
) -> Result<Option<can_voice_update::Latest>, String> {
    let settings = app.settings();
    let origin = settings.endpoints.api_origin();
    let bundle = can_voice_autoupdate::bundle_name().unwrap_or("unknown");

    // 能自己换的那些走插件，这条路只剩 deb / rpm 用来显示横幅。
    if can_voice_update::self_replaceable(Some(bundle)) {
        return Ok(None);
    }

    let Some(target) = tauri_plugin_updater::target() else {
        return Ok(None);
    };
    let (os, arch) = target.split_once('-').unwrap_or((target.as_str(), ""));

    Ok(can_voice_update::check(
        &app.http,
        &origin,
        "audio-for-can",
        os,
        arch,
        bundle,
        env!("CARGO_PKG_VERSION"),
    )
    .await)
}
```

Per app: `"atis-for-can"` with `settings_snapshot()` and `check_once` replaced by `check(&app.http, …)` — atis has an `http` field and there is no reason for it to be the only one opening a fresh client; `"xpc-for-can"` and `"msfs-for-can"` with `settings_snapshot()`.

`tauri_plugin_updater::target()` returns `"<os>-<arch>"` built from the same `updater_os()`/`updater_arch()` the plugin uses to fill `{{target}}` and `{{arch}}`, so the banner and the plugin ask about the same platform.

- [ ] **Step 7: Build all four**

```bash
for app in controller atis xpc msfs; do
  ( cd "apps/$app/src-tauri" && mkdir -p ../dist \
    && cargo check --all-targets \
    && cargo clippy --all-targets -- -D warnings ) || echo "FAILED: $app"
done
cargo fmt --all --check
```

Expected: four clean checks. A missing `../dist` is what makes `tauri::generate_context!` fail, which is why the `mkdir` is there — CI does the same.

- [ ] **Step 8: Commit**

```bash
git add apps/*/src-tauri/Cargo.toml apps/*/src-tauri/Cargo.lock apps/*/src-tauri/tauri.conf.json apps/*/src-tauri/capabilities apps/*/src-tauri/src/lib.rs
git commit -m "$(cat <<'MSG'
feat(apps): install updates at startup

tauri-plugin-updater in all four apps, keyed on {{bundle_type}} so a deb
and an AppImage are not offered the same file. requireSignedVersion is on
from the first signed release, so there is nothing older to reject.

The existing banner stays, and is now the deb/rpm path only.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 10: The startup screen

Without this the window comes up, the user starts typing a callsign, and the app restarts under them.

**Files:** for each of the four apps:
- Create: `apps/<app>/src/components/StartupGate.vue`
- Modify: `apps/<app>/src/App.vue`
- Modify: `apps/<app>/src/locales/common.zh.json`
- Modify: `apps/<app>/src/locales/common.en.json`

**Interfaces:**
- Consumes: the `update://state` event from Task 8.
- Produces: `StartupGate.vue`, default slot rendered once the gate opens.

- [ ] **Step 1: Add the strings**

`common.zh.json` and `common.en.json` are byte-identical across all four apps and a test enforces it, so edit one pair and copy. In the existing `update` object:

```json
  "update": {
    "available": "有新版本 {version}。本程序不会自动更新，装不装由你决定。",
    "available_sized": "有新版本 {version}（{size}）。本程序不会自动更新，装不装由你决定。",
    "notes": "更新说明",
    "download": "下载",
    "skip": "跳过这一版",
    "checking": "正在检查更新…",
    "downloading": "正在下载新版本…",
    "downloading_sized": "正在下载新版本… {done} / {total}",
    "installing": "正在安装，装完会自己重启…"
  },
```

English:

```json
  "update": {
    "available": "Version {version} is available. This program never updates itself; whether to install is up to you.",
    "available_sized": "Version {version} ({size}) is available. This program never updates itself; whether to install is up to you.",
    "notes": "Release notes",
    "download": "Download",
    "skip": "Skip this version",
    "checking": "Checking for updates…",
    "downloading": "Downloading the new version…",
    "downloading_sized": "Downloading the new version… {done} / {total}",
    "installing": "Installing. It will restart itself when done…"
  },
```

`available` and `available_sized` keep their wording: after Task 9 the banner only appears on deb and rpm, where "never updates itself" is still true.

```bash
for app in atis xpc msfs; do
  cp apps/controller/src/locales/common.zh.json "apps/$app/src/locales/common.zh.json"
  cp apps/controller/src/locales/common.en.json "apps/$app/src/locales/common.en.json"
done
```

- [ ] **Step 2: Run the dictionary tests and watch them pass**

Run: `cargo test -p can-voice-i18n`

Expected: PASS. `both_languages_have_the_same_keys_and_none_is_empty`, `placeholders_agree_between_the_languages` and `the_common_dictionaries_are_identical_in_every_app` all bear on this edit. If `every_key_the_interface_asks_for_exists` complains about an unused key, ignore it for now — Step 3 uses them.

- [ ] **Step 3: Write the gate**

`apps/controller/src/components/StartupGate.vue`:

```vue
<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { t } from "../i18n";

/** 和 Rust 侧 `can_voice_autoupdate::State` 一一对应。 */
type State =
  | { phase: "checking" }
  | { phase: "downloading"; received: number; total: number }
  | { phase: "installing" }
  | { phase: "done" };

const open = ref(false);
const state = ref<State>({ phase: "checking" });

let unlisten: UnlistenFn | null = null;
let firstEvent: ReturnType<typeof setTimeout> | null = null;

function mb(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(0)} MB`;
}

const message = () => {
  const s = state.value;
  if (s.phase === "downloading") {
    return s.total > 0
      ? t("update.downloading_sized", { done: mb(s.received), total: mb(s.total) })
      : t("update.downloading");
  }
  if (s.phase === "installing") return t("update.installing");
  return t("update.checking");
};

onMounted(async () => {
  // 只有 `done` 放行。**但如果 Rust 那边一个事件都没发**——它崩了，或者这个
  // 版本根本没装更新器——就不能永远挡着：启动不能被更新拖住。所以第一个事件
  // 之前有一道短的兜底，收到任何事件之后就取消，免得把一个正常的长下载切断。
  firstEvent = setTimeout(() => {
    open.value = true;
  }, 8_000);

  try {
    unlisten = await listen<State>("update://state", (event) => {
      if (firstEvent) {
        clearTimeout(firstEvent);
        firstEvent = null;
      }
      state.value = event.payload;
      if (event.payload.phase === "done") open.value = true;
    });
  } catch {
    open.value = true;
  }
});

onUnmounted(() => {
  if (firstEvent) clearTimeout(firstEvent);
  unlisten?.();
});
</script>

<template>
  <slot v-if="open" />
  <p v-else class="startup">{{ message() }}</p>
</template>

<style scoped>
.startup {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 100vh;
  margin: 0;
  font-size: 0.875rem;
  opacity: 0.7;
}
</style>
```

```bash
for app in atis xpc msfs; do
  cp apps/controller/src/components/StartupGate.vue "apps/$app/src/components/StartupGate.vue"
done
```

- [ ] **Step 4: Mount it**

In each `apps/<app>/src/App.vue`, import beside the existing `UpdateBanner` import and wrap the template's root content:

```ts
import StartupGate from "./components/StartupGate.vue";
```

```vue
<template>
  <StartupGate>
    <!-- the existing root content, unchanged -->
  </StartupGate>
</template>
```

`UpdateBanner` stays inside the gate: on deb and rpm nothing gates anything (`self_replaceable` is false, so Rust emits `Done` immediately) and the banner behaves exactly as it does today.

- [ ] **Step 5: Build all four frontends**

```bash
for app in controller atis xpc msfs; do
  ( cd "apps/$app" && bun install --frozen-lockfile && bun run build ) || echo "FAILED: $app"
done
cargo test -p can-voice-i18n
```

Expected: four clean builds (`vue-tsc --noEmit && vite build`) and green dictionary tests. `the_interface_code_has_no_hardcoded_chinese` fails on any Chinese literal in a `.vue` file — every string above goes through `t()`.

- [ ] **Step 6: Commit**

```bash
git add apps/*/src apps/*/src/locales
git commit -m "$(cat <<'MSG'
feat(apps): hold the interface until the startup update finishes

Only `done` opens the gate. A short fallback covers the case where no
event arrives at all; it is cancelled by the first event so a long
download is not cut off.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
```

---

## Task 11: Sign and publish the artifacts

The signature is the only real door on this path: Windows and Linux bundles are not code-signed, so a swapped installer and the real one look identical to the operating system. Without this task the plugin has nothing to verify and every update fails closed.

**Files:**
- Modify: `.github/workflows/release.yml`
- Modify: `docs/manual-test.md`
- Modify: `docs/superpowers/specs/2026-09-21-auto-update-design.md`

**Interfaces:** none.

- [ ] **Step 1: Pass the signing key to the build**

In the `build` job, on the `bun run tauri build` step:

```yaml
      - name: 打包
        working-directory: apps/${{ matrix.app }}
        run: bun run tauri build
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}
```

With `createUpdaterArtifacts: true` and no key, `tauri build` fails rather than publishing unsigned bundles. That is the behaviour to want: a release that silently shipped without signatures would be a release no client could install.

- [ ] **Step 2: Harvest the signatures**

Windows:

```pwsh
$bundle = "apps/${{ matrix.app }}/src-tauri/target/release/bundle"
Get-ChildItem -Path $bundle -Recurse -Include *.exe,*.msi,*.sig |
  ForEach-Object { Copy-Item $_.FullName $out }
```

Linux:

```bash
bundle="apps/${{ matrix.app }}/src-tauri/target/release/bundle"
find "$bundle" -type f \( -name '*.deb' -o -name '*.AppImage' -o -name '*.rpm' \
  -o -name '*.AppImage.tar.gz' -o -name '*.sig' \) -exec cp {} dist-out/ \;
```

And in the `release` job's flatten step, add `*.sig` and `*.AppImage.tar.gz` to the copied extensions.

`.AppImage.tar.gz` is caught as well as `.AppImage` so that switching `createUpdaterArtifacts` to `"v1Compatible"` later does not silently stop publishing the payload — `install_appimage` unwraps gzip itself and `classify` maps both to the same platform.

- [ ] **Step 3: Check the release with a dry run**

```bash
gh workflow run release.yml -f version=v27.0.9 -f dry_run=true
gh run watch
```

Then download the artifacts and confirm a `.sig` sits beside every bundle, with its name being the bundle's full name plus `.sig` — `audio-for-can_27.0.9_amd64.AppImage.sig`, not `audio-for-can_27.0.9_amd64.sig`. `classify` and the resolver's pairing both depend on that.

Record the exact filenames produced. If any differs from the table in Task 2's test, fix the test and `bundleSuffix` before a real release — that test is the only thing standing between a Linux member and a `.exe`.

- [ ] **Step 4: Write the manual test**

Nothing automated proves this works. Add to `docs/manual-test.md`, in its own section:

```markdown
## 自动更新

自动化不了：要有一个真的旧版在真的机器上自己换成新版。

1. 装上上一个版本（不是这一版）。
2. 确认 `api.ceruleanavi.net` 通。
3. 开一次。应当先看到「正在检查更新…」，然后是下载进度，然后程序自己重启。
4. 重启之后在设置里核对版本号，确认是新的。
5. **Linux 上分两种**：AppImage 走上面这条；deb 和 rpm 应当**不**自动更新，
   而是照旧显示横幅。两种都要走一遍。
6. 断网再开一次。应当**直接进主界面**，不卡、不报错、不弹框。
   这一条比上面任何一条都重要：更新服务挂掉不能让人上不了线。
```

- [ ] **Step 5: Correct the spec**

§6 of the design document says AppImage detection reads the `APPIMAGE` environment variable via `Env::default()`. That is not what 2.12.0 does: `install_inner` dispatches on `tauri_utils::platform::bundle_type()`, which reads `__TAURI_BUNDLE_TYPE` — a static string the bundler patches into the binary. Replace that paragraph:

```markdown
判别不要自己发明：插件 `install_inner` 分派用的是
`tauri_utils::platform::bundle_type()`，读的是 `__TAURI_BUNDLE_TYPE`——**打包时
写进二进制的一个静态字符串**，不是环境变量。我们调同一个函数，理由是两处判据
一旦不同，就会出现"我们以为是 AppImage 而插件以为不是"的那一类分歧。没有这个
标记的构建（`cargo run` 出来的）返回 `None`，两边都当作不自动更新。
```

Also add to §4.2, after the placeholder paragraph:

```markdown
清单还有一种**扁平**形态：插件的反序列化在没有 `platforms` 时要求顶层直接带
`url` 和 `signature`。本设计用的是这一种——请求路径里已经写明了平台，再建一个
只有一个键的 map 没有意义。
```

- [ ] **Step 6: Commit and open the PR**

```bash
git add .github/workflows/release.yml docs/manual-test.md docs/superpowers/specs/2026-09-21-auto-update-design.md
git commit -m "$(cat <<'MSG'
ci: sign the bundles and publish the signatures

createUpdaterArtifacts fails the build without a key, so a release
cannot silently ship bundles no client can install.

Corrects the spec: bundle detection reads a string the bundler patches
into the binary, not the APPIMAGE environment variable.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
)"
git push -u origin HEAD
gh pr create --base main --title "feat: install updates at startup" --body "$(cat <<'MSG'
The four desktop clients install a newer version of themselves at startup,
before the user is on voice. AppImage, NSIS and MSI self-install; deb and rpm
keep the banner they have today, now pointed at the same new can-api route.

Needs `JianyueLab/can-api` to have shipped `/api/v1/voice/update/...` first.

What to check:
- `TAURI_SIGNING_PRIVATE_KEY` and its password are set, and the public key in
  the four `tauri.conf.json` is the matching half. Losing the private key means
  every installed client refuses every future update.
- A `.sig` is published beside every bundle, named `<full bundle name>.sig`.
- `docs/manual-test.md`'s new section, all six steps. Step 6 — offline start
  goes straight to the main interface — is the one that matters most.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
MSG
)"
```

---

## Self-Review

**Spec coverage.**

| Spec section | Task |
|---|---|
| §2 full-auto, startup only | 8, 10 |
| §2 Linux deb/rpm to the banner | 7, 9 |
| §3 plugin pinned at 2.12.0 | 8 |
| §4.1 can-api, never GitHub | 4, 5 |
| §4.2 the new route and its fields | 4 |
| §4.3 the legacy routes untouched | 1, 6 |
| §4.4 the broken relay | 2, 3 — defused by construction: the voice family has its own resolver and the legacy one never resolves can-voice. Task 1 pins the legacy behaviour instead of changing it, because can-audio's real asset names are not knowable from here and that route is live. |
| §5 silent failure, never blocking | 8, 10 |
| §6 the platform matrix | 7, 9 |
| §7 signing and keys | 9 step 1, 11 |
| §8 the test table | 2, 3, 4, 5, 11 step 4 |
| §9 the risks | 11 step 4 |

Two things §8 asks for that are not separate tasks: "两代不串" is Task 1 plus the `routes_test` rows; "不挡启动" has no Rust unit test because every path through `run()` needs a live `AppHandle` — the offline-start step in the manual test is what covers it, and `start()` is kept thin enough that reading it is the proof.

**Corrections to the spec, found while writing this.** §6's detection mechanism was wrong (Task 11 step 5). §4.2 did not mention the flat manifest form, which this plan uses throughout (Task 11 step 5). Neither changes a decision.

**Not in scope.** can-api's `const repo` for the legacy routes, which moves on cutover day. macOS, which has no build.

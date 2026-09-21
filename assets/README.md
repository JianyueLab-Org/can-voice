# assets

`can-mark.svg` is the Cerulean Aviation Network mark — the roundel alone, no
wordmark, 256×256, transparent.

It is a copy. The artwork belongs to `can-ui` (`src/assets/logo/`, and
`src/components/LogoMark.vue` for the inline form); this file is byte-identical
to `can-docs/public/favicon.svg` and `can-efb/public/favicon.svg`. can-voice is
a separate repository and cannot import across the boundary, which is the same
reason the eight web repositories each carry their own copy.

The four desktop apps' icons are generated from it:

```sh
for app in controller atis xpc msfs; do
  (cd "apps/$app" && bunx @tauri-apps/cli icon ../../assets/can-mark.svg -o src-tauri/icons)
done
```

That command also writes an `android/` and an `ios/` directory, a `64x64.png`,
and the `Square*Logo.png` / `StoreLogo.png` set. None are kept: there are no
mobile projects, and the Square and Store logos are read only by the Microsoft
Store bundle target, which is not one of the targets built here. Delete them
after regenerating.

`icon.icns` is not reproducible — two runs produce files of the same length
with different bytes. The four apps' copies are made identical by hand so a
future diff means something.

Do not take the icon from `can-audio`. Its `favicon.ico` is the **AirwaySN**
mark, from before the network was renamed, and it is a 64×64 PNG carrying a
`.ico` extension.

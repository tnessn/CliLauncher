#!/bin/sh
set -eu

project_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
build_dir="$project_root/build"
app_dir="$build_dir/CliLauncher.app"
contents="$app_dir/Contents"

cd "$project_root"
cargo build --release
mkdir -p "$build_dir"
touch "$build_dir/.metadata_never_index"
rm -rf "$app_dir"
mkdir -p "$contents/MacOS" "$contents/Resources"
cp "$project_root/target/release/cli-launcher" "$contents/MacOS/CliLauncher"
chmod +x "$contents/MacOS/CliLauncher"

iconset="$build_dir/CliLauncher.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
sips -z 16 16 "$project_root/assets/logo.png" --out "$iconset/icon_16x16.png" >/dev/null
sips -z 32 32 "$project_root/assets/logo.png" --out "$iconset/icon_16x16@2x.png" >/dev/null
sips -z 32 32 "$project_root/assets/logo.png" --out "$iconset/icon_32x32.png" >/dev/null
sips -z 64 64 "$project_root/assets/logo.png" --out "$iconset/icon_32x32@2x.png" >/dev/null
sips -z 128 128 "$project_root/assets/logo.png" --out "$iconset/icon_128x128.png" >/dev/null
sips -z 256 256 "$project_root/assets/logo.png" --out "$iconset/icon_128x128@2x.png" >/dev/null
sips -z 256 256 "$project_root/assets/logo.png" --out "$iconset/icon_256x256.png" >/dev/null
sips -z 512 512 "$project_root/assets/logo.png" --out "$iconset/icon_256x256@2x.png" >/dev/null
sips -z 512 512 "$project_root/assets/logo.png" --out "$iconset/icon_512x512.png" >/dev/null
sips -z 1024 1024 "$project_root/assets/logo.png" --out "$iconset/icon_512x512@2x.png" >/dev/null
# shortcut: use macOS's generic app icon when iconutil rejects the generated set; revisit if custom branding is required.
icon_key=''
if iconutil -c icns "$iconset" -o "$contents/Resources/CliLauncher.icns" 2>/dev/null; then
  icon_key='  <key>CFBundleIconFile</key><string>CliLauncher</string>'
else
  printf '%s\n' 'Warning: icon conversion failed; packaging with the generic app icon.' >&2
fi
rm -rf "$iconset"

printf '%s\n' \
  '<?xml version="1.0" encoding="UTF-8"?>' \
  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
  '<plist version="1.0"><dict>' \
  '  <key>CFBundleName</key><string>CLI Launcher</string>' \
  '  <key>CFBundleDisplayName</key><string>CLI Launcher</string>' \
  '  <key>CFBundleIdentifier</key><string>local.clilauncher</string>' \
  '  <key>CFBundleExecutable</key><string>CliLauncher</string>' \
  "$icon_key" \
  '  <key>CFBundlePackageType</key><string>APPL</string>' \
  '  <key>CFBundleShortVersionString</key><string>0.1.1</string>' \
  '  <key>NSHighResolutionCapable</key><true/>' \
  '</dict></plist>' > "$contents/Info.plist"

if [ -n "${APPLE_SIGN_IDENTITY:-}" ]; then
  codesign --force --deep --options runtime --sign "$APPLE_SIGN_IDENTITY" "$app_dir"
else
  codesign --force --deep --sign - "$app_dir"
fi
codesign --verify --deep --strict "$app_dir"

rm -f "$build_dir/CliLauncher.dmg"
hdiutil create -volname "CLI Launcher" -srcfolder "$app_dir" \
  -ov -format UDZO "$build_dir/CliLauncher.dmg"
printf 'Created: %s\n' "$build_dir/CliLauncher.dmg"

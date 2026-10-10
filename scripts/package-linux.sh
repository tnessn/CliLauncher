#!/bin/sh
set -eu

project_root="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
package_root="$project_root/build/cli-launcher-linux"

cd "$project_root"
cargo build --release
rm -rf "$package_root"
mkdir -p "$package_root/bin" "$package_root/share/applications" \
  "$package_root/share/icons/hicolor/512x512/apps"
cp "$project_root/target/release/cli-launcher" "$package_root/bin/"
chmod +x "$package_root/bin/cli-launcher"
cp "$project_root/assets/logo.png" \
  "$package_root/share/icons/hicolor/512x512/apps/cli-launcher.png"

printf '%s\n' \
  '[Desktop Entry]' \
  'Type=Application' \
  'Name=CLI Launcher' \
  'Exec=cli-launcher' \
  'Icon=cli-launcher' \
  'Terminal=false' \
  'Categories=Development;Utility;' \
  > "$package_root/share/applications/cli-launcher.desktop"

tar -C "$project_root/build" -czf "$project_root/build/CliLauncher-linux.tar.gz" \
  "cli-launcher-linux"
printf 'Created: %s\n' "$project_root/build/CliLauncher-linux.tar.gz"

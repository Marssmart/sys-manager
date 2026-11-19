#!/usr/bin/env bash
# Builds a release "Sys Manager.app" bundle and a drag-to-install .dmg.
# Code signing and notarization are not part of this script. See README.md.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"

app_name="Sys Manager"
binary_name="sys-manager"
bundle_id="com.marssmart.sysmanager"
version="$(awk -F'"' '/^version/ { print $2; exit }' "$repo_root/Cargo.toml")"

dist_dir="$repo_root/target/dist"
app_dir="$dist_dir/$app_name.app"
staging_dir="$dist_dir/dmg-staging"
dmg_path="$dist_dir/Sys-Manager-$version.dmg"

cargo build --release --manifest-path "$repo_root/Cargo.toml" -p "$binary_name"

rm -rf "$dist_dir"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
cp "$repo_root/target/release/$binary_name" "$app_dir/Contents/MacOS/$binary_name"
sed \
  -e "s/@VERSION@/$version/g" \
  -e "s/@BUNDLE_ID@/$bundle_id/g" \
  -e "s/@EXECUTABLE@/$binary_name/g" \
  "$script_dir/Info.plist.template" > "$app_dir/Contents/Info.plist"
plutil -lint "$app_dir/Contents/Info.plist"

mkdir -p "$staging_dir"
cp -R "$app_dir" "$staging_dir/"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -volname "$app_name" -srcfolder "$staging_dir" -ov -format UDZO "$dmg_path"
rm -rf "$staging_dir"

echo "App bundle: $app_dir"
echo "Disk image: $dmg_path"

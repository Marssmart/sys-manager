#!/usr/bin/env bash
# Builds a release "Sys Manager.app" bundle, a zipped copy of it and a drag-to-install .dmg.
# The bundle is ad-hoc signed (no certificate). Developer ID signing and notarization are not part of this script. See README.md.
#
# Optional environment variables:
#   APP_VERSION=<version>  use this version instead of the one in the workspace Cargo.toml.
#   UNIVERSAL=1            build arm64 and x86_64 and combine them into one binary with lipo.
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../../.." && pwd)"

app_name="Sys Manager"
binary_name="sys-manager"
bundle_id="com.marssmart.sysmanager"
version="${APP_VERSION:-$(awk -F'"' '/^version/ { print $2; exit }' "$repo_root/Cargo.toml")}"

dist_dir="$repo_root/target/dist"
app_dir="$dist_dir/$app_name.app"
staging_dir="$dist_dir/dmg-staging"
zip_path="$dist_dir/Sys-Manager-$version.app.zip"
dmg_path="$dist_dir/Sys-Manager-$version.dmg"

universal="${UNIVERSAL:-0}"
if [[ "$universal" == 1 ]]; then
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --release --manifest-path "$repo_root/Cargo.toml" -p "$binary_name" --target "$target"
  done
else
  cargo build --release --manifest-path "$repo_root/Cargo.toml" -p "$binary_name"
fi

rm -rf "$dist_dir"
mkdir -p "$app_dir/Contents/MacOS" "$app_dir/Contents/Resources"
if [[ "$universal" == 1 ]]; then
  lipo -create -output "$app_dir/Contents/MacOS/$binary_name" \
    "$repo_root/target/aarch64-apple-darwin/release/$binary_name" \
    "$repo_root/target/x86_64-apple-darwin/release/$binary_name"
else
  cp "$repo_root/target/release/$binary_name" "$app_dir/Contents/MacOS/$binary_name"
fi
sed \
  -e "s/@VERSION@/$version/g" \
  -e "s/@BUNDLE_ID@/$bundle_id/g" \
  -e "s/@EXECUTABLE@/$binary_name/g" \
  "$script_dir/Info.plist.template" > "$app_dir/Contents/Info.plist"
plutil -lint "$app_dir/Contents/Info.plist"

# Ad-hoc seal (no certificate). The linker's signature does not cover Info.plist, and Gatekeeper treats that as an invalid signature.
codesign --force --sign - "$app_dir"

ditto -c -k --sequesterRsrc --keepParent "$app_dir" "$zip_path"

mkdir -p "$staging_dir"
cp -R "$app_dir" "$staging_dir/"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -volname "$app_name" -srcfolder "$staging_dir" -ov -format UDZO "$dmg_path"
rm -rf "$staging_dir"

echo "App bundle: $app_dir"
echo "Zipped app: $zip_path"
echo "Disk image: $dmg_path"

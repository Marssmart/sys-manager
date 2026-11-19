# macOS packaging

`build-app.sh` builds a release binary and packages it as `Sys Manager.app` and a drag-to-install `.dmg`. The bundle sets `LSUIElement`, so the app has a menu bar item and no Dock icon.

## Requirements

- macOS 11 or later. The status item uses an SF Symbol, and `LSMinimumSystemVersion` is 11.0.
- Rust 1.95 or later from rustup. `sysinfo` requires 1.95, so make sure `cargo` on your `PATH` is the rustup one.
- Xcode Command Line Tools, for the linker.
- `plutil` and `hdiutil`, which ship with macOS.

## Build and package

```sh
platform/macos/packaging/build-app.sh
```

The script writes:

- `target/dist/Sys Manager.app`
- `target/dist/Sys-Manager-<version>.dmg`

It builds for the host architecture only and does not sign or notarize. The version comes from the workspace `Cargo.toml`.

## Install and run

Open the `.dmg` and drag **Sys Manager** into **Applications**. Or run the bundle directly:

```sh
open "target/dist/Sys Manager.app"
```

The app has no Dock icon. Quit it from the **Quit** button in the popover.

Unsigned builds that were downloaded are blocked by Gatekeeper. Open **System Settings → Privacy & Security** and choose **Open Anyway**, or sign and notarize the build as described below. A build made on the same Mac runs without this step.

## Code signing and notarization (not implemented here)

These steps need an Apple Developer Program membership. Certificates and credentials stay in your keychain and must never be committed to this repository. The build script does not perform them.

1. Install a **Developer ID Application** certificate in your login keychain.

2. Sign the app bundle with the hardened runtime and check the signature:

   ```sh
   codesign --force --options runtime --timestamp \
     --sign "Developer ID Application: Your Name (TEAMID)" \
     "target/dist/Sys Manager.app"
   codesign --verify --strict --verbose=2 "target/dist/Sys Manager.app"
   ```

3. Rebuild the disk image around the signed app, then sign the image:

   ```sh
   staging=target/dist/signed-staging
   mkdir -p "$staging"
   cp -R "target/dist/Sys Manager.app" "$staging/"
   ln -s /Applications "$staging/Applications"
   hdiutil create -volname "Sys Manager" -srcfolder "$staging" -ov -format UDZO \
     target/dist/Sys-Manager-signed.dmg
   rm -rf "$staging"
   codesign --force --timestamp \
     --sign "Developer ID Application: Your Name (TEAMID)" \
     target/dist/Sys-Manager-signed.dmg
   ```

4. Store notarization credentials once in the keychain, then submit and wait for the result. Use an app-specific password, not your Apple ID password.

   ```sh
   xcrun notarytool store-credentials sys-manager-notary \
     --apple-id "you@example.com" --team-id TEAMID --password "<app-specific password>"
   xcrun notarytool submit target/dist/Sys-Manager-signed.dmg \
     --keychain-profile sys-manager-notary --wait
   ```

5. Staple the ticket to the disk image and verify it:

   ```sh
   xcrun stapler staple target/dist/Sys-Manager-signed.dmg
   xcrun stapler validate target/dist/Sys-Manager-signed.dmg
   spctl --assess --type open --context context:primary-signature --verbose \
     target/dist/Sys-Manager-signed.dmg
   ```

The app currently needs no entitlements. If a future feature needs one, add it to the codesign step and review the hardened runtime settings.

## Changing the bundle

The app name, bundle identifier (`com.marssmart.sysmanager`) and binary name are set at the top of `build-app.sh`. The plist keys live in `Info.plist.template`.

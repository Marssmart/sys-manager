# macOS packaging

`build-app.sh` builds a release binary and packages it as `Sys Manager.app` and a drag-to-install `.dmg`. The bundle sets `LSUIElement`, so the app has a menu bar item and no Dock icon.

## Requirements

- macOS 11 or later. The status item uses an SF Symbol, and `LSMinimumSystemVersion` is 11.0.
- Rust 1.95 or later from rustup. `sysinfo` requires 1.95, so make sure `cargo` on your `PATH` is the rustup one.
- Xcode Command Line Tools, for the linker.
- `plutil`, `hdiutil`, `ditto` and `lipo`, which ship with macOS.

## Build and package

```sh
platform/macos/packaging/build-app.sh
```

The script writes:

- `target/dist/Sys Manager.app`
- `target/dist/Sys-Manager-<version>.app.zip`, the app zipped with `ditto`, which keeps its metadata
- `target/dist/Sys-Manager-<version>.dmg`

It is ad-hoc signed with no certificate. The script does not use a Developer ID or notarize the app. Two optional environment variables change what it builds:

- `UNIVERSAL=1` builds `aarch64-apple-darwin` and `x86_64-apple-darwin` and joins the two binaries with `lipo`. It runs `rustup target add` for both targets first. Without it, the script builds for the host architecture only.
- `APP_VERSION=<version>` replaces the version from the workspace `Cargo.toml`.

## Install and run

Open the `.zip` and move **Sys Manager.app** to **Applications**, or open the `.dmg` and drag the app there. Or run the bundle directly:

```sh
open "target/dist/Sys Manager.app"
```

The app has no Dock icon. Quit it from the **Quit** button in the popover.

### First launch of an unsigned build

A downloaded build has no Developer ID signature and is not notarized, so Gatekeeper blocks it the first time it opens:

1. Open **Sys Manager**. macOS blocks it.
2. Open **System Settings → Privacy & Security**, scroll to **Security**, and choose **Open Anyway** next to the message about Sys Manager. If the button is not there, open the app once more and check again.
3. Confirm with your password or Touch ID. If macOS asks again, choose **Open**.

A build made on the same Mac runs without these steps. Signing and notarizing the build, described below, also avoids them.

## Release

The **Release macOS app** workflow (`.github/workflows/release-macos.yml`) builds a universal app on a GitHub-hosted macOS runner. Every run uploads the zip and the dmg as the `sys-manager-macos` artifact. A version tag also creates a GitHub Release with both files attached and the first-launch steps in its notes.

To cut a release:

1. Set `version` under `[workspace.package]` in the root `Cargo.toml` to the new version, and merge that change to `main`. The bundle version comes from the tag, but keeping `Cargo.toml` in step avoids a mismatch.
2. Tag the merged commit and push the tag. The tag must be `v` followed by numbers and dots, such as `v0.1.0`. A suffix such as `-rc1` fails the run.

   ```sh
   git tag v0.1.0
   git push origin v0.1.0
   ```

To build without publishing, open the **Actions** tab, choose **Release macOS app**, and select **Run workflow**. Manual runs use the version from `Cargo.toml` and do not create a release. GitHub only offers that button once the workflow file is on the default branch.

A run's artifact downloads as one zip that contains the app zip and the dmg. If a release for the tag already exists, the release step fails. Delete that release on GitHub, then re-run the workflow run.

## Developer ID signing and notarization (not implemented here)

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

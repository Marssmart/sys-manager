# sys-manager

A Rust utility with a macOS menu bar app. OS-specific code is kept out of shared logic so other operating systems can be added later.

## Layout

- `common/` (`sys-manager-common`): platform-neutral logic. Must contain no OS-specific code.
- `platform/<os>/` (`sys-manager-platform-<os>`): OS-specific logic, one crate per operating system. macOS lives in `platform/macos/`.
- `app/` (`sys-manager`): thin binary and future menu bar entry point. Selects the platform crate for the current target with `cfg(target_os)`.

## Dependency rule

Platform crates depend on `common`. `common` never depends on a platform crate. `app` depends on `common` and the platform crate for the current target OS.

To add an OS, create `platform/<os>/` and add a `cfg(target_os)` dependency and alias in `app/`.

## Build

```sh
cargo build --workspace
cargo run -p sys-manager
```

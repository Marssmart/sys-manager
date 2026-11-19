//! Sys Manager entry point. Starts the platform crate for the current target OS.

#[cfg(target_os = "macos")]
fn main() {
    sys_manager_platform_macos::run();
}

#[cfg(not(target_os = "macos"))]
compile_error!("unsupported target OS: add a platform crate and select it in app/src/main.rs");

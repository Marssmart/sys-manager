use sys_manager_common::placeholder::Platform;

#[cfg(target_os = "macos")]
use sys_manager_platform_macos::placeholder::MacOs as CurrentPlatform;

#[cfg(not(target_os = "macos"))]
compile_error!("unsupported target OS: add a platform crate and select it in app/src/main.rs");

fn main() {
    println!("sys-manager placeholder on {}", CurrentPlatform.name());
}

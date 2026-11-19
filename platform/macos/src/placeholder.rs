use sys_manager_common::placeholder::Platform;

pub struct MacOs;

impl Platform for MacOs {
    fn name(&self) -> &'static str {
        "macOS"
    }
}

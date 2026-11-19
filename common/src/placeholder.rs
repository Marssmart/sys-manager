/// Contract that every platform crate implements.
pub trait Platform {
    fn name(&self) -> &'static str;
}

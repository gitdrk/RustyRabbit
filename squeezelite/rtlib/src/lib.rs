#![no_std]
// Empty on purpose: the archive exists to carry compiler_builtins.
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

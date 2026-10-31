#[path = "build/selected_core.rs"]
mod selected_core;
fn main() {
    selected_core::emit();
}

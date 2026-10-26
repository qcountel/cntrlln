//! Embeds the product icon in the client executables.

#[path = "../packaging/windows/exe_resources.rs"]
mod exe_resources;

fn main() {
    exe_resources::embed(None);
}

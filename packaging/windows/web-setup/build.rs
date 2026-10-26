//! Embeds the product icon and the installer's manifest: modern common controls, system DPI
//! awareness and no elevation.

#[path = "../exe_resources.rs"]
mod exe_resources;

fn main() {
    exe_resources::embed(Some(include_str!("setup.manifest")));
}

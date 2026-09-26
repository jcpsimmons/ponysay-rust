// Keep the original artwork and per-image licensing metadata intact. Assets are
// compiled into the executable so an installation never depends on Homebrew.
use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let mut source = String::from("static ASSETS: &[Asset] = &[\n");
    for group in [
        "ponies",
        "extraponies",
        "ttyponies",
        "extrattyponies",
        "ponyquotes",
        "balloons",
    ] {
        println!("cargo:rerun-if-changed={group}");
        let mut entries: Vec<_> = fs::read_dir(root.join(group))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.is_file())
            .collect();
        entries.sort();
        for path in entries {
            let name = path.file_name().unwrap().to_str().unwrap();
            if name.starts_with('.') || (group.contains("ponies") && !name.ends_with(".pony")) {
                continue;
            }
            let canonical = path.canonicalize().unwrap();
            let canonical_name = canonical.file_name().unwrap().to_str().unwrap();
            // include_str follows the original symlinks, preserving pony aliases.
            source.push_str(&format!("Asset {{ group: {group:?}, name: {name:?}, canonical: {canonical_name:?}, data: include_str!({:?}) }},\n", path.to_str().unwrap()));
        }
    }
    source.push_str("];\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("assets.rs"),
        source,
    )
    .unwrap();
}

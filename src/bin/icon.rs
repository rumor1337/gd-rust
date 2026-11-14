// dump dart_01_001 for orientation check
fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let assets = format!("{}/Documents/gd-clone", std::env::var("HOME").unwrap());
    let mut store = gdrust::render::atlas::SpriteStore::load(std::path::Path::new(&assets));
    if let Some(p) = store.sprite("dart_01_001.png") {
        std::fs::write("/tmp/opencode/dart.png", p.encode_png().unwrap()).unwrap();
        println!("{}x{}", p.width(), p.height());
    } else {
        println!("miss");
    }
}

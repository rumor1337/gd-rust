// Dump every p2 (sim p0) input toggle in frames 0..500 — the opening corridor.
fn main() {
    let m = gdrust::bot::gdr2::parse_gdr2(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap()).unwrap();
    println!("framerate={} deaths={:?} inputs={}", m.framerate, m.deaths, m.inputs.len());
    for i in m.inputs.iter().filter(|i| i.player2) {
        if i.frame <= 500 {
            println!("p2 {} {} {}", i.frame, if i.down { 'T' } else { 'F' }, i.button);
        }
    }
    let p1n = m.inputs.iter().filter(|i| !i.player2).count();
    let p2n = m.inputs.iter().filter(|i| i.player2).count();
    println!("p1 events: {p1n}, p2 events: {p2n}");
}


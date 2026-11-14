fn main() {
    let m = gdrust::bot::gdr2::parse_gdr2(&std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/macros/Cerealmilkbowl-GRIEF-xdBot.gdr2")).unwrap()).unwrap();
    let p2 = m.inputs.iter().filter(|i| i.player2).count();
    let p1 = m.inputs.len() - p2;
    println!("p1 inputs: {p1}, p2 inputs: {p2}");
    println!("first 10: {:?}", m.inputs.iter().take(10).map(|i| (i.frame, i.down, i.player2)).collect::<Vec<_>>());
    // longest hold run
    let mut runs = vec![];
    let mut cur = 0u64; let mut start = 0u64; let mut hold = false;
    for i in &m.inputs {
        if i.down != hold { runs.push((start, i.frame, hold)); hold = i.down; start = i.frame; cur = 0; }
    }
        println!("p1 events: {:?}", m.inputs.iter().filter(|i| !i.player2).map(|i| (i.frame, i.down)).collect::<Vec<_>>());
    println!("p2 first 5: {:?}", m.inputs.iter().filter(|i| i.player2).take(5).map(|i| (i.frame, i.down)).collect::<Vec<_>>());
println!("event count: {}", m.inputs.len());
    let ups: u64 = m.inputs.iter().map(|i| i.frame).zip(m.inputs.iter().skip(1).map(|i| i.frame)).map(|(a,b)| b-a).sum();
    println!("total span: {ups}");
}
// p1 events appended below in main via second run? simpler: separate print

use gdrust::level::parse_level;
fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let s = std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap();
    let lv = parse_level(&s);
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "list").unwrap_or(false) {
        // list id x0 x1: every object of that id in range
        let id: u16 = args[1].parse().unwrap();
        let (x0, x1): (f32, f32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
        for o in lv.objects.iter().filter(|o| o.id == id && o.x >= x0 && o.x <= x1) {
            println!("x={} y={} g={:?} raw={:?}", o.x, o.y, o.groups, o.raw);
        }
        if id == 0 {
            let mut c: std::collections::BTreeMap<(String, String), usize> = Default::default();
            for o in &lv.objects {
                let g = |k: u16| o.raw.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v.clone()).unwrap_or_default();
                *c.entry((g(24), g(25))).or_default() += 1;
            }
            for (k, n) in c.iter().filter(|(_, n)| **n > 300) { println!("z24={:?} z25={:?} n={n}", k.0, k.1); }
        }
        return;
    }
    if args.first().map(|a| a == "colortrig").unwrap_or(false) {
        let mut c: std::collections::BTreeMap<String, usize> = Default::default();
        for o in lv.objects.iter().filter(|o| matches!(o.id, 899 | 29 | 30)) {
            let has = |k: u16| o.raw.iter().any(|(kk, v)| *kk == k && v != "0");
            let key = format!("spawn={} touch={} copy={} opacity={} blend={}", has(62), has(11), has(50), has(35), has(17));
            *c.entry(key).or_default() += 1;
        }
        for (k, n) in c { println!("{n:6} {k}"); }
        return;
    }
    if args.first().map(|a| a == "channels").unwrap_or(false) {
        for c in &lv.header.channels {
            println!("{:5} rgb=({:3},{:3},{:3}) op={:.2} copy={:?} blend={}", c.id, c.r, c.g, c.b, c.opacity, c.copy, c.blending);
        }
        return;
    }
    if args.first().map(|a| a == "range").unwrap_or(false) {
        // range x0 x1: id histogram with color channel, groups and keys
        let (x0, x1): (f32, f32) = (args[1].parse().unwrap(), args[2].parse().unwrap());
        let mut c: std::collections::BTreeMap<(u16, Option<u16>, Option<u16>), (usize, String)> = Default::default();
        for o in lv.objects.iter().filter(|o| o.x >= x0 && o.x <= x1) {
            let e = c.entry((o.id, o.base_color, o.detail_color)).or_insert((0, String::new()));
            e.0 += 1;
            if e.1.is_empty() {
                e.1 = format!("groups={:?} raw={:?}", o.groups, o.raw);
            }
        }
        let mut v: Vec<_> = c.into_iter().collect();
        v.sort_by_key(|(_, (n, _))| std::cmp::Reverse(*n));
        for ((id, b, d), (n, ex)) in v.into_iter().take(2000) {
            println!("{n:5} id={id} base={b:?} detail={d:?} {ex}");
        }
        return;
    }
    let ids: Vec<usize> = args.iter().filter_map(|a| a.parse().ok()).collect();
    if ids.is_empty() {
        let mut c = std::collections::BTreeMap::new();
        for o in &lv.objects {
            *c.entry(o.id).or_insert(0) += 1;
        }
        for (id, n) in c {
            println!("{id} {n}");
        }
    }
    for i in ids {
        let o = &lv.objects[i];
        println!("{i}: id={} x={} y={} raw={:?}", o.id, o.x, o.y, o.raw);
    }
}

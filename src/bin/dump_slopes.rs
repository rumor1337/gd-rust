use gdrust::level::parse_level;
fn main() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let s = std::fs::read_to_string(format!("{manifest}/levels/149992717.gmd")).unwrap();
    let lv = parse_level(&s);
    for o in lv.objects.iter() {
        if matches!(o.id, 326 | 327 | 1717 | 1718 | 1338 | 328) && o.x < 300.0 {
            println!("id={} x={} y={} fx={} fy={} rot={} sx={} sy={}",
                o.id, o.x, o.y, o.flip_x, o.flip_y, o.rot, o.scale_x, o.scale_y);
        }
    }
}

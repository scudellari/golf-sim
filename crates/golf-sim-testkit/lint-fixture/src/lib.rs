use std::collections::BTreeMap;
use std::collections::HashMap; // banned
use std::collections::HashSet; // banned

pub fn transcendentals(x: f64, y: f32) -> f64 {
    let a = x.sin(); // banned
    let b = f64::cos(x); // banned
    let c = x.atan2(a); // banned
    let d = x.powi(2) + x.sqrt(); // banned
    let e = f64::from(y.exp()); // banned
    a + b + c + d + e
}

pub fn fused(a: f64, b: f64, c: f64) -> f64 {
    let unfused = a * b + c;
    unfused + a.mul_add(b, c) // banned
}

pub fn threads() {
    std::thread::spawn(|| ()); // banned
    std::thread::scope(|_| ()); // banned
}

pub fn maps() -> usize {
    let ordered: BTreeMap<u8, u8> = BTreeMap::new();
    let hashed: HashMap<u8, u8> = HashMap::new(); // banned
    let set: HashSet<u8> = HashSet::new(); // banned
    ordered.len() + hashed.len() + set.len()
}

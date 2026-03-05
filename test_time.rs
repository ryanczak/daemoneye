fn main() {
    let now_local = chrono::Local::now();
    let now_utc = chrono::Utc::now();
    println!("Local: {:?}", now_local);
    println!("UTC: {:?}", now_utc);
}

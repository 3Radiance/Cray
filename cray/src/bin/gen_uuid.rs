use uuid::Uuid;

fn main() {
    let id = Uuid::new_v4();

    println!("--------------------------------------------------");
    println!("Generated VLESS UUID:");
    println!("String format: {}", id);
    println!("Hyphenless:    {}", id.simple());
    let bytes = id.as_bytes();
    println!("Raw bytes:     {:?}", bytes);
    println!("--------------------------------------------------");
}

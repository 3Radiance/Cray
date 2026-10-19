use rand::RngCore;

fn main() {
    let mut key = [0u8; 32];
    rand::rng().fill_bytes(&mut key);

    println!("--------------------------------------------------");
    println!("Generated auth key:");
    println!("Hex format: {}", hex::encode(key));
    println!("Raw bytes:  {:?}", key);
    println!("--------------------------------------------------");
    println!("Put the hex string into the 'key' field of the");
    println!("auth layer in the config, on both sides (identical");
    println!("key everywhere).");
}

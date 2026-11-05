pub fn generate_session_id() -> String {
    
    hex::encode(rand::random::<[u8; 16]>())
}

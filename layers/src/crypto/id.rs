pub fn generate_session_id() -> String {
    let sid = hex::encode(rand::random::<[u8; 16]>());
    sid
}

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, KeyInit};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use sha2::{Digest, Sha256};
use std::io;

fn derive_key(password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

pub fn encrypt_field(plaintext: &str, password: &str) -> io::Result<String> {
    let key = derive_key(password);
    let cipher = Aes256Gcm::new(&key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| io::Error::other(format!("encrypt error: {}", e)))?;
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(BASE64.encode(&combined))
}

pub fn decrypt_field(ciphertext_b64: &str, password: &str) -> io::Result<String> {
    let key = derive_key(password);
    let cipher = Aes256Gcm::new(&key.into());
    let combined = BASE64
        .decode(ciphertext_b64)
        .map_err(|e| io::Error::other(format!("base64 decode error: {}", e)))?;
    if combined.len() < 12 {
        return Err(io::Error::other("ciphertext too short"));
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| io::Error::other(format!("decrypt error: {}", e)))?;
    String::from_utf8(plaintext).map_err(|e| io::Error::other(format!("utf8 error: {}", e)))
}


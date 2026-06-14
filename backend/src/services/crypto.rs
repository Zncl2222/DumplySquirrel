use aes_gcm::{
    aead::{rand_core::RngCore, Aead, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};

pub fn encrypt_string(plaintext: &str, key_material: &str) -> AppResult<(String, String)> {
    let cipher = Aes256Gcm::new_from_slice(&derive_key(key_material))
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;
    let mut nonce_bytes = [0_u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext.as_bytes())
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;

    Ok((STANDARD.encode(ciphertext), STANDARD.encode(nonce_bytes)))
}

pub fn decrypt_string(ciphertext: &str, nonce: &str, key_material: &str) -> AppResult<String> {
    let cipher = Aes256Gcm::new_from_slice(&derive_key(key_material))
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;
    let ciphertext = STANDARD
        .decode(ciphertext)
        .map_err(|err| AppError::Internal(err.into()))?;
    let nonce = STANDARD
        .decode(nonce)
        .map_err(|err| AppError::Internal(err.into()))?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;

    String::from_utf8(plaintext).map_err(|err| AppError::Internal(err.into()))
}

fn derive_key(key_material: &str) -> [u8; 32] {
    Sha256::digest(key_material.as_bytes()).into()
}

pub fn mask_database_url(input: &str) -> String {
    match url::Url::parse(input) {
        Ok(mut url) => {
            if url.password().is_some() {
                let _ = url.set_password(Some("****"));
            }
            url.to_string()
        }
        Err(_) => "****".to_string(),
    }
}

#[cfg(test)]
mod tests;

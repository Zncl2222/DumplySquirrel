use aes_gcm::{
    aead::{rand_core::RngCore, Aead, OsRng},
    Aes256Gcm, KeyInit, Nonce,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

const AES_GCM_NONCE_BYTES: usize = 12;
const AES_GCM_TAG_BYTES: usize = 16;

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
    if ciphertext.len() < AES_GCM_TAG_BYTES {
        return Err(AppError::Internal(anyhow::anyhow!(
            "encrypted value is shorter than the AES-GCM authentication tag"
        )));
    }
    let nonce = STANDARD
        .decode(nonce)
        .map_err(|err| AppError::Internal(err.into()))?;
    let nonce_length = nonce.len();
    let nonce: [u8; AES_GCM_NONCE_BYTES] = nonce.try_into().map_err(|_| {
        AppError::Internal(anyhow::anyhow!(
            "invalid AES-GCM nonce length: expected {AES_GCM_NONCE_BYTES} bytes, got {}",
            nonce_length
        ))
    })?;
    let nonce = Nonce::from(nonce);
    let plaintext = cipher
        .decrypt(&nonce, ciphertext.as_ref())
        .map_err(|err| AppError::Internal(anyhow::anyhow!(err.to_string())))?;

    String::from_utf8(plaintext).map_err(|err| AppError::Internal(err.into()))
}

fn derive_key(key_material: &str) -> [u8; 32] {
    Sha256::digest(key_material.as_bytes()).into()
}

/// Fails startup when the configured current key cannot decrypt every saved target URL.
/// This turns a forgotten `DATABASE_ENCRYPTION_KEY_PREVIOUS` into an explicit deployment error
/// instead of allowing the service to appear healthy until the next API call or backup run.
pub async fn validate_database_encryption_key(pool: &PgPool, current_key: &str) -> AppResult<u64> {
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, db_url_encrypted, db_url_nonce FROM backup_configs ORDER BY id",
    )
    .fetch_all(pool)
    .await?;

    for (id, ciphertext, nonce) in &rows {
        decrypt_string(ciphertext, nonce, current_key).map_err(|_| {
            AppError::Validation(format!(
                "DATABASE_ENCRYPTION_KEY cannot decrypt backup config {id}; restore the prior key or follow the DATABASE_ENCRYPTION_KEY_PREVIOUS rotation workflow"
            ))
        })?;
    }
    Ok(rows.len() as u64)
}

/// Transactionally re-encrypts saved target URLs from a previous key to the configured key.
/// Rows already encrypted with the current key are skipped, making a restart safe after success.
pub async fn rotate_database_encryption_key(
    pool: &PgPool,
    previous_key: &str,
    current_key: &str,
) -> AppResult<u64> {
    if previous_key == current_key {
        return Ok(0);
    }

    let mut transaction = pool.begin().await?;
    let rows = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT id, db_url_encrypted, db_url_nonce
        FROM backup_configs
        ORDER BY id
        FOR UPDATE
        "#,
    )
    .fetch_all(&mut *transaction)
    .await?;

    let mut rotated = 0_u64;
    for (id, ciphertext, nonce) in rows {
        let Some((new_ciphertext, new_nonce)) =
            reencrypt_if_needed(&ciphertext, &nonce, previous_key, current_key)?
        else {
            continue;
        };
        sqlx::query(
            r#"
            UPDATE backup_configs
            SET db_url_encrypted = $2, db_url_nonce = $3, updated_at = now()
            WHERE id = $1
            "#,
        )
        .bind(id)
        .bind(new_ciphertext)
        .bind(new_nonce)
        .execute(&mut *transaction)
        .await?;
        rotated += 1;
    }

    transaction.commit().await?;
    Ok(rotated)
}

fn reencrypt_if_needed(
    ciphertext: &str,
    nonce: &str,
    previous_key: &str,
    current_key: &str,
) -> AppResult<Option<(String, String)>> {
    if decrypt_string(ciphertext, nonce, current_key).is_ok() {
        return Ok(None);
    }
    let plaintext = decrypt_string(ciphertext, nonce, previous_key).map_err(|_| {
        AppError::Validation(
            "DATABASE_ENCRYPTION_KEY_PREVIOUS cannot decrypt one or more saved target URLs".into(),
        )
    })?;
    encrypt_string(&plaintext, current_key).map(Some)
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

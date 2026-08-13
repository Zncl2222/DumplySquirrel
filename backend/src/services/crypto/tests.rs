use super::*;

#[test]
fn encrypt_decrypt_roundtrip() {
    let key = "test key with enough entropy";
    let plaintext = "postgres://user:secret@example.com:5432/app";

    let (ciphertext, nonce) = encrypt_string(plaintext, key).unwrap();
    assert_ne!(ciphertext, plaintext);
    assert_eq!(decrypt_string(&ciphertext, &nonce, key).unwrap(), plaintext);
}

#[test]
fn mask_database_url_hides_password() {
    let masked = mask_database_url(
        "postgres://user:secret@example.com:5432/app?sslmode=require&access_token=query-secret",
    );

    assert!(masked.contains("user:****@"));
    assert!(!masked.contains("secret"));
    assert!(masked.contains("sslmode=require"));
    assert!(!masked.contains("query-secret"));
}

#[test]
fn decrypt_with_wrong_key_fails() {
    let (ciphertext, nonce) = encrypt_string("mysql://user:secret@example.com/app", "right key")
        .expect("encryption should succeed");

    assert!(decrypt_string(&ciphertext, &nonce, "wrong key").is_err());
}

#[test]
fn malformed_nonce_returns_an_error_without_panicking() {
    let (ciphertext, _) = encrypt_string("postgres://db/app", "test-key").unwrap();
    let malformed_nonces = [
        "not-base64".to_string(),
        STANDARD.encode([0_u8; AES_GCM_NONCE_BYTES - 1]),
        STANDARD.encode([0_u8; AES_GCM_NONCE_BYTES + 1]),
    ];

    for nonce in malformed_nonces {
        let result = std::panic::catch_unwind(|| decrypt_string(&ciphertext, &nonce, "test-key"));
        assert!(result.is_ok(), "malformed nonce must not panic");
        assert!(result.unwrap().is_err(), "malformed nonce must be rejected");
    }
}

#[test]
fn malformed_ciphertext_returns_an_error_without_panicking() {
    let (_, nonce) = encrypt_string("postgres://db/app", "test-key").unwrap();
    let malformed_ciphertexts = [
        "not-base64".to_string(),
        STANDARD.encode([0_u8; AES_GCM_TAG_BYTES - 1]),
    ];

    for ciphertext in malformed_ciphertexts {
        let result = std::panic::catch_unwind(|| decrypt_string(&ciphertext, &nonce, "test-key"));
        assert!(result.is_ok(), "malformed ciphertext must not panic");
        assert!(
            result.unwrap().is_err(),
            "malformed ciphertext must be rejected"
        );
    }
}

#[test]
fn reencrypts_old_ciphertext_and_skips_current_ciphertext() {
    let (ciphertext, nonce) = encrypt_string("postgres://db/app", "old-key").unwrap();
    let (rotated, rotated_nonce) = reencrypt_if_needed(&ciphertext, &nonce, "old-key", "new-key")
        .unwrap()
        .expect("old ciphertext should rotate");
    assert_eq!(
        decrypt_string(&rotated, &rotated_nonce, "new-key").unwrap(),
        "postgres://db/app"
    );
    assert!(
        reencrypt_if_needed(&rotated, &rotated_nonce, "old-key", "new-key")
            .unwrap()
            .is_none()
    );
    assert!(reencrypt_if_needed(&ciphertext, &nonce, "wrong-key", "new-key").is_err());
}

#[test]
fn mask_database_url_handles_urls_without_password_and_invalid_inputs() {
    assert_eq!(
        mask_database_url("postgres://user@example.com/app"),
        "postgres://user@example.com/app"
    );
    assert_eq!(mask_database_url("not a url"), "****");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn database_encryption_key_rotation_is_transactional_and_idempotent() {
    let database_url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to a disposable PostgreSQL database");
    let pool = crate::db::connect(&database_url).await.unwrap();
    crate::db::migrate(&pool).await.unwrap();
    let id = Uuid::new_v4();
    let plaintext = "postgres://user:secret@db.example/app";
    let (ciphertext, nonce) = encrypt_string(plaintext, "previous-key").unwrap();
    sqlx::query(
        r#"
        INSERT INTO backup_configs (id, name, db_type, db_url_encrypted, db_url_nonce)
        VALUES ($1, $2, 'postgres', $3, $4)
        "#,
    )
    .bind(id)
    .bind(format!("key-rotation-test-{id}"))
    .bind(ciphertext)
    .bind(nonce)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        rotate_database_encryption_key(&pool, "previous-key", "current-key")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        rotate_database_encryption_key(&pool, "previous-key", "current-key")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        validate_database_encryption_key(&pool, "current-key")
            .await
            .unwrap(),
        1
    );
    assert!(validate_database_encryption_key(&pool, "wrong-key")
        .await
        .is_err());
    let (rotated, rotated_nonce): (String, String) =
        sqlx::query_as("SELECT db_url_encrypted, db_url_nonce FROM backup_configs WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        decrypt_string(&rotated, &rotated_nonce, "current-key").unwrap(),
        plaintext
    );

    sqlx::query("DELETE FROM backup_configs WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
}

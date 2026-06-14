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
    let masked = mask_database_url("postgres://user:secret@example.com:5432/app");

    assert!(masked.contains("user:****@"));
    assert!(!masked.contains("secret"));
}

#[test]
fn decrypt_with_wrong_key_fails() {
    let (ciphertext, nonce) = encrypt_string("mysql://user:secret@example.com/app", "right key")
        .expect("encryption should succeed");

    assert!(decrypt_string(&ciphertext, &nonce, "wrong key").is_err());
}

#[test]
fn mask_database_url_handles_urls_without_password_and_invalid_inputs() {
    assert_eq!(
        mask_database_url("postgres://user@example.com/app"),
        "postgres://user@example.com/app"
    );
    assert_eq!(mask_database_url("not a url"), "****");
}

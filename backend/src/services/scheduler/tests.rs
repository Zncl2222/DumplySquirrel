use super::*;

#[test]
fn validate_cron_expression_accepts_six_field_schedule() {
    assert!(validate_cron_expression("0 0 2 * * *").is_ok());
}

#[test]
fn validate_cron_expression_rejects_invalid_schedule() {
    assert!(matches!(
        validate_cron_expression("not a cron"),
        Err(AppError::Validation(message)) if message.starts_with("invalid cron_schedule:")
    ));
}

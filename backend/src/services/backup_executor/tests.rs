use super::*;

#[test]
fn validates_matching_database_url_scheme() {
    assert!(validate_database_url("postgres", "postgres://user:pass@localhost/app").is_ok());
    assert!(validate_database_url("mysql", "mysql://user:pass@localhost/app").is_ok());
    assert!(validate_database_url("mysql", "postgres://user:pass@localhost/app").is_err());
}

#[test]
fn rejects_newlines_in_database_url_components() {
    assert!(validate_database_url("postgres", "postgres://user%0A:pass@localhost/app").is_err());
}

#[test]
fn parses_supported_postgres_server_versions() {
    assert_eq!(
        postgres_major_from_server_version_num("160002").unwrap(),
        "16"
    );
    assert_eq!(
        postgres_major_from_server_version_num("180000").unwrap(),
        "18"
    );
    assert!(postgres_major_from_server_version_num("130000").is_err());
}

#[test]
fn parses_pg_dump_major_version() {
    assert_eq!(
        postgres_major_from_pg_dump_version("pg_dump (PostgreSQL) 16.4").unwrap(),
        "16"
    );
}

#[test]
fn dump_target_applies_default_ports_and_decodes_components() {
    let target = DumpTarget::parse("postgres://user%2Bname:pa%25ss@db.example.com/app%2Ddb")
        .expect("target should parse");

    assert_eq!(target.scheme, "postgres");
    assert_eq!(target.host, "db.example.com");
    assert_eq!(target.port, 5432);
    assert_eq!(target.username, "user+name");
    assert_eq!(target.password.as_deref(), Some("pa%ss"));
    assert_eq!(target.database, "app-db");

    let target = DumpTarget::parse("mysql://user:pass@db.example.com:3307/app")
        .expect("target should parse");
    assert_eq!(target.port, 3307);
}

#[test]
fn dump_target_rejects_missing_username_database_and_unsupported_scheme() {
    assert!(matches!(
        DumpTarget::parse("postgres://localhost/app"),
        Err(AppError::Validation(message)) if message == "db_url username is required"
    ));
    assert!(matches!(
        DumpTarget::parse("postgres://user:pass@localhost"),
        Err(AppError::Validation(message)) if message == "db_url database name is required"
    ));
    assert!(matches!(
        DumpTarget::parse("sqlite://user:pass@localhost/app"),
        Err(AppError::Validation(message)) if message == "unsupported db_url scheme"
    ));
}

#[test]
fn parses_pg_dump_major_version_from_distribution_output() {
    assert_eq!(
        postgres_major_from_pg_dump_version("pg_dump (Ubuntu 16.9-0ubuntu0.24.04.1) 16.9").unwrap(),
        "16"
    );
    assert!(postgres_major_from_pg_dump_version("pg_dump version unknown").is_err());
}

#[test]
fn mysql_option_file_contains_credentials_without_url_arguments() {
    let target = DumpTarget::parse("mysql://user:secret@localhost/app").unwrap();
    let option_file = mysql_option_file(&target).unwrap();
    let contents = std::fs::read_to_string(option_file.path()).unwrap();

    assert!(contents.contains("[client]"));
    assert!(contents.contains("host=localhost"));
    assert!(contents.contains("port=3306"));
    assert!(contents.contains("user=user"));
    assert!(contents.contains("password=secret"));
}

use pip_mirror::config::validator::{ConfigError, ConfigValidator};
use pip_mirror::config::{Config, PackageSpec, PackageUrlSpec};

#[test]
fn test_config_deserializes_mixed_packages() {
    let toml = r#"
packages = [
    "requests",
    { url = "https://example.com/foo-1.0-py3-none-any.whl" },
    { url = "file:///opt/wheels/bar-1.0-py3-none-any.whl", sha256 = "abc123" },
]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    assert_eq!(cfg.packages.len(), 3);
    assert!(
        matches!(cfg.packages[0], PackageSpec::Name(ref n) if n == "requests")
    );
    assert_eq!(
        cfg.packages[1].as_url(),
        Some("https://example.com/foo-1.0-py3-none-any.whl")
    );
    let (url, sha256) = match &cfg.packages[2] {
        PackageSpec::Url(u) => (u.url.as_str(), u.sha256.as_deref()),
        _ => panic!("expected Url variant"),
    };
    assert_eq!(url, "file:///opt/wheels/bar-1.0-py3-none-any.whl");
    assert_eq!(sha256, Some("abc123"));
}

#[test]
fn test_config_accepts_uv_embed_section() {
    let toml = r#"
packages = ["requests"]
repository_dir = "./packages"

[uv_embed]
version = "0.11.14"
"#;
    let cfg: Config =
        toml::from_str(toml).expect("should parse uv_embed section");
    assert_eq!(cfg.uv_embed.version, Some("0.11.14".to_string()));
}

#[test]
fn test_config_backward_compatible_strings_only() {
    let toml = r#"
packages = ["requests", "openai"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    assert_eq!(cfg.packages.len(), 2);
    assert!(
        matches!(cfg.packages[0], PackageSpec::Name(ref n) if n == "requests")
    );
    assert!(
        matches!(cfg.packages[1], PackageSpec::Name(ref n) if n == "openai")
    );
}

#[test]
fn test_config_accepts_version_spec() {
    let toml = r#"
packages = [
    "numpy==2.5.0",
    "geopandas[all]==5.0.0",
    "pandas>=1.20,<2.0",
]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_config_rejects_invalid_version_spec() {
    let toml = r#"
packages = ["numpy==abc"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("无效") || err.contains("版本"));
}

#[test]
fn test_config_rejects_space_in_version_spec() {
    let toml = r#"
packages = ["numpy == 2.5.0"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("空格"));
}

#[test]
fn test_config_rejects_duplicate_version_spec() {
    let toml = r#"
packages = ["numpy>=1.0", "numpy==2.5.0"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("should parse");
    let err = cfg.validate().expect_err("should fail");
    assert!(err.contains("重复") || err.contains("duplicate"));
}

#[test]
fn test_config_url_variant_rejects_unknown_fields() {
    let toml = r#"
packages = [
    { url = "https://example.com/foo.whl", sha265 = "abc123" },
]
repository_dir = "./packages"
"#;
    assert!(toml::from_str::<Config>(toml).is_err());
}

#[test]
fn test_config_rejects_url_string_mistaken_for_name() {
    let toml = r#"
packages = ["https://example.com/foo.whl"]
repository_dir = "./packages"
"#;
    let cfg: Config = toml::from_str(toml).expect("parse ok");
    let err = cfg.validate().expect_err("should fail validation");
    assert!(err.contains("URL") || err.contains("url"));
}

#[test]
fn test_config_validate_is_case_insensitive_for_schemes() {
    let cfg = Config {
        packages: vec![PackageSpec::Name(
            "HTTPS://example.com/foo.whl".to_string(),
        )],
        ..Config::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_validate_rejects_unsupported_url_scheme() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            url: "ftp://example.com/foo.whl".to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_validate_rejects_non_whl_url() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            url: "https://example.com/foo.tar.gz".to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    assert!(cfg.validate().is_err());
}

#[test]
fn test_config_validate_accepts_whl_url_with_query() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            url: "https://example.com/foo-1.0-py3-none-any.whl?token=secret"
                .to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    assert!(cfg.validate().is_ok());
}

#[test]
fn test_config_validate_redacts_malformed_url_error() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            // Valid scheme, but bad port causes Url::parse to fail.
            url: "https://user:pass@example.com:badport/foo.whl".to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    let err = cfg.validate().expect_err("should fail");
    let msg = err.to_string();
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
}

#[test]
fn test_config_validate_url_string_mistake_does_not_leak_credentials() {
    let cfg = Config {
        packages: vec![PackageSpec::Name(
            "https://user:pass@example.com/foo.whl?token=secret".to_string(),
        )],
        ..Config::default()
    };
    let err = cfg.validate().expect_err("should fail");
    let msg = err.to_string();
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=secret"), "error leaked token: {msg}");
}

#[test]
fn test_config_validate_unsupported_scheme_does_not_leak_credentials() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            url: "ftp://user:pass@example.com/foo.whl?token=secret".to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    let err = cfg.validate().expect_err("should fail");
    let msg = err.to_string();
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=secret"), "error leaked token: {msg}");
}

#[test]
fn test_config_validate_non_whl_url_does_not_leak_credentials() {
    let cfg = Config {
        packages: vec![PackageSpec::Url(PackageUrlSpec {
            url: "https://user:pass@example.com/foo.tar.gz?token=secret"
                .to_string(),
            sha256: None,
        })],
        ..Config::default()
    };
    let err = cfg.validate().expect_err("should fail");
    let msg = err.to_string();
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=secret"), "error leaked token: {msg}");
}

#[test]
fn test_config_validator_accepts_valid_package_urls() {
    let urls = [
        "https://example.com/foo-1.0-py3-none-any.whl",
        "http://example.com/foo-1.0-py3-none-any.whl",
        "file:///opt/wheels/bar-1.0-py3-none-any.whl",
    ];
    for url in urls {
        let spec = PackageUrlSpec {
            url: url.to_string(),
            sha256: None,
        };
        assert!(
            ConfigValidator::validate_package_url(&spec).is_ok(),
            "should accept {url}"
        );
    }
}

#[test]
fn test_config_validator_rejects_ftp_package_url() {
    let spec = PackageUrlSpec {
        url: "ftp://example.com/foo.whl".to_string(),
        sha256: None,
    };
    let err =
        ConfigValidator::validate_package_url(&spec).expect_err("should fail");
    assert!(
        matches!(err, ConfigError::InvalidPackageUrl { .. }),
        "unexpected error variant: {err}"
    );
}

#[test]
fn test_config_validator_rejects_non_whl_url() {
    let spec = PackageUrlSpec {
        url: "https://example.com/foo.tar.gz".to_string(),
        sha256: None,
    };
    let err =
        ConfigValidator::validate_package_url(&spec).expect_err("should fail");
    assert!(
        matches!(err, ConfigError::InvalidPackageUrl { .. }),
        "unexpected error variant: {err}"
    );
}

#[test]
fn test_config_validator_redacts_credentials_in_package_url_error() {
    let spec = PackageUrlSpec {
        url: "ftp://user:pass@example.com/foo.whl?token=secret".to_string(),
        sha256: None,
    };
    let msg = ConfigValidator::validate_package_url(&spec)
        .expect_err("should fail")
        .to_string();
    assert!(
        !msg.contains("user:pass"),
        "error leaked credentials: {msg}"
    );
    assert!(!msg.contains("token=secret"), "error leaked token: {msg}");
}

#[test]
fn test_config_validator_validate_mirrors() {
    assert!(
        ConfigValidator::validate_mirrors(&[
            "https://pypi.org".to_string(),
            "http://mirror.example.com".to_string(),
        ])
        .is_ok()
    );

    let err = ConfigValidator::validate_mirrors(&[
        "ftp://mirror.example.com".to_string()
    ])
    .expect_err("should reject ftp");
    assert!(
        matches!(err, ConfigError::InvalidMirror { .. }),
        "unexpected error variant: {err}"
    );

    let err =
        ConfigValidator::validate_mirrors(&["file:///opt/mirror".to_string()])
            .expect_err("should reject file");
    assert!(
        matches!(err, ConfigError::InvalidMirror { .. }),
        "unexpected error variant: {err}"
    );
}

#[test]
fn test_config_validator_looks_like_url() {
    assert!(ConfigValidator::looks_like_url(
        "https://example.com/foo.whl"
    ));
    assert!(ConfigValidator::looks_like_url(
        "HTTP://example.com/foo.whl"
    ));
    assert!(ConfigValidator::looks_like_url(
        "file:///opt/wheels/foo.whl"
    ));
    assert!(!ConfigValidator::looks_like_url("requests"));
    assert!(!ConfigValidator::looks_like_url(""));
}

#[test]
fn test_config_error_debug_redacts_credentials() {
    let spec = PackageUrlSpec {
        url: "ftp://user:pass@example.com/foo.whl?token=secret".to_string(),
        sha256: None,
    };
    let err =
        ConfigValidator::validate_package_url(&spec).expect_err("should fail");
    let debug = format!("{err:?}");
    assert!(
        !debug.contains("user:pass"),
        "Debug output leaked credentials: {debug}"
    );
    assert!(
        !debug.contains("token=secret"),
        "Debug output leaked token: {debug}"
    );
}

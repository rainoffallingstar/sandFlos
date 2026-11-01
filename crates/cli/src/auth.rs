use crate::args::GlobalOptions;
use crate::error::CliError;
use boox_core::auth_service::{
    credential_store, AppPaths, AuthError, AuthErrorKind, AuthService, AuthStatus,
    CredentialStoreKind, HttpAuthClient, LogoutOutcome, QrLoginChallenge, QrLoginOptions,
};
use qrcodegen::{QrCode, QrCodeEcc};
use serde::Serialize;
use std::{
    io::{self, IsTerminal, Read},
    path::PathBuf,
    time::Duration,
};

const TOKEN_ENV: &str = "BOOXCTL_TOKEN";
const STORE_ENV: &str = "BOOXCTL_CREDENTIAL_STORE";
const TEST_AUTH_ENDPOINT_ENV: &str = "BOOXCTL_TEST_AUTH_BASE_URL";
const TEST_API_ENDPOINT_ENV: &str = "BOOXCTL_TEST_API_BASE_URL";
const TEST_OSS_ENDPOINT_ENV: &str = "BOOXCTL_TEST_OSS_BASE_URL";

pub fn test_api_endpoints() -> Result<Option<(String, String)>, CliError> {
    fn read(name: &str) -> Result<Option<String>, CliError> {
        match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(std::env::VarError::NotUnicode(_)) => Err(invalid_test_endpoint()),
        }
    }
    let api = read(TEST_API_ENDPOINT_ENV)?;
    let oss = read(TEST_OSS_ENDPOINT_ENV)?;
    match (api, oss) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(invalid_test_endpoint()),
        (Some(api), oss) => {
            let oss = oss.unwrap_or_else(|| api.clone());
            boox_core::api::ApiEndpoints::loopback(&api, &oss)
                .map_err(|_| invalid_test_endpoint())?;
            Ok(Some((api, oss)))
        }
    }
}

fn invalid_test_endpoint() -> CliError {
    CliError::usage("test API/OSS endpoints must be literal loopback origins; OSS requires API")
        .with_stable_code("invalid_test_endpoint")
}

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Err(CliError::usage(
            "auth requires a subcommand: import, status, logout, or login",
        ));
    };
    let rest = &args[1..];

    match subcommand {
        "import" => import(options, rest),
        "status" => status(options, rest),
        "logout" => logout(options, rest),
        "login" => login(options, rest),
        other => Err(CliError::usage(format!(
            "unknown auth subcommand: {other}; expected import, status, logout, or login"
        ))),
    }
}

pub fn service(options: &GlobalOptions) -> Result<AuthService, CliError> {
    let paths =
        AppPaths::from_process_env(options.config_dir.clone()).map_err(CliError::from_auth)?;
    let store = credential_store(store_kind()?, &paths);
    let environment_token = std::env::var(TOKEN_ENV).ok();
    let timeout = Duration::from_secs(options.timeout_secs.unwrap_or(20));
    let client = match std::env::var(TEST_AUTH_ENDPOINT_ENV) {
        Ok(endpoint) => loopback_auth_client(timeout, &endpoint)?,
        Err(std::env::VarError::NotPresent) => HttpAuthClient::new(timeout),
        Err(std::env::VarError::NotUnicode(_)) => return Err(invalid_auth_endpoint()),
    };
    Ok(AuthService::new(
        paths,
        Box::new(client),
        store,
        environment_token,
    ))
}

fn invalid_auth_endpoint() -> CliError {
    CliError::usage("BOOXCTL_TEST_AUTH_BASE_URL must be a literal http(s) loopback origin")
        .with_stable_code("invalid_test_auth_endpoint")
}

fn loopback_auth_client(timeout: Duration, endpoint: &str) -> Result<HttpAuthClient, CliError> {
    boox_core::api::ApiEndpoints::loopback(endpoint, endpoint)
        .map_err(|_| invalid_auth_endpoint())?;
    let normalized = url::Url::parse(endpoint)
        .map_err(|_| invalid_auth_endpoint())?
        .origin()
        .ascii_serialization();
    HttpAuthClient::with_base_url(timeout, normalized).map_err(CliError::from_auth)
}

pub fn credential(options: &GlobalOptions) -> Result<String, CliError> {
    service(options)?
        .resolve_token()
        .map_err(CliError::from_auth)?
        .map(|token| token.expose().to_string())
        .ok_or_else(|| {
            CliError::new(
                crate::error::CliErrorCode::Auth,
                "not authenticated; run `booxctl auth import --token-stdin` or set BOOXCTL_TOKEN",
            )
            .with_stable_code("missing_credentials")
        })
}

pub fn config_dir(options: &GlobalOptions) -> Result<PathBuf, CliError> {
    AppPaths::from_process_env(options.config_dir.clone())
        .map(|paths| paths.config_dir().to_path_buf())
        .map_err(CliError::from_auth)
}

pub fn store_kind() -> Result<CredentialStoreKind, CliError> {
    match std::env::var(STORE_ENV) {
        Ok(value) => CredentialStoreKind::parse(&value).map_err(CliError::from_auth),
        Err(_) => Ok(CredentialStoreKind::platform_default()),
    }
}

fn import(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if args != ["--token-stdin"] {
        return Err(CliError::usage("usage: booxctl auth import --token-stdin"));
    }
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw).map_err(|err| {
        CliError::new(
            crate::error::CliErrorCode::LocalIo,
            format!("cannot read token from stdin: {err}"),
        )
    })?;
    let status = service(options)?
        .import_token(&raw)
        .map_err(CliError::from_auth)?;
    emit_status(options.json, &status)
}

fn status(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let verify = match args {
        [] => false,
        [flag] if flag == "--verify" => true,
        _ => return Err(CliError::usage("usage: booxctl auth status [--verify]")),
    };
    let status = service(options)?
        .status(verify)
        .map_err(CliError::from_auth)?;
    emit_status(options.json, &status)
}

fn logout(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !args.is_empty() {
        return Err(CliError::usage("usage: booxctl auth logout"));
    }
    let outcome = service(options)?.logout().map_err(CliError::from_auth)?;
    if options.json {
        emit_json(&outcome)?;
    } else if outcome.environment_token_active {
        println!("cleared={} environment_token_active=true", outcome.cleared);
        eprintln!("{TOKEN_ENV} is still active; the process-level token has not been revoked.");
    } else if outcome.cleared {
        println!("cleared=true");
    } else {
        println!("cleared=false");
    }
    Ok(())
}

fn login(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !(args.is_empty() || args == ["--qr"]) {
        return Err(CliError::usage("usage: booxctl auth login --qr"));
    }
    if options.json {
        return Err(CliError::usage(
            "auth login --qr is interactive and cannot be used with --json; use auth import --token-stdin",
        )
        .with_stable_code("interactive_terminal_required"));
    }
    if options.no_input || !io::stderr().is_terminal() {
        return Err(CliError::usage(
            "auth login --qr requires a terminal; use auth import --token-stdin in non-interactive runs",
        )
        .with_stable_code("interactive_terminal_required"));
    }
    let service = service(options)?;
    let status = service
        .login_with_qr(
            QrLoginOptions::default(),
            display_qr,
            crate::cancel::requested,
        )
        .map_err(CliError::from_auth)?;
    emit_status(false, &status)
}

fn display_qr(challenge: &QrLoginChallenge) -> Result<(), AuthError> {
    let qr = QrCode::encode_text(&challenge.qrcode_data, QrCodeEcc::Medium).map_err(|_| {
        AuthError::new(
            AuthErrorKind::Unsupported,
            "qr_render_failed",
            "无法在终端渲染二维码",
        )
    })?;
    eprintln!("Scan this QR code with the BOOX assistant app:\n");
    eprint!("{}", render_terminal_qr(&qr));
    eprintln!("QR payload: {}", challenge.qrcode_data);
    eprintln!("Waiting for confirmation. Expires automatically in 5 minutes.\n");
    Ok(())
}

fn render_terminal_qr(qr: &QrCode) -> String {
    const QUIET_ZONE: i32 = 2;
    let size = qr.size();
    let mut out = String::new();
    for y in -QUIET_ZONE..size + QUIET_ZONE {
        for x in -QUIET_ZONE..size + QUIET_ZONE {
            let dark = (0..size).contains(&x) && (0..size).contains(&y) && qr.get_module(x, y);
            out.push_str(if dark { "██" } else { "  " });
        }
        out.push('\n');
    }
    out
}

fn emit_status(json: bool, status: &AuthStatus) -> Result<(), CliError> {
    if json {
        emit_json(status)?;
    } else {
        println!("authorized={}", status.authorized);
        println!("source={}", source_text(status));
        if let Some(updated_ms) = status.updated_ms {
            println!("updated_ms={updated_ms}");
        }
    }
    Ok(())
}

fn source_text(status: &AuthStatus) -> &'static str {
    use boox_core::auth_service::AuthSource;
    match status.source {
        AuthSource::None => "none",
        AuthSource::Environment => "environment",
        AuthSource::File => "file",
        AuthSource::Keyring => "keyring",
    }
}

fn emit_json<T: Serialize>(value: &T) -> Result<(), CliError> {
    let data = serde_json::to_value(value).map_err(|_| {
        CliError::new(
            crate::error::CliErrorCode::Internal,
            "cannot serialize output",
        )
    })?;
    println!(
        "{}",
        serde_json::to_string(&crate::output::success_envelope(data)).map_err(
            |_| CliError::new(
                crate::error::CliErrorCode::Internal,
                "cannot serialize output"
            )
        )?
    );
    Ok(())
}

#[allow(dead_code)]
fn _assert_logout_serializes(outcome: &LogoutOutcome) -> String {
    serde_json::to_string(outcome).unwrap_or_default()
}

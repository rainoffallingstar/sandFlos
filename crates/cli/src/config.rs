use crate::{
    args::GlobalOptions,
    error::{CliError, CliErrorCode},
};
use std::{fs, path::PathBuf};
pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    let source = match args {
        [cmd, flag, path] if cmd == "migrate" && flag == "--from" => PathBuf::from(path),
        _ => {
            return Err(CliError::usage(
                "usage: sandflos config migrate --from PATH",
            ))
        }
    };
    let source = if source.is_dir() {
        source.join("auth_cache.json")
    } else {
        source
    };
    let service = crate::auth::service(options)?;
    if service.environment_token_active()
        || service
            .resolve_token()
            .map_err(CliError::from_auth)?
            .is_some()
    {
        return Err(CliError::usage(
            "target credentials already exist or environment token is active",
        ));
    }
    let bytes = fs::read(&source)
        .map_err(|_| CliError::new(CliErrorCode::LocalIo, "cannot read migration source"))?;
    let legacy: boox_core::PersistedAuthState = serde_json::from_slice(&bytes).map_err(|_| {
        CliError::new(CliErrorCode::LocalIo, "invalid legacy auth configuration")
            .with_stable_code("invalid_migration_source")
    })?;
    let token = legacy.token.ok_or_else(|| {
        CliError::usage("legacy configuration has no token")
            .with_stable_code("migration_source_missing_token")
    })?;
    let status = service.import_token(&token).map_err(CliError::from_auth)?;
    crate::output::emit_success(
        options.json,
        serde_json::json!({"migrated":true,"authorized":status.authorized,"source_preserved":true}),
    );
    Ok(())
}

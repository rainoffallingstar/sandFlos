use crate::args::GlobalOptions;
use crate::error::CliError;
use boox_core::cli_libraries::{calibre_directories, zotero_directory};
use serde_json::{json, Value};
use std::{
    env,
    path::{Path, MAIN_SEPARATOR},
};

pub fn run(options: &GlobalOptions, args: &[String]) -> Result<(), CliError> {
    if !args.is_empty() {
        return Err(CliError::usage("usage: sandflos diagnostics"));
    }
    let report = report(options);
    if options.json {
        crate::output::emit_success(true, report);
    } else {
        print_report(&report);
    }
    Ok(())
}

fn report(options: &GlobalOptions) -> Value {
    let credential_store = match crate::auth::store_kind() {
        Ok(kind) => serde_json::to_value(kind).unwrap_or(Value::Null),
        Err(error) => json!({
            "configured": env::var("SANDFLOS_CREDENTIAL_STORE").ok(),
            "error": error_body(&error),
        }),
    };
    let environment_token_active = env::var("SANDFLOS_TOKEN")
        .ok()
        .is_some_and(|value| !value.trim().is_empty());

    let config = match crate::auth::config_dir(options) {
        Ok(dir) => config_report(&dir),
        Err(error) => json!({ "error": error_body(&error) }),
    };

    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "os": env::consts::OS,
        "arch": env::consts::ARCH,
        "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
        "config_dir": config,
        "credential_store": credential_store,
        "auth": {
            "environment_token_active": environment_token_active,
        },
    })
}

fn config_report(dir: &Path) -> Value {
    let calibre = match calibre_directories(dir) {
        Ok(libraries) => json!({
            "configured": !libraries.is_empty(),
            "count": libraries.len(),
        }),
        Err(error) => json!({ "error": error.message }),
    };
    let zotero = match zotero_directory(dir) {
        Ok(path) => json!({
            "configured": path.is_some(),
            "path": path.as_deref().map(redact_path),
        }),
        Err(error) => json!({ "error": error.message }),
    };

    json!({
        "path": redact_path(dir),
        "exists": dir.exists(),
        "credential_file_present": dir.join("auth.json").exists(),
        "libraries_file_present": dir.join("libraries.json").exists(),
        "calibre": calibre,
        "zotero": zotero,
    })
}

fn error_body(error: &CliError) -> Value {
    json!({
        "code": error.stable_code.as_deref().unwrap_or_else(|| error.code.as_str()),
        "message": error.message,
    })
}

fn redact_path(path: &Path) -> String {
    let home = env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from);
    if let Some(home) = home {
        if let Ok(relative) = path.strip_prefix(&home) {
            if relative.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~{MAIN_SEPARATOR}{}", relative.display());
        }
    }
    path.to_string_lossy().to_string()
}

fn print_report(report: &Value) {
    let text = |key: &str| report[key].as_str().unwrap_or("-").to_string();
    println!("version={}", text("version"));
    println!("os={}", text("os"));
    println!("arch={}", text("arch"));
    println!("profile={}", text("profile"));

    let config = &report["config_dir"];
    if let Some(path) = config["path"].as_str() {
        println!("config_dir={path}");
        println!("config_dir_exists={}", config["exists"]);
        println!(
            "credential_file_present={}",
            config["credential_file_present"]
        );
        println!(
            "libraries_file_present={}",
            config["libraries_file_present"]
        );
        if let Some(configured) = config["calibre"]["configured"].as_bool() {
            println!("calibre_configured={configured}");
            println!("calibre_libraries={}", config["calibre"]["count"]);
        } else {
            println!("calibre_configured=unknown");
        }
        if let Some(configured) = config["zotero"]["configured"].as_bool() {
            println!("zotero_configured={configured}");
        } else {
            println!("zotero_configured=unknown");
        }
    } else {
        println!("config_dir=unavailable");
        if let Some(message) = config["error"]["message"].as_str() {
            println!("config_error={message}");
        }
    }

    let store = &report["credential_store"];
    if let Some(kind) = store.as_str() {
        println!("credential_store={kind}");
    } else {
        println!("credential_store=invalid");
        if let Some(message) = store["error"]["message"].as_str() {
            println!("credential_store_error={message}");
        }
    }
    println!(
        "environment_token_active={}",
        report["auth"]["environment_token_active"]
    );
}

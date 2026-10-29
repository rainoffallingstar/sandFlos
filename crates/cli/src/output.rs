use crate::error::CliError;
use serde::Serialize;
use serde_json::{json, Value};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct ErrorBody<'a> {
    pub code: &'a str,
    pub message: &'a str,
}

pub fn success_envelope(data: Value) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": true,
        "data": data,
        "error": Value::Null,
    })
}

pub fn error_envelope(error: &CliError) -> Value {
    json!({
        "schema_version": SCHEMA_VERSION,
        "ok": false,
        "data": error.data.clone().unwrap_or(Value::Null),
        "error": ErrorBody {
            code: error.stable_code.as_deref().unwrap_or_else(|| error.code.as_str()),
            message: error.message.as_str(),
        },
    })
}

pub fn emit_success(json: bool, data: Value) {
    if json {
        println!(
            "{}",
            serde_json::to_string(&success_envelope(data)).expect("serialize success envelope")
        );
    }
}

pub fn emit_error(json: bool, error: &CliError) {
    if json {
        println!(
            "{}",
            serde_json::to_string(&error_envelope(error)).expect("serialize error envelope")
        );
    } else {
        eprintln!("sandflos: {}", error.message);
        if error.code == crate::error::CliErrorCode::Usage {
            eprintln!();
            eprint!("{}", crate::help::USAGE);
        }
    }
}

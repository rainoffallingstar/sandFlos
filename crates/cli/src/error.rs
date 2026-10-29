use serde_json::Value;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliErrorCode {
    Usage,
    Internal,
    Auth,
    Network,
    LocalIo,
    #[allow(dead_code)]
    Partial,
    Cancelled,
}

impl CliErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Usage => "usage",
            Self::Internal => "internal",
            Self::Auth => "auth",
            Self::Network => "network",
            Self::LocalIo => "local_io",
            Self::Partial => "partial",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn exit_code(self) -> i32 {
        match self {
            Self::Usage => 2,
            Self::Internal => 1,
            Self::Auth => 3,
            Self::Network => 4,
            Self::LocalIo => 5,
            Self::Partial => 6,
            Self::Cancelled => 130,
        }
    }
}

#[derive(Debug)]
pub struct CliError {
    pub code: CliErrorCode,
    pub stable_code: Option<String>,
    pub message: String,
    pub data: Option<Value>,
}

impl CliError {
    pub fn new(code: CliErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            stable_code: None,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_stable_code(mut self, stable_code: &str) -> Self {
        self.stable_code = Some(stable_code.to_string());
        self
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn from_exit_code(exit_code: i32, message: impl Into<String>) -> Self {
        let code = match exit_code {
            2 => CliErrorCode::Usage,
            3 => CliErrorCode::Auth,
            4 => CliErrorCode::Network,
            5 => CliErrorCode::LocalIo,
            6 => CliErrorCode::Partial,
            130 => CliErrorCode::Cancelled,
            _ => CliErrorCode::Internal,
        };
        Self::new(code, message)
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(CliErrorCode::Usage, message)
    }

    pub fn from_service(error: boox_core::services::ServiceError) -> Self {
        use boox_core::services::ServiceErrorKind;
        let code = match error.kind {
            ServiceErrorKind::Input => CliErrorCode::Usage,
            ServiceErrorKind::Authentication => CliErrorCode::Auth,
            ServiceErrorKind::LocalIo => CliErrorCode::LocalIo,
            ServiceErrorKind::Network | ServiceErrorKind::Protocol => CliErrorCode::Network,
            ServiceErrorKind::NotFound => CliErrorCode::Usage,
            ServiceErrorKind::Cancelled => CliErrorCode::Cancelled,
            ServiceErrorKind::Partial | ServiceErrorKind::Unknown => CliErrorCode::Partial,
            ServiceErrorKind::Internal => CliErrorCode::Internal,
        };
        Self::new(code, error.message).with_stable_code(error.code)
    }

    pub fn from_auth(error: boox_core::auth_service::AuthError) -> Self {
        use boox_core::auth_service::AuthErrorKind;
        let code = match error.kind {
            AuthErrorKind::Input => CliErrorCode::Usage,
            AuthErrorKind::Authentication => CliErrorCode::Auth,
            AuthErrorKind::Storage => CliErrorCode::LocalIo,
            AuthErrorKind::Network | AuthErrorKind::Timeout => CliErrorCode::Network,
            AuthErrorKind::Cancelled => CliErrorCode::Cancelled,
            AuthErrorKind::Unsupported => CliErrorCode::LocalIo,
        };
        let stable_code = error.code.to_string();
        Self::new(code, error.message).with_stable_code(&stable_code)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

impl From<boox_core::services::ServiceError> for CliError {
    fn from(error: boox_core::services::ServiceError) -> Self {
        Self::from_service(error)
    }
}

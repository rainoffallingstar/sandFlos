use crate::models::{AuthState, DashboardSnapshot, PersistedAuthState, UploadRuntimeState};
use crate::util::{normalize_optional, unix_ms_now};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

const AUTH_CACHE_FILE: &str = "auth_cache.json";

#[derive(Debug, Clone)]
pub struct LoginPortalState {
    pub _state: String,
    pub _port: u16,
    pub _started_ms: u128,
}

#[derive(Default)]
pub struct RuntimeState {
    pub upload_in_progress: Mutex<bool>,
    pub auth_state: Mutex<AuthState>,
    pub upload_runtime_state: Mutex<UploadRuntimeState>,
    pub dashboard_cache: Mutex<Option<DashboardSnapshot>>,
    pub last_tray_anchor: Mutex<Option<(f64, f64, f64, f64)>>,
    pub login_portal: Mutex<Option<LoginPortalState>>,
}

static RUNTIME_STATE: OnceLock<Arc<RuntimeState>> = OnceLock::new();

pub fn global_state() -> Arc<RuntimeState> {
    RUNTIME_STATE
        .get_or_init(|| Arc::new(RuntimeState::default()))
        .clone()
}

pub fn app_data_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("com.fallingstar.send2boox");
    let _ = fs::create_dir_all(&dir);
    dir
}

fn auth_cache_path() -> Option<PathBuf> {
    let dir = app_data_dir();
    Some(dir.join(AUTH_CACHE_FILE))
}

pub fn hydrate_auth_state() {
    let state = global_state();
    state.hydrate_auth();
}

impl RuntimeState {
    pub fn hydrate_auth(&self) {
        let path = match auth_cache_path() {
            Some(path) => path,
            None => return,
        };
        let raw = match fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(_) => return,
        };
        let persisted: PersistedAuthState = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(_) => return,
        };
        if let Ok(mut state) = self.auth_state.lock() {
            state.token = normalize_optional(persisted.token);
            state.updated_ms = persisted.updated_ms;
        }
    }

    pub fn get_auth(&self) -> AuthState {
        match self.auth_state.lock() {
            Ok(state) => state.clone(),
            Err(_) => AuthState::default(),
        }
    }

    pub fn set_auth(&self, token: Option<String>) {
        let normalized = normalize_optional(token);
        let updated_ms = Some(unix_ms_now());
        if let Ok(mut state) = self.auth_state.lock() {
            state.token = normalized.clone();
            state.updated_ms = updated_ms;
        }

        if let Ok(mut cache) = self.dashboard_cache.lock() {
            *cache = None;
        }

        if let Some(path) = auth_cache_path() {
            if let Some(token_value) = normalized {
                let payload = PersistedAuthState {
                    token: Some(token_value),
                    updated_ms,
                };
                if let Ok(text) = serde_json::to_string(&payload) {
                    let _ = fs::write(path, text);
                }
            } else {
                let _ = fs::remove_file(path);
            }
        }
    }

    pub fn get_dashboard_cached(&self, max_age_ms: u128) -> Option<DashboardSnapshot> {
        let now = unix_ms_now();
        match self.dashboard_cache.lock() {
            Ok(cache) => cache.clone().filter(|snap| {
                let age = now.saturating_sub(snap.fetched_at_ms);
                age <= max_age_ms
            }),
            Err(_) => None,
        }
    }

    pub fn set_dashboard_cached(&self, snapshot: DashboardSnapshot) {
        if let Ok(mut cache) = self.dashboard_cache.lock() {
            *cache = Some(snapshot);
        }
    }

    pub fn update_dashboard_cached_after_delete(
        &self,
        deleted_id: &str,
    ) -> Option<DashboardSnapshot> {
        match self.dashboard_cache.lock() {
            Ok(mut cache) => {
                let snapshot = cache.as_mut()?;
                snapshot.push_queue.retain(|item| item.id != deleted_id);
                snapshot.fetched_at_ms = unix_ms_now();
                Some(snapshot.clone())
            }
            Err(_) => None,
        }
    }

    pub fn get_upload_state(&self) -> UploadRuntimeState {
        match self.upload_runtime_state.lock() {
            Ok(state) => state.clone(),
            Err(_) => UploadRuntimeState::default(),
        }
    }

    pub fn update_upload_state<F>(&self, mutator: F)
    where
        F: FnOnce(&mut UploadRuntimeState),
    {
        if let Ok(mut state) = self.upload_runtime_state.lock() {
            mutator(&mut state);
            state.updated_ms = unix_ms_now();
        }
    }

    pub fn try_begin_upload(&self) -> bool {
        match self.upload_in_progress.lock() {
            Ok(mut in_progress) => {
                if *in_progress {
                    return false;
                }
                *in_progress = true;
                self.update_upload_state(|state| {
                    state.in_progress = true;
                    state.last_error = None;
                    state.current_file = None;
                    state.bytes_sent = None;
                    state.bytes_total = None;
                    state.progress_percent = None;
                    state.speed_bps = None;
                    state.eta_seconds = None;
                });
                true
            }
            Err(_) => false,
        }
    }

    pub fn finish_upload(&self) {
        if let Ok(mut in_progress) = self.upload_in_progress.lock() {
            *in_progress = false;
        }
        self.update_upload_state(|state| {
            state.in_progress = false;
            state.speed_bps = None;
            state.eta_seconds = None;
        });
    }
}

pub fn get_auth_state() -> AuthState {
    global_state().get_auth()
}

pub fn set_auth_state(token: Option<String>) {
    global_state().set_auth(token);
}

pub fn get_dashboard_cache(max_age_ms: u128) -> Option<DashboardSnapshot> {
    global_state().get_dashboard_cached(max_age_ms)
}

pub fn set_dashboard_cache(snapshot: DashboardSnapshot) {
    global_state().set_dashboard_cached(snapshot);
}

pub fn update_dashboard_cache_after_delete(deleted_id: &str) -> Option<DashboardSnapshot> {
    global_state().update_dashboard_cached_after_delete(deleted_id)
}

pub fn get_upload_runtime_state() -> UploadRuntimeState {
    global_state().get_upload_state()
}

pub fn update_upload_runtime_state<F>(mutator: F)
where
    F: FnOnce(&mut UploadRuntimeState),
{
    global_state().update_upload_state(mutator);
}

pub fn try_begin_upload_task() -> bool {
    global_state().try_begin_upload()
}

pub fn finish_upload_task() {
    global_state().finish_upload();
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct AppStatus {
    pub app_name: String,
    pub version: String,
    pub architecture: String,
    pub target_os: String,
    pub local_first_mode: bool,
    pub phase: String,
}

#[tauri::command]
fn ping(message: String) -> Result<String, String> {
    Ok(format!("Pong from Rust backend: received '{}'", message))
}

#[tauri::command]
fn get_app_status() -> Result<AppStatus, String> {
    Ok(AppStatus {
        app_name: "Instagram Chat History Viewer".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        architecture: std::env::consts::ARCH.to_string(),
        target_os: std::env::consts::OS.to_string(),
        local_first_mode: true,
        phase: "Phase 1: Architecture Scaffolding".to_string(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![ping, get_app_status])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ping_command() {
        let response = ping("test payload".to_string());
        assert!(response.is_ok());
        assert_eq!(
            response.unwrap(),
            "Pong from Rust backend: received 'test payload'"
        );
    }

    #[test]
    fn test_app_status_command() {
        let status = get_app_status();
        assert!(status.is_ok());
        let status = status.unwrap();
        assert_eq!(status.app_name, "Instagram Chat History Viewer");
        assert!(status.local_first_mode);
        assert_eq!(status.phase, "Phase 1: Architecture Scaffolding");
    }
}

// Windows Platform Integration: Registry Startup & Session-Local Single Instance Mutex
// Governed by TRD.md Section 7, PRD.md Section FR-SYNC-009, and GEMINI.md Section 6.3

use std::path::Path;

pub const REGISTRY_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub const APP_REGISTRY_NAME: &str = "InstagramChatHistoryViewer";
pub const SINGLE_INSTANCE_MUTEX_NAME: &str = r"Local\InstagramChatHistoryViewer_InstanceMutex";

/// Formats an executable path into a safe Windows startup command.
/// Always encloses the executable path in double quotes to handle spaces correctly.
pub fn format_startup_command(exe_path: &Path) -> String {
    format!("\"{}\" --autostart", exe_path.to_string_lossy())
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    type HKEY = usize;
    type HANDLE = *mut std::ffi::c_void;
    type DWORD = u32;
    type LONG = i32;

    const HKEY_CURRENT_USER: HKEY = 0x80000001;
    const KEY_READ: DWORD = 0x20019;
    const KEY_SET_VALUE: DWORD = 0x0002;
    const REG_SZ: DWORD = 1;
    const ERROR_SUCCESS: LONG = 0;
    const ERROR_FILE_NOT_FOUND: LONG = 2;
    const ERROR_ALREADY_EXISTS: DWORD = 183;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegOpenKeyExW(
            hKey: HKEY,
            lpSubKey: *const u16,
            ulOptions: DWORD,
            samDesired: DWORD,
            phkResult: *mut HKEY,
        ) -> LONG;

        fn RegQueryValueExW(
            hKey: HKEY,
            lpValueName: *const u16,
            lpReserved: *mut DWORD,
            lpType: *mut DWORD,
            lpData: *mut u8,
            lpcbData: *mut DWORD,
        ) -> LONG;

        fn RegSetValueExW(
            hKey: HKEY,
            lpValueName: *const u16,
            Reserved: DWORD,
            dwType: DWORD,
            lpData: *const u8,
            cbData: DWORD,
        ) -> LONG;

        fn RegDeleteValueW(hKey: HKEY, lpValueName: *const u16) -> LONG;

        fn RegCloseKey(hKey: HKEY) -> LONG;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(
            lpMutexAttributes: *mut std::ffi::c_void,
            bInitialOwner: i32,
            lpName: *const u16,
        ) -> HANDLE;

        fn CloseHandle(hObject: HANDLE) -> i32;

        fn GetLastError() -> DWORD;
    }

    fn to_wide_null(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn is_autostart_registered() -> Result<bool, String> {
        let subkey = to_wide_null(REGISTRY_RUN_KEY);
        let val_name = to_wide_null(APP_REGISTRY_NAME);

        unsafe {
            let mut hkey: HKEY = 0;
            let status = RegOpenKeyExW(HKEY_CURRENT_USER, subkey.as_ptr(), 0, KEY_READ, &mut hkey);
            if status != ERROR_SUCCESS {
                return Ok(false);
            }

            let mut data_type: DWORD = 0;
            let mut data_len: DWORD = 0;
            let query_status = RegQueryValueExW(
                hkey,
                val_name.as_ptr(),
                std::ptr::null_mut(),
                &mut data_type,
                std::ptr::null_mut(),
                &mut data_len,
            );

            RegCloseKey(hkey);

            if query_status == ERROR_SUCCESS {
                Ok(true)
            } else if query_status == ERROR_FILE_NOT_FOUND {
                Ok(false)
            } else {
                Err(format!(
                    "RegQueryValueExW failed with code {}",
                    query_status
                ))
            }
        }
    }

    pub fn register_autostart(exe_path: &Path) -> Result<(), String> {
        let subkey = to_wide_null(REGISTRY_RUN_KEY);
        let val_name = to_wide_null(APP_REGISTRY_NAME);
        let cmd = format_startup_command(exe_path);
        let wide_cmd = to_wide_null(&cmd);
        let byte_len = (wide_cmd.len() * std::mem::size_of::<u16>()) as DWORD;

        unsafe {
            let mut hkey: HKEY = 0;
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if status != ERROR_SUCCESS {
                return Err(format!(
                    "Failed to open HKCU Run key for write (code {})",
                    status
                ));
            }

            let set_status = RegSetValueExW(
                hkey,
                val_name.as_ptr(),
                0,
                REG_SZ,
                wide_cmd.as_ptr() as *const u8,
                byte_len,
            );

            RegCloseKey(hkey);

            if set_status == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(format!(
                    "Failed to write HKCU Run value (code {})",
                    set_status
                ))
            }
        }
    }

    pub fn unregister_autostart() -> Result<(), String> {
        let subkey = to_wide_null(REGISTRY_RUN_KEY);
        let val_name = to_wide_null(APP_REGISTRY_NAME);

        unsafe {
            let mut hkey: HKEY = 0;
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                subkey.as_ptr(),
                0,
                KEY_SET_VALUE,
                &mut hkey,
            );
            if status != ERROR_SUCCESS {
                return Ok(());
            }

            let del_status = RegDeleteValueW(hkey, val_name.as_ptr());
            RegCloseKey(hkey);

            if del_status == ERROR_SUCCESS || del_status == ERROR_FILE_NOT_FOUND {
                Ok(())
            } else {
                Err(format!(
                    "Failed to delete HKCU Run value (code {})",
                    del_status
                ))
            }
        }
    }

    pub struct SingleInstanceGuard {
        handle: HANDLE,
    }

    impl Drop for SingleInstanceGuard {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe {
                    CloseHandle(self.handle);
                }
            }
        }
    }

    // Safety: HANDLE can be sent across threads if wrapped
    unsafe impl Send for SingleInstanceGuard {}
    unsafe impl Sync for SingleInstanceGuard {}

    pub fn acquire_single_instance(name: &str) -> Result<Option<SingleInstanceGuard>, String> {
        let wide_name = to_wide_null(name);
        unsafe {
            let handle = CreateMutexW(std::ptr::null_mut(), 1, wide_name.as_ptr());
            if handle.is_null() {
                return Err("Failed to create session mutex".to_string());
            }

            let last_error = GetLastError();
            if last_error == ERROR_ALREADY_EXISTS {
                CloseHandle(handle);
                return Ok(None);
            }

            Ok(Some(SingleInstanceGuard { handle }))
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub struct SingleInstanceGuard;

    pub fn is_autostart_registered() -> Result<bool, String> {
        Ok(false)
    }

    pub fn register_autostart(_exe_path: &Path) -> Result<(), String> {
        Ok(())
    }

    pub fn unregister_autostart() -> Result<(), String> {
        Ok(())
    }

    pub fn acquire_single_instance(_name: &str) -> Result<Option<SingleInstanceGuard>, String> {
        Ok(Some(SingleInstanceGuard))
    }
}

pub use platform::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_startup_command_quotes_spaces() {
        let p = Path::new(r"C:\Program Files\Instagram Chat\viewer.exe");
        let cmd = format_startup_command(p);
        assert_eq!(
            cmd,
            r#""C:\Program Files\Instagram Chat\viewer.exe" --autostart"#
        );
    }

    #[test]
    fn test_format_startup_command_simple_path() {
        let p = Path::new(r"C:\Tools\app.exe");
        let cmd = format_startup_command(p);
        assert_eq!(cmd, r#""C:\Tools\app.exe" --autostart"#);
    }

    #[test]
    #[cfg(windows)]
    fn test_single_instance_mutex_prevents_duplicate() {
        let test_name = format!(r"Local\TestMutex_{}", uuid::Uuid::new_v4());
        let guard1 = acquire_single_instance(&test_name).expect("Failed to acquire first mutex");
        assert!(guard1.is_some(), "First acquisition should succeed");

        // Second acquisition with same name in same session should return None
        let guard2 = acquire_single_instance(&test_name).expect("Failed to check second mutex");
        assert!(
            guard2.is_none(),
            "Second acquisition must detect existing instance"
        );

        // Release first guard
        drop(guard1);

        // Third acquisition should now succeed
        let guard3 = acquire_single_instance(&test_name).expect("Failed to re-acquire mutex");
        assert!(guard3.is_some(), "Acquisition after drop should succeed");
    }

    #[test]
    #[cfg(windows)]
    fn test_autostart_registry_roundtrip() {
        // Test registering a dummy test command and then unregistering
        let dummy_exe = Path::new(r"C:\TestPath With Spaces\dummy.exe");
        let reg_res = register_autostart(dummy_exe);
        assert!(
            reg_res.is_ok(),
            "Writing to HKCU Run must succeed without elevation: {:?}",
            reg_res
        );

        let is_reg = is_autostart_registered().expect("Reading HKCU Run must succeed");
        assert!(is_reg, "Registry entry must report true when registered");

        let unreg_res = unregister_autostart();
        assert!(unreg_res.is_ok(), "Deleting from HKCU Run must succeed");

        let is_reg_after = is_autostart_registered().expect("Reading HKCU Run must succeed");
        assert!(
            !is_reg_after,
            "Registry entry must report false after unregister"
        );
    }
}

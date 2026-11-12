use serde::{Deserialize, Serialize};
#[cfg(windows)]
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForegroundSample {
    pub app: String,
    pub name: String,
    pub title: String,
    pub pid: u32,
    pub idle_ms: i64,
    pub site: String,
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::{
        ffi::c_void,
        mem::size_of,
        path::{Path, PathBuf},
    };
    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HWND},
            Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
            System::{
                Com::{
                    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
                    CoInitializeEx, CoUninitialize,
                },
                StationsAndDesktops::{
                    CloseDesktop, DESKTOP_READOBJECTS, GetUserObjectInformationW, OpenInputDesktop,
                    UOI_NAME,
                },
                SystemInformation::GetTickCount,
                Threading::{
                    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                    QueryFullProcessImageNameW,
                },
                Variant::VARIANT,
            },
            UI::{
                Accessibility::{
                    CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
                    UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
                },
                Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO},
                WindowsAndMessaging::{
                    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
                },
            },
        },
        core::{PCWSTR, PWSTR, w},
    };

    #[derive(Clone)]
    struct ProcessInfo {
        label: String,
        name: String,
    }

    pub struct NativeSampler {
        process_cache: HashMap<u32, ProcessInfo>,
        automation: Option<IUIAutomation>,
        com_initialized: bool,
    }

    impl NativeSampler {
        pub fn new() -> Self {
            let com_initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }.is_ok();
            let automation = if com_initialized {
                unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() }
            } else {
                None
            };
            Self {
                process_cache: HashMap::new(),
                automation,
                com_initialized,
            }
        }

        pub fn sample(&mut self, capture_sites: bool) -> ForegroundSample {
            let idle_ms = idle_ms();
            unsafe {
                let hwnd = GetForegroundWindow();
                if hwnd.is_invalid() {
                    return ForegroundSample {
                        idle_ms,
                        ..Default::default()
                    };
                }
                let mut title_buffer = [0_u16; 1024];
                let length = GetWindowTextW(hwnd, &mut title_buffer).max(0) as usize;
                let title = String::from_utf16_lossy(&title_buffer[..length.min(1024)]);
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let process = self.process_info(pid);
                let site = if capture_sites {
                    self.browser_site(&process.name, hwnd)
                } else {
                    String::new()
                };
                ForegroundSample {
                    app: process.label,
                    name: process.name,
                    title,
                    pid,
                    idle_ms,
                    site,
                }
            }
        }

        fn process_info(&mut self, pid: u32) -> ProcessInfo {
            if let Some(info) = self.process_cache.get(&pid) {
                return info.clone();
            }
            let path = process_path(pid).unwrap_or_default();
            let name = path
                .file_stem()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let label = file_description(&path)
                .filter(|label| !label.is_empty())
                .unwrap_or_else(|| name.clone());
            let info = ProcessInfo { label, name };
            if self.process_cache.len() > 400 {
                self.process_cache.clear();
            }
            self.process_cache.insert(pid, info.clone());
            info
        }

        fn browser_site(&self, process_name: &str, hwnd: HWND) -> String {
            const BROWSERS: [&str; 6] =
                ["chrome", "msedge", "brave", "opera", "vivaldi", "firefox"];
            if !BROWSERS.contains(&process_name.to_lowercase().as_str()) {
                return String::new();
            }
            let Some(automation) = &self.automation else {
                return String::new();
            };
            unsafe {
                let Ok(root) = automation.ElementFromHandle(hwnd) else {
                    return String::new();
                };
                let condition_value = VARIANT::from(UIA_EditControlTypeId.0);
                let Ok(condition) =
                    automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &condition_value)
                else {
                    return String::new();
                };
                let Ok(edits) = root.FindAll(TreeScope_Descendants, &condition) else {
                    return String::new();
                };
                let count = edits.Length().unwrap_or(0).min(100);
                for index in 0..count {
                    let Ok(element) = edits.GetElement(index) else {
                        continue;
                    };
                    let name = element
                        .CurrentName()
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let automation_id = element
                        .CurrentAutomationId()
                        .map(|value| value.to_string())
                        .unwrap_or_default();
                    let label = format!("{name} {automation_id}").to_lowercase();
                    if !["address", "omnibox", "search bar", "地址", "搜索栏", "网址"]
                        .iter()
                        .any(|part| label.contains(part))
                    {
                        continue;
                    }
                    let Ok(pattern) = element
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    else {
                        continue;
                    };
                    if let Ok(value) = pattern.CurrentValue() {
                        let site = crate::recorder::normalize_site_host(&value.to_string());
                        if !site.is_empty() {
                            return site;
                        }
                    }
                }
            }
            String::new()
        }
    }

    impl Drop for NativeSampler {
        fn drop(&mut self) {
            self.automation.take();
            if self.com_initialized {
                unsafe { CoUninitialize() };
            }
        }
    }

    fn idle_ms() -> i64 {
        unsafe {
            let mut info = LASTINPUTINFO {
                cbSize: size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            if !GetLastInputInfo(&mut info).as_bool() {
                return -1;
            }
            GetTickCount().wrapping_sub(info.dwTime).min(3_600_000) as i64
        }
    }

    fn process_path(pid: u32) -> Option<PathBuf> {
        if pid == 0 {
            return None;
        }
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buffer = [0_u16; 32_768];
            let mut length = buffer.len() as u32;
            let result = QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            );
            let _ = CloseHandle(process);
            result.ok()?;
            Some(PathBuf::from(String::from_utf16_lossy(
                &buffer[..length as usize],
            )))
        }
    }

    fn file_description(path: &Path) -> Option<String> {
        if path.as_os_str().is_empty() {
            return None;
        }
        let path_wide = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        unsafe {
            let size = GetFileVersionInfoSizeW(PCWSTR(path_wide.as_ptr()), None);
            if size == 0 {
                return None;
            }
            let mut data = vec![0_u8; size as usize];
            GetFileVersionInfoW(
                PCWSTR(path_wide.as_ptr()),
                None,
                size,
                data.as_mut_ptr().cast::<c_void>(),
            )
            .ok()?;

            let mut translations = Vec::new();
            let mut value = std::ptr::null_mut::<c_void>();
            let mut length = 0_u32;
            if VerQueryValueW(
                data.as_ptr().cast(),
                w!(r"\VarFileInfo\Translation"),
                &mut value,
                &mut length,
            )
            .as_bool()
                && !value.is_null()
            {
                let count = (length as usize / 4).min(16);
                let values = std::slice::from_raw_parts(value.cast::<u32>(), count);
                translations.extend(
                    values
                        .iter()
                        .map(|value| (value & 0xffff, (value >> 16) & 0xffff)),
                );
            }
            translations.extend([(0x0409, 0x04b0), (0x0804, 0x04b0)]);
            for (language, codepage) in translations {
                let query =
                    format!("\\StringFileInfo\\{language:04x}{codepage:04x}\\FileDescription\0")
                        .encode_utf16()
                        .collect::<Vec<_>>();
                let mut value = std::ptr::null_mut::<c_void>();
                let mut length = 0_u32;
                if VerQueryValueW(
                    data.as_ptr().cast(),
                    PCWSTR(query.as_ptr()),
                    &mut value,
                    &mut length,
                )
                .as_bool()
                    && !value.is_null()
                    && length > 0
                {
                    let text = std::slice::from_raw_parts(value.cast::<u16>(), length as usize);
                    let end = text.iter().position(|ch| *ch == 0).unwrap_or(text.len());
                    let description = String::from_utf16_lossy(&text[..end]).trim().to_owned();
                    if !description.is_empty() {
                        return Some(description);
                    }
                }
            }
        }
        None
    }

    pub fn workstation_is_locked() -> bool {
        unsafe {
            let Ok(desktop) = OpenInputDesktop(Default::default(), false, DESKTOP_READOBJECTS)
            else {
                return false;
            };
            let mut name = [0_u16; 256];
            let mut needed = 0_u32;
            let result = GetUserObjectInformationW(
                HANDLE(desktop.0),
                UOI_NAME,
                Some(name.as_mut_ptr().cast()),
                (name.len() * 2) as u32,
                Some(&mut needed),
            );
            let _ = CloseDesktop(desktop);
            if result.is_err() {
                return false;
            }
            let length = name.iter().position(|ch| *ch == 0).unwrap_or(name.len());
            String::from_utf16_lossy(&name[..length]).eq_ignore_ascii_case("winlogon")
        }
    }

    use std::os::windows::ffi::OsStrExt;

    pub type Sampler = NativeSampler;
}

#[cfg(windows)]
pub use imp::{Sampler, workstation_is_locked};

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub struct NativeSampler;
    impl NativeSampler {
        pub fn new() -> Self {
            Self
        }
        pub fn sample(&mut self, _capture_sites: bool) -> ForegroundSample {
            ForegroundSample::default()
        }
    }
    pub type Sampler = NativeSampler;
    pub fn workstation_is_locked() -> bool {
        false
    }
}

#[cfg(not(windows))]
pub use imp::{Sampler, workstation_is_locked};

#[cfg(windows)]
pub fn set_window_icon(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{
            CreateIconFromResourceEx, ICON_BIG, ICON_SMALL, LR_DEFAULTSIZE, SendMessageW,
            WM_SETICON,
        },
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_ref() else {
        return;
    };
    let hwnd = HWND(handle.hwnd.get() as *mut std::ffi::c_void);
    let icon = unsafe {
        CreateIconFromResourceEx(
            include_bytes!("../assets/icon.ico"),
            true,
            0x0003_0000,
            0,
            0,
            LR_DEFAULTSIZE,
        )
    };
    if let Ok(icon) = icon {
        unsafe {
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_BIG as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
            SendMessageW(
                hwnd,
                WM_SETICON,
                Some(WPARAM(ICON_SMALL as usize)),
                Some(LPARAM(icon.0 as isize)),
            );
        }
    }
}

#[cfg(windows)]
pub fn set_window_visible(window: &gpui::Window, visible: bool) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{SW_HIDE, SW_RESTORE, SetForegroundWindow, ShowWindow},
    };
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    if let RawWindowHandle::Win32(handle) = handle.as_ref() {
        let hwnd = HWND(handle.hwnd.get() as *mut std::ffi::c_void);
        unsafe {
            let _ = ShowWindow(hwnd, if visible { SW_RESTORE } else { SW_HIDE });
            if visible {
                let _ = SetForegroundWindow(hwnd);
            }
        }
    }
}

#[cfg(windows)]
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE},
    };
    let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
        r"Software\Microsoft\Windows\CurrentVersion\Run",
        KEY_QUERY_VALUE | KEY_SET_VALUE,
    )?;
    if enabled {
        let executable = std::env::current_exe()?;
        let command = format!("\"{}\"", executable.to_string_lossy());
        run.set_value("TraceBit", &command)?;
    } else {
        match run.delete_value("TraceBit") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(windows)]
pub fn show_notification(title: &str, body: &str) {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    let app_id = "TraceBit.Desktop";
    if let Ok((key, _)) = RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(format!(r"Software\Classes\AppUserModelId\{app_id}"))
    {
        let _ = key.set_value("DisplayName", &"TraceBit");
        let _ = key.set_value("IconBackgroundColor", &"0");
    }
    if let Err(error) = tauri_winrt_notification::Toast::new(app_id)
        .title(title)
        .text1(body)
        .show()
    {
        eprintln!("[Notification] {error}");
    }
}

#[cfg(windows)]
pub fn single_instance() -> anyhow::Result<windows::Win32::Foundation::HANDLE> {
    use windows::{Win32::System::Threading::CreateMutexW, core::w};
    let handle = unsafe { CreateMutexW(None, true, w!("Local\\TraceBit.Desktop.SingleInstance"))? };
    if unsafe { windows::Win32::Foundation::GetLastError() }
        == windows::Win32::Foundation::ERROR_ALREADY_EXISTS
    {
        unsafe { windows::Win32::Foundation::CloseHandle(handle)? };
        anyhow::bail!("TraceBit 已经在运行");
    }
    Ok(handle)
}

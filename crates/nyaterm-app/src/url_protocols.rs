//! MSI owns files; protocol cleanup checks the current handler before removing it.
//! These early-exit entry points never initialize the app or open user data.
use std::path::Path;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, KEY_WRITE, REG_OPTION_NON_VOLATILE,
    REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW,
    RegSetValueExW,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn create(root: HKEY, path: &str) -> std::io::Result<Key> {
    let mut key = std::ptr::null_mut();
    let status = unsafe {
        RegCreateKeyExW(
            root,
            wide(path).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE | KEY_WOW64_64KEY,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    Ok(Key(key))
}
fn set(root: HKEY, path: &str, name: &str, value: &str) -> std::io::Result<()> {
    let key = create(root, path)?;
    let value = wide(value);
    let status = unsafe {
        RegSetValueExW(
            key.0,
            wide(name).as_ptr(),
            0,
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}
fn get(root: HKEY, path: &str) -> std::io::Result<Option<String>> {
    let mut bytes = 0;
    let status = unsafe {
        RegGetValueW(
            root,
            wide(path).as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    let mut value = vec![0u16; bytes as usize / 2 + 1];
    let status = unsafe {
        RegGetValueW(
            root,
            wide(path).as_ptr(),
            std::ptr::null(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    let length = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    Ok(Some(String::from_utf16_lossy(&value[..length])))
}
fn command(executable: &Path) -> String {
    format!("\"{}\" \"%1\"", executable.display())
}
fn register(root: HKEY, scheme: &str, executable: &Path) -> std::io::Result<()> {
    set(root, scheme, "", "URL:NyaTerm Protocol")?;
    set(root, scheme, "URL Protocol", "")?;
    set(
        root,
        &format!("{scheme}\\DefaultIcon"),
        "",
        &format!("{},0", executable.display()),
    )?;
    set(
        root,
        &format!("{scheme}\\shell\\open\\command"),
        "",
        &command(executable),
    )
}
fn unregister(root: HKEY, scheme: &str, executable: &Path) -> std::io::Result<()> {
    if get(root, &format!("{scheme}\\shell\\open\\command"))?.as_deref()
        != Some(command(executable).as_str())
    {
        return Ok(());
    }
    let status = unsafe { RegDeleteTreeW(root, wide(scheme).as_ptr()) };
    if status != ERROR_SUCCESS && status != ERROR_FILE_NOT_FOUND {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    Ok(())
}
pub(crate) fn run_if_requested() -> Option<anyhow::Result<()>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 1 {
        return None;
    }
    let install = if args[0] == "--register-url-protocols" {
        true
    } else if args[0] == "--unregister-url-protocols" {
        false
    } else {
        return None;
    };
    Some((|| {
        let executable = std::env::current_exe()?;
        let root = create(HKEY_LOCAL_MACHINE, r"Software\Classes")?;
        let schemes: &[&str] = if nyaterm_core::app_identity::AppFlavor::current()
            == nyaterm_core::app_identity::AppFlavor::Stable
        {
            &["nyaterm", "ssh", "telnet"]
        } else {
            &["nyaterm-preview"]
        };
        for scheme in schemes {
            if install {
                register(root.0, scheme, &executable)?;
            } else {
                unregister(root.0, scheme, &executable)?;
            }
        }
        Ok(())
    })())
}

#[cfg(test)]
mod tests {
    use super::{command, create, get, register, set, unregister};
    use std::path::Path;
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
    #[test]
    fn uninstall_removes_only_handlers_still_owned_by_this_executable() {
        let path = format!("Software\\NyaTermProtocolTests\\{}", uuid::Uuid::new_v4());
        {
            let root = create(HKEY_CURRENT_USER, &path).unwrap();
            let executable = Path::new(r"C:\Program Files\NyaTerm\NyaTerm.exe");
            register(root.0, "ssh", executable).unwrap();
            assert_eq!(
                get(root.0, r"ssh\shell\open\command").unwrap().as_deref(),
                Some(command(executable).as_str())
            );
            set(root.0, r"ssh\shell\open\command", "", "another-handler").unwrap();
            unregister(root.0, "ssh", executable).unwrap();
            assert_eq!(
                get(root.0, r"ssh\shell\open\command").unwrap().as_deref(),
                Some("another-handler")
            );
            register(root.0, "telnet", executable).unwrap();
            unregister(root.0, "telnet", executable).unwrap();
            assert!(get(root.0, r"telnet\shell\open\command").unwrap().is_none());
        }
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, super::wide(&path).as_ptr());
        }
    }
}

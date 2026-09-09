//! What the system says about NGX, without creating a device.
//!
//! NGX Core ships with the NVIDIA driver ("during an advanced driver
//! installation the module is called NGX Core", NVIDIA's NGX Programming
//! Guide) and registers itself at
//! `HKLM\SOFTWARE\NVIDIA Corporation\Global\NGXCore` with `Installed` and
//! `FullPath`. When it is absent, `NVSDK_NGX_D3D12_Init` answers
//! `0xBAD00001` (FeatureNotSupported) on hardware that is otherwise fine —
//! the failure two reporters hit on an RTX 4070 and an RTX 5080.
use crate::lang;

/// `(installed, full path)` from the NGX Core registry key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NgxCore {
    pub installed: bool,
    pub path: String,
}

#[cfg(windows)]
pub fn ngx_core() -> Option<NgxCore> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let sub = wide(r"SOFTWARE\NVIDIA Corporation\Global\NGXCore");
    let mut buf = [0u16; 512];
    let mut size: u32 = (buf.len() * 2) as u32;
    let name = wide("FullPath");
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    if rc != 0 {
        return None;
    }
    let n = (size as usize / 2).saturating_sub(1).min(buf.len());
    let path = String::from_utf16_lossy(&buf[..n])
        .trim_end_matches('\0')
        .to_owned();
    let mut flag: u32 = 0;
    let mut flag_size: u32 = 4;
    let name = wide("Installed");
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut flag as *mut u32 as *mut _,
            &mut flag_size,
        )
    };
    Some(NgxCore {
        installed: rc == 0 && flag == 1,
        path,
    })
}

#[cfg(not(windows))]
pub fn ngx_core() -> Option<NgxCore> {
    None
}

/// NGX Core registered, its folder present, and the driver at or past the
/// version the neural runtime needs (616.56, the minimum OptiScaler's fork
/// documents for DLSS-NR). When this is true, an NGX init failure is not the
/// system's NGX being absent.
pub fn healthy() -> bool {
    let core_ok = ngx_core().is_some_and(|c| c.installed && std::path::Path::new(&c.path).is_dir());
    let driver_ok = crate::gpu::nvidia_driver()
        .as_deref()
        .and_then(version_key)
        .is_some_and(|v| v >= (616, 56));
    core_ok && driver_ok
}

fn version_key(v: &str) -> Option<(u32, u32)> {
    let (a, b) = v.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// One line for a report: driver number, NGX Core state and where it points.
pub fn describe() -> String {
    let driver = match crate::gpu::nvidia_driver() {
        Some(v) => crate::trfmt!("NVIDIA driver {v}; ", "NVIDIA 驱动 {v}；"),
        None => String::new(),
    };
    format!("{driver}{}", describe_core())
}

fn describe_core() -> String {
    match ngx_core() {
        Some(c) if c.installed && std::path::Path::new(&c.path).is_dir() => {
            crate::trfmt!("NGX Core installed ({})", "NGX Core 已安装（{}）", c.path)
        }
        Some(c) if c.installed => crate::trfmt!("NGX Core registered but its folder is missing: {} — reinstall the NVIDIA driver \
             (Custom install, keep every component)", "NGX Core 已注册但其文件夹缺失：{} —— 请重新安装 NVIDIA 驱动（自定义安装，保留所有组件）",
            c.path
        ),
        Some(_) => lang::tr("NGX Core is registered as NOT installed — reinstall the NVIDIA driver \
             (Custom install, keep every component; NVCleanstall and \"minimal\" installs drop it)", "NGX Core 注册为未安装 —— 请重新安装 NVIDIA 驱动（自定义安装，保留所有组件；NVCleanstall 和「最小」安装会丢弃它）")
            .to_owned(),
        None => lang::tr("NGX Core is not registered on this system (no HKLM\\SOFTWARE\\NVIDIA \
             Corporation\\Global\\NGXCore) — the NVIDIA driver was installed without it, so no \
             DLSS-based tool can start. Reinstall the driver with a Custom install and keep \
             every component.", "此系统未注册 NGX Core（没有 HKLM\\SOFTWARE\\NVIDIA Corporation\\Global\\NGXCore）—— NVIDIA 驱动在安装时没有包含它，因此任何基于 DLSS 的工具都无法启动。请用自定义安装重装驱动并保留所有组件。")
            .to_owned(),
    }
}

// ── which neural model is installed ────────────────────────────────

/// The DLSS 5 model's own version string, e.g. `310.8.0.0` for NVIDIA's build
/// and `310.8.SF.0` for ShortFuse's repack. Read from the file's version
/// resource, which is the only field that separates them: both carry the same
/// `OriginalFilename` (`CL 38718415`), `ProductName` and `CompanyName`, and
/// differ by 10 KB in size. Verified against both builds from rhi-repo.
#[cfg(windows)]
pub fn file_version(path: &std::path::Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };
    let w: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    use std::os::windows::ffi::OsStrExt;
    let size = unsafe { GetFileVersionInfoSizeW(w.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return None;
    }
    let mut buf = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(w.as_ptr(), 0, size, buf.as_mut_ptr() as *mut _) } == 0 {
        return None;
    }
    // The language-neutral block first, then the two codepages NVIDIA ships.
    for sub in [
        r"\StringFileInfo\040904B0\FileVersion",
        r"\StringFileInfo\000004B0\FileVersion",
        r"\StringFileInfo\040904E4\FileVersion",
    ] {
        let q: Vec<u16> = sub.encode_utf16().chain(std::iter::once(0)).collect();
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        if unsafe { VerQueryValueW(buf.as_ptr() as *const _, q.as_ptr(), &mut p, &mut len) } != 0
            && len > 0
        {
            let s = unsafe { std::slice::from_raw_parts(p as *const u16, len as usize) };
            let v = String::from_utf16_lossy(s)
                .trim_end_matches('\0')
                .trim()
                .to_owned();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

#[cfg(not(windows))]
pub fn file_version(_path: &std::path::Path) -> Option<String> {
    None
}

/// How that version string reads to a user: ShortFuse's multi-generation build
/// or NVIDIA's original.
pub fn model_build(version: &str) -> &'static str {
    if version.to_ascii_uppercase().contains(".SF") {
        lang::tr("ShortFuse .SF build (adds Ada/Turing paths)", "ShortFuse .SF 版本（新增 Ada/Turing 路径）")
    } else {
        lang::tr("NVIDIA original build", "NVIDIA 原版")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two builds of the model differ only here: NVIDIA's reads
    /// `310,8,0,0`, ShortFuse's `310.8.SF.0`. Every other version field is
    /// identical, including `OriginalFilename` (`CL 38718415`).
    #[test]
    fn model_build_names_the_sf_repack() {
        assert_eq!(
            super::model_build("310.8.SF.0"),
            lang::tr("ShortFuse .SF build (adds Ada/Turing paths)", "ShortFuse .SF 版本（新增 Ada/Turing 路径）")
        );
        assert_eq!(super::model_build("310,8,0,0"), lang::tr("NVIDIA original build", "NVIDIA 原版"));
        assert_eq!(super::model_build("310.8.0.0"), lang::tr("NVIDIA original build", "NVIDIA 原版"));
    }

    #[test]
    fn version_key_orders_driver_numbers() {
        assert!(super::version_key("616.56").unwrap() >= (616, 56));
        assert!(super::version_key("620.10").unwrap() > (616, 56));
        assert!(super::version_key("572.83").unwrap() < (616, 56));
        assert_eq!(super::version_key("nope"), None);
    }

    #[test]
    fn describe_says_something() {
        assert!(!super::describe().is_empty());
    }

    #[test]
    fn driver_number_maps_windows_version() {
        // This machine: 32.0.16.1656 is what the NVIDIA App calls 616.56.
        assert_eq!(
            crate::gpu::nvidia_driver_number("32.0.16.1656").as_deref(),
            Some("616.56")
        );
        assert_eq!(
            crate::gpu::nvidia_driver_number("31.0.15.5222").as_deref(),
            Some("552.22")
        );
        assert_eq!(crate::gpu::nvidia_driver_number("1.2").as_deref(), None);
    }
}

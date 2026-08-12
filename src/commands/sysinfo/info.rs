//! Pure system-info collection and formatting for `orbital sysinfo`.
//!
//! [`collect_system_info`] works entirely off a [`Raw`] snapshot, so the tests
//! feed fixtures rather than probing the machine they run on.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemInfo {
    pub os_name: String,
    pub platform: String,
    pub arch: String,
    pub kernel: String,
    pub hostname: String,
    pub username: String,
    pub uptime: String,
    pub cpu_model: String,
    pub cpu_count: usize,
    pub mem_total: String,
    pub mem_used: String,
    pub load_average: Option<String>,
    pub shell: Option<String>,
}

/// A snapshot of the side-effecting readings `collect_system_info` formats.
///
/// `platform` uses the short names the rest of the app keys off — `linux`,
/// `darwin`, `win32` — and `arch` the short `x64`/`arm64` spelling.
#[derive(Debug, Clone, Default)]
pub struct Raw {
    pub platform: String,
    pub arch: String,
    pub release: String,
    pub hostname: String,
    pub uptime_secs: u64,
    pub total_mem: u64,
    pub free_mem: u64,
    pub cpus: Vec<String>,
    pub load_avg: [f64; 3],
    pub username: String,
    pub shell: Option<String>,
    /// Contents of `/etc/os-release`, when it could be read.
    pub os_release: Option<String>,
    /// `sw_vers` output on macOS: product name and version.
    pub sw_vers: Option<(String, Option<String>)>,
    /// The OS's own marketing name on Windows, e.g. `Windows 11 Pro`.
    pub product_name: Option<String>,
}

pub fn collect_system_info(raw: &Raw) -> SystemInfo {
    SystemInfo {
        os_name: pretty_os_name(raw),
        platform: raw.platform.clone(),
        arch: raw.arch.clone(),
        kernel: raw.release.clone(),
        hostname: raw.hostname.clone(),
        username: raw.username.clone(),
        uptime: format_uptime(raw.uptime_secs),
        cpu_model: raw
            .cpus
            .first()
            .map(|m| m.trim().to_string())
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "Unknown".to_string()),
        cpu_count: raw.cpus.len(),
        mem_total: format_bytes(raw.total_mem),
        mem_used: format_bytes(raw.total_mem.saturating_sub(raw.free_mem)),
        load_average: if raw.platform == "win32" {
            None
        } else {
            Some(format_load(&raw.load_avg))
        },
        shell: raw.shell.clone(),
    }
}

/// Best-effort human OS name with graceful per-platform fallback.
fn pretty_os_name(raw: &Raw) -> String {
    match raw.platform.as_str() {
        "linux" => raw
            .os_release
            .as_deref()
            .and_then(parse_os_release)
            .unwrap_or_else(|| format!("{} {}", raw.platform, raw.release)),
        "darwin" => match &raw.sw_vers {
            Some((product, Some(version))) => format!("{product} {version}"),
            Some((product, None)) => product.clone(),
            None => "macOS".to_string(),
        },
        "win32" => raw
            .product_name
            .clone()
            .unwrap_or_else(|| format!("Windows {}", raw.release)),
        other => format!("{other} {}", raw.release),
    }
}

fn parse_os_release(content: &str) -> Option<String> {
    let mut name = None;
    for line in content.lines() {
        let (key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "PRETTY_NAME" => return Some(value),
            "NAME" if name.is_none() => name = Some(value),
            _ => {}
        }
    }
    name
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86400;
    let hours = (seconds % 86400) / 3600;
    let mins = (seconds % 3600) / 60;
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    parts.push(format!("{mins}m"));
    parts.join(" ")
}

fn format_bytes(bytes: u64) -> String {
    let gib = bytes as f64 / 1024f64.powi(3);
    if gib >= 1.0 {
        format!("{gib:.2} GiB")
    } else {
        format!("{:.0} MiB", bytes as f64 / 1024f64.powi(2))
    }
}

fn format_load(load: &[f64; 3]) -> String {
    load.iter()
        .map(|n| format!("{n:.2}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Read the real machine.
pub fn real_raw() -> Raw {
    use sysinfo::System;

    let mut system = System::new();
    system.refresh_memory();
    system.refresh_cpu_all();

    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
    .to_string();

    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
    .to_string();

    let load = System::load_average();

    Raw {
        release: System::kernel_version()
            .or_else(System::os_version)
            .unwrap_or_else(|| "unknown".to_string()),
        hostname: System::host_name().unwrap_or_else(|| "unknown".to_string()),
        uptime_secs: System::uptime(),
        total_mem: system.total_memory(),
        free_mem: system.free_memory(),
        cpus: system
            .cpus()
            .iter()
            .map(|c| c.brand().to_string())
            .collect(),
        load_avg: [load.one, load.five, load.fifteen],
        username: current_username(),
        shell: std::env::var("SHELL").ok().filter(|s| !s.is_empty()),
        os_release: if platform == "linux" {
            std::fs::read_to_string("/etc/os-release").ok()
        } else {
            None
        },
        sw_vers: if platform == "darwin" {
            sw_vers()
        } else {
            None
        },
        product_name: if platform == "win32" {
            System::long_os_version().filter(|name| !name.trim().is_empty())
        } else {
            None
        },
        platform,
        arch,
    }
}

fn current_username() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Ask macOS for its marketing name. Any failure falls back to "macOS".
fn sw_vers() -> Option<(String, Option<String>)> {
    let read = |arg: &str| {
        std::process::Command::new("sw_vers")
            .arg(arg)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let product = read("-productName")?;
    Some((product, read("-productVersion")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw() -> Raw {
        Raw {
            platform: "linux".into(),
            arch: "x64".into(),
            release: "6.1.0".into(),
            hostname: "box".into(),
            uptime_secs: 90061, // 1d 1h 1m
            total_mem: 16 * 1024u64.pow(3),
            free_mem: 8 * 1024u64.pow(3),
            cpus: vec!["Test CPU @ 3.0GHz".into(), "Test CPU @ 3.0GHz".into()],
            load_avg: [0.5, 0.75, 1.0],
            username: "tester".into(),
            shell: Some("/bin/bash".into()),
            os_release: None,
            sw_vers: None,
            product_name: None,
        }
    }

    #[test]
    fn formats_memory_uptime_cpu_and_load() {
        let info = collect_system_info(&raw());
        assert_eq!(info.mem_total, "16.00 GiB");
        assert_eq!(info.mem_used, "8.00 GiB");
        assert_eq!(info.uptime, "1d 1h 1m");
        assert_eq!(info.cpu_model, "Test CPU @ 3.0GHz");
        assert_eq!(info.cpu_count, 2);
        assert_eq!(info.load_average.as_deref(), Some("0.50, 0.75, 1.00"));
    }

    #[test]
    fn reads_pretty_name_from_os_release_on_linux() {
        let mut raw = raw();
        raw.os_release = Some("NAME=\"Ubuntu\"\nPRETTY_NAME=\"Ubuntu 24.04 LTS\"\n".into());
        assert_eq!(collect_system_info(&raw).os_name, "Ubuntu 24.04 LTS");
    }

    #[test]
    fn falls_back_to_name_when_pretty_name_is_absent() {
        let mut raw = raw();
        raw.os_release = Some("ID=alpine\nNAME=\"Alpine Linux\"\n".into());
        assert_eq!(collect_system_info(&raw).os_name, "Alpine Linux");
    }

    #[test]
    fn falls_back_to_platform_and_release_when_os_release_is_missing() {
        assert_eq!(collect_system_info(&raw()).os_name, "linux 6.1.0");
    }

    #[test]
    fn uses_sw_vers_on_darwin() {
        let mut raw = raw();
        raw.platform = "darwin".into();
        raw.sw_vers = Some(("macOS".into(), Some("15.0".into())));
        assert_eq!(collect_system_info(&raw).os_name, "macOS 15.0");
    }

    #[test]
    fn darwin_without_sw_vers_still_names_the_os() {
        let mut raw = raw();
        raw.platform = "darwin".into();
        assert_eq!(collect_system_info(&raw).os_name, "macOS");
    }

    #[test]
    fn omits_load_average_on_windows() {
        let mut raw = raw();
        raw.platform = "win32".into();
        let info = collect_system_info(&raw);
        assert!(info.load_average.is_none());
        assert_eq!(info.os_name, "Windows 6.1.0");
    }

    #[test]
    fn prefers_the_windows_product_name_when_it_is_known() {
        let mut raw = raw();
        raw.platform = "win32".into();
        raw.product_name = Some("Windows 11 Pro".into());
        assert_eq!(collect_system_info(&raw).os_name, "Windows 11 Pro");
    }

    #[test]
    fn drops_the_day_and_hour_parts_when_zero() {
        assert_eq!(format_uptime(59), "0m");
        assert_eq!(format_uptime(3600), "1h 0m");
        assert_eq!(format_uptime(86400), "1d 0m");
    }

    #[test]
    fn shows_sub_gigabyte_memory_in_mebibytes() {
        assert_eq!(format_bytes(512 * 1024 * 1024), "512 MiB");
        assert_eq!(format_bytes(0), "0 MiB");
    }

    #[test]
    fn reports_unknown_when_no_cpu_is_detected() {
        let mut raw = raw();
        raw.cpus = Vec::new();
        let info = collect_system_info(&raw);
        assert_eq!(info.cpu_model, "Unknown");
        assert_eq!(info.cpu_count, 0);
    }

    #[test]
    fn free_memory_above_total_does_not_underflow() {
        let mut raw = raw();
        raw.free_mem = raw.total_mem + 1;
        assert_eq!(collect_system_info(&raw).mem_used, "0 MiB");
    }
}

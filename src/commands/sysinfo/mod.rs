//! `orbital sysinfo` — neofetch-style system information.

pub mod info;

use info::{collect_system_info, real_raw, SystemInfo};

use crate::commands::{Command, Ctx, Stdin};
use crate::components::key_value::key_value;
use crate::style;
use crate::term;
use crate::Res;

pub const COMMAND: Command = Command {
    name: "sysinfo",
    description: "Show system information (neofetch-style)",
    aliases: &["sys", "neofetch"],
    run: None,
    view,
    stdin: Stdin::Never,
};

/// Small ASCII logos keyed by platform, neofetch-style.
fn logo_for(platform: &str) -> &'static [&'static str] {
    match platform {
        "darwin" => &[
            "   .:'    ",
            " _ :'_    ",
            " (_'\\/_)  ",
            " /     \\  ",
            " \\     /  ",
            "  `---'   ",
        ],
        "linux" => &[
            "   .--.   ",
            "  |o_o |  ",
            "  |:_/ |  ",
            " //   \\ \\ ",
            "(|     | )",
            "/'\\_   _/`\\",
        ],
        "win32" => &[
            " .---.---.",
            " |   |   |",
            " |---+---|",
            " |   |   |",
            " '---'---'",
            "          ",
        ],
        _ => &[
            "  ___  ", " / _ \\ ", "| | | |", "| |_| |", " \\___/ ", "orbital",
        ],
    }
}

fn view(_ctx: &Ctx) -> Res {
    term::emit(&render(&collect_system_info(&real_raw())));
    Ok(())
}

fn render(info: &SystemInfo) -> Vec<String> {
    let rows: Vec<(&str, String)> = [
        ("User", Some(format!("{}@{}", info.username, info.hostname))),
        ("OS", Some(info.os_name.clone())),
        ("Kernel", Some(info.kernel.clone())),
        ("Arch", Some(info.arch.clone())),
        ("Uptime", Some(info.uptime.clone())),
        ("Shell", info.shell.clone()),
        (
            "CPU",
            Some(format!("{} ({})", info.cpu_model, info.cpu_count)),
        ),
        (
            "Memory",
            Some(format!("{} / {}", info.mem_used, info.mem_total)),
        ),
        ("Load", info.load_average.clone()),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.map(|v| (label, v)))
    .collect();

    let logo = logo_for(&info.platform);
    let width = logo.iter().map(|l| l.chars().count()).max().unwrap_or(0);

    (0..logo.len().max(rows.len()))
        .map(|i| {
            // Two spaces of gutter between the logo and the detail column.
            let art = style::green(&style::pad(logo.get(i).copied().unwrap_or(""), width));
            match rows.get(i) {
                Some((label, value)) => format!("{art}  {}", key_value(label, value)),
                None => art,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> SystemInfo {
        SystemInfo {
            os_name: "Ubuntu 24.04 LTS".into(),
            platform: "linux".into(),
            arch: "x64".into(),
            kernel: "6.1.0".into(),
            hostname: "box".into(),
            username: "tester".into(),
            uptime: "1d 1h 1m".into(),
            cpu_model: "Test CPU".into(),
            cpu_count: 2,
            mem_total: "16.00 GiB".into(),
            mem_used: "8.00 GiB".into(),
            load_average: Some("0.50, 0.75, 1.00".into()),
            shell: Some("/bin/bash".into()),
        }
    }

    #[test]
    fn renders_every_populated_row() {
        let out = render(&info()).join("\n");
        assert!(out.contains("tester@box"));
        assert!(out.contains("Ubuntu 24.04 LTS"));
        assert!(out.contains("Test CPU (2)"));
        assert!(out.contains("8.00 GiB / 16.00 GiB"));
        assert!(out.contains("0.50, 0.75, 1.00"));
    }

    #[test]
    fn omits_rows_without_a_value() {
        let mut info = info();
        info.load_average = None;
        info.shell = None;
        let out = render(&info).join("\n");
        assert!(!out.contains("Load"));
        assert!(!out.contains("Shell"));
    }

    #[test]
    fn always_prints_at_least_the_whole_logo() {
        let mut info = info();
        info.load_average = None;
        info.shell = None;
        let lines = render(&info);
        assert!(lines.len() >= logo_for("linux").len());
    }

    #[test]
    fn every_platform_has_a_logo() {
        for platform in ["darwin", "linux", "win32", "freebsd"] {
            assert!(!logo_for(platform).is_empty());
        }
    }

    #[test]
    fn detail_rows_all_start_at_the_same_column() {
        let lines = render(&info());
        let logo_width = logo_for("linux")
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap();
        let gutter = logo_width + 2;
        assert_eq!(lines[0].find("User"), Some(gutter));
        assert_eq!(lines[1].find("OS"), Some(gutter));
        assert_eq!(lines[2].find("Kernel"), Some(gutter));
    }
}

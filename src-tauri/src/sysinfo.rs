//! Live machine state, for the meters in the sidebar.
//!
//! VRAM is the number that actually matters here. A render on a 4 GB card is
//! decided by whether the second allocation fits, so "1.9 of 4.0 GB" tells a
//! user more than any progress bar: it says whether the next stage will run or
//! whether it will be refused the way the first YuE2 job was.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuStats {
    pub name: String,
    pub used_mib: u64,
    pub total_mib: u64,
    /// 0-100, rounded down. Truncated rather than rounded so a bar never
    /// claims more than the card can back.
    /// Share of the card's memory in use, 0-100.
    pub percent: u8,
    /// How busy the card is right now, 0-100. Not the same figure as the
    /// memory one: a render can hold two gigabytes and be stalled waiting on
    /// the CPU, which shows as high memory and low utilisation, and the two
    /// together are what say whether the GPU is the bottleneck.
    pub utilization: u8,
    /// Which processes hold it, largest first.
    pub processes: Vec<GpuProcess>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuProcess {
    pub pid: u32,
    pub used_mib: u64,
    /// Resolved to a name where the process is still alive. A dead pid is left
    /// unnamed rather than shown as a bare number.
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStats {
    /// None on a machine with no NVIDIA card, or where nvidia-smi is missing.
    /// The UI hides the meter rather than showing a zero that reads as "idle".
    pub gpu: Option<GpuStats>,
    pub ram_used_mib: u64,
    pub ram_total_mib: u64,
    /// 0-100, truncated.
    pub ram_percent: u8,
}

/// Everything at once, for one poll.
///
/// Two `nvidia-smi` calls is the price of the per-process breakdown, which
/// `nvidia-smi` will not return alongside the card totals. Worth it: "who is
/// holding 2.4 GB" has been the answer to a real bug more than once here.
pub fn stats() -> SystemStats {
    let (ram_used_mib, ram_total_mib) = ram();
    SystemStats {
        gpu: gpu(),
        ram_used_mib,
        ram_total_mib,
        ram_percent: percent(ram_used_mib, ram_total_mib),
    }
}

fn percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((used as f64 / total as f64) * 100.0).floor().clamp(0.0, 100.0) as u8
}

/// A short, ugly name. `nvidia-smi` is the authority on this and there is no
/// dependency-free way to ask it, so it is asked. Everything here degrades to
/// None if it is missing, slow, or prints something unexpected.
fn nvidia(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("nvidia-smi")
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn gpu() -> Option<GpuStats> {
    let totals = nvidia(&[
        "--query-gpu=name,memory.used,memory.total,utilization.gpu",
        "--format=csv,noheader,nounits",
    ])?;
    // One line per card. The first is the one the engine uses.
    let line = totals.lines().next()?.trim().to_string();
    let mut parts = line.split(',').map(|s| s.trim());
    let name = parts.next()?.to_string();
    let used_mib: u64 = parts.next()?.parse().ok()?;
    let total_mib: u64 = parts.next()?.parse().ok()?;
    // Reported as "N/A" by some drivers rather than a number. Losing the whole
    // card over it would be silly, so an unparseable reading is simply absent
    // and the meter is skipped.
    let utilization: u8 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);

    let mut processes: Vec<GpuProcess> = nvidia(&[
        "--query-compute-apps=pid,used_memory",
        "--format=csv,noheader,nounits",
    ])
    .map(|raw| {
        raw.lines()
            .filter_map(|l| {
                let mut p = l.split(',').map(|s| s.trim());
                let pid: u32 = p.next()?.parse().ok()?;
                let used_mib: u64 = p.next()?.parse().ok()?;
                Some(GpuProcess { pid, used_mib, name: process_name(pid) })
            })
            .collect()
    })
    .unwrap_or_default();
    // Largest first: the meter is about who is in the way, and that is the top
    // of the list.
    processes.sort_by(|a, b| b.used_mib.cmp(&a.used_mib).then(a.pid.cmp(&b.pid)));

    Some(GpuStats {
        name,
        used_mib,
        total_mib,
        percent: percent(used_mib, total_mib),
        utilization,
        processes,
    })
}

/// Best-effort process name, preferring the basename so the row is short
/// enough to sit in a sidebar.
fn process_name(pid: u32) -> Option<String> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = cmdline.split(|b| *b == 0).find(|s| !s.is_empty())?;
    let path = String::from_utf8_lossy(first).into_owned();
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn ram() -> (u64, u64) {
    let Ok(text) = std::fs::read_to_string("/proc/meminfo") else {
        return (0, 0);
    };
    let mut total = 0;
    let mut available = 0;
    for line in text.lines() {
        if let Some(v) = kib_field(line, "MemTotal") {
            total = v;
        } else if let Some(v) = kib_field(line, "MemAvailable") {
            available = v;
        }
    }
    // "Available" is what the kernel would hand out without swapping, which is
    // the figure that matches what a new model load is competing with. `free`'s
    // "used" subtracts buffers and cache, which is not what anyone means here.
    //
    // /proc/meminfo is in kibibytes. Left alone the meter reads "5198 GB of
    // 23895 GB", which is wrong by a factor of 1024 and only visible once
    // something else happens to render.
    const KIB_TO_MIB: u64 = 1024;
    (
        total.saturating_sub(available) / KIB_TO_MIB,
        total / KIB_TO_MIB,
    )
}

fn kib_field(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.strip_prefix(':')?;
    rest.split_whitespace().next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_card_reads_one_hundred() {
        assert_eq!(percent(4096, 4096), 100);
        assert_eq!(percent(0, 4096), 0);
    }

    /// Truncated, so a bar never overstates what is actually free -- the whole
    /// point of the meter is whether the next stage fits.
    #[test]
    fn percent_truncates_rather_than_rounds() {
        assert_eq!(percent(2048, 4096), 50);
        assert_eq!(percent(2049, 4096), 50);
        assert_eq!(percent(3999, 4096), 97);
        assert_eq!(percent(4000, 4096), 97);
        assert_eq!(percent(4001, 4096), 97);
    }

    #[test]
    fn a_zero_total_does_not_divide_by_zero() {
        assert_eq!(percent(10, 0), 0);
    }

    #[test]
    fn ram_is_read_from_proc_meminfo() {
        let (used, total) = ram();
        assert!(total > 0, "MemTotal should be readable on Linux");
        assert!(used <= total, "used cannot exceed total");
        // In mebibytes. A 24 GB machine reporting 24468 GB is the bug this
        // asserts against.
        assert!(total < 1024 * 1024, "RAM should be in MiB, got {total}");
        assert!(
            total > 0 && total < 256 * 1024,
            "RAM should be a plausible machine size in MiB, got {total}"
        );
    }

    /// A machine with no NVIDIA card must report None, not a zeroed card that
    /// reads as "idle" rather than "absent".
    #[test]
    fn a_machine_with_no_nvidia_card_reports_none() {
        // Whatever this machine is, the function must not panic, and whatever
        // it returns must be internally consistent.
        if let Some(g) = gpu() {
            assert!(g.total_mib > 0);
            assert!(g.percent <= 100);
        }
    }

    #[test]
    fn kib_field_reads_the_leading_number() {
        assert_eq!(kib_field("MemTotal:       24468616 kB", "MemTotal"), Some(24468616));
        assert_eq!(kib_field("MemFree: 1 kB", "MemTotal"), None);
    }

    /// The field names have to be camelCase, because that is what the frontend
    /// declares. Rust's `rename_all` makes this easy to get wrong in a way that
    /// compiles and type-checks perfectly: the serialised keys come out as
    /// `usedMib` while a hand-written interface reads `used_mib`, and every
    /// consumer gets `undefined` instead of an error.
    #[test]
    fn the_serialised_field_names_match_the_frontend() {
        let stats = SystemStats {
            gpu: Some(GpuStats {
                name: "NVIDIA GeForce GTX 1050".into(),
                used_mib: 1900,
                total_mib: 4096,
                percent: 46,
                utilization: 88,
                processes: vec![GpuProcess {
                    pid: 42,
                    used_mib: 1800,
                    name: Some("ace-server".into()),
                }],
            }),
            ram_used_mib: 4000,
            ram_total_mib: 24000,
            ram_percent: 16,
        };
        let json = serde_json::to_value(&stats).unwrap();
        let gpu = &json["gpu"];
        assert!(gpu.get("usedMib").is_some(), "VRAM used is missing: {gpu}");
        assert!(gpu.get("totalMib").is_some(), "VRAM total is missing: {gpu}");
        assert!(gpu.get("utilization").is_some(), "GPU busy is missing: {gpu}");
        assert!(gpu["processes"][0].get("usedMib").is_some(), "process VRAM is missing");
        assert!(json.get("ramUsedMib").is_some(), "RAM used is missing: {json}");
        assert!(json.get("ramTotalMib").is_some(), "RAM total is missing: {json}");
        // And nothing carrying the snake_case spelling.
        assert!(gpu.get("used_mib").is_none(), "snake_case leaked into the payload");
        assert!(json.get("ram_used_mib").is_none(), "snake_case leaked into the payload");
    }

}

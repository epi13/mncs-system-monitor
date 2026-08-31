use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use mncs_monitor_core::{
    CollectionIssue, CpuCounters, CpuSample, DiskSample, IoSample, MemorySample, NetworkSample,
    ObservationSource, ObservationStatus, ObservationTime, PressureKind, PressureLevel, Process,
    ProcessId, ProcessIdentity, ProcessState, ResourcePressure, SystemSnapshot,
};

use crate::{CollectError, Collector};

const DEFAULT_PAGE_SIZE: u64 = 4096;
const SECTOR_SIZE: u64 = 512;

/// Injectable Linux source paths make parser and partial-failure behavior deterministic in tests.
#[derive(Clone, Debug)]
pub struct LinuxPaths {
    pub proc_root: PathBuf,
    pub sys_root: PathBuf,
    pub hostname: PathBuf,
    pub passwd: PathBuf,
}

impl Default for LinuxPaths {
    fn default() -> Self {
        Self {
            proc_root: PathBuf::from("/proc"),
            sys_root: PathBuf::from("/sys"),
            hostname: PathBuf::from("/etc/hostname"),
            passwd: PathBuf::from("/etc/passwd"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ClockReading {
    wall_time: SystemTime,
    monotonic_ns: u64,
}

#[derive(Debug)]
struct SystemClock {
    origin: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl SystemClock {
    fn now(&self) -> ClockReading {
        let elapsed = self.origin.elapsed().as_nanos().min(u64::MAX as u128) as u64;
        ClockReading {
            wall_time: SystemTime::now(),
            monotonic_ns: elapsed,
        }
    }
}

/// Linux `/proc` and `/sys` collector. It returns the successfully acquired portions of a
/// snapshot and records per-process/source failures instead of turning missing facts into zeroes.
#[derive(Debug)]
pub struct PlatformCollector {
    paths: LinuxPaths,
    clock: SystemClock,
    page_size: u64,
}

impl Default for PlatformCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformCollector {
    pub fn new() -> Self {
        Self::with_paths(LinuxPaths::default())
    }

    pub fn with_paths(paths: LinuxPaths) -> Self {
        Self {
            paths,
            clock: SystemClock::default(),
            page_size: page_size(),
        }
    }

    pub fn paths(&self) -> &LinuxPaths {
        &self.paths
    }

    /// Collect at a supplied time. This is useful for deterministic fixtures and makes the
    /// monotonic/wall-clock distinction explicit at the host boundary.
    pub fn collect_at(
        &mut self,
        wall_time: SystemTime,
        monotonic_ns: u64,
    ) -> Result<SystemSnapshot, CollectError> {
        self.collect_at_reading(ClockReading {
            wall_time,
            monotonic_ns,
        })
    }

    fn collect_at_reading(&mut self, now: ClockReading) -> Result<SystemSnapshot, CollectError> {
        let proc_stat = read_required(&self.paths.proc_root.join("stat"))?;
        let cpu = parse_proc_stat_cpu(&proc_stat).map_err(|detail| CollectError::Malformed {
            resource: format!("{}/stat: {detail}", self.paths.proc_root.display()),
        })?;
        let cpu_count = parse_cpu_count(&proc_stat)
            .or_else(|| {
                read_optional(&self.paths.sys_root.join("devices/system/cpu/online"))
                    .ok()
                    .flatten()
                    .and_then(|value| parse_cpu_online(&value))
            })
            .or_else(|| {
                std::thread::available_parallelism()
                    .ok()
                    .map(|n| n.get() as u32)
            });

        let mut issues = Vec::new();
        let meminfo = read_optional(&self.paths.proc_root.join("meminfo"));
        let (memory_total, memory_available, memory_free, swap_total, swap_free) = match meminfo {
            Ok(Some(text)) => match parse_meminfo(&text) {
                Ok(values) => (
                    values.get("MemTotal").copied(),
                    values.get("MemAvailable").copied(),
                    values.get("MemFree").copied(),
                    values.get("SwapTotal").copied(),
                    values.get("SwapFree").copied(),
                ),
                Err(detail) => {
                    issues.push(CollectionIssue {
                        subject: None,
                        status: ObservationStatus::Malformed,
                        source: ObservationSource::ProcStatus,
                        detail: format!("meminfo: {detail}"),
                    });
                    (None, None, None, None, None)
                }
            },
            Ok(None) => (None, None, None, None, None),
            Err(error) => {
                issues.push(CollectionIssue {
                    subject: None,
                    status: status_for_error(&error),
                    source: ObservationSource::ProcStatus,
                    detail: format!(
                        "{}: {error}",
                        self.paths.proc_root.join("meminfo").display()
                    ),
                });
                (None, None, None, None, None)
            }
        };

        let uptime = read_optional(&self.paths.proc_root.join("uptime"))
            .ok()
            .flatten()
            .and_then(|text| parse_first_f64(&text).map(Duration::from_secs_f64));
        let load_average = read_optional(&self.paths.proc_root.join("loadavg"))
            .ok()
            .flatten()
            .and_then(|text| parse_load_average(&text).ok());
        let hostname = read_optional(&self.paths.hostname)
            .ok()
            .flatten()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let network = match read_optional(&self.paths.proc_root.join("net/dev")) {
            Ok(Some(text)) => parse_network(&text, now, &mut issues),
            Ok(None) => Vec::new(),
            Err(error) => {
                issues.push(CollectionIssue {
                    subject: None,
                    status: status_for_error(&error),
                    source: ObservationSource::ProcStat,
                    detail: format!("network counters unavailable: {error}"),
                });
                Vec::new()
            }
        };
        let disk = match read_optional(&self.paths.proc_root.join("diskstats")) {
            Ok(Some(text)) => parse_diskstats(&text, now, &mut issues),
            Ok(None) => Vec::new(),
            Err(error) => {
                issues.push(CollectionIssue {
                    subject: None,
                    status: status_for_error(&error),
                    source: ObservationSource::ProcStat,
                    detail: format!("disk counters unavailable: {error}"),
                });
                Vec::new()
            }
        };
        let pressure = collect_pressure(&self.paths.proc_root.join("pressure"), now, &mut issues);

        let passwd = read_optional(&self.paths.passwd)
            .ok()
            .flatten()
            .map(|text| parse_passwd(&text))
            .unwrap_or_default();
        let mut processes = Vec::new();
        let proc_entries = fs::read_dir(&self.paths.proc_root)
            .map_err(|error| map_io_error(&self.paths.proc_root, error))?;
        let mut pids: Vec<u32> = proc_entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
            .collect();
        pids.sort_unstable();

        for pid in pids {
            match read_process(&self.paths, pid, now, self.page_size, &passwd) {
                Ok((process, process_issues)) => {
                    issues.extend(process_issues);
                    processes.push(process);
                }
                Err((status, detail)) => issues.push(CollectionIssue {
                    subject: Some(ProcessId(pid)),
                    status,
                    source: ObservationSource::ProcStat,
                    detail,
                }),
            }
        }

        let memory_used_bytes = match (memory_total, memory_available) {
            (Some(total), Some(available)) if total >= available => Some(total - available),
            _ => None,
        };
        let swap_used_bytes = match (swap_total, swap_free) {
            (Some(total), Some(free)) if total >= free => Some(total - free),
            _ => None,
        };
        let time = ObservationTime {
            wall_time: now.wall_time,
            monotonic_ns: now.monotonic_ns,
        };
        Ok(SystemSnapshot {
            observed_at: Some(now.wall_time),
            sample_time: Some(time),
            hostname,
            processes,
            host_cpu: Some(CpuSample {
                interval: None,
                utilization: None,
                total_ticks: Some(cpu.total()),
                busy_ticks: Some(cpu.busy()),
                process_ticks: None,
                observed_at: time,
                source: ObservationSource::ProcStat,
                status: ObservationStatus::Observed,
            }),
            cpu_count,
            memory_total_bytes: memory_total,
            memory_available_bytes: memory_available,
            memory_free_bytes: memory_free,
            memory_used_bytes,
            swap_total_bytes: swap_total,
            swap_free_bytes: swap_free,
            swap_used_bytes,
            load_average,
            uptime,
            pressure,
            network,
            disk,
            issues,
            ..SystemSnapshot::default()
        })
    }
}

impl Collector for PlatformCollector {
    fn source(&self) -> ObservationSource {
        ObservationSource::ProcStat
    }

    fn collect(&mut self) -> Result<SystemSnapshot, CollectError> {
        self.collect_at_reading(self.clock.now())
    }
}

fn page_size() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: sysconf only reads process-independent kernel configuration.
        let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if value > 0 {
            return value as u64;
        }
    }
    DEFAULT_PAGE_SIZE
}

fn read_required(path: &Path) -> Result<String, CollectError> {
    fs::read_to_string(path).map_err(|error| map_io_error(path, error))
}

fn read_optional(path: &Path) -> Result<Option<String>, io::Error> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn map_io_error(path: &Path, error: io::Error) -> CollectError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => CollectError::PermissionDenied {
            resource: path.display().to_string(),
        },
        io::ErrorKind::NotFound => CollectError::Unavailable {
            resource: path.display().to_string(),
        },
        _ => CollectError::Io {
            resource: path.display().to_string(),
            detail: error.to_string(),
        },
    }
}

fn status_for_error(error: &io::Error) -> ObservationStatus {
    match error.kind() {
        io::ErrorKind::PermissionDenied => ObservationStatus::PermissionDenied,
        io::ErrorKind::NotFound => ObservationStatus::Unavailable,
        _ => ObservationStatus::Unknown,
    }
}

fn parse_u64(value: Option<&str>, field: &str) -> Result<u64, String> {
    value
        .ok_or_else(|| format!("missing {field}"))?
        .parse::<u64>()
        .map_err(|_| format!("invalid {field}"))
}

pub fn parse_proc_stat_cpu(input: &str) -> Result<CpuCounters, String> {
    let line = input
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or("missing aggregate cpu line")?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 5 {
        return Err("aggregate cpu line has too few fields".into());
    }
    Ok(CpuCounters {
        user: parse_u64(fields.get(1).copied(), "user")?,
        nice: parse_u64(fields.get(2).copied(), "nice")?,
        system: parse_u64(fields.get(3).copied(), "system")?,
        idle: parse_u64(fields.get(4).copied(), "idle")?,
        iowait: fields
            .get(5)
            .map(|value| parse_u64(Some(value), "iowait"))
            .transpose()?
            .unwrap_or(0),
        irq: fields
            .get(6)
            .map(|value| parse_u64(Some(value), "irq"))
            .transpose()?
            .unwrap_or(0),
        softirq: fields
            .get(7)
            .map(|value| parse_u64(Some(value), "softirq"))
            .transpose()?
            .unwrap_or(0),
        steal: fields
            .get(8)
            .map(|value| parse_u64(Some(value), "steal"))
            .transpose()?
            .unwrap_or(0),
    })
}

fn parse_cpu_count(input: &str) -> Option<u32> {
    let count = input
        .lines()
        .filter(|line| {
            line.strip_prefix("cpu").is_some_and(|tail| {
                tail.chars()
                    .next()
                    .is_some_and(|character| character.is_ascii_digit())
            })
        })
        .count();
    (count > 0).then_some(count as u32)
}

fn parse_cpu_online(input: &str) -> Option<u32> {
    let mut count = 0_u32;
    for range in input.trim().split(',') {
        let mut bounds = range.split('-');
        let first = bounds.next()?.parse::<u32>().ok()?;
        let last = bounds
            .next()
            .map_or(Ok(first), |value| value.parse::<u32>())
            .ok()?;
        if last < first {
            return None;
        }
        count = count.checked_add(last - first + 1)?;
    }
    (count > 0).then_some(count)
}

fn parse_meminfo(input: &str) -> Result<BTreeMap<String, u64>, String> {
    let mut values = BTreeMap::new();
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let (key, rest) = line
            .split_once(':')
            .ok_or_else(|| format!("missing ':' in {line:?}"))?;
        let mut fields = rest.split_whitespace();
        let value = parse_u64(fields.next(), key.trim())?;
        let bytes = match fields.next() {
            Some("kB") => value
                .checked_mul(1024)
                .ok_or_else(|| format!("{key} overflows bytes"))?,
            Some("MB") => value
                .checked_mul(1024 * 1024)
                .ok_or_else(|| format!("{key} overflows bytes"))?,
            Some("B") | None => value,
            Some(unit) => return Err(format!("unknown unit {unit} for {key}")),
        };
        values.insert(key.trim().to_string(), bytes);
    }
    Ok(values)
}

fn parse_first_f64(input: &str) -> Option<f64> {
    input.split_whitespace().next()?.parse().ok()
}

fn parse_load_average(input: &str) -> Result<[f64; 3], String> {
    let fields: Vec<&str> = input.split_whitespace().take(3).collect();
    if fields.len() != 3 {
        return Err("loadavg has fewer than three values".into());
    }
    let values: Vec<f64> = fields
        .iter()
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| "invalid load average".to_string())
        })
        .collect::<Result<_, _>>()?;
    Ok([values[0], values[1], values[2]])
}

pub fn parse_process_stat(input: &str) -> Result<ParsedProcessStat, String> {
    let open = input.find('(').ok_or("missing comm opening delimiter")?;
    let close = input.rfind(')').ok_or("missing comm closing delimiter")?;
    if close <= open {
        return Err("invalid comm delimiters".into());
    }
    let pid = input[..open]
        .trim()
        .parse::<u32>()
        .map_err(|_| "invalid pid")?;
    let comm = input[open + 1..close].to_string();
    let fields: Vec<&str> = input[close + 1..].split_whitespace().collect();
    if fields.len() < 22 {
        return Err("process stat has too few fields".into());
    }
    let state = fields[0].chars().next().ok_or("empty process state")?;
    let ppid = fields[1].parse::<u32>().map_err(|_| "invalid ppid")?;
    let utime = fields[11].parse::<u64>().map_err(|_| "invalid utime")?;
    let stime = fields[12].parse::<u64>().map_err(|_| "invalid stime")?;
    let threads = fields[17]
        .parse::<u32>()
        .map_err(|_| "invalid thread count")?;
    let start_marker = fields[19]
        .parse::<u64>()
        .map_err(|_| "invalid start marker")?;
    let virtual_bytes = fields[20]
        .parse::<u64>()
        .map_err(|_| "invalid virtual memory")?;
    let rss_pages = fields[21].parse::<i64>().map_err(|_| "invalid rss")?;
    let resident_bytes = (rss_pages >= 0).then_some(rss_pages as u64);
    Ok(ParsedProcessStat {
        pid,
        comm,
        state,
        ppid,
        utime,
        stime,
        threads,
        start_marker,
        virtual_bytes,
        resident_bytes,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedProcessStat {
    pub pid: u32,
    pub comm: String,
    pub state: char,
    pub ppid: u32,
    pub utime: u64,
    pub stime: u64,
    pub threads: u32,
    pub start_marker: u64,
    pub virtual_bytes: u64,
    pub resident_bytes: Option<u64>,
}

fn parse_status(input: &str) -> (Option<u32>, Option<u64>, Option<u64>, Option<u32>) {
    let mut uid = None;
    let mut resident = None;
    let mut virtual_bytes = None;
    let mut threads = None;
    for line in input.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("Uid:") => uid = fields.next().and_then(|value| value.parse().ok()),
            Some("VmRSS:") => {
                resident = fields
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .and_then(|value| value.checked_mul(1024))
            }
            Some("VmSize:") => {
                virtual_bytes = fields
                    .next()
                    .and_then(|value| value.parse::<u64>().ok())
                    .and_then(|value| value.checked_mul(1024))
            }
            Some("Threads:") => threads = fields.next().and_then(|value| value.parse().ok()),
            _ => {}
        }
    }
    (uid, resident, virtual_bytes, threads)
}

fn parse_io(input: &str, now: ObservationTime) -> Result<IoSample, ObservationStatus> {
    let mut read_bytes = None;
    let mut written_bytes = None;
    for line in input.lines() {
        let (key, value) = line.split_once(':').ok_or(ObservationStatus::Malformed)?;
        let value = value
            .trim()
            .parse::<u64>()
            .map_err(|_| ObservationStatus::Malformed)?;
        match key.trim() {
            "read_bytes" => read_bytes = Some(value),
            "write_bytes" => written_bytes = Some(value),
            _ => {}
        }
    }
    if read_bytes.is_none() && written_bytes.is_none() {
        return Err(ObservationStatus::Malformed);
    }
    Ok(IoSample {
        read_bytes,
        written_bytes,
        read_rate_bytes_per_sec: None,
        written_rate_bytes_per_sec: None,
        interval: None,
        observed_at: now,
        source: ObservationSource::ProcIo,
        status: ObservationStatus::Observed,
    })
}

fn parse_passwd(input: &str) -> BTreeMap<u32, String> {
    input
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() >= 3 {
                Some((fields[2].parse().ok()?, fields[0].to_string()))
            } else {
                None
            }
        })
        .collect()
}

fn read_process(
    paths: &LinuxPaths,
    pid: u32,
    now: ClockReading,
    page_size: u64,
    passwd: &BTreeMap<u32, String>,
) -> Result<(Process, Vec<CollectionIssue>), (ObservationStatus, String)> {
    let root = paths.proc_root.join(pid.to_string());
    let stat_path = root.join("stat");
    let stat_text = fs::read_to_string(&stat_path).map_err(|error| {
        let status = if error.kind() == io::ErrorKind::NotFound {
            ObservationStatus::Disappeared
        } else {
            status_for_error(&error)
        };
        (
            status,
            format!("process {pid} stat could not be read: {error}"),
        )
    })?;
    let parsed = parse_process_stat(&stat_text).map_err(|detail| {
        (
            ObservationStatus::Malformed,
            format!("process {pid} stat: {detail}"),
        )
    })?;
    let time = ObservationTime {
        wall_time: now.wall_time,
        monotonic_ns: now.monotonic_ns,
    };
    let status_text = match fs::read_to_string(root.join("status")) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(_) => None,
    };
    let (uid, status_rss, status_vm, status_threads) = status_text
        .as_deref()
        .map(parse_status)
        .unwrap_or((None, None, None, None));
    let resident_bytes = status_rss.or_else(|| {
        parsed
            .resident_bytes
            .and_then(|pages| pages.checked_mul(page_size))
    });
    let virtual_bytes = status_vm.or(Some(parsed.virtual_bytes));
    let command_line = match fs::read(root.join("cmdline")) {
        Ok(bytes) if !bytes.is_empty() => {
            let command = String::from_utf8_lossy(&bytes).replace('\0', " ");
            let command = command.trim().to_string();
            (!command.is_empty()).then_some(command)
        }
        Ok(_) | Err(_) => None,
    };
    let mut issues = Vec::new();
    let io_result = match fs::read_to_string(root.join("io")) {
        Ok(text) => parse_io(&text, time).map(Some),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            Err(ObservationStatus::PermissionDenied)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(ObservationStatus::Unavailable)
        }
        Err(_) => Err(ObservationStatus::Unknown),
    };
    let (io, io_status) = match io_result {
        Ok(io) => (io, ObservationStatus::Observed),
        Err(status) => {
            issues.push(CollectionIssue {
                subject: Some(ProcessId(pid)),
                status,
                source: ObservationSource::ProcIo,
                detail: format!("process {pid} I/O counters are {status:?}"),
            });
            (None, status)
        }
    };
    let identity = ProcessIdentity::new(pid, Some(parsed.start_marker));
    let process = Process {
        identity,
        parent: Some(ProcessId(parsed.ppid)),
        executable: parsed.comm,
        command_line,
        uid,
        user: uid.and_then(|uid| passwd.get(&uid).cloned()),
        state: process_state(parsed.state),
        threads: status_threads.or(Some(parsed.threads)),
        cpu: Some(CpuSample {
            interval: None,
            utilization: None,
            total_ticks: None,
            busy_ticks: None,
            process_ticks: parsed.utime.checked_add(parsed.stime),
            observed_at: time,
            source: ObservationSource::ProcStat,
            status: ObservationStatus::Observed,
        }),
        memory: Some(MemorySample {
            resident_bytes,
            virtual_bytes,
            observed_at: time,
            source: ObservationSource::ProcStatus,
            status: if resident_bytes.is_some() || virtual_bytes.is_some() {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Unknown
            },
        }),
        memory_status: if resident_bytes.is_some() || virtual_bytes.is_some() {
            ObservationStatus::Observed
        } else {
            ObservationStatus::Unknown
        },
        io,
        io_status,
    };
    Ok((process, issues))
}

fn collect_pressure(
    root: &Path,
    now: ClockReading,
    issues: &mut Vec<CollectionIssue>,
) -> Vec<ResourcePressure> {
    [
        ("cpu", PressureKind::Cpu),
        ("memory", PressureKind::Memory),
        ("io", PressureKind::Io),
    ]
    .into_iter()
    .filter_map(|(name, kind)| {
        let path = root.join(name);
        match read_optional(&path) {
            Ok(Some(text)) => match parse_pressure(&text, kind, now) {
                Ok(pressure) => Some(pressure),
                Err(detail) => {
                    issues.push(CollectionIssue {
                        subject: None,
                        status: ObservationStatus::Malformed,
                        source: ObservationSource::ProcPressure,
                        detail: format!("{}: {detail}", path.display()),
                    });
                    None
                }
            },
            Ok(None) => {
                issues.push(CollectionIssue {
                    subject: None,
                    status: ObservationStatus::Unavailable,
                    source: ObservationSource::ProcPressure,
                    detail: format!("{} is unavailable on this kernel", path.display()),
                });
                None
            }
            Err(error) => {
                issues.push(CollectionIssue {
                    subject: None,
                    status: status_for_error(&error),
                    source: ObservationSource::ProcPressure,
                    detail: format!("{}: {error}", path.display()),
                });
                None
            }
        }
    })
    .collect()
}

fn parse_pressure(
    input: &str,
    kind: PressureKind,
    now: ClockReading,
) -> Result<ResourcePressure, String> {
    let mut some = [None; 3];
    let mut full = [None; 3];
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let mut fields = line.split_whitespace();
        let class = fields.next().ok_or("missing pressure class")?;
        let target = match class {
            "some" => &mut some,
            "full" => &mut full,
            other => return Err(format!("unknown pressure class {other}")),
        };
        for field in fields {
            let (key, value) = field
                .split_once('=')
                .ok_or_else(|| format!("malformed pressure field {field}"))?;
            let index = match key {
                "avg10" => 0,
                "avg60" => 1,
                "avg300" => 2,
                "total" => continue,
                other => return Err(format!("unknown pressure field {other}")),
            };
            let parsed = value
                .parse::<f64>()
                .map_err(|_| format!("invalid pressure value {value}"))?;
            if !parsed.is_finite() || parsed < 0.0 {
                return Err(format!("invalid pressure value {value}"));
            }
            target[index] = Some(parsed);
        }
    }
    if some.iter().all(Option::is_none) && full.iter().all(Option::is_none) {
        return Err("pressure file had no averages".into());
    }
    let level = if full[0].is_some_and(|value| value > 0.0) {
        PressureLevel::Full
    } else if some[0].is_some_and(|value| value > 0.0) {
        PressureLevel::Some
    } else {
        PressureLevel::None
    };
    Ok(ResourcePressure {
        kind,
        level,
        some_avg10: some[0],
        some_avg60: some[1],
        some_avg300: some[2],
        full_avg10: full[0],
        full_avg60: full[1],
        full_avg300: full[2],
        window: Some(Duration::from_secs(300)),
        observed_at: ObservationTime {
            wall_time: now.wall_time,
            monotonic_ns: now.monotonic_ns,
        },
        source: ObservationSource::ProcPressure,
        status: ObservationStatus::Observed,
    })
}

fn process_state(state: char) -> ProcessState {
    match state {
        'R' => ProcessState::Running,
        'S' | 'D' | 'I' => ProcessState::Sleeping,
        'T' | 't' => ProcessState::Stopped,
        'Z' => ProcessState::Zombie,
        'X' | 'x' => ProcessState::Dead,
        _ => ProcessState::Unknown,
    }
}

fn parse_network(
    input: &str,
    now: ClockReading,
    issues: &mut Vec<CollectionIssue>,
) -> Vec<NetworkSample> {
    let time = ObservationTime {
        wall_time: now.wall_time,
        monotonic_ns: now.monotonic_ns,
    };
    input
        .lines()
        .skip(2)
        .filter_map(|line| {
            let (interface, counters) = line.split_once(':')?;
            let fields: Vec<&str> = counters.split_whitespace().collect();
            if fields.len() < 9 {
                issues.push(CollectionIssue {
                    subject: None,
                    status: ObservationStatus::Malformed,
                    source: ObservationSource::ProcStat,
                    detail: format!("network row for {} has too few fields", interface.trim()),
                });
                return None;
            }
            let received = fields[0].parse().ok();
            let transmitted = fields[8].parse().ok();
            let status = if received.is_some() && transmitted.is_some() {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Malformed
            };
            if !status.is_observed() {
                issues.push(CollectionIssue {
                    subject: None,
                    status,
                    source: ObservationSource::ProcStat,
                    detail: format!("network counters for {} are malformed", interface.trim()),
                });
            }
            Some(NetworkSample {
                interface: Some(interface.trim().to_string()),
                received_bytes: received,
                transmitted_bytes: transmitted,
                received_rate_bytes_per_sec: None,
                transmitted_rate_bytes_per_sec: None,
                interval: None,
                observed_at: time,
                source: ObservationSource::ProcStat,
                status,
            })
        })
        .collect()
}

fn parse_diskstats(
    input: &str,
    now: ClockReading,
    issues: &mut Vec<CollectionIssue>,
) -> Vec<DiskSample> {
    let time = ObservationTime {
        wall_time: now.wall_time,
        monotonic_ns: now.monotonic_ns,
    };
    input
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 14 {
                issues.push(CollectionIssue {
                    subject: None,
                    status: ObservationStatus::Malformed,
                    source: ObservationSource::ProcStat,
                    detail: "diskstats row has too few fields".into(),
                });
                return None;
            }
            let sectors_read = fields[5].parse::<u64>().ok();
            let sectors_written = fields[9].parse::<u64>().ok();
            let read_bytes = sectors_read.and_then(|value| value.checked_mul(SECTOR_SIZE));
            let written_bytes = sectors_written.and_then(|value| value.checked_mul(SECTOR_SIZE));
            let status = if read_bytes.is_some() && written_bytes.is_some() {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Malformed
            };
            if !status.is_observed() {
                issues.push(CollectionIssue {
                    subject: None,
                    status,
                    source: ObservationSource::ProcStat,
                    detail: format!("disk counters for {} are malformed", fields[2]),
                });
            }
            Some(DiskSample {
                device: fields[2].to_string(),
                read_bytes,
                written_bytes,
                read_rate_bytes_per_sec: None,
                written_rate_bytes_per_sec: None,
                interval: None,
                observed_at: time,
                source: ObservationSource::ProcStat,
                status,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_parser_accepts_kernel_extensions_without_losing_known_fields() {
        let cpu = parse_proc_stat_cpu("cpu  10 2 3 40 5 6 7 8 9 10\ncpu0 1 1 1 1").unwrap();
        assert_eq!(cpu.user, 10);
        assert_eq!(cpu.iowait, 5);
        assert_eq!(cpu.steal, 8);
    }

    #[test]
    fn process_stat_uses_last_closing_paren_for_comm_with_parens() {
        let mut fields = vec!["42", "(worker (io))", "S", "7"];
        fields.extend(std::iter::repeat_n("0", 22));
        fields[13] = "2";
        fields[19] = "3";
        fields[21] = "99";
        fields[22] = "4096";
        fields[23] = "8";
        let parsed = parse_process_stat(&fields.join(" ")).unwrap();
        assert_eq!(parsed.pid, 42);
        assert_eq!(parsed.comm, "worker (io)");
        assert_eq!(parsed.start_marker, 99);
    }

    #[test]
    fn meminfo_preserves_missing_values_and_converts_kib() {
        let values = parse_meminfo("MemTotal: 10 kB\nSwapTotal: 0 kB\n").unwrap();
        assert_eq!(values["MemTotal"], 10 * 1024);
        assert_eq!(values["SwapTotal"], 0);
        assert!(!values.contains_key("MemAvailable"));
    }

    #[test]
    fn cpu_online_parser_accepts_linux_ranges() {
        assert_eq!(parse_cpu_online("0-3,8,10-11\n"), Some(7));
        assert_eq!(parse_cpu_online("3-1"), None);
    }

    #[test]
    fn pressure_parser_retains_psi_averages_and_level() {
        let pressure = parse_pressure(
            "some avg10=1.00 avg60=0.50 avg300=0.25 total=100\nfull avg10=0.10 avg60=0.05 avg300=0.01 total=10\n",
            PressureKind::Io,
            ClockReading {
                wall_time: SystemTime::UNIX_EPOCH,
                monotonic_ns: 5,
            },
        )
        .unwrap();
        assert_eq!(pressure.level, PressureLevel::Full);
        assert_eq!(pressure.some_avg10, Some(1.0));
        assert_eq!(pressure.full_avg300, Some(0.01));
    }

    #[test]
    fn malformed_process_stat_is_not_a_zero_process() {
        assert!(parse_process_stat("1 (bad) R 1").is_err());
    }
}

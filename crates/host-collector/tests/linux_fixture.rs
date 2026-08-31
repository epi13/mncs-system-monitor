#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use mncs_host_collector::{LinuxPaths, PlatformCollector};
use mncs_monitor_core::{ObservationStatus, ProcessLifecycle, Sampler};

fn process_stat(pid: u32, start: u64, user_ticks: u64, system_ticks: u64) -> String {
    let mut fields = vec![String::from("0"); 22];
    fields[0] = "S".into();
    fields[1] = "1".into();
    fields[11] = user_ticks.to_string();
    fields[12] = system_ticks.to_string();
    fields[17] = "4".into();
    fields[19] = start.to_string();
    fields[20] = "8192".into();
    fields[21] = "16".into();
    format!("{pid} (fixture worker) {}", fields.join(" "))
}

fn make_fixture() -> (std::path::PathBuf, LinuxPaths) {
    let root = std::env::temp_dir().join(format!(
        "mncs-monitor-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let proc_root = root.join("proc");
    fs::create_dir_all(proc_root.join("100")).unwrap();
    fs::create_dir_all(proc_root.join("net")).unwrap();
    fs::create_dir_all(proc_root.join("pressure")).unwrap();
    fs::create_dir_all(&root).unwrap();
    fs::write(
        proc_root.join("stat"),
        "cpu 10 0 10 80 0 0 0 0 0\ncpu0 10 0 10 80 0 0 0 0\nprocesses 2\n",
    )
    .unwrap();
    fs::write(
        proc_root.join("meminfo"),
        "MemTotal: 1024 kB\nMemAvailable: 512 kB\nMemFree: 256 kB\nSwapTotal: 2048 kB\nSwapFree: 1024 kB\n",
    )
    .unwrap();
    fs::write(proc_root.join("uptime"), "12.50 8.00\n").unwrap();
    fs::write(proc_root.join("loadavg"), "0.10 0.20 0.30 1/2 100\n").unwrap();
    fs::write(
        proc_root.join("net/dev"),
        "Inter-| Receive | Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\neth0: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n",
    )
    .unwrap();
    fs::write(
        proc_root.join("diskstats"),
        "8 0 sda 1 0 2 0 3 0 4 0 0 0 0 0\n",
    )
    .unwrap();
    for (name, contents) in [
        (
            "cpu",
            "some avg10=0.10 avg60=0.20 avg300=0.30 total=10\n",
        ),
        (
            "memory",
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        ),
        (
            "io",
            "some avg10=1.00 avg60=0.50 avg300=0.25 total=100\nfull avg10=0.10 avg60=0.05 avg300=0.01 total=10\n",
        ),
    ] {
        fs::write(proc_root.join("pressure").join(name), contents).unwrap();
    }
    fs::write(proc_root.join("100/stat"), process_stat(100, 77, 10, 2)).unwrap();
    fs::write(
        proc_root.join("100/status"),
        "Name:\tfixture\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t64 kB\nVmSize:\t128 kB\nThreads:\t4\n",
    )
    .unwrap();
    fs::write(proc_root.join("100/cmdline"), b"fixture\0--test\0").unwrap();
    fs::write(
        proc_root.join("100/io"),
        "read_bytes: 1000\nwrite_bytes: 2000\n",
    )
    .unwrap();
    fs::write(root.join("hostname"), "fixture-host\n").unwrap();
    fs::write(
        root.join("passwd"),
        "fixture:x:1000:1000::/home/fixture:/bin/sh\n",
    )
    .unwrap();
    let paths = LinuxPaths {
        proc_root,
        sys_root: root.join("sys"),
        hostname: root.join("hostname"),
        passwd: root.join("passwd"),
    };
    (root, paths)
}

fn remove_fixture(root: &Path) {
    let _ = fs::remove_dir_all(root);
}

fn add_process(root: &Path, pid: u32) {
    let process_root = root.join("proc").join(pid.to_string());
    fs::create_dir_all(&process_root).unwrap();
    fs::write(
        process_root.join("stat"),
        process_stat(pid, u64::from(pid), 10, 2),
    )
    .unwrap();
    fs::write(
        process_root.join("status"),
        "Name:\tfixture\nUid:\t1000\t1000\t1000\t1000\nVmRSS:\t64 kB\nVmSize:\t128 kB\nThreads:\t1\n",
    )
    .unwrap();
    fs::write(process_root.join("cmdline"), b"fixture\0--scale\0").unwrap();
    fs::write(
        process_root.join("io"),
        "read_bytes: 1000\nwrite_bytes: 2000\n",
    )
    .unwrap();
}

#[test]
fn fixture_collects_host_process_io_network_and_disk_without_fake_zeroes() {
    let (root, paths) = make_fixture();
    let mut collector = PlatformCollector::with_paths(paths);
    let snapshot = collector
        .collect_at(SystemTime::UNIX_EPOCH, 1_000_000_000)
        .unwrap();

    assert_eq!(snapshot.hostname.as_deref(), Some("fixture-host"));
    assert_eq!(snapshot.cpu_count, Some(1));
    assert_eq!(snapshot.memory_total_bytes, Some(1024 * 1024));
    assert_eq!(snapshot.memory_available_bytes, Some(512 * 1024));
    assert_eq!(snapshot.swap_used_bytes, Some(1024 * 1024));
    assert_eq!(snapshot.process_count(), 1);
    assert_eq!(snapshot.processes[0].identity.start_marker, Some(77));
    assert_eq!(snapshot.processes[0].user.as_deref(), Some("fixture"));
    assert_eq!(
        snapshot.processes[0].io.as_ref().unwrap().read_bytes,
        Some(1000)
    );
    assert_eq!(snapshot.network[0].received_bytes, Some(100));
    assert_eq!(snapshot.disk[0].read_bytes, Some(1024));
    assert_eq!(snapshot.pressure.len(), 3);
    assert_eq!(
        snapshot.pressure[2].level,
        mncs_monitor_core::PressureLevel::Full
    );
    assert!(snapshot.network[0].received_rate_bytes_per_sec.is_none());
    assert!(snapshot.issues.is_empty());

    remove_fixture(&root);
}

#[test]
fn fixture_reconciliation_detects_pid_reuse_and_disappearance() {
    let (root, paths) = make_fixture();
    let mut collector = PlatformCollector::with_paths(paths);
    let mut sampler = Sampler::new(2);
    let first = collector
        .collect_at(SystemTime::UNIX_EPOCH, 1_000_000_000)
        .unwrap();
    let _ = sampler.accept(first);

    fs::write(root.join("proc/100/stat"), process_stat(100, 88, 12, 3)).unwrap();
    let second = collector
        .collect_at(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1),
            2_000_000_000,
        )
        .unwrap();
    let accepted = sampler.accept(second).snapshot;
    assert_eq!(
        accepted.transitions[0].lifecycle,
        ProcessLifecycle::IdentityChanged
    );
    assert!(accepted.processes[0]
        .cpu
        .as_ref()
        .unwrap()
        .utilization
        .is_none());

    fs::remove_dir_all(root.join("proc/100")).unwrap();
    let third = collector
        .collect_at(
            SystemTime::UNIX_EPOCH + Duration::from_secs(2),
            3_000_000_000,
        )
        .unwrap();
    let accepted = sampler.accept(third).snapshot;
    assert_eq!(
        accepted.transitions[0].lifecycle,
        ProcessLifecycle::NoLongerObserved
    );
    assert!(accepted
        .events
        .iter()
        .any(|event| event.detail.contains("clean exit is not asserted")));

    remove_fixture(&root);
}

#[test]
fn missing_io_is_explicit_and_process_remains_observable() {
    let (root, paths) = make_fixture();
    fs::remove_file(root.join("proc/100/io")).unwrap();
    let mut collector = PlatformCollector::with_paths(paths);
    let snapshot = collector
        .collect_at(SystemTime::UNIX_EPOCH, 1_000_000_000)
        .unwrap();
    assert_eq!(snapshot.process_count(), 1);
    assert_eq!(snapshot.processes[0].io, None);
    assert_eq!(
        snapshot.processes[0].io_status,
        ObservationStatus::Unavailable
    );
    assert!(snapshot
        .issues
        .iter()
        .any(|issue| issue.status == ObservationStatus::Unavailable));
    remove_fixture(&root);
}

#[test]
fn fixture_scales_from_empty_to_hundreds_of_processes() {
    for count in [0_usize, 1, 8, 32, 128] {
        let (root, paths) = make_fixture();
        if count == 0 {
            fs::remove_dir_all(root.join("proc/100")).unwrap();
        } else {
            for pid in 101..(100 + count as u32) {
                add_process(&root, pid);
            }
        }
        let mut collector = PlatformCollector::with_paths(paths);
        let snapshot = collector
            .collect_at(SystemTime::UNIX_EPOCH, 1_000_000_000)
            .unwrap();
        assert_eq!(snapshot.process_count(), count);
        let accepted = Sampler::new(2).accept(snapshot).snapshot;
        assert_eq!(accepted.relationships.len(), count);
        assert_eq!(accepted.history[0].processes.len(), count);
        remove_fixture(&root);
    }
}

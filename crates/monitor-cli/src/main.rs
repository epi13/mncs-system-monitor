use std::cmp::Ordering;
use std::time::Duration;

use mncs_execution_correlation::{
    correlate, correlation_report_value, execution_records_from_json,
    execution_records_from_test_result, reconcile_after_restart, restart_report_value,
    CorrelationReport, ExecutionRecord, HostSaturation,
};
use mncs_host_collector::{Collector, PlatformCollector};
use mncs_machine_projection::StructuredProjection;
use mncs_monitor_core::{Process, ProcessIdentity, ProcessState, Sampler, SystemSnapshot};
use mncs_tui_host::{Cell, Color, Event, Frame, Key, Size, Style, TerminalSession};

const DEFAULT_INTERVAL_MS: u64 = 1_000;
const DEFAULT_HISTORY: usize = 120;
const TEST_RESULT_SCHEMA_VERSION: &str = "mncs.test-result/1";

#[derive(Clone, Debug)]
struct Config {
    interval: Duration,
    history_capacity: usize,
    json: bool,
    once: bool,
    no_tui: bool,
    executions: Option<String>,
    watch_executables: Vec<String>,
    reconcile: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
            history_capacity: DEFAULT_HISTORY,
            json: false,
            once: false,
            no_tui: false,
            executions: None,
            watch_executables: Vec::new(),
            reconcile: None,
        }
    }
}

fn main() {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(2);
        }
    };
    let result = if config.reconcile.is_some() {
        run_reconcile(config)
    } else if config.json {
        run_machine(config)
    } else {
        run_human(config)
    };
    if let Err(error) = result {
        eprintln!("mncs-system-monitor: {error}");
        std::process::exit(1);
    }
}

fn parse_args<I>(args: I) -> Result<Config, String>
where
    I: IntoIterator<Item = String>,
{
    let mut config = Config::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => config.json = true,
            "--once" => config.once = true,
            "--no-tui" => config.no_tui = true,
            "--interval-ms" => {
                let value = args
                    .next()
                    .ok_or("--interval-ms needs a positive integer")?;
                let milliseconds = value
                    .parse::<u64>()
                    .map_err(|_| "--interval-ms needs a positive integer")?;
                if milliseconds == 0 {
                    return Err("--interval-ms must be greater than zero".into());
                }
                config.interval = Duration::from_millis(milliseconds);
            }
            "--history" => {
                let value = args
                    .next()
                    .ok_or("--history needs an integer from 1 to 1000")?;
                let capacity = value
                    .parse::<usize>()
                    .map_err(|_| "--history needs an integer from 1 to 1000")?;
                if !(1..=1_000).contains(&capacity) {
                    return Err("--history needs an integer from 1 to 1000".into());
                }
                config.history_capacity = capacity;
            }
            "--executions" => {
                let value = args
                    .next()
                    .ok_or("--executions needs a records file path")?;
                if value.is_empty() {
                    return Err("--executions needs a records file path".into());
                }
                config.executions = Some(value);
            }
            "--watch-exe" => {
                let value = args.next().ok_or("--watch-exe needs an executable name")?;
                if value.is_empty() {
                    return Err("--watch-exe needs an executable name".into());
                }
                config.watch_executables.push(value);
            }
            "--reconcile" => {
                let value = args.next().ok_or("--reconcile needs a records file path")?;
                if value.is_empty() {
                    return Err("--reconcile needs a records file path".into());
                }
                config.reconcile = Some(value);
            }
            "-h" | "--help" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}\n\n{}", usage())),
        }
    }
    Ok(config)
}

fn usage() -> &'static str {
    "Usage: mncs-system-monitor [--json] [--once] [--no-tui] [--interval-ms N] [--history N]\n                         [--executions FILE] [--watch-exe NAME]... [--reconcile FILE]\n\n\
Default mode is an interactive TUI when stdout is a terminal and a concise snapshot otherwise.\n\
--json emits the semantic snapshot envelope, including evidence status and bounded history.\n\
--executions correlates the snapshot against ingested canonical execution records (the\n\
monitor's own record envelope, or a mncs.test-result/1 envelope) and merges a `correlation`\n\
section into --json output.\n\
--watch-exe names an executable to flag when it runs with no linked Active execution; repeat\n\
for several names. --reconcile prints a restart-reconciliation report for FILE and exits."
}

/// Load canonical execution records. A `mncs.test-result/1` envelope is ingested for its
/// outcome section; anything else goes through the monitor's record envelope. A malformed
/// records file is a configuration error (nonzero exit), never silent UNKNOWN telemetry.
fn load_execution_records(path: &str) -> Result<Vec<ExecutionRecord>, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let marker: serde_json::Value = serde_json::from_str(&text)?;
    let is_test_result = marker
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        == Some(TEST_RESULT_SCHEMA_VERSION);
    if is_test_result {
        Ok(execution_records_from_test_result(&text)?)
    } else {
        Ok(execution_records_from_json(&text)?)
    }
}

/// Restart/interruption reconciliation: classify retained records against one fresh snapshot
/// and emit the versioned restart section. Exit status stays zero while the report itself is
/// produced; the classifications inside carry any staleness.
fn run_reconcile(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let path = config
        .reconcile
        .as_deref()
        .ok_or("reconcile mode needs a records file")?;
    let records = load_execution_records(path)?;
    let mut collector = PlatformCollector::new();
    let mut sampler = Sampler::new(1);
    let snapshot = sampler.accept(collector.collect()?).snapshot;
    let report = reconcile_after_restart(&records, &snapshot);
    println!(
        "{}",
        serde_json::to_string_pretty(&restart_report_value(&report))?
    );
    Ok(())
}

fn run_machine(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut collector = PlatformCollector::new();
    let mut sampler = Sampler::new(config.history_capacity);
    let projection = StructuredProjection;
    let records = match config.executions.as_deref() {
        Some(path) => load_execution_records(path)?,
        None => Vec::new(),
    };
    let correlated = !records.is_empty() || !config.watch_executables.is_empty();
    loop {
        let raw = collector.collect()?;
        let snapshot = sampler.accept(raw).snapshot;
        if correlated {
            let saturation = HostSaturation::from_snapshot(&snapshot);
            let report = correlate(&records, &snapshot, &config.watch_executables, saturation);
            println!(
                "{}",
                projection.json_with(
                    &snapshot,
                    &[("correlation", correlation_report_value(&report))]
                )?
            );
        } else {
            println!("{}", projection.json(&snapshot)?);
        }
        if config.once {
            return Ok(());
        }
        std::thread::sleep(config.interval);
    }
}

fn run_human(config: Config) -> Result<(), Box<dyn std::error::Error>> {
    let mut collector = PlatformCollector::new();
    let mut sampler = Sampler::new(config.history_capacity);
    let first = sampler.accept(collector.collect()?).snapshot;
    if config.once || config.no_tui {
        let correlation = match config.executions.as_deref() {
            Some(path) => {
                let records = load_execution_records(path)?;
                let saturation = HostSaturation::from_snapshot(&first);
                Some(correlate(
                    &records,
                    &first,
                    &config.watch_executables,
                    saturation,
                ))
            }
            None if !config.watch_executables.is_empty() => {
                let saturation = HostSaturation::from_snapshot(&first);
                Some(correlate(
                    &[],
                    &first,
                    &config.watch_executables,
                    saturation,
                ))
            }
            None => None,
        };
        print_summary(&first, correlation.as_ref());
        return Ok(());
    }

    let mut terminal = match TerminalSession::enter() {
        Ok(terminal) => terminal,
        Err(_) => {
            print_summary(&first, None);
            return Ok(());
        }
    };
    run_tui(
        &mut collector,
        &mut sampler,
        &mut terminal,
        first,
        config.interval,
    )
}

fn run_tui(
    collector: &mut PlatformCollector,
    sampler: &mut Sampler,
    terminal: &mut TerminalSession,
    mut snapshot: SystemSnapshot,
    interval: Duration,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut state = AppState::default();
    let mut next_sample = std::time::Instant::now();
    terminal.draw(&render(&snapshot, &mut state, terminal.size()))?;
    loop {
        if std::time::Instant::now() >= next_sample {
            match collector.collect() {
                Ok(raw) => {
                    snapshot = sampler.accept(raw).snapshot;
                    state.notice = None;
                }
                Err(error) => state.notice = Some(format!("collector UNKNOWN: {error}")),
            }
            next_sample = std::time::Instant::now() + interval;
            terminal.draw(&render(&snapshot, &mut state, terminal.size()))?;
        }
        match terminal.read_event(Duration::from_millis(100))? {
            Event::Tick => {}
            Event::Resize(size) => terminal.draw(&render(&snapshot, &mut state, size))?,
            Event::Key(key) => {
                if state.handle_key(key, &snapshot) {
                    return Ok(());
                }
                terminal.draw(&render(&snapshot, &mut state, terminal.size()))?;
            }
            Event::Unknown => state.notice = Some("input UNKNOWN: unsupported sequence".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortKey {
    Cpu,
    Memory,
    Pid,
    Io,
}

#[derive(Clone, Debug)]
enum View {
    Overview,
    Detail(ProcessIdentity),
}

#[derive(Clone, Debug)]
struct AppState {
    view: View,
    sort: SortKey,
    selected: usize,
    scroll: usize,
    filter: String,
    editing_filter: bool,
    notice: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            view: View::Overview,
            sort: SortKey::Cpu,
            selected: 0,
            scroll: 0,
            filter: String::new(),
            editing_filter: false,
            notice: None,
        }
    }
}

impl AppState {
    fn handle_key(&mut self, key: Key, snapshot: &SystemSnapshot) -> bool {
        if self.editing_filter {
            match key {
                Key::Enter => self.editing_filter = false,
                Key::Escape => {
                    self.filter.clear();
                    self.editing_filter = false;
                }
                Key::Backspace => {
                    self.filter.pop();
                    self.selected = 0;
                    self.scroll = 0;
                }
                Key::Character(character) if !character.is_control() => {
                    self.filter.push(character);
                    self.selected = 0;
                    self.scroll = 0;
                }
                _ => {}
            }
            return false;
        }
        match key {
            Key::Character('q') | Key::Ctrl('c') => return true,
            Key::Character('/') => self.editing_filter = true,
            Key::Character('c') => self.sort = SortKey::Cpu,
            Key::Character('m') => self.sort = SortKey::Memory,
            Key::Character('p') => self.sort = SortKey::Pid,
            Key::Character('i') => self.sort = SortKey::Io,
            Key::Escape => self.view = View::Overview,
            Key::Enter => {
                if let Some(process) = visible_processes(snapshot, self).get(self.selected) {
                    self.view = View::Detail(process.identity.clone());
                }
            }
            Key::Up => self.move_selection(snapshot, -1),
            Key::Down => self.move_selection(snapshot, 1),
            Key::PageUp => self.move_selection(snapshot, -10),
            Key::PageDown => self.move_selection(snapshot, 10),
            Key::Home => {
                self.selected = 0;
                self.scroll = 0;
            }
            Key::End => {
                self.selected = visible_processes(snapshot, self).len().saturating_sub(1);
                self.scroll = self.selected.saturating_sub(1);
            }
            _ => {}
        }
        false
    }

    fn move_selection(&mut self, snapshot: &SystemSnapshot, amount: isize) {
        let length = visible_processes(snapshot, self).len();
        if length == 0 {
            self.selected = 0;
            self.scroll = 0;
            return;
        }
        self.selected = if amount.is_negative() {
            self.selected.saturating_sub(amount.unsigned_abs())
        } else {
            self.selected
                .saturating_add(amount as usize)
                .min(length - 1)
        };
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected > self.scroll {
            self.scroll = self.selected.saturating_sub(1);
        }
    }
}

fn visible_processes<'a>(snapshot: &'a SystemSnapshot, state: &AppState) -> Vec<&'a Process> {
    let filter = state.filter.to_lowercase();
    let mut processes: Vec<&Process> = snapshot
        .processes
        .iter()
        .filter(|process| {
            filter.is_empty()
                || process.executable.to_lowercase().contains(&filter)
                || process
                    .command_line
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&filter)
                || process.identity.pid.0.to_string() == filter
        })
        .collect();
    processes.sort_by(|left, right| match state.sort {
        SortKey::Cpu => compare_f64(
            right.cpu.as_ref().and_then(|cpu| cpu.utilization),
            left.cpu.as_ref().and_then(|cpu| cpu.utilization),
        ),
        SortKey::Memory => right
            .memory
            .as_ref()
            .and_then(|memory| memory.resident_bytes)
            .cmp(
                &left
                    .memory
                    .as_ref()
                    .and_then(|memory| memory.resident_bytes),
            ),
        SortKey::Pid => left.identity.pid.cmp(&right.identity.pid),
        SortKey::Io => compare_f64(io_rate(right), io_rate(left)),
    });
    processes
}

fn compare_f64(left: Option<f64>, right: Option<f64>) -> Ordering {
    left.zip(right)
        .and_then(|(left, right)| left.partial_cmp(&right))
        .unwrap_or_else(|| right.is_some().cmp(&left.is_some()))
}

fn io_rate(process: &Process) -> Option<f64> {
    process.io.as_ref().and_then(|io| {
        match (io.read_rate_bytes_per_sec, io.written_rate_bytes_per_sec) {
            (Some(read), Some(write)) => Some(read + write),
            (Some(read), None) | (None, Some(read)) => Some(read),
            (None, None) => None,
        }
    })
}

fn render(snapshot: &SystemSnapshot, state: &mut AppState, size: Size) -> Frame {
    let mut frame = Frame::blank(size);
    let styles = Styles::new();
    frame.fill_row(
        0,
        Cell {
            glyph: ' ',
            style: styles.header,
        },
    );
    frame.write(0, 0, " mncs-system-monitor ", styles.header);
    let host = snapshot.hostname.as_deref().unwrap_or("host UNKNOWN");
    let uptime = snapshot
        .uptime
        .map(format_duration)
        .unwrap_or_else(|| "UNKNOWN".into());
    let load = snapshot
        .load_average
        .map(|load| format!("{:.2} {:.2} {:.2}", load[0], load[1], load[2]))
        .unwrap_or_else(|| "UNKNOWN".into());
    frame.write(
        22,
        0,
        &format!("{host} | up {uptime} | load {load}"),
        styles.muted,
    );
    if size.rows > 1 {
        let interval = snapshot
            .host_cpu
            .as_ref()
            .and_then(|cpu| cpu.interval)
            .map(|value| format!("{}ms", value.as_millis()))
            .unwrap_or_else(|| "warming".into());
        let cpu = snapshot
            .host_cpu
            .as_ref()
            .and_then(|cpu| cpu.utilization)
            .map(|value| format_percent(value * 100.0))
            .unwrap_or_else(|| "UNKNOWN".into());
        let memory = snapshot
            .memory_used_bytes
            .zip(snapshot.memory_total_bytes)
            .map(|(used, total)| format!("{} / {}", format_bytes(used), format_bytes(total)))
            .unwrap_or_else(|| "UNKNOWN".into());
        let swap = snapshot
            .swap_used_bytes
            .zip(snapshot.swap_total_bytes)
            .map(|(used, total)| format!("{} / {}", format_bytes(used), format_bytes(total)))
            .unwrap_or_else(|| "UNKNOWN".into());
        frame.write(
            0,
            1,
            &format!(
                " CPU {cpu}  MEM {memory}  SWAP {swap}  PROCS {}  Δ {interval}",
                snapshot.process_count()
            ),
            styles.metrics,
        );
    }
    if size.rows > 2 {
        render_history(&mut frame, snapshot, 2, styles.chart);
    }
    if size.rows > 3 {
        render_io_summary(&mut frame, snapshot, 3, styles.metrics);
    }
    match &state.view {
        View::Overview => render_process_table(&mut frame, snapshot, state, &styles),
        View::Detail(identity) => render_detail(&mut frame, snapshot, identity, styles),
    }
    let footer_row = size.rows.saturating_sub(1);
    frame.fill_row(
        footer_row,
        Cell {
            glyph: ' ',
            style: styles.footer,
        },
    );
    let filter = if state.editing_filter {
        format!("/{}", state.filter)
    } else if state.filter.is_empty() {
        "/ filter".into()
    } else {
        format!("/{}", state.filter)
    };
    frame.write(
        0,
        footer_row,
        &format!(" {filter}  c CPU  m MEM  p PID  i IO  Enter inspect  q quit"),
        styles.footer,
    );
    if let Some(notice) = &state.notice {
        frame.write(0, footer_row, &format!(" {notice}"), styles.warning);
    }
    frame
}

fn render_history(frame: &mut Frame, snapshot: &SystemSnapshot, row: u16, style: Style) {
    let width = usize::from(frame.size.columns);
    let points: Vec<f64> = snapshot
        .history
        .iter()
        .filter_map(|sample| sample.host_cpu_utilization)
        .collect();
    frame.write(0, row, " CPU history ", style);
    let start = points.len().saturating_sub(width.saturating_sub(13));
    for (offset, value) in points.iter().skip(start).enumerate() {
        let level = ((*value * 8.0).round() as usize).min(7);
        let glyph = char::from_u32(0x2581 + level as u32).unwrap_or('·');
        frame.set(12 + offset as u16, row, Cell { glyph, style });
    }
}

fn render_io_summary(frame: &mut Frame, snapshot: &SystemSnapshot, row: u16, style: Style) {
    let disk_read = sum_rates(
        snapshot
            .disk
            .iter()
            .map(|disk| disk.read_rate_bytes_per_sec),
    )
    .map(format_rate)
    .unwrap_or_else(|| "UNKNOWN".into());
    let disk_write = sum_rates(
        snapshot
            .disk
            .iter()
            .map(|disk| disk.written_rate_bytes_per_sec),
    )
    .map(format_rate)
    .unwrap_or_else(|| "UNKNOWN".into());
    let network_receive = sum_rates(
        snapshot
            .network
            .iter()
            .map(|network| network.received_rate_bytes_per_sec),
    )
    .map(format_rate)
    .unwrap_or_else(|| "UNKNOWN".into());
    let network_transmit = sum_rates(
        snapshot
            .network
            .iter()
            .map(|network| network.transmitted_rate_bytes_per_sec),
    )
    .map(format_rate)
    .unwrap_or_else(|| "UNKNOWN".into());
    frame.write(
        0,
        row,
        &format!(
            " DISK r {disk_read}  w {disk_write}  NET rx {network_receive}  tx {network_transmit}"
        ),
        style,
    );
}

fn sum_rates<I>(values: I) -> Option<f64>
where
    I: IntoIterator<Item = Option<f64>>,
{
    let mut total = 0.0;
    let mut observed = false;
    for value in values.into_iter().flatten() {
        total += value;
        observed = true;
    }
    observed.then_some(total)
}

fn render_process_table(
    frame: &mut Frame,
    snapshot: &SystemSnapshot,
    state: &mut AppState,
    styles: &Styles,
) {
    if frame.size.rows < 6 {
        return;
    }
    let processes = visible_processes(snapshot, state);
    state.selected = state.selected.min(processes.len().saturating_sub(1));
    let table_top = 5_u16;
    let table_rows = usize::from(frame.size.rows.saturating_sub(table_top + 1));
    if state.selected < state.scroll {
        state.scroll = state.selected;
    }
    if state.selected >= state.scroll + table_rows {
        state.scroll = state.selected.saturating_sub(table_rows.saturating_sub(1));
    }
    frame.write(
        0,
        table_top - 1,
        &format!(
            " Processes ({})  sort={}  filter={}",
            processes.len(),
            sort_name(state.sort),
            if state.filter.is_empty() {
                "none"
            } else {
                &state.filter
            }
        ),
        styles.section,
    );
    frame.write(
        0,
        table_top,
        " PID       CPU%    RSS       STATE  THR USER        COMMAND",
        styles.column,
    );
    for (line, process) in processes
        .iter()
        .skip(state.scroll)
        .take(table_rows)
        .enumerate()
    {
        let row = table_top + 1 + line as u16;
        let selected = state.selected == state.scroll + line;
        let style = if selected {
            styles.selected
        } else if process.state == ProcessState::Zombie {
            styles.warning
        } else {
            styles.body
        };
        let cpu = process
            .cpu
            .as_ref()
            .and_then(|cpu| cpu.utilization)
            .map(|value| format!("{:>6.1}", value * 100.0))
            .unwrap_or_else(|| "     ?".into());
        let rss = process
            .memory
            .as_ref()
            .and_then(|memory| memory.resident_bytes)
            .map(format_bytes)
            .unwrap_or_else(|| "UNKNOWN".into());
        let user = process
            .user
            .as_deref()
            .or_else(|| process.uid.map(|_| "uid"))
            .unwrap_or("UNKNOWN");
        let command = process
            .command_line
            .as_deref()
            .unwrap_or(&process.executable);
        frame.fill_row(row, Cell { glyph: ' ', style });
        frame.write(
            0,
            row,
            &format!(
                "{:>5} {:>7} {:>9} {:>6} {:>4} {:<10} {}",
                process.identity.pid.0,
                cpu,
                rss,
                state_name(process.state),
                process
                    .threads
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "?".into()),
                user,
                command
            ),
            style,
        );
    }
}

fn render_detail(
    frame: &mut Frame,
    snapshot: &SystemSnapshot,
    identity: &ProcessIdentity,
    styles: Styles,
) {
    let Some(process) = snapshot.process(identity) else {
        frame.write(
            0,
            4,
            "Process no longer observed (this does not assert a clean exit).",
            styles.warning,
        );
        return;
    };
    frame.write(
        0,
        3,
        &format!(" Process {} detail", process.identity.pid.0),
        styles.section,
    );
    frame.write(
        0,
        4,
        &format!(
            "identity: pid={} start_marker={:?}",
            process.identity.pid.0, process.identity.start_marker
        ),
        styles.body,
    );
    frame.write(
        0,
        5,
        &format!("executable: {}", process.executable),
        styles.body,
    );
    frame.write(
        0,
        6,
        &format!(
            "command line: {}",
            process.command_line.as_deref().unwrap_or("UNKNOWN")
        ),
        styles.body,
    );
    frame.write(
        0,
        7,
        &format!(
            "parent: {}   state: {}   threads: {}",
            process
                .parent
                .map(|pid| pid.0.to_string())
                .unwrap_or_else(|| "UNKNOWN".into()),
            state_name(process.state),
            process
                .threads
                .map(|value| value.to_string())
                .unwrap_or_else(|| "UNKNOWN".into())
        ),
        styles.body,
    );
    frame.write(
        0,
        8,
        &format!(
            "cpu: {}   rss: {}   virtual: {}",
            process
                .cpu
                .as_ref()
                .and_then(|cpu| cpu.utilization)
                .map(|value| format_percent(value * 100.0))
                .unwrap_or_else(|| "UNKNOWN".into()),
            process
                .memory
                .as_ref()
                .and_then(|memory| memory.resident_bytes)
                .map(format_bytes)
                .unwrap_or_else(|| "UNKNOWN".into()),
            process
                .memory
                .as_ref()
                .and_then(|memory| memory.virtual_bytes)
                .map(format_bytes)
                .unwrap_or_else(|| "UNKNOWN".into())
        ),
        styles.body,
    );
    frame.write(
        0,
        9,
        &format!(
            "I/O: read {}  write {}",
            process
                .io
                .as_ref()
                .and_then(|io| io.read_rate_bytes_per_sec)
                .map(format_rate)
                .unwrap_or_else(|| "UNKNOWN".into()),
            process
                .io
                .as_ref()
                .and_then(|io| io.written_rate_bytes_per_sec)
                .map(format_rate)
                .unwrap_or_else(|| "UNKNOWN".into())
        ),
        styles.body,
    );
    let history_count = snapshot
        .history
        .iter()
        .flat_map(|sample| sample.processes.iter())
        .filter(|point| &point.identity == identity)
        .count();
    frame.write(
        0,
        11,
        &format!("bounded recent observations: {history_count}"),
        styles.muted,
    );
    let lifecycle = snapshot
        .transitions
        .iter()
        .find(|transition| transition.pid == identity.pid)
        .map(|transition| format!("{:?}", transition.lifecycle))
        .unwrap_or_else(|| "Same/unknown".into());
    frame.write(
        0,
        12,
        &format!("latest lifecycle evidence: {lifecycle}"),
        styles.muted,
    );
}

#[derive(Clone, Copy)]
struct Styles {
    header: Style,
    metrics: Style,
    chart: Style,
    section: Style,
    column: Style,
    body: Style,
    selected: Style,
    muted: Style,
    footer: Style,
    warning: Style,
}

impl Styles {
    fn new() -> Self {
        Self {
            header: Style {
                foreground: Some(Color::BrightWhite),
                background: Some(Color::Blue),
                bold: true,
                ..Style::default()
            },
            metrics: Style {
                foreground: Some(Color::BrightCyan),
                bold: true,
                ..Style::default()
            },
            chart: Style {
                foreground: Some(Color::Green),
                ..Style::default()
            },
            section: Style {
                foreground: Some(Color::BrightYellow),
                bold: true,
                ..Style::default()
            },
            column: Style {
                foreground: Some(Color::BrightCyan),
                bold: true,
                ..Style::default()
            },
            body: Style::default(),
            selected: Style {
                foreground: Some(Color::Black),
                background: Some(Color::BrightCyan),
                bold: true,
                ..Style::default()
            },
            muted: Style {
                foreground: Some(Color::BrightBlack),
                ..Style::default()
            },
            footer: Style {
                foreground: Some(Color::BrightWhite),
                background: Some(Color::BrightBlack),
                ..Style::default()
            },
            warning: Style {
                foreground: Some(Color::BrightRed),
                bold: true,
                ..Style::default()
            },
        }
    }
}

fn sort_name(sort: SortKey) -> &'static str {
    match sort {
        SortKey::Cpu => "CPU",
        SortKey::Memory => "MEM",
        SortKey::Pid => "PID",
        SortKey::Io => "IO",
    }
}

fn state_name(state: ProcessState) -> &'static str {
    match state {
        ProcessState::Running => "RUN",
        ProcessState::Sleeping => "SLEEP",
        ProcessState::Stopped => "STOP",
        ProcessState::Zombie => "ZOMB",
        ProcessState::Dead => "DEAD",
        ProcessState::Unknown => "?",
    }
}

fn format_percent(value: f64) -> String {
    if value.is_finite() {
        format!("{value:.1}%")
    } else {
        "UNKNOWN".into()
    }
}

fn format_rate(value: f64) -> String {
    format!("{}/s", format_bytes(value.max(0.0) as u64))
}

fn format_bytes(value: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut number = value as f64;
    let mut unit = 0;
    while number >= 1024.0 && unit < UNITS.len() - 1 {
        number /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", number as u64, UNITS[unit])
    } else {
        format!("{number:.1} {}", UNITS[unit])
    }
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let seconds = seconds % 60;
    if days > 0 {
        format!("{days}d {hours:02}h")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}

fn print_summary(snapshot: &SystemSnapshot, correlation: Option<&CorrelationReport>) {
    println!("mncs-system-monitor");
    println!(
        "host: {}",
        snapshot.hostname.as_deref().unwrap_or("UNKNOWN")
    );
    println!(
        "uptime: {} | load: {} | processes: {}",
        snapshot
            .uptime
            .map(format_duration)
            .unwrap_or_else(|| "UNKNOWN".into()),
        snapshot
            .load_average
            .map(|load| format!("{:.2} {:.2} {:.2}", load[0], load[1], load[2]))
            .unwrap_or_else(|| "UNKNOWN".into()),
        snapshot.process_count()
    );
    println!(
        "cpu: {} | memory: {} / {} | swap: {} / {}",
        snapshot
            .host_cpu
            .as_ref()
            .and_then(|cpu| cpu.utilization)
            .map(|value| format_percent(value * 100.0))
            .unwrap_or_else(|| "warming/UNKNOWN".into()),
        snapshot
            .memory_used_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "UNKNOWN".into()),
        snapshot
            .memory_total_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "UNKNOWN".into()),
        snapshot
            .swap_used_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "UNKNOWN".into()),
        snapshot
            .swap_total_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "UNKNOWN".into())
    );
    let state = AppState::default();
    for process in visible_processes(snapshot, &state).into_iter().take(10) {
        println!(
            "{:>6} {:>6} {:>10} {}",
            process.identity.pid.0,
            process
                .cpu
                .as_ref()
                .and_then(|cpu| cpu.utilization)
                .map(|value| format_percent(value * 100.0))
                .unwrap_or_else(|| "UNKNOWN".into()),
            process
                .memory
                .as_ref()
                .and_then(|memory| memory.resident_bytes)
                .map(format_bytes)
                .unwrap_or_else(|| "UNKNOWN".into()),
            process
                .command_line
                .as_deref()
                .unwrap_or(&process.executable)
        );
    }
    if !snapshot.issues.is_empty() {
        println!(
            "issues: {} (see --json for evidence details)",
            snapshot.issues.len()
        );
    }
    if let Some(report) = correlation {
        let linked = report
            .links
            .iter()
            .filter(|link| link.process.is_some())
            .count();
        println!(
            "executions: {} linked: {} anomalies: {}",
            report.links.len(),
            linked,
            report.anomalies.len()
        );
        for anomaly in report.anomalies.iter().take(5) {
            println!("anomaly: {}", anomaly.detail);
        }
        if report.anomalies.len() > 5 {
            println!("anomaly: ... ({} more)", report.anomalies.len() - 5);
        }
    }
}

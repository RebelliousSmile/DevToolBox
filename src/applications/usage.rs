//! Privacy-preserving, prospective application usage tracking.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::ProcessProvider;

const HISTORY_VERSION: u32 = 1;
const MAX_TARGETS: usize = 4_096;
const MAX_OBSERVED_PATHS: usize = 32_768;
const COVERAGE_RETENTION_DAYS: usize = 400;
const SAMPLE_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsageTarget {
    pub app_id: String,
    pub executable_hints: Vec<PathBuf>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AppUsage {
    tracked_since: String,
    last_seen: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct UsageHistory {
    version: u32,
    apps: BTreeMap<String, AppUsage>,
    coverage: BTreeMap<String, u32>,
}

impl Default for UsageHistory {
    fn default() -> Self {
        Self {
            version: HISTORY_VERSION,
            apps: BTreeMap::new(),
            coverage: BTreeMap::new(),
        }
    }
}

fn load_history(path: &Path) -> UsageHistory {
    let Ok(text) = std::fs::read_to_string(path) else {
        return UsageHistory::default();
    };
    match serde_json::from_str::<UsageHistory>(&text) {
        Ok(history) if history.version == HISTORY_VERSION => history,
        _ => UsageHistory::default(),
    }
}

fn save_history(path: &Path, history: &UsageHistory) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let payload = serde_json::to_vec_pretty(history).map_err(io::Error::other)?;
    {
        use std::io::Write as _;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&payload)?;
        file.sync_all()?;
    }
    std::fs::rename(temporary, path)
}

fn normalize_for_platform(path: &Path, windows: bool) -> String {
    if windows {
        return path.to_string_lossy().replace('/', "\\").to_lowercase();
    }
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

fn normalize_executable(path: &Path) -> String {
    normalize_for_platform(path, cfg!(windows))
}

struct UsageTracker {
    path: PathBuf,
    provider: Arc<dyn ProcessProvider>,
    history: UsageHistory,
    targets_by_path: HashMap<String, Option<String>>,
}

impl UsageTracker {
    fn new(path: PathBuf, provider: Arc<dyn ProcessProvider>) -> Self {
        let history = load_history(&path);
        Self {
            path,
            provider,
            history,
            targets_by_path: HashMap::new(),
        }
    }

    fn replace_targets_at(&mut self, targets: Vec<UsageTarget>, timestamp: &str) {
        let mut paths: HashMap<String, Option<String>> = HashMap::new();
        for target in targets.into_iter().take(MAX_TARGETS) {
            if target.app_id.trim().is_empty() {
                continue;
            }
            self.history
                .apps
                .entry(target.app_id.clone())
                .or_insert_with(|| AppUsage {
                    tracked_since: timestamp.to_string(),
                    last_seen: None,
                });
            for hint in target.executable_hints {
                let key = normalize_executable(&hint);
                if key.is_empty() {
                    continue;
                }
                match paths.entry(key) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(Some(target.app_id.clone()));
                    }
                    std::collections::hash_map::Entry::Occupied(mut entry) => {
                        if entry.get().as_deref() != Some(target.app_id.as_str()) {
                            entry.insert(None);
                        }
                    }
                }
            }
        }
        self.targets_by_path = paths;
    }

    fn sample_at(&mut self, timestamp: &str) -> io::Result<()> {
        if self.targets_by_path.values().all(Option::is_none) {
            return Ok(());
        }
        let paths = self.provider.executable_paths()?;
        let mut seen_apps: std::collections::HashSet<String> = std::collections::HashSet::new();
        for path in paths.into_iter().take(MAX_OBSERVED_PATHS) {
            let normalized = normalize_executable(&path);
            if let Some(Some(app_id)) = self.targets_by_path.get(&normalized) {
                seen_apps.insert(app_id.clone());
            }
        }
        let day = timestamp
            .get(..10)
            .ok_or_else(|| io::Error::other("invalid UTC timestamp"))?;
        *self.history.coverage.entry(day.to_string()).or_insert(0) += 1;
        while self.history.coverage.len() > COVERAGE_RETENTION_DAYS {
            self.history.coverage.pop_first();
        }
        for app_id in seen_apps {
            if let Some(entry) = self.history.apps.get_mut(&app_id) {
                if entry
                    .last_seen
                    .as_deref()
                    .is_none_or(|previous| timestamp > previous)
                {
                    entry.last_seen = Some(timestamp.to_string());
                }
            }
        }
        save_history(&self.path, &self.history)
    }
}

enum WorkerCommand {
    Replace(Vec<UsageTarget>),
    Stop,
}

pub struct UsageService {
    sender: Sender<WorkerCommand>,
    worker: Option<JoinHandle<()>>,
}

impl UsageService {
    pub fn start(path: PathBuf, provider: Arc<dyn ProcessProvider>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut tracker = UsageTracker::new(path, provider);
            loop {
                match receiver.recv_timeout(SAMPLE_INTERVAL) {
                    Ok(WorkerCommand::Replace(targets)) => {
                        let now = utc_now();
                        tracker.replace_targets_at(targets, &now);
                        let _ = save_history(&tracker.path, &tracker.history);
                        let _ = tracker.sample_at(&now);
                    }
                    Ok(WorkerCommand::Stop) => break,
                    Err(RecvTimeoutError::Timeout) => {
                        let _ = tracker.sample_at(&utc_now());
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            sender,
            worker: Some(worker),
        }
    }

    pub fn replace_targets(&self, targets: Vec<UsageTarget>) -> Result<(), String> {
        self.sender
            .send(WorkerCommand::Replace(targets))
            .map_err(|error| error.to_string())
    }
}

impl Drop for UsageService {
    fn drop(&mut self) {
        let _ = self.sender.send(WorkerCommand::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn utc_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    format_unix_utc(seconds)
}

fn format_unix_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct FakeProvider {
        calls: AtomicUsize,
        paths: Mutex<io::Result<Vec<PathBuf>>>,
    }

    impl FakeProvider {
        fn success(paths: Vec<PathBuf>) -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                paths: Mutex::new(Ok(paths)),
            })
        }

        fn failure() -> Arc<Self> {
            Arc::new(Self {
                calls: AtomicUsize::new(0),
                paths: Mutex::new(Err(io::Error::other("unavailable"))),
            })
        }
    }

    impl ProcessProvider for FakeProvider {
        fn executable_paths(&self) -> io::Result<Vec<PathBuf>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let guard = self.paths.lock().unwrap();
            match &*guard {
                Ok(paths) => Ok(paths.clone()),
                Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
            }
        }
    }

    /// The thread name is what keeps two concurrently running tests from
    /// sharing a file, but under the libtest harness it is the *full test
    /// path* (`applications::usage::tests::…`). The `:` in it is illegal in
    /// a Windows filename, so the raw name has to be reduced to characters
    /// every platform accepts before it lands in a path — otherwise every
    /// write here fails with `ERROR_INVALID_NAME` (OS error 123).
    fn temp_path(label: &str) -> PathBuf {
        let thread = std::thread::current();
        let scope: String = thread
            .name()
            .unwrap_or("test")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        std::env::temp_dir().join(format!(
            "devtoolbox-usage-{label}-{}-{scope}.json",
            std::process::id(),
        ))
    }

    #[test]
    fn utc_formatter_has_known_epoch_and_leap_day() {
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix_utc(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn windows_normalization_is_case_insensitive_and_separator_stable() {
        assert_eq!(
            normalize_for_platform(Path::new("C:/Apps/Editor.EXE"), true),
            "c:\\apps\\editor.exe"
        );
    }

    #[test]
    fn no_target_performs_no_process_observation() {
        let provider = FakeProvider::success(vec![PathBuf::from("/usr/bin/editor")]);
        let mut tracker = UsageTracker::new(temp_path("empty"), provider.clone());
        tracker.sample_at("2026-01-01T00:00:00Z").unwrap();
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn exact_match_updates_last_seen_without_persisting_paths() {
        let path = temp_path("exact");
        let executable = std::env::current_exe().unwrap();
        let provider = FakeProvider::success(vec![executable.clone()]);
        let mut tracker = UsageTracker::new(path.clone(), provider);
        tracker.replace_targets_at(
            vec![UsageTarget {
                app_id: "apt:editor".to_string(),
                executable_hints: vec![executable],
            }],
            "2026-01-01T00:00:00Z",
        );
        tracker.sample_at("2026-01-02T00:00:00Z").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("apt:editor"));
        assert!(text.contains("2026-01-02T00:00:00Z"));
        assert!(!text.contains("executable"));
        assert!(!text.contains("pid"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ambiguous_path_updates_neither_application() {
        let path = temp_path("ambiguous");
        let executable = std::env::current_exe().unwrap();
        let provider = FakeProvider::success(vec![executable.clone()]);
        let mut tracker = UsageTracker::new(path.clone(), provider);
        tracker.replace_targets_at(
            vec![
                UsageTarget {
                    app_id: "apt:first".to_string(),
                    executable_hints: vec![executable.clone()],
                },
                UsageTarget {
                    app_id: "apt:second".to_string(),
                    executable_hints: vec![executable],
                },
            ],
            "2026-01-01T00:00:00Z",
        );
        tracker.sample_at("2026-01-02T00:00:00Z").unwrap();
        assert!(tracker
            .history
            .apps
            .values()
            .all(|entry| entry.last_seen.is_none()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn older_sample_never_moves_last_seen_backwards() {
        let path = temp_path("clock");
        let executable = std::env::current_exe().unwrap();
        let provider = FakeProvider::success(vec![executable.clone()]);
        let mut tracker = UsageTracker::new(path.clone(), provider);
        tracker.replace_targets_at(
            vec![UsageTarget {
                app_id: "apt:editor".to_string(),
                executable_hints: vec![executable],
            }],
            "2026-01-01T00:00:00Z",
        );
        tracker.sample_at("2026-02-01T00:00:00Z").unwrap();
        tracker.sample_at("2026-01-15T00:00:00Z").unwrap();
        assert_eq!(
            tracker.history.apps["apt:editor"].last_seen.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn coverage_is_success_only_and_retained_for_400_days() {
        let path = temp_path("retention");
        let executable = std::env::current_exe().unwrap();
        let provider = FakeProvider::success(vec![]);
        let mut tracker = UsageTracker::new(path.clone(), provider);
        tracker.replace_targets_at(
            vec![UsageTarget {
                app_id: "apt:editor".to_string(),
                executable_hints: vec![executable],
            }],
            "2025-01-01T00:00:00Z",
        );
        for day in 1..=401 {
            tracker
                .sample_at(&format_unix_utc(1_735_689_600 + day * 86_400))
                .unwrap();
        }
        assert_eq!(tracker.history.coverage.len(), 400);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_observation_adds_no_coverage() {
        let path = temp_path("failed");
        let executable = std::env::current_exe().unwrap();
        let provider = FakeProvider::failure();
        let mut tracker = UsageTracker::new(path.clone(), provider);
        tracker.replace_targets_at(
            vec![UsageTarget {
                app_id: "apt:editor".to_string(),
                executable_hints: vec![executable],
            }],
            "2026-01-01T00:00:00Z",
        );
        assert!(tracker.sample_at("2026-01-02T00:00:00Z").is_err());
        assert!(tracker.history.coverage.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replacing_service_targets_does_not_wait_for_process_scan() {
        struct SlowProvider;
        impl ProcessProvider for SlowProvider {
            fn executable_paths(&self) -> io::Result<Vec<PathBuf>> {
                std::thread::sleep(Duration::from_millis(200));
                Ok(vec![])
            }
        }
        let path = temp_path("async");
        let service = UsageService::start(path.clone(), Arc::new(SlowProvider));
        let started = std::time::Instant::now();
        service
            .replace_targets(vec![UsageTarget {
                app_id: "apt:editor".to_string(),
                executable_hints: vec![std::env::current_exe().unwrap()],
            }])
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
        drop(service);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn corrupt_history_and_unwritable_destination_do_not_panic() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "not json").unwrap();
        let provider = FakeProvider::success(vec![]);
        let tracker = UsageTracker::new(path.clone(), provider);
        assert!(tracker.history.apps.is_empty());
        let directory_as_file = path.with_extension("directory");
        std::fs::create_dir_all(&directory_as_file).unwrap();
        assert!(save_history(&directory_as_file, &UsageHistory::default()).is_err());
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_dir_all(directory_as_file);
    }
}

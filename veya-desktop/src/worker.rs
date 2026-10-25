//! Capture worker: platform events → enrich → FlowEngine → SQLite + UI snapshot.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use veya_core::{
    payload_hash, ClipboardPayload, ClipboardRecord, FlowEngine, InternalClipboardWrite,
    PasteConfidence, PasteTrigger, PasteTriggerRecord, SourceConfidence,
};
use veya_storage::Store;
use veya_windows::hotkey::Hotkey;
use veya_windows::platform::{enrich_clipboard, enrich_paste, PlatformEvent, WindowSignal};
use veya_windows::{write_payload, ClipboardWriteError};

use crate::capture::{
    CardPayloadView, CardView, ExpiryConfig, GroupSummary, HistoryQuery, MaxItemsConfig,
    QuickSettings, SharedUi, ThemeMode, UiState,
};
use crate::format::now_ms;

#[path = "history_loader.rs"]
mod history_loader;

const HISTORY_PAGE_SIZE: usize = 25;
const HISTORY_READ_BATCH: usize = 16;

pub enum WorkerCmd {
    Recopy {
        sequence: u32,
    },
    PreparePaste {
        sequence: u32,
        request_id: u64,
    },
    RecopyPlainText {
        sequence: u32,
    },
    LoadContent(u32),
    UnloadContent,
    ClearHistory,
    SetTracking {
        on: bool,
        request_id: Option<u64>,
    },
    SetThemeMode(ThemeMode),
    SetMaxItems(MaxItemsConfig),
    SetExpiry(ExpiryConfig),
    CreateGroup {
        name: String,
        /// Record to put into the new group right away (from a card's context menu).
        add_sequence: Option<u32>,
    },
    RenameGroup {
        id: i64,
        name: String,
    },
    DeleteGroup {
        id: i64,
    },
    AddToGroup {
        group_id: i64,
        sequence: u32,
    },
    RemoveFromGroup {
        group_id: i64,
        sequences: Vec<u32>,
    },
    ExcludeApp {
        exe: String,
    },
    UnexcludeApp {
        exe: String,
    },
    DeleteRecord {
        sequences: Vec<u32>,
    },
    SetPinned {
        sequences: Vec<u32>,
        pinned: bool,
    },
    /// Update the text content of an existing text record.
    UpdateRecordText {
        sequence: u32,
        text: String,
    },
    /// Insert a new text record into the database and flow engine.
    CreateTextRecord {
        text: String,
        created_at_ms: i64,
    },
    /// Set pin_rank for a record: 0 = unpinned, 1 = top, 2 = bottom.
    SetPinRank {
        sequence: u32,
        rank: u8,
    },
    /// Set the per-item global hotkey code string (empty = clear).
    SetItemHotkey {
        sequence: u32,
        code: String,
    },
    /// Toggle the auto-delete flag for a record.
    SetAutoDelete {
        sequence: u32,
        enabled: bool,
    },
    /// Move a record to the top of history (touch with current timestamp).
    MoveToTop {
        sequence: u32,
        at_ms: i64,
    },
    /// Move a record to the bottom of history.
    MoveToBottom {
        sequence: u32,
    },
    /// Replace the whole database with a backup file (after an automatic safety copy).
    RestoreDatabase {
        path: PathBuf,
    },
    SetExpandedRaw(bool),
    SetHotkey(Hotkey),
    SetQuickSettings(crate::capture::QuickSettings),
    QueryHistory(HistoryQuery),
    SetHistoryActive(bool),
    LoadFullImage(u32),
    UnloadFullImage,
}

/// What a successful restore did, for the confirmation message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreReport {
    pub records: usize,
    pub groups: usize,
    /// Copy of the data that was replaced, kept so the restore can be undone.
    pub safety_copy: PathBuf,
}

pub enum WorkerEvent {
    PastePrepared {
        request_id: u64,
        result: Result<u32, String>,
    },
    DatabaseRestored(Result<RestoreReport, String>),
}

pub struct WorkerHandle {
    pub cmd_tx: Sender<WorkerCmd>,
    pub event_rx: Receiver<WorkerEvent>,
}

pub fn spawn_worker(shared: SharedUi, activate_tx: Sender<WindowSignal>) -> WorkerHandle {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<WorkerCmd>();
    let (event_tx, event_rx) = std::sync::mpsc::channel::<WorkerEvent>();
    std::thread::Builder::new()
        .name("veya-capture".into())
        .spawn(move || run_worker(shared, cmd_rx, activate_tx, event_tx))
        .expect("spawn capture worker");
    WorkerHandle { cmd_tx, event_rx }
}

fn db_path() -> std::path::PathBuf {
    std::env::var_os("APPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("Veya")
        .join("veya.db")
}

/// Location of the live database, for backups made outside the worker.
pub fn database_path() -> PathBuf {
    db_path()
}

/// Automatic copies made before a restore are kept in a `backups` folder beside the
/// database; only this many of the newest ones are retained.
const SAFETY_COPIES_KEPT: usize = 5;
const SAFETY_COPY_PREFIX: &str = "veya-before-restore-";

fn load_theme_mode(store: &Store) -> ThemeMode {
    store
        .get_setting("theme")
        .ok()
        .flatten()
        .map(|s| ThemeMode::from_key(&s))
        .unwrap_or_default()
}

fn load_limits(store: &Store) -> (MaxItemsConfig, ExpiryConfig) {
    let setting = |key: &str| store.get_setting(key).ok().flatten();
    (
        MaxItemsConfig::from_settings(
            setting("max_items_enabled").as_deref(),
            setting("max_items").as_deref(),
        ),
        ExpiryConfig::from_settings(
            setting("expiry_enabled").as_deref(),
            setting("expiry_days").as_deref(),
            setting("retention").as_deref(),
        ),
    )
}

/// Load quick paste panel settings from the store; missing keys fall back to defaults.
fn load_quick_settings(store: &Store) -> QuickSettings {
    use crate::capture::QuickSettings;
    let setting = |key: &str| store.get_setting(key).ok().flatten();
    QuickSettings::from_settings(
        setting("quick_move_to_top").as_deref(),
        setting("quick_display_lines").as_deref(),
        setting("quick_popup_position").as_deref(),
        setting("quick_always_on_top").as_deref(),
        setting("quick_auto_dismiss").as_deref(),
        setting("quick_opacity").as_deref(),
        setting("quick_title_position").as_deref(),
        setting("quick_font_size").as_deref(),
        setting("quick_font_family").as_deref(),
        setting("quick_hover_expand_enabled").as_deref(),
    )
}

/// Delete a record that is marked "auto delete" once it has been pasted.
fn delete_after_paste(
    store: &mut Store,
    flow: &mut FlowEngine,
    sequence: u32,
) -> Result<bool, String> {
    if !flow
        .record(sequence)
        .is_some_and(|record| record.auto_delete)
    {
        return Ok(false);
    }
    store
        .delete_records(&[sequence])
        .map_err(|error| format!("粘贴后自动删除失败：{error}"))?;
    flow.delete_record(sequence);
    Ok(true)
}

/// Delete all but the newest `keep` safety copies. The timestamp in the name sorts
/// chronologically.
fn prune_safety_copies(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut copies: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SAFETY_COPY_PREFIX) && name.ends_with(".db"))
        })
        .collect();
    copies.sort();
    let excess = copies.len().saturating_sub(keep);
    for old in copies.into_iter().take(excess) {
        let _ = std::fs::remove_file(old);
    }
}

/// Replace the live database with `backup`.
///
/// Order matters: the file is validated, then the current data is copied to a safety
/// file (abort if that fails), and only then is the database replaced. The hotkey is a
/// per-machine setting that is registered with the system, so it is kept as it was.
fn restore_database(
    store: &mut Store,
    live: &Path,
    backup: &Path,
    stamp: &str,
) -> Result<RestoreReport, String> {
    if let (Ok(a), Ok(b)) = (live.canonicalize(), backup.canonicalize()) {
        if a == b {
            return Err("不能用当前正在使用的数据库文件自身来还原".into());
        }
    }
    Store::inspect_backup(backup)?;
    let dir = live
        .parent()
        .ok_or_else(|| "无法确定数据库所在目录".to_string())?
        .join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建备份目录：{e}"))?;
    let safety_copy = dir.join(format!("{SAFETY_COPY_PREFIX}{stamp}.db"));
    Store::backup_file(live, &safety_copy)
        .map_err(|e| format!("还原前无法备份当前数据，已取消还原：{e}"))?;

    let hotkey = store.get_setting("window_hotkey").ok().flatten();
    let info = store.restore_from(backup)?;
    let _ = match hotkey {
        Some(value) => store.set_setting("window_hotkey", &value),
        None => store.remove_setting("window_hotkey"),
    };
    prune_safety_copies(&dir, SAFETY_COPIES_KEPT);
    Ok(RestoreReport {
        records: info.records,
        groups: info.groups,
        safety_copy,
    })
}

/// The colour theme saved in the database, read before the worker starts so the first frame
/// already uses it. Falls back to the default theme when nothing is saved yet.
pub fn saved_theme_mode() -> ThemeMode {
    Store::read_setting_readonly(db_path(), "theme")
        .map(|value| ThemeMode::from_key(&value))
        .unwrap_or_default()
}

fn write_internal_payload(
    flow: &mut FlowEngine,
    payload: &ClipboardPayload,
    writer: impl FnOnce(&ClipboardPayload) -> Result<u32, ClipboardWriteError>,
) -> Result<u32, String> {
    let hash = payload_hash(payload);
    // Arm before touching the clipboard. Windows may publish the update with
    // a sequence newer than the value observed inside write_unicode_text.
    flow.begin_internal_write(InternalClipboardWrite {
        expected_sequence: None,
        hash,
    });
    match writer(payload) {
        Ok(sequence) => {
            flow.confirm_internal_write(sequence, std::process::id());
            Ok(sequence)
        }
        Err(error) => {
            if error.changed {
                flow.fail_internal_write_after_change(std::process::id());
            } else {
                flow.cancel_internal_write();
            }
            Err(error.message)
        }
    }
}

fn load_replay_record(
    flow: &FlowEngine,
    store: &Store,
    sequence: u32,
) -> Result<ClipboardRecord, String> {
    if let Some(record) = flow.record(sequence) {
        return Ok(record.clone());
    }
    store
        .load_record(sequence)
        .map_err(|error| format!("读取记录失败：{error}"))?
        .ok_or_else(|| "记录已不存在".into())
}

fn recopy_plain_text(
    flow: &mut FlowEngine,
    store: &Store,
    sequence: u32,
    writer: impl FnOnce(&ClipboardPayload) -> Result<u32, ClipboardWriteError>,
) -> Result<u32, String> {
    let text = store
        .load_content(sequence)
        .map_err(|error| format!("读取记录失败：{error}"))?
        .ok_or_else(|| "记录已不存在".to_string())?;
    write_internal_payload(flow, &ClipboardPayload::Text(text), writer)
}

fn prepare_record_paste(
    flow: &mut FlowEngine,
    store: &Store,
    sequence: u32,
    writer: impl FnOnce(&ClipboardPayload) -> Result<u32, ClipboardWriteError>,
) -> Result<u32, String> {
    let record = load_replay_record(flow, store, sequence)?;
    let written_sequence = write_internal_payload(flow, &record.payload, writer)?;
    flow.activate_replayed_record(record);
    discard_inactive_records(flow, Some(sequence));
    Ok(written_sequence)
}

#[cfg(test)]
fn load_history_page(store: &Store, query: &HistoryQuery) -> Result<(Vec<CardView>, bool), String> {
    history_loader::load_page(store, query, false, &|| true)?.ok_or_else(|| "cancelled".into())
}

fn persist_paste_trigger(
    store: &mut Store,
    flow: &mut FlowEngine,
    trigger: PasteTrigger,
) -> Result<bool, String> {
    let Some(sequence) = flow
        .current_sequence()
        .filter(|seq| flow.record(*seq).is_some())
    else {
        return Ok(false);
    };
    let paste = PasteTriggerRecord {
        target_app: trigger.target_exe.clone(),
        target_pid: trigger.target_pid,
        target_window: trigger.target_window.clone(),
        method: trigger.method,
        confidence: PasteConfidence::HotkeyObserved,
        triggered_at_ms: trigger.timestamp_ms,
    };
    // Keep core and persisted history in agreement if this write fails.
    store
        .append_paste(sequence, &paste)
        .map_err(|error| format!("粘贴记录保存失败：{error}"))?;
    let outcome = flow.on_paste_trigger(trigger);
    debug_assert_eq!(outcome, veya_core::FlowOutcome::PasteAttached { sequence });
    Ok(true)
}

fn discard_inactive_records(flow: &mut FlowEngine, keep: Option<u32>) {
    let stale: Vec<_> = flow
        .records()
        .map(|record| record.sequence)
        .filter(|sequence| Some(*sequence) != keep)
        .collect();
    for sequence in stale {
        flow.delete_record(sequence);
    }
}

fn discard_purged_record(store: &Store, flow: &mut FlowEngine) {
    let current = flow.records().next().map(|record| record.sequence);
    if let Some(sequence) = current {
        if matches!(store.load_record(sequence), Ok(None)) {
            flow.delete_record(sequence);
        }
    }
}

/// Apply the expiry and maximum-count settings. Pinned and grouped records are never removed.
/// Returns how many records were deleted.
fn enforce_limits(
    store: &mut Store,
    expiry: ExpiryConfig,
    max_items: MaxItemsConfig,
    now_ms: i64,
) -> usize {
    let mut removed = 0;
    if expiry.enabled {
        let cutoff = now_ms - i64::from(expiry.days) * 86_400_000;
        removed += store.purge_older_than(cutoff).unwrap_or(0);
    }
    if max_items.enabled {
        removed += store
            .trim_to_max_count(max_items.count as usize)
            .unwrap_or(0);
    }
    removed
}

fn limits_note(name: &str, enabled: bool, value: String, removed: usize) -> String {
    let mut note = if enabled {
        format!("{name}已设为 {value}")
    } else {
        format!("{name}已关闭")
    };
    if removed > 0 {
        note.push_str(&format!("，已清理 {removed} 条记录"));
    }
    note
}

/// Owns only the current immutable page; status updates can share it without
/// rereading payloads or cloning each card's full text.
#[derive(Default)]
struct HistoryPage {
    loader: Option<history_loader::Loader>,
    generation: u64,
    query: Option<HistoryQuery>,
    cards: Arc<[CardView]>,
    has_next: bool,
    error: Option<String>,
    invalidated: bool,
    loading: bool,
    /// True when the in-flight request appends the next page to `cards`
    /// instead of replacing them. Set by `refresh` when only `page` changed,
    /// consumed by `apply`.
    appending: bool,
    detail_content: Option<(u32, Arc<str>)>,
    detail_error: Option<(u32, String)>,
}

impl HistoryPage {
    fn invalidate(&mut self) {
        self.invalidated = true;
        if let Some(loader) = &self.loader {
            self.generation = loader.cancel();
        }
    }

    fn refresh(&mut self, _store: &Store, query: &HistoryQuery, active: bool) {
        if !active {
            if self.query.is_some() || self.loading {
                if let Some(loader) = &self.loader {
                    self.generation = loader.cancel();
                }
            }
            self.query = None;
            self.cards = Arc::default();
            self.has_next = false;
            self.error = None;
            self.loading = false;
            self.appending = false;
            self.detail_content = None;
            self.detail_error = None;
            return;
        }
        // Infinite scroll: when only `page` changed (next page, same filters),
        // keep accumulated cards and append the incoming page instead of resetting.
        let is_append = !self.invalidated
            && self.query.as_ref().is_some_and(|prev| {
                prev.search == query.search
                    && prev.filter == query.filter
                    && prev.pinned_only == query.pinned_only
                    && prev.newest_first == query.newest_first
                    && prev.group_id == query.group_id
                    && prev.page.wrapping_add(1) == query.page
            });
        if is_append {
            self.loading = true;
            self.appending = true;
            self.error = None;
            if let Some(loader) = &self.loader {
                self.generation = loader.request(query);
            }
            self.query = Some(query.clone());
        } else if self.invalidated || self.query.as_ref() != Some(query) {
            self.cards = Arc::default();
            self.has_next = false;
            self.error = None;
            self.loading = true;
            self.appending = false;
            if let Some(loader) = &self.loader {
                self.generation = loader.request_through(query);
            }
            self.query = Some(query.clone());
            self.invalidated = false;
        }
    }

    fn receive(&mut self) -> bool {
        let mut changed = false;
        loop {
            let Some(result) = self
                .loader
                .as_ref()
                .and_then(|loader| loader.results.try_recv().ok())
            else {
                break;
            };
            let initial_cards = matches!(result, history_loader::ResultEvent::Cards { .. });
            changed |= self.apply(result);
            // Publish text cards independently before consuming decoded images.
            if changed && initial_cards {
                break;
            }
        }
        changed
    }

    fn apply(&mut self, result: history_loader::ResultEvent) -> bool {
        if self.query.is_none() || self.invalidated {
            return false;
        }
        match result {
            history_loader::ResultEvent::Cards {
                generation,
                cards,
                has_next,
            } if generation == self.generation => {
                if self.appending {
                    // Accumulate: extend the existing cards with the new page.
                    let mut combined = Vec::with_capacity(self.cards.len() + cards.len());
                    combined.extend(self.cards.iter().cloned());
                    combined.extend(cards);
                    self.cards = combined.into();
                    self.appending = false;
                } else {
                    self.cards = cards.into();
                }
                self.has_next = has_next;
                self.loading = false;
                self.error = None;
                true
            }
            history_loader::ResultEvent::Thumbnail {
                generation,
                sequence,
                hash,
                handle,
            } if generation == self.generation => {
                if let Some(index) = self
                    .cards
                    .iter()
                    .position(|c| c.sequence == sequence && c.content_hash == hash)
                {
                    if let CardPayloadView::Image { handle: slot, .. } =
                        &mut Arc::make_mut(&mut self.cards)[index].payload
                    {
                        *slot = Some(handle);
                        return true;
                    }
                }
                false
            }
            history_loader::ResultEvent::Error {
                generation,
                message,
            } if generation == self.generation => {
                self.loading = false;
                self.error = Some(message);
                true
            }
            history_loader::ResultEvent::ThumbnailUnavailable {
                generation,
                sequence,
                hash,
            } if generation == self.generation => {
                if let Some(index) = self
                    .cards
                    .iter()
                    .position(|c| c.sequence == sequence && c.content_hash == hash)
                {
                    if let CardPayloadView::Image {
                        thumbnail_failed, ..
                    } = &mut Arc::make_mut(&mut self.cards)[index].payload
                    {
                        *thumbnail_failed = true;
                        return true;
                    }
                }
                false
            }
            _ => false,
        }
    }
}

fn publish(
    shared: &SharedUi,
    store: &Store,
    flow: &FlowEngine,
    history_query: &HistoryQuery,
    history_active: bool,
    history_page: &mut HistoryPage,
    modal_image: &Option<(u32, iced::widget::image::Handle)>,
    tracking: bool,
    tracking_ack: u64,
    excluded_apps: Vec<String>,
    expanded_raw: bool,
    status_note: String,
    hotkey_selected: Hotkey,
    hotkey_active: Hotkey,
    hotkey_error: Option<String>,
    theme_mode: ThemeMode,
    max_items: MaxItemsConfig,
    expiry: ExpiryConfig,
    quick_settings: QuickSettings,
    groups: Vec<GroupSummary>,
) {
    history_page.refresh(store, history_query, history_active);
    let status_note = history_page.error.clone().unwrap_or(status_note);
    let mut snap = UiState {
        revision: 0,
        cards: Arc::clone(&history_page.cards),
        history_query: history_query.clone(),
        history_active,
        history_has_next: history_page.has_next,
        history_loading: history_page.loading,
        history_appending: history_page.appending,
        detail_content: history_page.detail_content.clone(),
        detail_error: history_page.detail_error.clone(),
        modal_image: if history_active {
            modal_image.clone()
        } else {
            None
        },
        selected: None,
        record_count: store.count_records().unwrap_or(flow.len()),
        tracking,
        tracking_ack,
        excluded_apps,
        expanded_raw,
        status_note,
        hotkey_selected,
        hotkey_active,
        hotkey_error,
        theme_mode,
        max_items,
        expiry,
        quick_settings,
        groups,
    };
    if let Ok(mut guard) = shared.lock() {
        snap.revision = guard.revision.wrapping_add(1);
        snap.selected = guard.selected;
        *guard = snap;
        if !history_page.loading {
            if let Some(loader) = &history_page.loader {
                loader.acknowledge(history_page.generation);
            }
        }
    }
}

fn base_exe(name: &str) -> &str {
    name.split(" (").next().unwrap_or(name)
}

fn exe_key(name: &str) -> String {
    base_exe(name).trim().to_lowercase()
}

fn is_excluded_source(excluded_apps: &[String], source_exe: &str) -> bool {
    let key = exe_key(source_exe);
    !key.is_empty() && excluded_apps.iter().any(|exe| exe_key(exe) == key)
}

fn exclude_app(
    store: &mut Store,
    excluded_apps: &mut Vec<String>,
    name: &str,
) -> Result<String, String> {
    let exe = exe_key(name);
    if exe.is_empty() {
        return Err("应用文件名不能为空".into());
    }
    if !is_excluded_source(excluded_apps, &exe) {
        store
            .upsert_application(&exe, &exe, "", true)
            .map_err(|error| format!("保存排除设置失败：{error}"))?;
        excluded_apps.push(exe.clone());
    }
    Ok(exe)
}

fn unexclude_app(
    store: &mut Store,
    excluded_apps: &mut Vec<String>,
    name: &str,
) -> Result<String, String> {
    let exe = exe_key(name);
    if exe.is_empty() {
        return Err("应用文件名不能为空".into());
    }
    // Older databases can contain mixed-case keys, including several spellings
    // of the same Windows executable. Clear every persisted matching entry.
    let matching: Vec<_> = excluded_apps
        .iter()
        .filter(|existing| exe_key(existing) == exe)
        .cloned()
        .collect();
    for existing in matching {
        store
            .upsert_application(&existing, &existing, "", false)
            .map_err(|error| format!("取消排除失败：{error}"))?;
    }
    excluded_apps.retain(|existing| exe_key(existing) != exe);
    Ok(exe)
}

fn run_worker(
    shared: SharedUi,
    cmd_rx: Receiver<WorkerCmd>,
    activate_tx: Sender<WindowSignal>,
    event_tx: Sender<WorkerEvent>,
) {
    #[cfg(not(windows))]
    {
        let _ = (shared, cmd_rx, activate_tx, event_tx);
        return;
    }

    #[cfg(windows)]
    {
        let (tx, rx) = std::sync::mpsc::channel::<PlatformEvent>();

        let store_path = db_path();
        if let Some(parent) = store_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut store = Store::open(&store_path).expect("open sqlite store");
        let mut flow = FlowEngine::new();
        let mut history_query = HistoryQuery::default();
        let mut history_active = false;
        let mut history_page = HistoryPage {
            loader: Some(history_loader::Loader::spawn(store_path.clone())),
            ..HistoryPage::default()
        };
        let mut modal_image = None;
        let mut tracking = true;
        let mut tracking_ack = 0;
        let mut expanded_raw = false;
        let mut status_note = String::new();
        let mut excluded_apps = store.list_excluded().unwrap_or_default();
        let stored_hotkey = store.get_setting("window_hotkey").ok().flatten();
        let mut invalid_hotkey_setting = stored_hotkey
            .as_deref()
            .is_some_and(|value| Hotkey::parse(value).is_none());
        let mut hotkey_selected = stored_hotkey
            .as_deref()
            .and_then(Hotkey::parse)
            .unwrap_or_default();
        let mut hotkey_active = Hotkey::Disabled;
        let mut hotkey_error = None;
        let mut last_purge_ms = 0i64;
        let mut theme_mode = load_theme_mode(&store);
        let (mut max_items, mut expiry) = load_limits(&store);
        let mut quick_settings = load_quick_settings(&store);

        publish(
            &shared,
            &store,
            &flow,
            &history_query,
            history_active,
            &mut history_page,
            &modal_image,
            tracking,
            tracking_ack,
            excluded_apps.clone(),
            expanded_raw,
            status_note.clone(),
            hotkey_selected,
            hotkey_active,
            hotkey_error.clone(),
            theme_mode,
            max_items,
            expiry,
            quick_settings,
            store.list_groups().unwrap_or_default(),
        );

        let pump = std::thread::Builder::new()
            .name("veya-win-pump".into())
            .spawn(move || {
                if let Err(e) = veya_windows::run(tx, hotkey_selected) {
                    eprintln!("platform error: {e}");
                }
            })
            .expect("spawn win pump");

        loop {
            let mut dirty = false;
            while let Ok(cmd) = cmd_rx.try_recv() {
                dirty = true;
                match cmd {
                    WorkerCmd::Recopy { sequence } => {
                        status_note = match prepare_record_paste(
                            &mut flow,
                            &store,
                            sequence,
                            write_payload,
                        ) {
                            Ok(_) => {
                                if quick_settings.move_to_top_after_paste {
                                    let _ = store.touch_record(sequence, now_ms());
                                    history_page.invalidate();
                                }
                                "已复制到剪贴板".into()
                            }
                            Err(error) => format!("复制失败：{error}"),
                        };
                    }
                    WorkerCmd::PreparePaste {
                        sequence,
                        request_id,
                    } => {
                        let result =
                            prepare_record_paste(&mut flow, &store, sequence, write_payload);
                        status_note = match &result {
                            Ok(_) => {
                                if quick_settings.move_to_top_after_paste {
                                    let _ = store.touch_record(sequence, now_ms());
                                    history_page.invalidate();
                                }
                                "已准备粘贴".into()
                            }
                            Err(error) => format!("粘贴准备失败：{error}"),
                        };
                        let _ = event_tx.send(WorkerEvent::PastePrepared { request_id, result });
                    }
                    WorkerCmd::RecopyPlainText { sequence } => {
                        status_note =
                            match recopy_plain_text(&mut flow, &store, sequence, write_payload) {
                                Ok(_) => "已复制为纯文本".into(),
                                Err(error) => format!("复制失败：{error}"),
                            };
                    }
                    WorkerCmd::LoadContent(sequence) => {
                        history_page.detail_content = None;
                        history_page.detail_error = None;
                        if history_active {
                            match store.load_content(sequence) {
                                Ok(Some(text)) => {
                                    history_page.detail_content = Some((sequence, Arc::from(text)))
                                }
                                Ok(None) => {
                                    history_page.detail_error =
                                        Some((sequence, "记录已不存在".into()))
                                }
                                Err(error) => {
                                    history_page.detail_error =
                                        Some((sequence, format!("读取内容失败：{error}")))
                                }
                            }
                        }
                    }
                    WorkerCmd::UnloadContent => {
                        history_page.detail_content = None;
                        history_page.detail_error = None;
                    }
                    WorkerCmd::ClearHistory => {
                        status_note = match store.clear() {
                            Ok(()) => {
                                history_page.invalidate();
                                flow.clear();
                                history_page.detail_content = None;
                                history_page.detail_error = None;
                                modal_image = None;
                                history_query.page = 0;
                                "历史已清空".into()
                            }
                            Err(error) => format!("清空历史失败：{error}"),
                        };
                    }
                    WorkerCmd::SetTracking { on, request_id } => {
                        tracking = on;
                        if let Some(request_id) = request_id {
                            tracking_ack = request_id;
                        }
                        veya_windows::set_tray_paused(!on);
                        status_note = if on {
                            "已恢复记录".into()
                        } else {
                            "已暂停记录".into()
                        };
                    }
                    WorkerCmd::ExcludeApp { exe } => {
                        status_note = match exclude_app(&mut store, &mut excluded_apps, &exe) {
                            Ok(exe) => format!("已排除 {exe} 的后续复制"),
                            Err(error) => error,
                        };
                    }
                    WorkerCmd::UnexcludeApp { exe } => {
                        status_note = match unexclude_app(&mut store, &mut excluded_apps, &exe) {
                            Ok(exe) => format!("已取消排除 {exe}"),
                            Err(error) => error,
                        };
                    }
                    WorkerCmd::DeleteRecord { sequences } => {
                        if history_page
                            .detail_content
                            .as_ref()
                            .is_some_and(|(sequence, _)| sequences.contains(sequence))
                        {
                            history_page.detail_content = None;
                        }
                        if history_page
                            .detail_error
                            .as_ref()
                            .is_some_and(|(sequence, _)| sequences.contains(sequence))
                        {
                            history_page.detail_error = None;
                        }
                        if modal_image
                            .as_ref()
                            .is_some_and(|(sequence, _)| sequences.contains(sequence))
                        {
                            modal_image = None;
                        }
                        status_note = match store.delete_records(&sequences) {
                            Ok(removed) => {
                                if removed > 0 {
                                    history_page.invalidate();
                                }
                                for sequence in sequences {
                                    flow.delete_record(sequence);
                                }
                                "记录已删除".into()
                            }
                            Err(error) => format!("删除失败：{error}"),
                        };
                    }
                    WorkerCmd::SetPinned { sequences, pinned } => {
                        status_note = match store.set_pinned(&sequences, pinned) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                flow.set_pinned(&sequences, pinned);
                                if pinned {
                                    "已固定记录"
                                } else {
                                    "已取消固定"
                                }
                                .into()
                            }
                            Err(error) => format!("固定状态保存失败：{error}"),
                        };
                    }
                    WorkerCmd::UpdateRecordText { sequence, text } => {
                        status_note = match store.update_record_text(sequence, &text) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                    if let Some(record) = flow.record(sequence) {
                                        let mut updated = record.clone();
                                        updated.content = text.clone();
                                        updated.payload = ClipboardPayload::Text(text);
                                        updated.content_hash = payload_hash(&updated.payload);
                                        flow.replace_record(updated);
                                    }
                                    "内容已更新".into()
                                } else {
                                    "无法编辑此记录（仅支持文本）".into()
                                }
                            }
                            Err(error) => format!("编辑失败：{error}"),
                        };
                    }
                    WorkerCmd::CreateTextRecord {
                        text,
                        created_at_ms,
                    } => {
                        status_note = match store.next_sequence() {
                            Ok(sequence) => {
                                let payload = ClipboardPayload::Text(text.clone());
                                let rec = ClipboardRecord {
                                    sequence,
                                    content_type: "text".into(),
                                    content: text.clone(),
                                    content_hash: payload_hash(&payload),
                                    payload,
                                    source_app: String::new(),
                                    source_pid: 0,
                                    source_window: String::new(),
                                    source_confidence: SourceConfidence::Exact,
                                    created_at_ms,
                                    pinned: false,
                                    pin_rank: 0,
                                    item_hotkey: String::new(),
                                    auto_delete: false,
                                    pastes: Vec::new(),
                                };
                                match store.insert_record(&rec) {
                                    Ok(()) => {
                                        flow.insert_record(rec);
                                        history_page.invalidate();
                                        "新记录已创建".into()
                                    }
                                    Err(error) => format!("创建失败：{error}"),
                                }
                            }
                            Err(error) => format!("创建失败：{error}"),
                        };
                    }
                    WorkerCmd::SetPinRank { sequence, rank } => {
                        status_note = match store.set_pin_rank(sequence, rank) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                if let Some(record) = flow.record(sequence) {
                                    let mut updated = record.clone();
                                    updated.pin_rank = rank;
                                    updated.pinned = rank != 0;
                                    flow.replace_record(updated);
                                }
                                match rank {
                                    0 => "已移除固定位置".into(),
                                    1 => "已固定到顶部".into(),
                                    2 => "已固定到底部".into(),
                                    _ => "固定状态已更新".into(),
                                }
                            }
                            Err(error) => format!("固定位置保存失败：{error}"),
                        };
                    }
                    WorkerCmd::SetItemHotkey { sequence, code } => {
                        status_note = match store.set_item_hotkey(sequence, &code) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                if let Some(record) = flow.record(sequence) {
                                    let mut updated = record.clone();
                                    updated.item_hotkey = code.clone();
                                    flow.replace_record(updated);
                                }
                                if code.is_empty() {
                                    "已清除条目快捷键".into()
                                } else {
                                    "条目快捷键已设置".into()
                                }
                            }
                            Err(error) => format!("快捷键保存失败：{error}"),
                        };
                    }
                    WorkerCmd::SetAutoDelete { sequence, enabled } => {
                        status_note = match store.set_auto_delete(sequence, enabled) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                if let Some(record) = flow.record(sequence) {
                                    let mut updated = record.clone();
                                    updated.auto_delete = enabled;
                                    flow.replace_record(updated);
                                }
                                if enabled {
                                    "已开启自动删除".into()
                                } else {
                                    "已关闭自动删除".into()
                                }
                            }
                            Err(error) => format!("自动删除设置失败：{error}"),
                        };
                    }
                    WorkerCmd::MoveToTop { sequence, at_ms } => {
                        status_note = match store.touch_record(sequence, at_ms) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                "已移到顶部".into()
                            }
                            Err(error) => format!("移动失败：{error}"),
                        };
                    }
                    WorkerCmd::MoveToBottom { sequence } => {
                        status_note = match store.move_to_bottom(sequence) {
                            Ok(changed) => {
                                if changed > 0 {
                                    history_page.invalidate();
                                }
                                "已移到底部".into()
                            }
                            Err(error) => format!("移动失败：{error}"),
                        };
                    }
                    WorkerCmd::SetExpandedRaw(on) => {
                        expanded_raw = on;
                    }
                    WorkerCmd::QueryHistory(query) => {
                        history_query = query;
                    }
                    WorkerCmd::SetHistoryActive(active) => {
                        history_active = active;
                        if !active {
                            history_page.refresh(&store, &history_query, false);
                            modal_image = None;
                        }
                    }
                    WorkerCmd::LoadFullImage(sequence) => {
                        if !history_active {
                            continue;
                        }
                        modal_image =
                            store
                                .load_record(sequence)
                                .ok()
                                .flatten()
                                .and_then(|record| match record.payload {
                                    ClipboardPayload::Image { png, .. } => Some((
                                        sequence,
                                        iced::widget::image::Handle::from_bytes(png),
                                    )),
                                    _ => None,
                                });
                    }
                    WorkerCmd::UnloadFullImage => {
                        modal_image = None;
                    }
                    WorkerCmd::SetHotkey(selection) => {
                        if !veya_windows::platform::request_hotkey_change(selection) {
                            hotkey_error = Some("快捷键服务尚未就绪，请稍后重试".into());
                            veya_windows::platform::request_hotkey_recording(false);
                        }
                    }
                    WorkerCmd::SetQuickSettings(settings) => {
                        quick_settings = settings;
                        let _ = store.set_setting(
                            "quick_move_to_top",
                            if settings.move_to_top_after_paste {
                                "1"
                            } else {
                                "0"
                            },
                        );
                        let _ = store.set_setting(
                            "quick_display_lines",
                            &settings.display_lines.to_string(),
                        );
                        let _ = store
                            .set_setting("quick_popup_position", settings.popup_position.as_key());
                        let _ = store.set_setting(
                            "quick_always_on_top",
                            if settings.always_on_top { "1" } else { "0" },
                        );
                        let _ = store.set_setting(
                            "quick_auto_dismiss",
                            if settings.auto_dismiss { "1" } else { "0" },
                        );
                        let _ = store.set_setting("quick_opacity", &settings.opacity.to_string());
                        let _ = store
                            .set_setting("quick_title_position", settings.title_position.as_key());
                        let _ = store.set_setting(
                            "quick_font_size",
                            &settings.font_size.to_string(),
                        );
                        let _ = store
                            .set_setting("quick_font_family", settings.font_family.as_key());
                        let _ = store.set_setting(
                            "quick_hover_expand_enabled",
                            if settings.hover_expand_enabled { "1" } else { "0" },
                        );
                    }
                    WorkerCmd::RestoreDatabase { path } => {
                        let result = restore_database(
                            &mut store,
                            &store_path,
                            &path,
                            &veya_windows::platform::dialog::local_timestamp(),
                        );
                        match &result {
                            Ok(report) => {
                                // Everything the worker mirrors from the database is stale now.
                                history_page.invalidate();
                                history_page.detail_content = None;
                                history_page.detail_error = None;
                                modal_image = None;
                                history_query.page = 0;
                                flow.clear();
                                excluded_apps = store.list_excluded().unwrap_or_default();
                                theme_mode = load_theme_mode(&store);
                                (max_items, expiry) = load_limits(&store);
                                quick_settings = load_quick_settings(&store);
                                status_note = format!("已还原 {} 条记录", report.records);
                            }
                            Err(error) => status_note = format!("还原失败：{error}"),
                        }
                        let _ = event_tx.send(WorkerEvent::DatabaseRestored(result));
                    }
                    WorkerCmd::SetThemeMode(mode) => {
                        theme_mode = mode;
                        let _ = store.set_setting("theme", mode.as_key());
                    }
                    WorkerCmd::SetMaxItems(config) => {
                        max_items = config;
                        let _ = store.set_setting(
                            "max_items_enabled",
                            if config.enabled { "1" } else { "0" },
                        );
                        let _ = store.set_setting("max_items", &config.count.to_string());
                        let removed = enforce_limits(&mut store, expiry, max_items, now_ms());
                        if removed > 0 {
                            history_page.invalidate();
                            discard_purged_record(&store, &mut flow);
                        }
                        status_note = limits_note(
                            "最大保存条数",
                            config.enabled,
                            format!("{} 条", config.count),
                            removed,
                        );
                    }
                    WorkerCmd::SetExpiry(config) => {
                        expiry = config;
                        let _ = store
                            .set_setting("expiry_enabled", if config.enabled { "1" } else { "0" });
                        let _ = store.set_setting("expiry_days", &config.days.to_string());
                        let removed = enforce_limits(&mut store, expiry, max_items, now_ms());
                        if removed > 0 {
                            history_page.invalidate();
                            discard_purged_record(&store, &mut flow);
                        }
                        status_note = limits_note(
                            "记录过期时间",
                            config.enabled,
                            format!("{} 天", config.days),
                            removed,
                        );
                    }
                    WorkerCmd::CreateGroup { name, add_sequence } => {
                        let name = name.trim();
                        if name.is_empty() {
                            status_note = "分组名称不能为空".into();
                        } else {
                            status_note = match store.create_group(name, now_ms()) {
                                Ok(group_id) => {
                                    if let Some(sequence) = add_sequence {
                                        let _ =
                                            store.add_group_member(group_id, sequence, now_ms());
                                        format!("已创建分组「{name}」并加入当前记录")
                                    } else {
                                        format!("已创建分组「{name}」")
                                    }
                                }
                                Err(error) => {
                                    if error.to_string().contains("UNIQUE") {
                                        "分组名称已存在".into()
                                    } else {
                                        format!("创建分组失败：{error}")
                                    }
                                }
                            };
                        }
                    }
                    WorkerCmd::RenameGroup { id, name } => {
                        let name = name.trim();
                        if name.is_empty() {
                            status_note = "分组名称不能为空".into();
                        } else {
                            status_note = match store.rename_group(id, name) {
                                Ok(0) => "分组不存在".into(),
                                Ok(_) => format!("分组已重命名为「{name}」"),
                                Err(error) => {
                                    if error.to_string().contains("UNIQUE") {
                                        "分组名称已存在".into()
                                    } else {
                                        format!("重命名分组失败：{error}")
                                    }
                                }
                            };
                        }
                    }
                    WorkerCmd::DeleteGroup { id } => {
                        status_note = match store.delete_group(id) {
                            Ok(0) => "分组不存在".into(),
                            Ok(_) => {
                                history_page.invalidate();
                                "分组已删除，其中的记录已保留".into()
                            }
                            Err(error) => format!("删除分组失败：{error}"),
                        };
                    }
                    WorkerCmd::AddToGroup { group_id, sequence } => {
                        status_note = match store.add_group_member(group_id, sequence, now_ms()) {
                            Ok(_) => {
                                if history_query.group_id == Some(group_id) {
                                    history_page.invalidate();
                                }
                                "已加入分组".into()
                            }
                            Err(error) => format!("加入分组失败：{error}"),
                        };
                    }
                    WorkerCmd::RemoveFromGroup {
                        group_id,
                        sequences,
                    } => {
                        let removed = sequences.iter().try_fold(0usize, |total, &sequence| {
                            store
                                .remove_group_member(group_id, sequence)
                                .map(|count| total + count)
                        });
                        status_note = match removed {
                            Ok(_) => {
                                if history_query.group_id == Some(group_id) {
                                    history_page.invalidate();
                                }
                                "已从分组移除".into()
                            }
                            Err(error) => format!("从分组移除失败：{error}"),
                        };
                    }
                }
            }

            dirty |= history_page.receive();

            // Periodic purge: expiry days and maximum item count (when enabled).
            let now = now_ms();
            if now - last_purge_ms > 60_000 {
                last_purge_ms = now;
                if enforce_limits(&mut store, expiry, max_items, now) > 0 {
                    history_page.invalidate();
                    discard_purged_record(&store, &mut flow);
                    dirty = true;
                }
            }

            if dirty {
                publish(
                    &shared,
                    &store,
                    &flow,
                    &history_query,
                    history_active,
                    &mut history_page,
                    &modal_image,
                    tracking,
                    tracking_ack,
                    excluded_apps.clone(),
                    expanded_raw,
                    status_note.clone(),
                    hotkey_selected,
                    hotkey_active,
                    hotkey_error.clone(),
                    theme_mode,
                    max_items,
                    expiry,
                    quick_settings,
                    store.list_groups().unwrap_or_default(),
                );
            }

            let event_received = match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(ev) => {
                    match ev {
                        PlatformEvent::ToggleWindow { visible, target } => {
                            let _ = activate_tx.send(WindowSignal::Hotkey { visible, target });
                        }
                        PlatformEvent::HotkeyStatus {
                            requested,
                            active,
                            error,
                        } => {
                            hotkey_active = active;
                            hotkey_error = error;
                            if hotkey_error.is_none() {
                                hotkey_selected = requested;
                                status_note = if requested == Hotkey::Disabled {
                                    "窗口快捷键已关闭".into()
                                } else {
                                    format!("窗口快捷键已设为 {requested}")
                                };
                                if let Err(error) =
                                    store.set_setting("window_hotkey", &requested.as_key())
                                {
                                    hotkey_error = Some(format!(
                                        "{requested} 已生效，但保存失败；重启后可能恢复旧设置：{error}"
                                    ));
                                } else if invalid_hotkey_setting {
                                    hotkey_error =
                                        Some("保存的窗口快捷键无效，已恢复默认 Alt+V".into());
                                }
                                invalid_hotkey_setting = false;
                            }
                            if let Some(message) = &hotkey_error {
                                status_note = message.clone();
                            }
                        }
                        PlatformEvent::ClipboardChange(raw) if tracking => {
                            let sequence = raw.sequence;
                            match enrich_clipboard(raw) {
                                Some(change) => {
                                    if is_excluded_source(&excluded_apps, &change.source_exe) {
                                        flow.on_untracked_clipboard_change(change.sequence);
                                        discard_inactive_records(&mut flow, None);
                                    } else {
                                        let seq = change.sequence;
                                        let outcome = flow.on_clipboard_change(change);
                                        if matches!(
                                            outcome,
                                            veya_core::FlowOutcome::Recorded { .. }
                                        ) {
                                            if let Some(rec) = flow.record(seq) {
                                                if let Err(error) = store.insert_record(rec) {
                                                    flow.delete_record(seq);
                                                    status_note = format!("记录保存失败：{error}");
                                                } else {
                                                    history_page.invalidate();
                                                    // Enforce the maximum-count limit right away.
                                                    last_purge_ms = 0;
                                                }
                                            }
                                        }
                                        let active = flow.current_sequence();
                                        discard_inactive_records(&mut flow, active);
                                    }
                                }
                                None => {
                                    flow.on_untracked_clipboard_change(sequence);
                                    discard_inactive_records(&mut flow, None);
                                    status_note = "剪贴板内容无法读取或超出大小上限，已跳过".into();
                                }
                            }
                        }
                        PlatformEvent::ClipboardSkipped { sequence } if tracking => {
                            flow.on_untracked_clipboard_change(sequence);
                            discard_inactive_records(&mut flow, None);
                            status_note = "该剪贴板格式暂不支持或无法读取，已跳过".into();
                        }
                        PlatformEvent::PasteTrigger(raw) if tracking => {
                            let trigger = enrich_paste(raw);
                            let pasted = flow.current_sequence();
                            match persist_paste_trigger(&mut store, &mut flow, trigger) {
                                Ok(true) => {
                                    history_page.invalidate();
                                    // "自动删除": the record goes once it has been pasted.
                                    if let Some(sequence) = pasted {
                                        match delete_after_paste(&mut store, &mut flow, sequence) {
                                            Ok(true) => {
                                                status_note = "已按设置在粘贴后删除该记录".into()
                                            }
                                            Ok(false) => {}
                                            Err(error) => status_note = error,
                                        }
                                    }
                                }
                                Ok(false) => {}
                                Err(error) => status_note = error,
                            }
                        }
                        _ => {}
                    }
                    true
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => false,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };

            if event_received {
                publish(
                    &shared,
                    &store,
                    &flow,
                    &history_query,
                    history_active,
                    &mut history_page,
                    &modal_image,
                    tracking,
                    tracking_ack,
                    excluded_apps.clone(),
                    expanded_raw,
                    status_note.clone(),
                    hotkey_selected,
                    hotkey_active,
                    hotkey_error.clone(),
                    theme_mode,
                    max_items,
                    expiry,
                    quick_settings,
                    store.list_groups().unwrap_or_default(),
                );
            }
        }

        let _ = pump.join();
        let _ = PasteConfidence::HotkeyObserved;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::HistoryFilter;
    use veya_core::{
        ClipboardChange, FlowOutcome, PasteConfidence, PasteMethod, PasteTriggerRecord,
        SourceConfidence,
    };

    #[test]
    fn enforce_limits_applies_expiry_and_count_and_spares_pinned_records() {
        let day = 86_400_000;
        let now = 100 * day;
        let mut store = Store::open_in_memory().unwrap();
        // 1: pinned and ancient, 2..=4: ancient, 5..=8: recent.
        store.insert_record(&text_record(1, "p", 0)).unwrap();
        for sequence in 2..=4 {
            store
                .insert_record(&text_record(sequence, "old", day))
                .unwrap();
        }
        for sequence in 5..=8 {
            store
                .insert_record(&text_record(sequence, "new", now - i64::from(sequence)))
                .unwrap();
        }
        store.set_pinned(&[1], true).unwrap();

        let off = MaxItemsConfig {
            enabled: false,
            count: 1,
        };
        let expiry = ExpiryConfig {
            enabled: true,
            days: 30,
        };
        assert_eq!(enforce_limits(&mut store, expiry, off, now), 3);
        assert_eq!(store.count_records().unwrap(), 5);

        // Disabled settings never delete anything.
        let none = ExpiryConfig {
            enabled: false,
            days: 1,
        };
        assert_eq!(enforce_limits(&mut store, none, off, now), 0);

        // Keep the newest 2 unpinned records; the pinned one stays.
        let max = MaxItemsConfig {
            enabled: true,
            count: 2,
        };
        assert_eq!(enforce_limits(&mut store, none, max, now), 2);
        let kept: Vec<u32> = store
            .load_all()
            .unwrap()
            .iter()
            .map(|record| record.sequence)
            .collect();
        // Oldest first: the pinned record, then 6 (now - 6) and 5 (now - 5).
        assert_eq!(kept, vec![1, 6, 5]);
    }

    #[test]
    fn reloading_a_scrolled_page_keeps_every_row_from_the_top() {
        let mut store = Store::open_in_memory().unwrap();
        let total = HISTORY_PAGE_SIZE * 2 + 5;
        for index in 0..total {
            store
                .insert_record(&text_record(
                    index as u32 + 1,
                    &format!("row {index}"),
                    1_000 + index as i64,
                ))
                .unwrap();
        }
        let query = HistoryQuery {
            page: 1,
            ..HistoryQuery::default()
        };
        // A plain page-1 load only returns the second page ...
        let (single, _) = history_loader::load_page(&store, &query, false, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(single.len(), HISTORY_PAGE_SIZE);
        // ... while a reload "through" page 1 returns pages 0 and 1 together.
        let (through, has_next) = history_loader::load_page(&store, &query, true, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(through.len(), HISTORY_PAGE_SIZE * 2);
        assert!(has_next);
        assert_eq!(through[0].sequence, total as u32);
        assert_eq!(
            through[HISTORY_PAGE_SIZE].sequence, single[0].sequence,
            "page 1 must follow page 0 without a gap or repeat"
        );
    }

    #[test]
    fn group_view_keeps_a_card_whose_newest_copy_is_not_the_saved_one() {
        let mut store = Store::open_in_memory().unwrap();
        store.insert_record(&text_record(1, "same", 1_000)).unwrap();
        store
            .insert_record(&text_record(2, "other", 2_000))
            .unwrap();
        // The user groups the first "same", then copies the identical text again right
        // after it; the newest copy (3) becomes the card's representative.
        store.insert_record(&text_record(3, "same", 3_000)).unwrap();
        store.insert_record(&text_record(4, "same", 4_000)).unwrap();
        let group = store.create_group("keep", 1).unwrap();
        store.add_group_member(group, 3, 1).unwrap();
        let query = HistoryQuery {
            group_id: Some(group),
            ..HistoryQuery::default()
        };
        let (cards, _) = history_loader::load_page(&store, &query, false, &|| true)
            .unwrap()
            .unwrap();
        assert_eq!(cards.len(), 1);
        let mut raw = cards[0].raw_sequences.clone();
        raw.sort_unstable();
        assert_eq!(raw, vec![3, 4]);
        // Another group sees nothing.
        let empty = store.create_group("empty", 2).unwrap();
        let query = HistoryQuery {
            group_id: Some(empty),
            ..HistoryQuery::default()
        };
        let (cards, _) = history_loader::load_page(&store, &query, false, &|| true)
            .unwrap()
            .unwrap();
        assert!(cards.is_empty());
    }

    pub(super) fn text_record(sequence: u32, text: &str, created_at_ms: i64) -> ClipboardRecord {
        let payload = ClipboardPayload::Text(text.to_string());
        ClipboardRecord {
            sequence,
            content_type: "text".into(),
            content: text.into(),
            content_hash: payload_hash(&payload),
            payload,
            source_app: "test.exe".into(),
            source_pid: 1,
            source_window: String::new(),
            source_confidence: SourceConfidence::Exact,
            created_at_ms,
            pinned: false,
            pin_rank: 0,
            item_hotkey: String::new(),
            auto_delete: false,
            pastes: Vec::new(),
        }
    }

    #[test]
    fn persisted_pastes_match_core_including_repeated_timestamps_and_methods() {
        let mut store = Store::open_in_memory().unwrap();
        let mut flow = FlowEngine::new();
        let record = text_record(1, "paste source", 1000);
        store.insert_record(&record).unwrap();
        let trigger = PasteTrigger {
            target_pid: 42,
            target_exe: "notepad.exe".into(),
            target_window: "目标窗口".into(),
            method: PasteMethod::CtrlV,
            timestamp_ms: 2000,
        };
        assert_eq!(
            persist_paste_trigger(&mut store, &mut flow, trigger.clone()),
            Ok(false)
        );
        flow.activate_replayed_record(record);
        for method in [
            PasteMethod::CtrlV,
            PasteMethod::ShiftInsert,
            PasteMethod::CtrlV,
        ] {
            let mut trigger = trigger.clone();
            trigger.method = method;
            assert_eq!(
                persist_paste_trigger(&mut store, &mut flow, trigger),
                Ok(true)
            );
        }
        assert_eq!(flow.record(1), store.load_record(1).unwrap().as_ref());
        assert_eq!(flow.record(1).unwrap().pastes.len(), 3);
        assert!(flow
            .record(1)
            .unwrap()
            .pastes
            .iter()
            .all(|p| p.confidence == PasteConfidence::HotkeyObserved && p.triggered_at_ms == 2000));
    }

    #[test]
    fn failed_paste_persistence_does_not_mutate_core_or_saved_history() {
        let path = std::env::temp_dir().join(format!(
            "veya-worker-paste-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut store = Store::open(&path).unwrap();
            let mut flow = FlowEngine::new();
            let record = text_record(1, "paste source", 1000);
            flow.activate_replayed_record(record.clone());
            let trigger = PasteTrigger {
                target_pid: 42,
                target_exe: "notepad.exe".into(),
                target_window: "target".into(),
                method: PasteMethod::ShiftInsert,
                timestamp_ms: 2000,
            };
            let error = persist_paste_trigger(&mut store, &mut flow, trigger.clone()).unwrap_err();
            assert!(error.starts_with("粘贴记录保存失败："));
            assert_eq!(flow.record(1), Some(&record));
            assert!(store.load_record(1).unwrap().is_none());
            store.insert_record(&record).unwrap();
            // Establish an older saved association before injecting a real SQL failure.
            assert_eq!(
                persist_paste_trigger(&mut store, &mut flow, trigger.clone()),
                Ok(true)
            );
            let before = store.load_record(1).unwrap().unwrap();
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TRIGGER reject_paste BEFORE INSERT ON paste_trigger BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
            let error = persist_paste_trigger(&mut store, &mut flow, trigger).unwrap_err();
            assert!(error.contains("injected write failure"));
            assert_eq!(flow.record(1), Some(&before));
            assert_eq!(store.load_record(1).unwrap(), Some(before.clone()));
            drop(store);
            assert_eq!(
                Store::open(&path).unwrap().load_record(1).unwrap(),
                Some(before)
            );
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn plain_copy_preserves_full_text_beyond_preview() {
        let mut store = Store::open_in_memory().unwrap();
        let full = format!("{}END", "long unicode 内容".repeat(1000));
        store.insert_record(&text_record(7, &full, 10_000)).unwrap();
        let mut flow = FlowEngine::new();
        assert_eq!(
            recopy_plain_text(&mut flow, &store, 7, |payload| {
                assert_eq!(payload, &ClipboardPayload::Text(full.clone()));
                Ok(42)
            }),
            Ok(42)
        );
        assert!(recopy_plain_text(&mut flow, &store, 99, |_| panic!(
            "missing record cannot write"
        ))
        .is_err());
    }

    #[test]
    fn prepare_paste_restores_typed_payloads_and_suppresses_own_clipboard_update() {
        let payloads = [
            ClipboardPayload::Text("original text".into()),
            ClipboardPayload::Files(vec![r"C:\Pictures\one.png".into(), r"C:\two.txt".into()]),
            ClipboardPayload::Image {
                png: vec![137, 80, 78, 71],
                width: 3,
                height: 2,
            },
        ];
        for payload in payloads {
            let mut store = Store::open_in_memory().unwrap();
            let mut record = text_record(7, "display summary must not be replayed", 10_000);
            record.payload = payload.clone();
            record.content_hash = payload_hash(&payload);
            store.insert_record(&record).unwrap();
            let mut flow = FlowEngine::new();

            let result = prepare_record_paste(&mut flow, &store, 7, |written| {
                assert_eq!(written, &payload);
                Ok(41)
            });
            assert_eq!(result, Ok(41));
            let outcome = flow.on_clipboard_change(ClipboardChange {
                sequence: 41,
                payload: payload.clone(),
                content_hash: payload_hash(&payload),
                source_pid: std::process::id(),
                source_exe: "veya.exe".into(),
                source_window: "Veya".into(),
                source_confidence: SourceConfidence::Likely,
                timestamp_ms: 11_000,
            });
            assert_eq!(
                outcome,
                FlowOutcome::SuppressedInternalWrite { sequence: 41 }
            );
            assert_eq!(flow.len(), 1);
            assert_eq!(flow.current_sequence(), Some(7));
        }
    }

    #[test]
    fn prepare_paste_reads_the_active_flow_record_before_storage() {
        let store = Store::open_in_memory().unwrap();
        let mut flow = FlowEngine::new();
        let payload = ClipboardPayload::Text("active clipboard".into());
        flow.on_clipboard_change(ClipboardChange {
            sequence: 7,
            payload: payload.clone(),
            content_hash: payload_hash(&payload),
            source_pid: 200,
            source_exe: "Code.exe".into(),
            source_window: "VS Code".into(),
            source_confidence: SourceConfidence::Exact,
            timestamp_ms: 10_000,
        });
        assert_eq!(
            prepare_record_paste(&mut flow, &store, 7, |written| {
                assert_eq!(written, &payload);
                Ok(41)
            }),
            Ok(41)
        );
    }

    #[test]
    fn prepare_paste_missing_record_does_not_touch_clipboard() {
        let store = Store::open_in_memory().unwrap();
        let mut flow = FlowEngine::new();
        assert_eq!(
            prepare_record_paste(&mut flow, &store, 7, |_| {
                panic!("missing record must not reach the clipboard writer")
            }),
            Err("记录已不存在".into())
        );
    }

    #[test]
    fn prepare_paste_storage_decode_error_does_not_touch_clipboard() {
        let mut store = Store::open_in_memory().unwrap();
        let mut record = text_record(7, "invalid persisted image", 10_000);
        record.payload = ClipboardPayload::Image {
            png: Vec::new(),
            width: 1,
            height: 1,
        };
        store.insert_record(&record).unwrap();
        let mut flow = FlowEngine::new();
        let result = prepare_record_paste(&mut flow, &store, 7, |_| {
            panic!("unreadable record must not reach the clipboard writer")
        });
        assert!(result.unwrap_err().starts_with("读取记录失败："));
    }

    #[test]
    fn prepare_paste_write_failure_is_returned_and_does_not_suppress_user_copy() {
        let mut store = Store::open_in_memory().unwrap();
        let record = text_record(7, "saved text", 10_000);
        store.insert_record(&record).unwrap();
        let mut flow = FlowEngine::new();
        assert_eq!(
            prepare_record_paste(
                &mut flow,
                &store,
                7,
                |_| Err("clipboard unavailable".into())
            ),
            Err("clipboard unavailable".into())
        );
        let outcome = flow.on_clipboard_change(ClipboardChange {
            sequence: 50,
            payload: record.payload,
            content_hash: record.content_hash,
            source_pid: 200,
            source_exe: "Code.exe".into(),
            source_window: "VS Code".into(),
            source_confidence: SourceConfidence::Exact,
            timestamp_ms: 11_000,
        });
        assert_eq!(outcome, FlowOutcome::Recorded { sequence: 50 });
    }

    #[test]
    fn prepare_paste_unconfirmed_write_preserves_only_own_update_suppression() {
        let mut store = Store::open_in_memory().unwrap();
        let record = text_record(7, "saved text", 10_000);
        store.insert_record(&record).unwrap();
        let mut flow = FlowEngine::new();
        let result = prepare_record_paste(&mut flow, &store, 7, |_| {
            Err(ClipboardWriteError {
                message: "confirmation busy".into(),
                changed: true,
            })
        });
        assert_eq!(result, Err("confirmation busy".into()));
        assert_eq!(flow.current_sequence(), None);
        let change = ClipboardChange {
            sequence: 50,
            payload: record.payload.clone(),
            content_hash: record.content_hash.clone(),
            source_pid: std::process::id(),
            source_exe: "veya.exe".into(),
            source_window: "Veya".into(),
            source_confidence: SourceConfidence::Exact,
            timestamp_ms: 11_000,
        };
        assert_eq!(
            flow.on_clipboard_change(change.clone()),
            FlowOutcome::SuppressedInternalWrite { sequence: 50 }
        );
        assert!(flow.is_empty());
        let mut user_change = change;
        user_change.sequence = 51;
        user_change.source_pid = 12345;
        user_change.source_exe = "Code.exe".into();
        assert_eq!(
            flow.on_clipboard_change(user_change),
            FlowOutcome::Recorded { sequence: 51 }
        );
    }

    #[test]
    fn history_paging_searches_all_records_without_retaining_all_cards() {
        let mut store = Store::open_in_memory().unwrap();
        for sequence in 1..=30 {
            store
                .insert_record(&text_record(
                    sequence,
                    &format!("entry-{sequence:02}-X"),
                    i64::from(sequence) * 10_000,
                ))
                .unwrap();
        }

        let mut query = HistoryQuery::default();
        let (first, has_next) = load_history_page(&store, &query).unwrap();
        assert_eq!(first.len(), HISTORY_PAGE_SIZE);
        assert_eq!(first.first().unwrap().sequence, 30);
        assert_eq!(first.last().unwrap().sequence, 6);
        assert!(has_next);

        query.page = 1;
        let (second, has_next) = load_history_page(&store, &query).unwrap();
        assert_eq!(second.len(), 5);
        assert_eq!(second.first().unwrap().sequence, 5);
        assert_eq!(second.last().unwrap().sequence, 1);
        assert!(!has_next);

        query.page = 0;
        query.search = "entry-03-X".into();
        let (matches, has_next) = load_history_page(&store, &query).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].sequence, 3);
        assert!(!has_next);

        query.search.clear();
        query.newest_first = false;
        let (oldest, has_next) = load_history_page(&store, &query).unwrap();
        assert_eq!(oldest.first().unwrap().sequence, 1);
        assert_eq!(oldest.last().unwrap().sequence, 25);
        assert!(has_next);
    }

    #[test]
    fn loading_page_rejects_old_results_and_releases_inactive_cards() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert_record(&text_record(1, "saved", 10_000))
            .unwrap();
        let query = HistoryQuery::default();
        let (cards, has_next) = load_history_page(&store, &query).unwrap();
        let mut page = HistoryPage {
            generation: 2,
            query: Some(query.clone()),
            loading: true,
            ..HistoryPage::default()
        };
        assert!(!page.apply(history_loader::ResultEvent::Cards {
            generation: 1,
            cards: cards.clone(),
            has_next
        }));
        assert!(page.loading);
        assert!(page.apply(history_loader::ResultEvent::Cards {
            generation: 2,
            cards,
            has_next
        }));
        assert!(!page.loading);
        let old = Arc::downgrade(&page.cards);
        page.refresh(&store, &query, false);
        assert!(old.upgrade().is_none());
        assert!(!page.apply(history_loader::ResultEvent::Error {
            generation: 2,
            message: "stale".into()
        }));
        assert!(page.error.is_none());
    }

    #[test]
    fn status_only_publication_shares_bounded_cards() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .insert_record(&text_record(1, &"large text ".repeat(100_000), 10_000))
            .unwrap();
        let query = HistoryQuery::default();
        let (cards, _) = load_history_page(&store, &query).unwrap();
        assert!(cards[0].content_excerpt.chars().count() <= 512);
        let mut page = HistoryPage {
            query: Some(query.clone()),
            cards: cards.into(),
            ..HistoryPage::default()
        };
        let old = page.cards.clone();
        page.refresh(&store, &query, true);
        assert!(Arc::ptr_eq(&old, &page.cards));
        page.invalidate();
        assert!(!page.apply(history_loader::ResultEvent::Error {
            generation: 0,
            message: "stale".into()
        }));
        page.refresh(&store, &query, true);
        assert!(page.loading);
        assert!(page.cards.is_empty());
    }

    #[test]
    fn history_group_survives_storage_batch_boundary() {
        let mut store = Store::open_in_memory().unwrap();
        for sequence in 1..=18 {
            let grouped = (2..=4).contains(&sequence);
            let mut record = text_record(
                sequence,
                if grouped { "repeated" } else { "other" },
                if grouped {
                    20_000 + i64::from(sequence) * 100
                } else {
                    i64::from(sequence) * 10_000
                },
            );
            if !grouped {
                record.content_hash = format!("unique-{sequence}");
            }
            store.insert_record(&record).unwrap();
        }

        let (cards, has_next) = load_history_page(&store, &HistoryQuery::default()).unwrap();
        let grouped = cards.iter().find(|card| card.sequence == 2).unwrap();
        assert_eq!(grouped.raw_count, 3);
        assert_eq!(grouped.raw_sequences, vec![2, 3, 4]);
        assert!(!has_next);
    }

    #[test]
    fn history_filters_use_payload_kind_pin_and_paste_target_across_storage() {
        let mut store = Store::open_in_memory().unwrap();
        let mut link = text_record(1, "https://example.com", 10_000);
        link.pastes.push(PasteTriggerRecord {
            target_app: "target-app.exe".into(),
            target_pid: 2,
            target_window: String::new(),
            method: PasteMethod::CtrlV,
            confidence: PasteConfidence::HotkeyObserved,
            triggered_at_ms: 11_000,
        });
        let mut code = text_record(2, "SELECT * FROM notes", 20_000);
        code.pinned = true;
        let path_text = text_record(3, r"C:\Pictures\image.png", 30_000);
        let mut files = text_record(4, "", 40_000);
        files.payload = ClipboardPayload::Files(vec![r"C:\Pictures\image.png".into()]);
        files.content_hash = payload_hash(&files.payload);
        let mut image = text_record(5, "", 50_000);
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        image.payload = ClipboardPayload::Image {
            png,
            width: 1,
            height: 1,
        };
        image.content_hash = payload_hash(&image.payload);
        for record in [&link, &code, &path_text, &files, &image] {
            store.insert_record(record).unwrap();
        }

        let mut query = HistoryQuery::default();
        for (filter, expected) in [
            (HistoryFilter::Link, 1),
            (HistoryFilter::Code, 2),
            (HistoryFilter::Text, 3),
            (HistoryFilter::File, 4),
            (HistoryFilter::Image, 5),
        ] {
            query.filter = filter;
            let (cards, _) = load_history_page(&store, &query).unwrap();
            assert_eq!(
                cards.iter().map(|card| card.sequence).collect::<Vec<_>>(),
                [expected]
            );
        }
        query.filter = HistoryFilter::All;
        query.pinned_only = true;
        let (cards, _) = load_history_page(&store, &query).unwrap();
        assert_eq!(
            cards.iter().map(|card| card.sequence).collect::<Vec<_>>(),
            [2]
        );
        query.pinned_only = false;
        query.search = "target-app".into();
        let (cards, _) = load_history_page(&store, &query).unwrap();
        assert_eq!(
            cards.iter().map(|card| card.sequence).collect::<Vec<_>>(),
            [1]
        );
    }

    #[test]
    fn excluded_app_matches_win32_exe_name_regardless_of_case() {
        let excluded_apps = ["chatgpt.exe".to_string()];
        let observed_source = "ChatGPT.exe";

        assert!(is_excluded_source(&excluded_apps, observed_source));
        assert!(is_excluded_source(&excluded_apps, "CHATGPT.EXE (likely)"));
        assert!(!is_excluded_source(&excluded_apps, "ChatGPT-helper.exe"));
    }

    #[test]
    fn exclude_and_unexclude_round_trip_with_mixed_case_legacy_keys() {
        let mut store = Store::open_in_memory().unwrap();
        let mut excluded_apps = Vec::new();

        assert_eq!(
            exclude_app(&mut store, &mut excluded_apps, " chatgpt.exe ").unwrap(),
            "chatgpt.exe"
        );
        assert!(is_excluded_source(&excluded_apps, "ChatGPT.exe"));
        assert_eq!(store.list_excluded().unwrap(), vec!["chatgpt.exe"]);

        store
            .upsert_application("ChatGPT.exe", "ChatGPT.exe", "", true)
            .unwrap();
        excluded_apps = store.list_excluded().unwrap();
        unexclude_app(&mut store, &mut excluded_apps, "CHATGPT.EXE").unwrap();
        assert!(excluded_apps.is_empty());
        assert!(store.list_excluded().unwrap().is_empty());
    }

    #[test]
    fn internal_copy_is_suppressed_when_platform_event_has_the_written_sequence() {
        let mut flow = FlowEngine::new();
        let text = "copied from Veya";

        let payload = ClipboardPayload::Text(text.to_string());
        assert!(write_internal_payload(&mut flow, &payload, |_| Ok(41)).is_ok());
        let outcome = flow.on_clipboard_change(ClipboardChange {
            sequence: 41,
            payload: veya_core::ClipboardPayload::Text(text.to_string()),
            content_hash: payload_hash(&payload),
            source_pid: std::process::id(),
            source_exe: "veya.exe".to_string(),
            source_window: "Veya".to_string(),
            source_confidence: SourceConfidence::Likely,
            timestamp_ms: 1_000,
        });

        assert_eq!(
            outcome,
            FlowOutcome::SuppressedInternalWrite { sequence: 41 }
        );
        assert!(flow.is_empty());
    }

    #[test]
    fn failed_internal_copy_does_not_suppress_a_later_user_copy() {
        let mut flow = FlowEngine::new();
        let text = "copy that failed";

        let payload = ClipboardPayload::Text(text.to_string());
        assert!(write_internal_payload(&mut flow, &payload, |_| Err(
            "clipboard unavailable".into()
        ))
        .is_err());
        let outcome = flow.on_clipboard_change(ClipboardChange {
            sequence: 50,
            payload: veya_core::ClipboardPayload::Text(text.to_string()),
            content_hash: payload_hash(&payload),
            source_pid: 200,
            source_exe: "Code.exe".to_string(),
            source_window: "VS Code".to_string(),
            source_confidence: SourceConfidence::Exact,
            timestamp_ms: 2_000,
        });

        assert_eq!(outcome, FlowOutcome::Recorded { sequence: 50 });
        assert_eq!(flow.len(), 1);
    }

    fn restore_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "veya-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn restore_keeps_a_safety_copy_and_the_current_hotkey_and_reloads_settings() {
        let dir = restore_dir("restore");
        let live = dir.join("veya.db");
        let backup = dir.join("saved.db");
        let mut store = Store::open(&live).unwrap();
        store.insert_record(&text_record(1, "kept", 10)).unwrap();
        store.set_setting("theme", "light").unwrap();
        store.set_setting("max_items_enabled", "1").unwrap();
        store.set_setting("max_items", "50").unwrap();
        store.set_setting("window_hotkey", "Alt+V").unwrap();
        Store::backup_file(&live, &backup).unwrap();

        store.insert_record(&text_record(2, "newer", 20)).unwrap();
        store.set_setting("theme", "dark").unwrap();
        store.set_setting("max_items", "99").unwrap();
        store.set_setting("window_hotkey", "Ctrl+Shift+V").unwrap();

        let report = restore_database(&mut store, &live, &backup, "20261023-120000").unwrap();
        assert_eq!(report.records, 1);
        assert_eq!(store.count_records().unwrap(), 1);
        assert!(report.safety_copy.exists());
        assert_eq!(
            Store::inspect_backup(&report.safety_copy).unwrap().records,
            2
        );
        assert_eq!(load_theme_mode(&store), ThemeMode::from_key("light"));
        let (max_items, _) = load_limits(&store);
        assert_eq!((max_items.enabled, max_items.count), (true, 50));
        assert_eq!(
            store.get_setting("window_hotkey").unwrap().as_deref(),
            Some("Ctrl+Shift+V"),
            "the live hotkey registration is not rolled back"
        );
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_of_an_invalid_file_changes_nothing_and_leaves_no_safety_copy() {
        let dir = restore_dir("restore-invalid");
        let live = dir.join("veya.db");
        let mut store = Store::open(&live).unwrap();
        store.insert_record(&text_record(1, "kept", 10)).unwrap();
        let bogus = dir.join("bogus.db");
        std::fs::write(&bogus, "definitely not a sqlite database file").unwrap();

        assert!(restore_database(&mut store, &live, &bogus, "x").is_err());
        assert!(restore_database(&mut store, &live, &live, "x").is_err());
        assert_eq!(store.count_records().unwrap(), 1);
        assert!(!dir.join("backups").exists());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_newest_safety_copies_are_kept() {
        let dir = restore_dir("restore-prune");
        for day in 1..=8 {
            std::fs::write(
                dir.join(format!("{SAFETY_COPY_PREFIX}202610{day:02}-000000.db")),
                b"x",
            )
            .unwrap();
        }
        std::fs::write(dir.join("my-own-backup.db"), b"x").unwrap();
        prune_safety_copies(&dir, 5);
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names.len(), 6, "five copies plus the unrelated file");
        assert!(names.contains(&"my-own-backup.db".to_string()));
        assert!(names
            .iter()
            .all(|n| !n.contains("20261001") && !n.contains("20261003")));
        assert!(names.iter().any(|n| n.contains("20261008")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn auto_delete_removes_only_records_marked_for_it_once_pasted() {
        let dir = restore_dir("auto-delete");
        let mut store = Store::open(dir.join("veya.db")).unwrap();
        let mut flow = FlowEngine::default();
        let mut keep = text_record(1, "keep", 10);
        let mut gone = text_record(2, "gone", 20);
        gone.auto_delete = true;
        keep.auto_delete = false;
        for record in [keep, gone] {
            store.insert_record(&record).unwrap();
            flow.insert_record(record);
        }

        assert_eq!(delete_after_paste(&mut store, &mut flow, 1), Ok(false));
        assert!(store.load_record(1).unwrap().is_some());
        assert_eq!(delete_after_paste(&mut store, &mut flow, 2), Ok(true));
        assert!(store.load_record(2).unwrap().is_none());
        assert!(flow.record(2).is_none());
        assert_eq!(delete_after_paste(&mut store, &mut flow, 99), Ok(false));
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}

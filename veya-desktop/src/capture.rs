//! Shared UI state fed by the capture worker.

use std::sync::{Arc, Mutex};

use veya_core::{
    ClipboardPayload, HistoryPayload, HistorySummary, PasteConfidence, SourceConfidence,
};
pub use veya_storage::GroupSummary;
use veya_windows::hotkey::Hotkey;

use crate::format::{self, ContentKind};

#[derive(Debug, Clone)]
pub struct UsedInView {
    pub target_app: String,
    pub target_window: String,
    pub method_label: String,
    pub time_label: String,
}

#[derive(Debug, Clone)]
pub enum CardPayloadView {
    Text,
    Files(Vec<String>),
    Image {
        handle: Option<iced::widget::image::Handle>,
        thumbnail_failed: bool,
        width: u32,
        height: u32,
        encoded_bytes: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    Text,
    Link,
    Code,
    File,
    Image,
}

impl HistoryFilter {
    pub fn matches(self, kind: ContentKind) -> bool {
        match self {
            Self::All => true,
            Self::Text => kind == ContentKind::Text,
            Self::Link => kind == ContentKind::Link,
            Self::Code => kind == ContentKind::Code,
            Self::File => kind == ContentKind::File,
            Self::Image => kind == ContentKind::Image,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryQuery {
    pub search: String,
    pub filter: HistoryFilter,
    pub pinned_only: bool,
    pub newest_first: bool,
    pub page: usize,
    /// When `Some`, the query is scoped to the members of that card group
    /// (ordered by most-recently-added first).
    pub group_id: Option<i64>,
}

impl Default for HistoryQuery {
    fn default() -> Self {
        Self {
            search: String::new(),
            filter: HistoryFilter::All,
            pinned_only: false,
            newest_first: true,
            page: 0,
            group_id: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CardView {
    pub sequence: u32,
    pub raw_count: usize,
    pub raw_sequences: Vec<u32>,
    pub content_preview: String,
    pub content_excerpt: String,
    pub content_chars: usize,
    pub content_hash: String,
    pub payload: CardPayloadView,
    pub kind: ContentKind,
    pub pinned: bool,
    pub pin_rank: u8,
    pub item_hotkey: String,
    pub auto_delete: bool,
    pub source_app: String,
    pub source_confidence: SourceConfidence,
    pub source_window: String,
    pub time_full: String,
    pub created_at_ms: i64,
    pub time_range: String,
    pub used_in: Vec<UsedInView>,
    pub used_in_apps: Vec<String>,
    pub has_paste_activity: bool,
    pub paste_detail: &'static str,
}

impl CardView {
    pub fn relative_time(&self) -> String {
        format::relative_time(self.created_at_ms, format::now_ms())
    }
}

/// UI theme mode (dark / light).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    Dark,
    #[default]
    Light,
}

impl ThemeMode {
    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "暗色",
            ThemeMode::Light => "亮色",
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::Light,
        }
    }
}

/// Maximum-saved-items setting (checkbox + count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaxItemsConfig {
    pub enabled: bool,
    pub count: u32,
}

impl Default for MaxItemsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            count: Self::DEFAULT_COUNT,
        }
    }
}

impl MaxItemsConfig {
    pub const DEFAULT_COUNT: u32 = 1000;
    pub const MIN_COUNT: u32 = 1;
    pub const MAX_COUNT: u32 = 100_000;

    /// Parse a user-entered count. Only a plain integer inside the allowed range is valid.
    pub fn parse_count(input: &str) -> Option<u32> {
        let value = input.trim().parse::<u32>().ok()?;
        (Self::MIN_COUNT..=Self::MAX_COUNT)
            .contains(&value)
            .then_some(value)
    }

    /// Rebuild the setting from its stored values; anything missing or invalid falls back to
    /// "disabled, 1000".
    pub fn from_settings(enabled: Option<&str>, count: Option<&str>) -> Self {
        Self {
            enabled: enabled == Some("1"),
            count: count
                .and_then(Self::parse_count)
                .unwrap_or(Self::DEFAULT_COUNT),
        }
    }
}

/// Custom expiry-days setting (checkbox + days).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpiryConfig {
    pub enabled: bool,
    pub days: u32,
}

impl Default for ExpiryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            days: Self::DEFAULT_DAYS,
        }
    }
}

impl ExpiryConfig {
    pub const DEFAULT_DAYS: u32 = 90;
    pub const MIN_DAYS: u32 = 1;
    pub const MAX_DAYS: u32 = 3650;

    /// Parse a user-entered number of days. Only a plain integer inside the allowed range is
    /// valid.
    pub fn parse_days(input: &str) -> Option<u32> {
        let value = input.trim().parse::<u32>().ok()?;
        (Self::MIN_DAYS..=Self::MAX_DAYS)
            .contains(&value)
            .then_some(value)
    }

    /// Rebuild the setting from its stored values.
    ///
    /// Versions before 0.2.6 stored a fixed `retention` choice (`day1`, `day7`, `day30`,
    /// `never`). When no expiry value has been saved yet that choice is carried over, so an
    /// upgrade keeps the user's old policy. With neither stored, expiry is on for 90 days.
    pub fn from_settings(
        enabled: Option<&str>,
        days: Option<&str>,
        legacy_retention: Option<&str>,
    ) -> Self {
        let (legacy_enabled, legacy_days) = match legacy_retention {
            Some("day1") => (true, 1),
            Some("day7") => (true, 7),
            Some("day30") => (true, 30),
            Some("never") => (false, Self::DEFAULT_DAYS),
            _ => (true, Self::DEFAULT_DAYS),
        };
        Self {
            enabled: match enabled {
                Some(value) => value == "1",
                None => legacy_enabled,
            },
            days: days.and_then(Self::parse_days).unwrap_or(
                if enabled.is_some() || days.is_some() {
                    Self::DEFAULT_DAYS
                } else {
                    legacy_days
                },
            ),
        }
    }

    pub fn label(self) -> String {
        if self.enabled {
            format!("{} 天", self.days)
        } else {
            "永不".to_string()
        }
    }
}

// ── Quick paste panel settings ──────────────────────────────────────────────

/// Where the quick paste panel should appear when triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuickPopupPosition {
    /// Near the text insertion caret of the target window.
    InsertSymbol,
    /// At the mouse cursor position.
    Cursor,
    /// At the position the panel was last shown at.
    #[default]
    LastPosition,
}

impl QuickPopupPosition {
    pub fn label(self) -> &'static str {
        match self {
            Self::InsertSymbol => "插入符号处",
            Self::Cursor => "光标处",
            Self::LastPosition => "上次弹出位置",
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::InsertSymbol => "insert_symbol",
            Self::Cursor => "cursor",
            Self::LastPosition => "last_position",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "insert_symbol" => Self::InsertSymbol,
            "cursor" => Self::Cursor,
            _ => Self::LastPosition,
        }
    }
}

/// Title bar position on the quick paste panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuickTitlePosition {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
}

impl QuickTitlePosition {
    pub fn label(self) -> &'static str {
        match self {
            Self::Top => "顶部",
            Self::Bottom => "底部",
            Self::Left => "左侧",
            Self::Right => "右侧",
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Bottom => "bottom",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "bottom" => Self::Bottom,
            "left" => Self::Left,
            "right" => Self::Right,
            _ => Self::Top,
        }
    }
}

/// Quick panel font family. `Default` keeps the CJK-capable UI font; `Mono` uses Consolas,
/// which has no Han glyphs so it only suits ASCII snippets (codes, tokens).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuickFontFamily {
    #[default]
    Default,
    Mono,
}

impl QuickFontFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "默认",
            Self::Mono => "等宽",
        }
    }

    pub fn as_key(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Mono => "mono",
        }
    }

    pub fn from_key(s: &str) -> Self {
        match s {
            "mono" => Self::Mono,
            _ => Self::Default,
        }
    }

    /// Resolve to the iced [`Font`] to apply on text widgets.
    pub fn font(self) -> iced::Font {
        match self {
            Self::Default => crate::font::name_font(),
            Self::Mono => crate::font::mono_font(),
        }
    }
}

/// All settings for the quick paste panel, persisted via `quick_*` keys.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QuickSettings {
    /// Move the pasted item to the top of the list after a paste.
    pub move_to_top_after_paste: bool,
    /// Number of lines to display per clipboard item (default 1).
    pub display_lines: u32,
    /// Where the panel pops up.
    pub popup_position: QuickPopupPosition,
    /// Always-on-top window level (bound to Ctrl+Space toggle).
    pub always_on_top: bool,
    /// Auto-dismiss the panel when it loses focus.
    pub auto_dismiss: bool,
    /// Panel opacity: 1.0 = fully opaque, 0.0 = fully transparent.
    pub opacity: f32,
    /// Title bar position.
    pub title_position: QuickTitlePosition,
    /// Body font size for quick panel rows (11–18, default 13).
    pub font_size: u32,
    /// Body font family for quick panel rows.
    pub font_family: QuickFontFamily,
}

impl Default for QuickSettings {
    fn default() -> Self {
        Self {
            move_to_top_after_paste: false,
            display_lines: 1,
            popup_position: QuickPopupPosition::default(),
            always_on_top: true,
            auto_dismiss: true,
            opacity: 1.0,
            title_position: QuickTitlePosition::default(),
            font_size: Self::DEFAULT_FONT_SIZE,
            font_family: QuickFontFamily::default(),
        }
    }
}

impl QuickSettings {
    pub const MIN_DISPLAY_LINES: u32 = 1;
    pub const MAX_DISPLAY_LINES: u32 = 10;
    pub const MIN_OPACITY: f32 = 0.1;
    pub const MAX_OPACITY: f32 = 1.0;
    pub const MIN_FONT_SIZE: u32 = 11;
    pub const MAX_FONT_SIZE: u32 = 18;
    pub const DEFAULT_FONT_SIZE: u32 = 13;

    /// Parse a user-entered line count. Only a plain integer in the allowed range is valid.
    pub fn parse_display_lines(input: &str) -> Option<u32> {
        let value = input.trim().parse::<u32>().ok()?;
        (Self::MIN_DISPLAY_LINES..=Self::MAX_DISPLAY_LINES)
            .contains(&value)
            .then_some(value)
    }

    /// Parse a user-entered opacity percentage (0–100). Returns 0.1–1.0.
    pub fn parse_opacity(input: &str) -> Option<f32> {
        let value = input.trim().parse::<u32>().ok()?;
        if value == 0 {
            return None; // 0% is not allowed — would be invisible
        }
        let clamped = value.min(100) as f32 / 100.0;
        Some(clamped.max(Self::MIN_OPACITY))
    }

    /// Parse a user-entered font size (11–18).
    pub fn parse_font_size(input: &str) -> Option<u32> {
        let value = input.trim().parse::<u32>().ok()?;
        (Self::MIN_FONT_SIZE..=Self::MAX_FONT_SIZE)
            .contains(&value)
            .then_some(value)
    }

    /// Rebuild the setting from its stored values; anything missing falls back to defaults.
    pub fn from_settings(
        move_to_top: Option<&str>,
        display_lines: Option<&str>,
        popup_position: Option<&str>,
        always_on_top: Option<&str>,
        auto_dismiss: Option<&str>,
        opacity: Option<&str>,
        title_position: Option<&str>,
        font_size: Option<&str>,
        font_family: Option<&str>,
    ) -> Self {
        Self {
            move_to_top_after_paste: move_to_top == Some("1"),
            display_lines: display_lines
                .and_then(Self::parse_display_lines)
                .unwrap_or(1),
            popup_position: popup_position
                .map(QuickPopupPosition::from_key)
                .unwrap_or_default(),
            always_on_top: always_on_top.map(|s| s != "0").unwrap_or(true),
            auto_dismiss: auto_dismiss.map(|s| s != "0").unwrap_or(true),
            opacity: opacity
                .and_then(|s| s.parse::<f32>().ok())
                .map(|v| v.clamp(Self::MIN_OPACITY, Self::MAX_OPACITY))
                .unwrap_or(1.0),
            title_position: title_position
                .map(QuickTitlePosition::from_key)
                .unwrap_or_default(),
            font_size: font_size
                .and_then(Self::parse_font_size)
                .unwrap_or(Self::DEFAULT_FONT_SIZE),
            font_family: font_family
                .map(QuickFontFamily::from_key)
                .unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct UiState {
    /// Advances only when the worker publishes a changed snapshot.
    pub revision: u64,
    pub cards: Arc<[CardView]>,
    pub history_query: HistoryQuery,
    pub history_active: bool,
    pub history_has_next: bool,
    pub history_loading: bool,
    /// The in-flight load adds the next page to the rows already shown (infinite scroll),
    /// rather than replacing them.
    pub history_appending: bool,
    pub detail_content: Option<(u32, Arc<str>)>,
    pub detail_error: Option<(u32, String)>,
    pub modal_image: Option<(u32, iced::widget::image::Handle)>,
    pub selected: Option<u32>,
    pub record_count: usize,
    pub tracking: bool,
    /// Last tracking toggle requested by the UI and applied by the worker.
    pub tracking_ack: u64,
    pub excluded_apps: Vec<String>,
    pub expanded_raw: bool,
    pub status_note: String,
    pub hotkey_selected: Hotkey,
    pub hotkey_active: Hotkey,
    pub hotkey_error: Option<String>,
    pub theme_mode: ThemeMode,
    pub max_items: MaxItemsConfig,
    pub expiry: ExpiryConfig,
    /// Quick paste panel settings.
    pub quick_settings: QuickSettings,
    /// Card groups for the quick-panel tab bar.
    pub groups: Vec<GroupSummary>,
}

pub type SharedUi = Arc<Mutex<UiState>>;

pub fn card_view_from_summary(
    summary: &HistorySummary,
    cached_image: Option<&iced::widget::image::Handle>,
) -> CardView {
    let c = &summary.representative;
    let (kind, payload, content_preview) = match &c.payload {
        HistoryPayload::Text => (
            format::history_kind(&c.payload, &c.content),
            CardPayloadView::Text,
            format::preview_line(&c.content),
        ),
        HistoryPayload::Files(paths) => (
            ContentKind::File,
            CardPayloadView::Files(paths.clone()),
            format::payload_preview(&ClipboardPayload::Files(paths.clone()), ""),
        ),
        HistoryPayload::Image {
            width,
            height,
            encoded_bytes,
        } => (
            ContentKind::Image,
            CardPayloadView::Image {
                handle: cached_image.cloned(),
                thumbnail_failed: false,
                width: *width,
                height: *height,
                encoded_bytes: *encoded_bytes,
            },
            format!("图片 · {width} × {height}"),
        ),
    };
    let used_in: Vec<_> = summary
        .used_in
        .iter()
        .map(|u| UsedInView {
            target_app: u.target_app.clone(),
            target_window: u.target_window.clone(),
            method_label: u.method_label.clone(),
            time_label: format::time_label(u.triggered_at_ms),
        })
        .collect();
    let mut used_in_apps = Vec::new();
    for u in &used_in {
        if !used_in_apps.contains(&u.target_app) {
            used_in_apps.push(u.target_app.clone());
        }
    }
    CardView {
        sequence: c.sequence,
        raw_count: summary.copy_count(),
        raw_sequences: summary.raw_sequences.clone(),
        content_preview,
        content_excerpt: c.content.chars().take(512).collect(),
        content_chars: c.content.chars().count(),
        content_hash: c.content_hash.clone(),
        payload,
        kind,
        pinned: c.pinned,
        pin_rank: c.pin_rank,
        item_hotkey: c.item_hotkey.clone(),
        auto_delete: c.auto_delete,
        source_app: c.source_app.clone(),
        source_confidence: c.source_confidence,
        source_window: c.source_window.clone(),
        time_full: format::full_time_label(c.created_at_ms),
        created_at_ms: c.created_at_ms,
        time_range: format::time_range_label(c.created_at_ms, summary.last_created_at_ms),
        has_paste_activity: !used_in.is_empty(),
        used_in,
        used_in_apps,
        paste_detail: PasteConfidence::HotkeyObserved.detail_copy(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn expiry_defaults_to_ninety_days_enabled() {
        assert_eq!(
            ExpiryConfig::from_settings(None, None, None),
            ExpiryConfig {
                enabled: true,
                days: 90
            }
        );
    }

    #[test]
    fn expiry_carries_over_the_legacy_retention_choice() {
        let migrated = |legacy| ExpiryConfig::from_settings(None, None, Some(legacy));
        assert_eq!(
            migrated("day1"),
            ExpiryConfig {
                enabled: true,
                days: 1
            }
        );
        assert_eq!(
            migrated("day7"),
            ExpiryConfig {
                enabled: true,
                days: 7
            }
        );
        assert_eq!(
            migrated("day30"),
            ExpiryConfig {
                enabled: true,
                days: 30
            }
        );
        assert_eq!(
            migrated("never"),
            ExpiryConfig {
                enabled: false,
                days: 90
            }
        );
    }

    #[test]
    fn saved_expiry_values_win_over_the_legacy_retention_choice() {
        assert_eq!(
            ExpiryConfig::from_settings(Some("0"), Some("14"), Some("day1")),
            ExpiryConfig {
                enabled: false,
                days: 14
            }
        );
        assert_eq!(
            ExpiryConfig::from_settings(Some("1"), Some("bad"), Some("day1")),
            ExpiryConfig {
                enabled: true,
                days: 90
            }
        );
    }

    #[test]
    fn expiry_and_max_items_inputs_must_be_plain_in_range_integers() {
        assert_eq!(ExpiryConfig::parse_days("90"), Some(90));
        assert_eq!(ExpiryConfig::parse_days(" 7 "), Some(7));
        assert_eq!(ExpiryConfig::parse_days(""), None);
        assert_eq!(ExpiryConfig::parse_days("0"), None);
        assert_eq!(ExpiryConfig::parse_days("3651"), None);
        assert_eq!(ExpiryConfig::parse_days("-3"), None);
        assert_eq!(ExpiryConfig::parse_days("1.5"), None);
        assert_eq!(MaxItemsConfig::parse_count("1000"), Some(1000));
        assert_eq!(MaxItemsConfig::parse_count("0"), None);
        assert_eq!(MaxItemsConfig::parse_count("100001"), None);
        assert_eq!(MaxItemsConfig::parse_count("abc"), None);
    }

    #[test]
    fn max_items_defaults_to_disabled_with_a_thousand_and_ignores_bad_values() {
        assert_eq!(
            MaxItemsConfig::from_settings(None, None),
            MaxItemsConfig {
                enabled: false,
                count: 1000
            }
        );
        assert_eq!(
            MaxItemsConfig::from_settings(Some("1"), Some("250")),
            MaxItemsConfig {
                enabled: true,
                count: 250
            }
        );
        assert_eq!(
            MaxItemsConfig::from_settings(Some("1"), Some("0")),
            MaxItemsConfig {
                enabled: true,
                count: 1000
            }
        );
    }

    use super::*;

    pub(super) fn summary(content: String) -> HistorySummary {
        HistorySummary {
            representative: veya_core::HistoryRecord {
                sequence: 1,
                content,
                payload: HistoryPayload::Text,
                content_hash: "text-hash".into(),
                source_app: "editor.exe".into(),
                source_window: "source".into(),
                source_confidence: SourceConfidence::Likely,
                created_at_ms: 1,
                pinned: false,
                pin_rank: 0,
                item_hotkey: String::new(),
                auto_delete: false,
                pastes: Vec::new(),
            },
            raw_sequences: vec![1],
            used_in: Vec::new(),
            last_created_at_ms: 1,
        }
    }

    #[test]
    fn bounded_excerpt_preserves_full_text_classification_and_unicode() {
        let full = format!("{} fn tail() {{}}", "中".repeat(1_000));
        let card = card_view_from_summary(&summary(full.clone()), None);
        assert_eq!(card.kind, ContentKind::Code);
        assert_eq!(card.content_excerpt.chars().count(), 512);
        assert_eq!(card.content_chars, full.chars().count());
        assert!(!card.content_excerpt.contains("fn tail"));
        assert_eq!(card.source_confidence, SourceConfidence::Likely);
    }

    #[test]
    fn image_projection_is_selectable_without_original_bytes_or_thumbnail() {
        let mut summary = summary("图片 2048 × 2048".into());
        summary.representative.payload = HistoryPayload::Image {
            width: 2048,
            height: 2048,
            encoded_bytes: 80_000,
        };
        let card = card_view_from_summary(&summary, None);
        assert_eq!(card.kind, ContentKind::Image);
        assert_eq!(card.sequence, 1);
        assert!(matches!(
            card.payload,
            CardPayloadView::Image {
                handle: None,
                width: 2048,
                ..
            }
        ));
    }
}

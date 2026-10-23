//! The hotkey picker reuses bounded worker history and typed clipboard replay.
use super::history::card_menu_origin;
use super::*;
use iced::widget::column;

const QUICK_WIDTH: f32 = 440.0;
/// History rows that are fully visible when the picker opens (on a screen tall enough).
const QUICK_VISIBLE_ROWS: usize = 10;
const ROW_HEIGHT: f32 = 52.0;
/// Everything in the picker except the list: header, search box, group tabs, dividers,
/// footer and padding. Slightly generous, so ten full rows fit even with larger fonts.
const QUICK_CHROME_HEIGHT: f32 = 204.0;
const QUICK_HEIGHT: f32 = QUICK_VISIBLE_ROWS as f32 * ROW_HEIGHT + QUICK_CHROME_HEIGHT;
const QUICK_MENU_WIDTH: f32 = 200.0;
const QUICK_MENU_ITEM_HEIGHT: f32 = 34.0;
/// Rows from the end of the loaded list at which keyboard navigation loads the next page.
const LOAD_MORE_LOOKAHEAD: usize = 5;
/// Width of the right-hand strip (close / fold / title). It is also the whole
/// window width while the panel is folded.
const STRIP_WIDTH: f32 = 28.0;
/// The first rows are numbered 1-9 then 0, matching Ctrl+digit.
const NUMBERED_ROWS: usize = 10;
const ROW_NUMBER_LABELS: [&str; NUMBERED_ROWS] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
const NUMBER_COLUMN_WIDTH: f32 = 22.0;
/// Two presses on the strip closer than this toggle the always-on-top pin.
const STRIP_DOUBLE_CLICK: std::time::Duration = std::time::Duration::from_millis(400);

pub(super) struct QuickPanel {
    target: Option<PasteTarget>,
    saved: ManagementState,
    pending: Option<u64>,
    pub(super) sending: bool,
    error: Option<String>,
    strip: Strip,
    ctrl_down: bool,
}

/// Pin / fold state of the picker. It lives inside `QuickPanel`, so leaving
/// the picker (Esc, close button, opening the manager) resets it.
#[derive(Default)]
struct Strip {
    pinned: bool,
    folded: bool,
    last_press: Option<std::time::Instant>,
    /// Window size to return to when unfolding.
    expanded_size: iced::Size,
    /// Whether the panel was auto-expanded because the mouse hovered over the
    /// collapsed strip.  When true, moving the mouse off the panel folds it
    /// back; clicking the fold button clears the flag and keeps the panel
    /// manually unfolded.
    hover_expanded: bool,
}

struct ManagementState {
    search: String,
    filter: Filter,
    pinned_only: bool,
    sort_order: SortOrder,
    history_page: usize,
    selected: Option<u32>,
    size: iced::Size,
}

impl QuickPanel {
    pub(super) fn strip_folded(&self) -> bool {
        self.strip.folded
    }

    pub(super) fn set_ctrl_down(&mut self, down: bool) {
        self.ctrl_down = down;
    }

    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.sending
    }

    fn accepts(&self, request_id: u64) -> bool {
        self.pending == Some(request_id)
    }
}

impl App {
    pub(super) fn advance_window_generation(&self) -> u64 {
        self.window_generation
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            .wrapping_add(1)
    }

    pub(super) fn generation_is_current(&self, generation: u64) -> bool {
        self.window_generation
            .load(std::sync::atomic::Ordering::SeqCst)
            == generation
    }

    pub(super) fn active_scroll_id(&self) -> scrollable::Id {
        if self.quick.is_some() {
            quick_scroll_id()
        } else {
            history_scroll_id()
        }
    }

    pub(super) fn show_quick(&mut self, target: Option<PasteTarget>) -> Task<Message> {
        let generation = self.advance_window_generation();
        self.cancel_hotkey_recording();
        self.unload_content();
        self.unload_full_image();
        if self.quick.is_none() {
            self.quick = Some(QuickPanel {
                target,
                saved: ManagementState {
                    search: self.search.clone(),
                    filter: self.filter,
                    pinned_only: self.pinned_only,
                    sort_order: self.sort_order,
                    history_page: self.history_page,
                    selected: self.state.selected,
                    size: self.window_size,
                },
                pending: None,
                sending: false,
                error: self.last_paste_error.take(),
                strip: Strip::default(),
                ctrl_down: false,
            });
        } else if let Some(quick) = &mut self.quick {
            quick.ctrl_down = false;
            quick.target = target;
            quick.pending = None;
            quick.error = None;
        }
        self.search.clear();
        self.filter = Filter::All;
        self.pinned_only = false;
        self.sort_order = SortOrder::Newest;
        self.history_page = 0;
        // Every opening starts on "全部" with no half-finished group edit.
        self.quick_group_sel = None;
        self.quick_group_input = None;
        self.page = Page::History;
        self.detail_menu_open = false;
        self.card_menu = None;
        self.clear_history_confirmation_open = false;
        self.window_hidden = false;
        self.set_history_active(true);
        let query = self.request_history_query(true);
        if self.quick.as_ref().is_some_and(|quick| quick.strip.pinned) {
            // A pinned picker keeps the place the user dragged it to; the hotkey
            // only refreshes the paste target and brings the panel back to focus.
            let unfold = self.set_quick_folded(false);
            return Task::batch([query, unfold, activate_window(generation)]);
        }
        #[cfg(windows)]
        let placement =
            veya_windows::platform::window_place::startup_placement(QUICK_WIDTH, QUICK_HEIGHT);
        #[cfg(not(windows))]
        let placement: Option<()> = None;
        let size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        #[cfg(windows)]
        let size = placement
            .map(|p| iced::Size::new(p.logical_size.0, p.logical_size.1))
            .unwrap_or(size);
        let window = iced::window::get_latest().then(move |id| {
            let Some(id) = id else { return Task::none() };
            #[cfg(windows)]
            let position = placement.map(|p| p.physical_position);
            #[cfg(not(windows))]
            let position = {
                let _ = placement;
                None
            };
            iced::window::get_scale_factor(id).map(move |scale| Message::QuickPositioned {
                generation,
                id,
                size,
                position: position.map(|physical| {
                    #[cfg(windows)]
                    let logical = veya_windows::platform::window_place::physical_to_window_logical(
                        physical, scale,
                    );
                    #[cfg(not(windows))]
                    let logical = (physical.0 / scale, physical.1 / scale);
                    iced::Point::new(logical.0, logical.1)
                }),
            })
        });
        Task::batch([query, window])
    }

    fn restore_management_state(&mut self) -> Option<iced::Size> {
        let quick = self.quick.take()?;
        self.search = quick.saved.search;
        self.filter = quick.saved.filter;
        self.pinned_only = quick.saved.pinned_only;
        self.sort_order = quick.saved.sort_order;
        self.history_page = quick.saved.history_page;
        self.state.selected = quick.saved.selected;
        Some(quick.saved.size)
    }

    pub(super) fn open_manager(&mut self) -> Task<Message> {
        if self.quick.as_ref().is_some_and(|quick| quick.sending) {
            return Task::none();
        }
        let leave = self.leave_strip_state();
        let size = self.restore_management_state();
        let generation = self.advance_window_generation();
        self.window_hidden = false;
        self.set_history_active(self.page == Page::History);
        let query = self.request_history_query(false);
        let window = leave.chain(iced::window::get_latest().then(move |id| {
            let resize = match (id, size) {
                (Some(id), Some(size)) => iced::window::resize(id, size),
                _ => Task::none(),
            };
            resize.chain(activate_window(generation))
        }));
        Task::batch([query, window])
    }

    /// Undo what pinning and folding did to the native window (topmost level,
    /// shifted position) before the shared window returns to manager use.
    fn leave_strip_state(&self) -> Task<Message> {
        let Some(quick) = &self.quick else {
            return Task::none();
        };
        let shift = if quick.strip.folded {
            shift_window_x(quick.strip.expanded_size.width - STRIP_WIDTH)
        } else {
            Task::none()
        };
        if quick.strip.pinned {
            shift.chain(set_window_level(iced::window::Level::Normal))
        } else {
            shift
        }
    }

    pub(super) fn dismiss_quick(&mut self, restore: bool) -> Task<Message> {
        let Some(quick) = &self.quick else {
            return Task::none();
        };
        if quick.sending {
            return Task::none();
        }
        let target = quick.target;
        self.quick_group_input = None;
        let leave = self.leave_strip_state();
        let generation = self.advance_window_generation();
        let size = self.restore_management_state();
        self.window_hidden = true;
        self.set_history_active(false);
        hide_window()
            .chain(leave)
            .chain(iced::window::get_latest().then(move |id| match (id, size) {
                (Some(id), Some(size)) => iced::window::resize(id, size),
                _ => Task::none(),
            }))
            .chain(Task::done(Message::QuickHidden {
                generation,
                target,
                restore,
            }))
    }

    pub(super) fn position_quick(
        &mut self,
        generation: u64,
        id: iced::window::Id,
        size: iced::Size,
        position: Option<iced::Point>,
    ) -> Task<Message> {
        if !self.generation_is_current(generation) || self.quick.is_none() || self.window_hidden {
            return Task::none();
        }
        iced::window::maximize(id, false)
            .chain(iced::window::resize(id, size))
            .chain(
                position
                    .map(|point| iced::window::move_to(id, point))
                    .unwrap_or_else(Task::none),
            )
            .chain(iced::window::change_mode(id, iced::window::Mode::Windowed))
            .chain(iced::window::minimize(id, false))
            .chain(Task::done(Message::QuickShown { generation, id }))
    }

    pub(super) fn focus_quick(&mut self, generation: u64, id: iced::window::Id) -> Task<Message> {
        if !self.generation_is_current(generation) || self.quick.is_none() || self.window_hidden {
            return if self.window_hidden {
                hide_window()
            } else {
                Task::none()
            };
        }
        self.window_id = Some(id);
        self.chrome_sync_pending = true;
        self.chrome_sync_attempts = 0;
        iced::window::gain_focus(id).chain(text_input::focus(search_input_id()))
    }

    pub(super) fn quick_hidden(
        &mut self,
        generation: u64,
        target: Option<PasteTarget>,
        restore: bool,
    ) -> Task<Message> {
        if !self.generation_is_current(generation) || !self.window_hidden || !restore {
            return Task::none();
        }
        let Some(target) = target else {
            return Task::none();
        };
        let token = self.window_generation.clone();
        Task::perform(
            run_system_action(move || {
                if token.load(std::sync::atomic::Ordering::SeqCst) != generation {
                    return Ok(());
                }
                veya_windows::platform::paste::restore_target(target)
            }),
            move |result| Message::TargetRestored { generation, result },
        )
    }

    pub(super) fn quick_paste(&mut self, sequence: u32) -> Task<Message> {
        if self.history_list_pending()
            || !self
                .state
                .cards
                .iter()
                .any(|card| card.sequence == sequence)
        {
            return Task::none();
        }
        let Some(quick) = &mut self.quick else {
            return Task::none();
        };
        if quick.busy() {
            return Task::none();
        }
        if quick.target.is_none() {
            quick.error = Some("请在需要粘贴的应用中按快捷键打开面板".into());
            return Task::none();
        }
        self.next_paste_request = self.next_paste_request.wrapping_add(1);
        let request_id = self.next_paste_request;
        quick.pending = Some(request_id);
        quick.error = None;
        if self
            .worker
            .cmd_tx
            .send(WorkerCmd::PreparePaste {
                sequence,
                request_id,
            })
            .is_err()
        {
            quick.pending = None;
            quick.error = Some("剪贴板后台已停止，请重新启动 Veya".into());
        }
        Task::none()
    }

    pub(super) fn quick_prepared(
        &mut self,
        request_id: u64,
        result: Result<u32, String>,
    ) -> Task<Message> {
        let Some(quick) = &mut self.quick else {
            return Task::none();
        };
        if !quick.accepts(request_id) || quick.sending {
            return Task::none();
        }
        match result {
            Err(error) => {
                quick.pending = None;
                quick.error = Some(error);
                Task::none()
            }
            Ok(sequence) => {
                let Some(target) = quick.target else {
                    return Task::none();
                };
                quick.sending = true;
                let pinned = quick.strip.pinned;
                let paste = Task::perform(
                    run_system_action(move || {
                        veya_windows::platform::paste::paste_to_target(target, sequence)
                    }),
                    move |result| Message::QuickFinished { request_id, result },
                );
                if pinned {
                    // A pinned picker stays on screen while the paste is sent.
                    return paste;
                }
                self.window_hidden = true;
                self.set_history_active(false);
                hide_window().chain(paste)
            }
        }
    }

    pub(super) fn finish_quick_paste(
        &mut self,
        request_id: u64,
        result: Result<(), String>,
    ) -> Task<Message> {
        let Some(quick) = &mut self.quick else {
            return Task::none();
        };
        if !quick.accepts(request_id) || !quick.sending {
            return Task::none();
        }
        quick.pending = None;
        quick.sending = false;
        if quick.strip.pinned {
            // Pinned: report a failure in place and keep the panel open either way.
            if let Err(error) = result {
                quick.error = Some(error);
            }
            self.window_hidden = false;
            self.set_history_active(true);
            return Task::none();
        }
        if let Err(error) = result {
            let target = quick.target;
            let current = veya_windows::platform::paste::capture_target();
            if current.is_some() && current != target {
                self.last_paste_error = Some(error);
                return self.dismiss_quick(false);
            }
            quick.error = Some(error);
            self.window_hidden = false;
            self.set_history_active(true);
            activate_window(
                self.window_generation
                    .load(std::sync::atomic::Ordering::SeqCst),
            )
        } else {
            let size = self.restore_management_state();
            self.advance_window_generation();
            iced::window::get_latest().then(move |id| match (id, size) {
                (Some(id), Some(size)) => iced::window::resize(id, size),
                _ => Task::none(),
            })
        }
    }

    /// Ctrl+1..9,0 pastes the matching numbered row; the number is the row's
    /// position in the list as currently filtered and sorted.
    pub(super) fn quick_paste_nth(&mut self, index: usize) -> Task<Message> {
        if index >= NUMBERED_ROWS {
            return Task::none();
        }
        let Some(sequence) = self.state.cards.iter().nth(index).map(|card| card.sequence) else {
            return Task::none();
        };
        self.card_menu = None;
        self.quick_paste(sequence)
    }

    /// Windows reports Ctrl+1/9/0 with a text payload, so the focused search
    /// box would also type the digit. While Ctrl is held, a change that only
    /// inserts one digit is that echo and must not edit the search text.
    pub(super) fn is_ctrl_digit_echo(&self, new: &str) -> bool {
        self.quick.as_ref().is_some_and(|quick| quick.ctrl_down)
            && is_single_digit_insertion(&self.search, new)
    }

    /// Press on the blank part of the right strip: a quick second press pins or
    /// unpins the window, a single press starts dragging it.
    pub(super) fn quick_strip_press(&mut self) -> Task<Message> {
        let Some(quick) = &mut self.quick else {
            return Task::none();
        };
        let now = std::time::Instant::now();
        let double = quick
            .strip
            .last_press
            .is_some_and(|last| now.duration_since(last) < STRIP_DOUBLE_CLICK);
        if !double {
            quick.strip.last_press = Some(now);
            return self
                .window_id
                .map(iced::window::drag)
                .unwrap_or_else(Task::none);
        }
        quick.strip.last_press = None;
        if quick.strip.folded {
            // A folded panel is always pinned; unfold it first.
            return Task::none();
        }
        quick.strip.pinned = !quick.strip.pinned;
        let level = if quick.strip.pinned {
            iced::window::Level::AlwaysOnTop
        } else {
            iced::window::Level::Normal
        };
        set_window_level(level)
    }

    /// Fold the picker down to the right strip, or unfold it again. Folding
    /// pins the window, otherwise losing focus would close it immediately.
    pub(super) fn set_quick_folded(&mut self, folded: bool) -> Task<Message> {
        let window_size = self.window_size;
        let Some(quick) = &mut self.quick else {
            return Task::none();
        };
        if quick.strip.folded == folded {
            return Task::none();
        }
        quick.strip.folded = folded;
        if folded {
            quick.strip.expanded_size = window_size;
            let shift = window_size.width - STRIP_WIDTH;
            let fold = resize_window(iced::Size::new(STRIP_WIDTH, window_size.height))
                .chain(shift_window_x(shift));
            if quick.strip.pinned {
                fold
            } else {
                quick.strip.pinned = true;
                Task::batch([set_window_level(iced::window::Level::AlwaysOnTop), fold])
            }
        } else {
            let size = quick.strip.expanded_size;
            shift_window_x(-(size.width - STRIP_WIDTH)).chain(resize_window(size))
        }
    }

    /// Focus loss closes the picker, unless it is pinned.
    pub(super) fn quick_unfocused(&mut self) -> Task<Message> {
        if self.quick.as_ref().is_some_and(|quick| quick.strip.pinned) {
            return Task::none();
        }
        if self.quick_group_input.is_some() {
            self.quick_group_input = None;
            return Task::none();
        }
        self.dismiss_quick(false)
    }

    /// While pinned, the user moves between other apps; keep the paste target
    /// on the last external window that was in front.
    pub(super) fn track_pinned_target(&mut self) {
        let Some(quick) = self.quick.as_mut() else {
            return;
        };
        if !quick.strip.pinned || quick.busy() {
            return;
        }
        if let Some(target) = veya_windows::platform::paste::capture_app_target() {
            quick.target = Some(target);
        }
    }

    /// Show (or clear) a warning line under the list of the quick picker.
    pub(super) fn set_quick_error(&mut self, error: Option<String>) {
        if let Some(quick) = &mut self.quick {
            quick.error = error;
        }
    }

    /// Why `name` cannot be used for a group, if it cannot.
    pub(super) fn group_name_problem(
        &self,
        name: &str,
        mode: QuickGroupInputMode,
    ) -> Option<String> {
        if name.is_empty() {
            return Some("分组名称不能为空".into());
        }
        let renamed = match mode {
            QuickGroupInputMode::Rename { id } => Some(id),
            QuickGroupInputMode::Create { .. } => None,
        };
        self.state
            .groups
            .iter()
            .any(|group| group.name == name && Some(group.id) != renamed)
            .then(|| "分组名称已存在".to_string())
    }

    pub(super) fn quick_move(&mut self, delta: i32) -> Task<Message> {
        if self.quick.as_ref().is_none_or(|quick| quick.busy()) || self.history_list_pending() {
            return Task::none();
        }
        let sequences: Vec<u32> = self.state.cards.iter().map(|card| card.sequence).collect();
        let Some(index) = moved_index(&sequences, self.state.selected, delta) else {
            return Task::none();
        };
        self.select_history_card(sequences[index]);
        // Arrow keys reach the end of the loaded rows without any scroll event, so ask for
        // the next page once the selection gets close to the end.
        if index + LOAD_MORE_LOOKAHEAD >= sequences.len() {
            self.request_history_load_more();
        }
        // Each row has a fixed height; keep the keyboard selection in view.
        let viewport = (self.window_size.height - QUICK_CHROME_HEIGHT).max(ROW_HEIGHT);
        let content = sequences.len() as f32 * ROW_HEIGHT;
        let overflow = (content - viewport).max(1.0);
        let offset =
            (index as f32 * ROW_HEIGHT - viewport * 0.5 + ROW_HEIGHT * 0.5).clamp(0.0, overflow);
        scrollable::snap_to(
            quick_scroll_id(),
            scrollable::RelativeOffset {
                x: 0.0,
                y: offset / overflow,
            },
        )
    }

    pub(super) fn quick_view(&self) -> Element<'_, Message> {
        let Some(quick) = &self.quick else {
            return Space::new(0, 0).into();
        };
        let busy = quick.busy();
        let pending = self.history_list_pending();
        let search: Element<'_, Message> = if let Some(input) = &self.quick_group_input {
            let field = text_input("输入分组名称…", &input.value)
                .id(group_input_id())
                .size(14)
                .padding(10)
                .style(theme::field_style)
                .on_input(Message::QuickGroupInputChanged)
                .on_submit(Message::QuickGroupInputSubmit);
            // In rename mode append a danger button to delete the group; create
            // mode has nothing to delete yet so only the input field is shown.
            if let QuickGroupInputMode::Rename { id } = input.mode {
                let delete = button(icons::icon(Icon::Trash, theme::DANGER(), 14.0))
                    .padding(8)
                    .style(theme::danger_button)
                    .on_press(Message::QuickDeleteGroup(id));
                row![field, delete]
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .into()
            } else {
                field.into()
            }
        } else {
            let field = text_input("搜索剪贴板历史…", &self.search)
                .id(search_input_id())
                .size(14)
                .padding(10)
                .style(theme::field_style);
            if busy {
                field.into()
            } else {
                field.on_input(Message::SearchChanged).into()
            }
        };
        let group_tabs = self.quick_group_tabs();
        let list: Element<'_, Message> = if pending || self.state.cards.is_empty() {
            container(
                column![
                    icons::icon(Icon::History, theme::FAINT(), 28.0),
                    meta(if pending {
                        "正在读取历史…"
                    } else if self.search.is_empty() {
                        "复制一些内容后，会显示在这里"
                    } else {
                        "没有匹配结果，试试其他关键词"
                    })
                    .size(13),
                ]
                .spacing(12)
                .align_x(Alignment::Center),
            )
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
        } else {
            let mut rows = column![].spacing(0);
            for (index, card) in self.state.cards.iter().enumerate() {
                let source = match card.source_confidence {
                    veya_core::SourceConfidence::Exact => short_app(&card.source_app),
                    veya_core::SourceConfidence::Likely => {
                        format!("{} · 推断", short_app(&card.source_app))
                    }
                    veya_core::SourceConfidence::Unknown => "来源未知".into(),
                };
                let mut metadata =
                    row![meta(source).size(11), meta(card.relative_time()).size(11)].spacing(8);
                if card.pinned {
                    metadata = metadata.push(icons::icon(Icon::Pin, theme::ACCENT(), 11.0));
                }
                let number: Element<'_, Message> = match ROW_NUMBER_LABELS.get(index) {
                    Some(label) => container(theme::keycap(label))
                        .width(Length::Fixed(NUMBER_COLUMN_WIDTH))
                        .center_x(Length::Fixed(NUMBER_COLUMN_WIDTH))
                        .into(),
                    None => Space::with_width(Length::Fixed(NUMBER_COLUMN_WIDTH)).into(),
                };
                let content = row![
                    number,
                    history::card_leading_visual(card, 32.0),
                    column![
                        body(compact_preview(&card.content_preview, 46))
                            .size(13)
                            .shaping(text::Shaping::Advanced),
                        metadata
                    ]
                    .spacing(3)
                    .width(Length::Fill),
                ]
                .spacing(8)
                .align_y(Alignment::Center);
                let row = button(content)
                    .width(Length::Fill)
                    .height(ROW_HEIGHT)
                    .padding(8)
                    .style(theme::card_button_quick(
                        self.state.selected == Some(card.sequence),
                        index % 2 == 1,
                        card.has_paste_activity,
                    ));
                let row: Element<'_, Message> = if busy {
                    mouse_area(row).into()
                } else {
                    mouse_area(row.on_press(Message::QuickPaste(card.sequence)))
                        .on_right_press(Message::QuickOpenCardMenu(card.sequence))
                        .into()
                };
                rows = rows.push(row);
            }
            scrollable(rows)
                .id(quick_scroll_id())
                .height(Length::Fill)
                .style(theme::scroll_style)
                .on_scroll(|viewport: scrollable::Viewport| Message::QuickScrolled(viewport))
                .into()
        };
        let footer = row![
            button(meta("管理历史").size(12))
                .style(theme::action_button)
                .on_press(Message::OpenManager),
            Space::with_width(Length::Fill),
            meta("↑ ↓ 选择 · Enter 粘贴 · Ctrl+1…0 快粘 · Esc 关闭").size(11),
        ]
        .spacing(4)
        .align_y(Alignment::Center);
        let mut layout = column![
            search,
            group_tabs,
            theme::hdivider(),
            list,
            theme::hdivider()
        ]
        .spacing(9);
        if let Some(error) = &quick.error {
            layout = layout.push(meta(error.clone()).size(12).color(theme::WARN()));
        } else if busy {
            layout = layout.push(meta("正在粘贴，请松开按键…").size(12));
        }
        let strip = self.quick_strip(quick);
        if quick.strip.folded {
            return container(strip)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::shell_style)
                .into();
        }
        let left = container(layout.push(footer))
            .padding(14)
            .width(Length::Fill)
            .height(Length::Fill);
        let base: Element<'_, Message> = container(row![left, theme::vdivider(), strip])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(theme::shell_style)
            .into();
        // When the panel was auto-expanded by hovering over the folded strip,
        // wrap the whole panel so that moving the mouse off it folds it back.
        let base = if quick.strip.hover_expanded {
            mouse_area(base).on_exit(Message::QuickHoverFold).into()
        } else {
            base
        };
        let overlay: Element<'_, Message> = self
            .card_menu
            .and_then(|menu| {
                self.state
                    .cards
                    .iter()
                    .find(|card| card.sequence == menu.sequence)
                    .map(|card| self.quick_card_menu(card, menu.position))
            })
            .unwrap_or_else(|| Space::new(Length::Fill, Length::Fill).into());
        stack([base, overlay]).into()
    }

    /// Right-hand strip: close and fold buttons on top, the title below. Blank
    /// space drags the window; a quick double press pins it on top.
    fn quick_strip(&self, quick: &QuickPanel) -> Element<'_, Message> {
        let pinned = quick.strip.pinned;
        let folded = quick.strip.folded;
        let tint = if pinned { theme::ACCENT() } else { theme::MUTED() };
        let close = button(icons::icon(Icon::Close, theme::MUTED(), 12.0))
            .padding(pad(5.0, 5.0, 5.0, 5.0))
            .style(theme::win_button)
            .on_press(Message::QuickDismiss { restore: true });
        let fold = button(meta(if folded { "<<" } else { ">>" }).size(13).color(tint))
            .padding(pad(2.0, 3.0, 2.0, 3.0))
            .style(theme::win_button)
            .on_press(Message::QuickToggleFold);
        let title = column![
            vertical_text("Veya", tint),
            Space::with_height(8),
            vertical_text(if pinned { "窗口置顶" } else { "快捷粘贴" }, tint),
        ]
        .align_x(Alignment::Center);
        let blank = mouse_area(
            container(title)
                .padding(pad(10.0, 0.0, 0.0, 0.0))
                .center_x(Length::Fill)
                .height(Length::Fill),
        )
        .on_press(Message::QuickStripPress)
        .on_enter(Message::QuickHoverExpand);
        container(
            column![close, fold, blank]
                .align_x(Alignment::Center)
                .spacing(2)
                .padding(pad(6.0, 0.0, 6.0, 0.0)),
        )
        .width(Length::Fixed(STRIP_WIDTH))
        .height(Length::Fill)
        .into()
    }

    /// Horizontal group-tab row: "全部" + one chip per group + a "+ 新建" button.
    /// Right-clicking a group tab offers rename / delete.
    fn quick_group_tabs(&self) -> Element<'_, Message> {
        let mut tabs: Vec<Element<'_, Message>> = Vec::new();
        let all_selected = self.quick_group_sel.is_none();
        let all_tab = button(meta("全部").size(12))
            .padding(pad(4.0, 10.0, 4.0, 10.0))
            .style(theme::chip_button(all_selected))
            .on_press(Message::QuickSelectGroup(None));
        tabs.push(all_tab.into());
        for group in self.state.groups.iter() {
            let selected = self.quick_group_sel == Some(group.id);
            let label = button(text(group.name.as_str()).size(12))
                .padding(pad(4.0, 10.0, 4.0, 10.0))
                .style(theme::chip_button(selected))
                .on_press(Message::QuickSelectGroup(Some(group.id)));
            let label = mouse_area(label).on_right_press(Message::QuickStartRenameGroup(group.id));
            tabs.push(label.into());
        }
        let add = button(
            row![
                icons::icon(Icon::Plus, theme::MUTED(), 12.0),
                meta("新建").size(11),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
        )
        .padding(pad(4.0, 8.0, 4.0, 8.0))
        .style(theme::ghost_button)
        .on_press(Message::QuickStartCreateGroup { add_sequence: None });
        tabs.push(add.into());
        let mut row = row![].spacing(6).align_y(Alignment::Center);
        for tab in tabs {
            row = row.push(tab);
        }
        // Many groups overflow the width: scroll the row sideways (a vertical scrollable
        // would clip them with no way to reach the rest).
        scrollable(row.padding(iced::Padding {
            top: 0.0,
            right: 0.0,
            bottom: 8.0,
            left: 0.0,
        }))
        .direction(scrollable::Direction::Horizontal(
            scrollable::Scrollbar::new().width(4).scroller_width(4),
        ))
        .width(Length::Fill)
        .height(Length::Shrink)
        .style(theme::scroll_style)
        .into()
    }

    /// Right-click context menu for a card inside the quick picker.
    ///
    /// While a group tab is selected the first entry removes the card from that group;
    /// the other groups can be added to, and "新建分组…" creates a group holding the card.
    fn quick_card_menu(&self, card: &CardView, position: iced::Point) -> Element<'_, Message> {
        let menu_item = |icon: Icon, label: String, message: Message| {
            button(
                row![
                    icons::icon(icon, theme::MUTED(), 14.0),
                    text(label).size(12).wrapping(text::Wrapping::None),
                ]
                .spacing(9)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(pad(8.0, 10.0, 8.0, 10.0))
            .on_press(message)
            .style(theme::context_menu_item(false))
        };
        let mut items: Vec<Element<'_, Message>> = Vec::new();
        if let Some(current) = self
            .quick_group_sel
            .and_then(|id| self.state.groups.iter().find(|group| group.id == id))
        {
            items.push(
                menu_item(
                    Icon::Close,
                    format!("从「{}」移除", current.name),
                    Message::QuickRemoveFromGroup {
                        group_id: current.id,
                        sequence: card.sequence,
                    },
                )
                .into(),
            );
        }
        for group in self
            .state
            .groups
            .iter()
            .filter(|group| Some(group.id) != self.quick_group_sel)
        {
            items.push(
                menu_item(
                    Icon::Pin,
                    format!("添加到「{}」", group.name),
                    Message::QuickAddToGroup {
                        group_id: group.id,
                        sequence: card.sequence,
                    },
                )
                .into(),
            );
        }
        items.push(
            menu_item(
                Icon::Plus,
                "新建分组…".to_string(),
                Message::QuickStartCreateGroup {
                    add_sequence: Some(card.sequence),
                },
            )
            .into(),
        );
        // Size the menu from its entries so it can be kept inside the window, and let it
        // scroll when there are more groups than fit.
        let content_height = quick_menu_content_height(items.len());
        let max_height = (self.window_size.height - 16.0).max(QUICK_MENU_ITEM_HEIGHT + 8.0);
        let menu_height = content_height.min(max_height);
        let list: Element<'_, Message> = if content_height > menu_height {
            scrollable(column(items).spacing(2))
                .height(Length::Fixed(menu_height - 8.0))
                .style(theme::scroll_style)
                .into()
        } else {
            column(items).spacing(2).into()
        };
        let menu = mouse_area(
            container(list)
                .width(Length::Fixed(QUICK_MENU_WIDTH))
                .padding(4)
                .style(theme::context_menu_panel),
        )
        .on_press(Message::QuickCloseCardMenu)
        .on_right_press(Message::QuickCloseCardMenu);
        let origin = card_menu_origin(
            position,
            self.window_size,
            iced::Size::new(QUICK_MENU_WIDTH, menu_height),
        );
        let positioned = container(menu)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .padding(iced::Padding {
                top: origin.y,
                left: origin.x,
                right: 0.0,
                bottom: 0.0,
            });
        let dismiss = mouse_area(Space::new(Length::Fill, Length::Fill))
            .on_press(Message::QuickCloseCardMenu);
        stack([dismiss.into(), positioned.into()]).into()
    }
}

/// Text laid out top to bottom, one glyph per line (iced cannot rotate text).
fn vertical_text<'a>(label: &'a str, color: Color) -> Element<'a, Message> {
    column(
        label
            .chars()
            .map(|glyph| meta(glyph.to_string()).size(12).color(color).into()),
    )
    .align_x(Alignment::Center)
    .spacing(1)
    .into()
}

fn set_window_level(level: iced::window::Level) -> Task<Message> {
    iced::window::get_latest().then(move |id| match id {
        Some(id) => iced::window::change_level(id, level),
        None => Task::none(),
    })
}

fn resize_window(size: iced::Size) -> Task<Message> {
    iced::window::get_latest().then(move |id| match id {
        Some(id) => iced::window::resize(id, size),
        None => Task::none(),
    })
}

/// Move the window horizontally by `dx` logical pixels (keeps the right edge
/// in place while folding and unfolding).
fn shift_window_x(dx: f32) -> Task<Message> {
    iced::window::get_latest().then(move |id| {
        let Some(id) = id else {
            return Task::none();
        };
        iced::window::get_position(id).then(move |position| match position {
            Some(at) => iced::window::move_to(id, iced::Point::new(at.x + dx, at.y)),
            None => Task::none(),
        })
    })
}

/// Height of a card menu with `items` entries, including its padding.
fn quick_menu_content_height(items: usize) -> f32 {
    let items = items as f32;
    items * QUICK_MENU_ITEM_HEIGHT + (items - 1.0).max(0.0) * 2.0 + 8.0
}

fn quick_scroll_id() -> scrollable::Id {
    scrollable::Id::new("quick-history")
}

fn group_input_id() -> text_input::Id {
    text_input::Id::new("quick-group-name")
}

fn moved_index(sequences: &[u32], selected: Option<u32>, delta: i32) -> Option<usize> {
    if sequences.is_empty() {
        return None;
    }
    let current = selected
        .and_then(|seq| sequences.iter().position(|candidate| *candidate == seq))
        .unwrap_or(0);
    Some((current as i64 + delta as i64).clamp(0, sequences.len() as i64 - 1) as usize)
}

async fn run_system_action(
    action: impl FnOnce() -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("veya-paste".into())
        .spawn(move || {
            let _ = tx.send(action());
        })
        .map_err(|error| format!("无法执行粘贴操作：{error}"))?;
    rx.await.map_err(|_| "粘贴操作已中断".to_string())?
}

/// Ctrl+1..9 map to rows 0..8 and Ctrl+0 to row 9, like the on-screen numbers.
fn ctrl_digit_index(key: &Key) -> Option<usize> {
    let Key::Character(text) = key else {
        return None;
    };
    let mut chars = text.chars();
    let digit = chars.next()?.to_digit(10)?;
    if chars.next().is_some() {
        return None;
    }
    Some((digit as usize + NUMBERED_ROWS - 1) % NUMBERED_ROWS)
}

/// True when `new` is `old` with exactly one ASCII digit inserted somewhere.
fn is_single_digit_insertion(old: &str, new: &str) -> bool {
    if new.chars().count() != old.chars().count() + 1 {
        return false;
    }
    new.char_indices().any(|(at, ch)| {
        ch.is_ascii_digit() && {
            let mut rest = String::with_capacity(old.len());
            rest.push_str(&new[..at]);
            rest.push_str(&new[at + ch.len_utf8()..]);
            rest == old
        }
    })
}

fn quick_key(key: Key, modifiers: Modifiers) -> Option<Message> {
    if modifiers == Modifiers::CTRL {
        return ctrl_digit_index(&key).map(Message::QuickPasteNth);
    }
    if !modifiers.is_empty() {
        return None;
    }
    match key {
        Key::Named(Named::ArrowUp) => Some(Message::QuickMove(-1)),
        Key::Named(Named::ArrowDown) => Some(Message::QuickMove(1)),
        Key::Named(Named::Enter) => Some(Message::RecopySelected),
        Key::Named(Named::Escape) => Some(Message::QuickDismiss { restore: true }),
        _ => None,
    }
}

pub(super) fn quick_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: iced::window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            quick_key(key, modifiers)
        }
        iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) => {
            Some(Message::QuickCtrl(modifiers.control()))
        }
        iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::QuickUnfocused),
        _ => None,
    }
}

#[cfg(test)]
pub(in crate::app) mod tests {
    use super::*;

    pub(in crate::app) fn isolated_app() -> App {
        let (cmd_tx, _) = std::sync::mpsc::channel();
        let (_, event_rx) = std::sync::mpsc::channel();
        let (_, activation_rx) = std::sync::mpsc::channel();
        App {
            quick: None,
            next_paste_request: 0,
            window_generation: Default::default(),
            last_paste_error: None,
            shared: Default::default(),
            worker: WorkerHandle { cmd_tx, event_rx },
            tray_rx: None,
            activation_rx,
            window_hidden: false,
            window_id: None,
            hotkey_recording: false,
            hotkey_record_error: None,
            state: UiState::default(),
            tracking_control: TrackingControl::default(),
            search: "saved search".into(),
            page: Page::History,
            filter: Filter::Image,
            pinned_only: true,
            sort_order: SortOrder::Oldest,
            history_page: 2,
            history_active: true,
            list_density: ListDensity::Detailed,
            exclude_input: String::new(),
            max_items_input: String::new(),
            expiry_days_input: String::new(),
            limit_inputs_synced: true,
            detail_menu_open: false,
            card_menu: None,
            quick_group_sel: None,
            quick_group_input: None,
            cursor_position: iced::Point::ORIGIN,
            window_size: iced::Size::new(1080.0, 700.0),
            content_modal_for: None,
            requested_content: None,
            clear_history_confirmation_open: false,
            data_job: Default::default(),
            copied_tick: 0,
            show_qrcode: false,
            source_paths: HashMap::new(),
            chrome_sync_pending: false,
            chrome_sync_attempts: 0,
            updates: UpdateCheck::default(),
        }
    }

    fn ready_text_app() -> (App, std::sync::mpsc::Receiver<WorkerCmd>) {
        let mut app = isolated_app();
        app.search.clear();
        app.filter = Filter::All;
        app.pinned_only = false;
        app.sort_order = SortOrder::Newest;
        app.history_page = 0;
        app.state.history_query = app.history_query();
        app.state.history_active = true;
        let summary = veya_core::HistorySummary {
            representative: veya_core::HistoryRecord {
                sequence: 1,
                content: format!("{}TAIL", "中".repeat(1_000)),
                payload: veya_core::HistoryPayload::Text,
                content_hash: "one".into(),
                source_app: "editor.exe".into(),
                source_window: "source".into(),
                source_confidence: veya_core::SourceConfidence::Exact,
                created_at_ms: 1,
                pinned: false,
                pastes: Vec::new(),
            },
            raw_sequences: vec![1],
            used_in: Vec::new(),
            last_created_at_ms: 1,
        };
        app.state.cards = vec![crate::capture::card_view_from_summary(&summary, None)].into();
        app.state.selected = Some(1);
        let (tx, rx) = std::sync::mpsc::channel();
        app.worker.cmd_tx = tx;
        (app, rx)
    }

    #[test]
    fn scrolling_to_the_end_requests_exactly_one_next_page() {
        let (mut app, rx) = ready_text_app();
        app.state.history_has_next = true;
        app.request_history_load_more();
        match rx.try_recv() {
            Ok(WorkerCmd::QueryHistory(query)) => assert_eq!(query.page, 1),
            _ => panic!("expected a query for the next page"),
        }
        // The next page is already on its way, so further scroll events do nothing.
        app.request_history_load_more();
        assert!(rx.try_recv().is_err());
        assert_eq!(app.history_page, 1);
    }

    #[test]
    fn no_next_page_is_requested_after_the_last_one() {
        let (mut app, rx) = ready_text_app();
        app.state.history_has_next = false;
        app.request_history_load_more();
        assert!(rx.try_recv().is_err());
        assert_eq!(app.history_page, 0);
    }

    #[test]
    fn loading_the_next_page_keeps_the_rows_on_screen() {
        let (mut app, _rx) = ready_text_app();
        app.history_active = true;
        assert!(!app.history_list_pending());

        // Next page requested but not yet reported by the worker: rows stay visible.
        app.history_page = 1;
        assert!(!app.history_list_pending());

        // Worker is appending: rows stay visible, yet further loads stay blocked.
        app.state.history_query = app.history_query();
        app.state.history_loading = true;
        app.state.history_appending = true;
        assert!(!app.history_list_pending());
        assert!(app.history_query_pending());

        // A full reload does replace the list.
        app.state.history_appending = false;
        assert!(app.history_list_pending());

        // So does changing the filter.
        app.state.history_loading = false;
        app.filter = Filter::Image;
        assert!(app.history_list_pending());
    }

    #[test]
    fn typing_in_a_limit_box_never_changes_the_saved_limit() {
        let (mut app, rx) = ready_text_app();
        let _ = app.update(Message::MaxItemsInput("5".into()));
        let _ = app.update(Message::MaxItemsInput("".into()));
        let _ = app.update(Message::ExpiryDaysInput("1".into()));
        assert!(rx.try_recv().is_err(), "typing must not reach the worker");
        assert_eq!(
            app.state.max_items,
            crate::capture::MaxItemsConfig::default()
        );

        // Letters and symbols are dropped from the draft.
        let _ = app.update(Message::MaxItemsInput("12ab3-".into()));
        assert_eq!(app.max_items_input, "123");
    }

    #[test]
    fn applying_a_limit_validates_the_draft_before_sending_it() {
        let (mut app, rx) = ready_text_app();
        app.state.max_items.enabled = true;

        let _ = app.update(Message::MaxItemsInput("0".into()));
        let _ = app.update(Message::ApplyMaxItems);
        assert!(rx.try_recv().is_err(), "an out-of-range value is rejected");
        assert_eq!(
            app.max_items_input, "1000",
            "the box returns to the saved value"
        );

        let _ = app.update(Message::MaxItemsInput("250".into()));
        let _ = app.update(Message::ApplyMaxItems);
        match rx.try_recv() {
            Ok(WorkerCmd::SetMaxItems(cfg)) => {
                assert!(cfg.enabled);
                assert_eq!(cfg.count, 250);
            }
            _ => panic!("expected the applied limit"),
        }

        let _ = app.update(Message::ExpiryDaysInput("30".into()));
        let _ = app.update(Message::ApplyExpiryDays);
        match rx.try_recv() {
            Ok(WorkerCmd::SetExpiry(cfg)) => assert_eq!(cfg.days, 30),
            _ => panic!("expected the applied expiry"),
        }
    }

    fn group(id: i64, name: &str) -> veya_storage::GroupSummary {
        veya_storage::GroupSummary {
            id,
            name: name.into(),
            member_count: 0,
        }
    }

    #[test]
    fn picker_has_room_for_ten_rows_above_the_fixed_chrome() {
        assert!(QUICK_HEIGHT - QUICK_CHROME_HEIGHT >= QUICK_VISIBLE_ROWS as f32 * ROW_HEIGHT);
        assert_eq!(QUICK_VISIBLE_ROWS, 10);
    }

    #[test]
    fn opening_the_picker_starts_on_all_groups_without_a_half_finished_edit() {
        let (mut app, _rx) = ready_text_app();
        app.state.groups = vec![group(1, "work")];
        app.quick_group_sel = Some(1);
        app.quick_group_input = Some(QuickGroupInput {
            mode: QuickGroupInputMode::Create { add_sequence: None },
            value: "half".into(),
        });
        let _ = app.show_quick(None);
        assert_eq!(app.quick_group_sel, None);
        assert!(app.quick_group_input.is_none());
        assert_eq!(app.history_query().group_id, None);
    }

    #[test]
    fn escape_only_closes_the_group_name_box() {
        let (mut app, _rx) = ready_text_app();
        let _ = app.show_quick(None);
        let _ = app.update(Message::QuickStartCreateGroup { add_sequence: None });
        assert!(app.quick_group_input.is_some());
        let _ = app.update(Message::QuickDismiss { restore: true });
        assert!(app.quick_group_input.is_none());
        assert!(app.quick.is_some(), "the picker itself stays open");
        assert!(!app.window_hidden);
    }

    #[test]
    fn losing_focus_still_dismisses_the_picker_while_naming_a_group() {
        let (mut app, _rx) = ready_text_app();
        let _ = app.show_quick(None);
        let _ = app.update(Message::QuickStartCreateGroup { add_sequence: None });
        let _ = app.update(Message::QuickDismiss { restore: false });
        assert!(
            app.window_hidden,
            "an unfocused picker must not stay on screen"
        );
        assert!(app.quick_group_input.is_none());
    }

    #[test]
    fn group_names_must_be_non_empty_and_unique_and_the_box_stays_open_on_error() {
        let (mut app, rx) = ready_text_app();
        app.state.groups = vec![group(1, "work"), group(2, "links")];
        let _ = app.show_quick(None);
        while rx.try_recv().is_ok() {} // drop the history query sent by opening the picker

        let _ = app.update(Message::QuickStartCreateGroup {
            add_sequence: Some(7),
        });
        let _ = app.update(Message::QuickGroupInputChanged("   ".into()));
        let _ = app.update(Message::QuickGroupInputSubmit);
        assert!(app.quick_group_input.is_some());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            app.quick.as_ref().unwrap().error.as_deref(),
            Some("分组名称不能为空")
        );

        let _ = app.update(Message::QuickGroupInputChanged("work".into()));
        assert!(
            app.quick.as_ref().unwrap().error.is_none(),
            "typing clears the warning"
        );
        let _ = app.update(Message::QuickGroupInputSubmit);
        assert!(app.quick_group_input.is_some());
        assert!(rx.try_recv().is_err());
        assert_eq!(
            app.quick.as_ref().unwrap().error.as_deref(),
            Some("分组名称已存在")
        );

        let _ = app.update(Message::QuickGroupInputChanged("  ideas ".into()));
        let _ = app.update(Message::QuickGroupInputSubmit);
        assert!(app.quick_group_input.is_none());
        match rx.try_recv() {
            Ok(WorkerCmd::CreateGroup { name, add_sequence }) => {
                assert_eq!(name, "ideas");
                assert_eq!(
                    add_sequence,
                    Some(7),
                    "the card that opened the menu joins the group"
                );
            }
            _ => panic!("expected the group to be created"),
        }
    }

    #[test]
    fn renaming_a_group_to_its_own_name_is_allowed() {
        let (mut app, rx) = ready_text_app();
        app.state.groups = vec![group(1, "work"), group(2, "links")];
        let _ = app.show_quick(None);
        while rx.try_recv().is_ok() {} // drop the history query sent by opening the picker
        let _ = app.update(Message::QuickStartRenameGroup(1));
        let _ = app.update(Message::QuickGroupInputSubmit);
        assert!(app.quick_group_input.is_none());
        match rx.try_recv() {
            Ok(WorkerCmd::RenameGroup { id, name }) => {
                assert_eq!((id, name.as_str()), (1, "work"));
            }
            _ => panic!("expected a rename"),
        }
        let _ = app.update(Message::QuickStartRenameGroup(1));
        let _ = app.update(Message::QuickGroupInputChanged("links".into()));
        let _ = app.update(Message::QuickGroupInputSubmit);
        assert!(
            app.quick_group_input.is_some(),
            "another group already has that name"
        );
    }

    #[test]
    fn deleting_a_group_closes_its_rename_box_and_leaves_its_tab() {
        let (mut app, rx) = ready_text_app();
        app.state.groups = vec![group(1, "work")];
        let _ = app.show_quick(None);
        let _ = app.update(Message::QuickSelectGroup(Some(1)));
        let _ = app.update(Message::QuickStartRenameGroup(1));
        let _ = app.update(Message::QuickDeleteGroup(1));
        assert!(app.quick_group_input.is_none());
        assert_eq!(app.quick_group_sel, None);
        let mut deleted = false;
        while let Ok(command) = rx.try_recv() {
            deleted |= matches!(command, WorkerCmd::DeleteGroup { id: 1 });
        }
        assert!(deleted);
    }

    #[test]
    fn removing_a_card_from_a_group_removes_every_record_it_stands_for() {
        let (mut app, rx) = ready_text_app();
        let mut cards = app.state.cards.to_vec();
        cards[0].raw_sequences = vec![1, 2, 3];
        app.state.cards = cards.into();
        let _ = app.update(Message::QuickRemoveFromGroup {
            group_id: 5,
            sequence: 1,
        });
        match rx.try_recv() {
            Ok(WorkerCmd::RemoveFromGroup {
                group_id,
                sequences,
            }) => {
                assert_eq!(group_id, 5);
                assert_eq!(sequences, vec![1, 2, 3]);
            }
            _ => panic!("expected the removal"),
        }
    }

    #[test]
    fn card_menu_stays_inside_the_window_however_many_groups_there_are() {
        let window = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        for items in [1usize, 3, 12, 40] {
            let content = quick_menu_content_height(items);
            let height = content.min((window.height - 16.0).max(QUICK_MENU_ITEM_HEIGHT + 8.0));
            let origin = card_menu_origin(
                iced::Point::new(window.width, window.height),
                window,
                iced::Size::new(QUICK_MENU_WIDTH, height),
            );
            assert!(origin.x + QUICK_MENU_WIDTH <= window.width);
            assert!(origin.y + height <= window.height);
        }
    }

    #[test]
    fn hidden_quick_dismissal_can_render_before_the_window_resizes() {
        let (mut app, _) = ready_text_app();
        let _ = app.show_quick(None);
        let size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::WindowResized(size));
        let _ = app.dismiss_quick(false);
        assert!(app.window_hidden);
        assert!(app.quick.is_none());

        render_at(&app, size);
    }

    #[test]
    fn opening_manager_can_render_before_the_window_resizes() {
        let (mut app, _) = ready_text_app();
        let _ = app.show_quick(None);
        let size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::WindowResized(size));
        let _ = app.open_manager();
        assert!(!app.window_hidden);
        assert!(app.quick.is_none());

        render_at(&app, size);
    }

    fn render_at(app: &App, size: iced::Size) {
        // Iced can draw the updated view before executing hide/resize tasks.
        // Exercise the actual CPU renderer at the previous quick-panel size.
        let mut renderer = iced::Renderer::new(crate::font::ui_font(), iced::Pixels(16.0));
        let mut ui = iced_runtime::UserInterface::build(
            app.view(),
            size,
            iced_runtime::user_interface::Cache::new(),
            &mut renderer,
        );
        ui.draw(
            &mut renderer,
            &Theme::Dark,
            &iced_runtime::core::renderer::Style {
                text_color: Color::WHITE,
            },
            iced::mouse::Cursor::Unavailable,
        );
        let physical_size = iced::Size::new(size.width as u32, size.height as u32);
        let viewport = iced_graphics::Viewport::with_physical_size(physical_size, 1.0);
        let mut pixels = tiny_skia::Pixmap::new(physical_size.width, physical_size.height).unwrap();
        let mut mask = tiny_skia::Mask::new(physical_size.width, physical_size.height).unwrap();
        renderer.draw::<String>(
            &mut pixels.as_mut(),
            &mut mask,
            &viewport,
            &[iced::Rectangle::with_size(size)],
            Color::BLACK,
            &[],
        );
    }

    #[test]
    fn plain_copy_requests_original_record_without_using_excerpt() {
        let (mut app, commands) = ready_text_app();
        assert!(!app.state.cards[0].content_excerpt.contains("TAIL"));
        let _ = app.update(Message::CopyPlainText(1));
        assert!(matches!(
            commands.try_recv(),
            Ok(WorkerCmd::RecopyPlainText { sequence: 1 })
        ));
        assert!(commands.try_recv().is_err());
    }

    #[test]
    fn detail_load_is_once_per_selection_and_rejects_late_old_content() {
        let (mut app, commands) = ready_text_app();
        app.sync_detail_content();
        assert!(matches!(commands.try_recv(), Ok(WorkerCmd::LoadContent(1))));
        app.sync_detail_content();
        assert!(commands.try_recv().is_err());
        app.state.detail_content = Some((99, std::sync::Arc::from("old tail")));
        app.sync_detail_content();
        assert!(app.full_content_for(1).is_none());
        assert!(app.state.detail_content.is_none());
        let full = format!("{}TAIL", "中".repeat(1_000));
        app.state.detail_content = Some((1, std::sync::Arc::from(full.clone())));
        assert_eq!(app.full_content_for(1), Some(full.as_str()));
        let _ = app.show_quick(None);
        assert!(app.state.detail_content.is_none());
        assert!(app.requested_content.is_none());
        app.sync_detail_content();
        assert!(!commands
            .try_iter()
            .any(|command| matches!(command, WorkerCmd::LoadContent(_))));
    }

    #[test]
    fn loading_query_cannot_copy_or_expand_previous_cards() {
        let (mut app, commands) = ready_text_app();
        app.state.history_loading = true;
        let _ = app.update(Message::CopyPlainText(1));
        let _ = app.update(Message::Copy(1));
        let _ = app.update(Message::OpenContentModal(1));
        assert!(commands.try_recv().is_err());
        assert!(app.content_modal_for.is_none());
    }

    #[test]
    fn cancelling_restores_management_query_and_ignores_late_preparation() {
        let mut app = isolated_app();
        let _ = app.show_quick(None);
        app.quick.as_mut().unwrap().pending = Some(7);
        let opening_generation = app
            .window_generation
            .load(std::sync::atomic::Ordering::SeqCst);
        let _ = app.dismiss_quick(true);
        let _ = app.quick_prepared(7, Ok(42));
        let _ = app.update(Message::WindowRestored {
            id: iced::window::Id::unique(),
            generation: opening_generation,
        });
        assert!(app.quick.is_none());
        assert!(app.window_hidden);
        assert!(!app.history_active);
        assert_eq!(app.search, "saved search");
        assert_eq!(app.filter, Filter::Image);
        assert!(app.pinned_only);
        assert_eq!(app.sort_order, SortOrder::Oldest);
        assert_eq!(app.history_page, 2);
        assert!(app.window_id.is_none());
    }

    #[test]
    fn old_window_and_restore_completions_do_not_change_a_new_picker_session() {
        let mut app = isolated_app();
        let _ = app.show_quick(None);
        let old = app
            .window_generation
            .load(std::sync::atomic::Ordering::SeqCst);
        let _ = app.dismiss_quick(true);
        let _ = app.show_quick(None);
        let _ = app.update(Message::QuickShown {
            generation: old,
            id: iced::window::Id::unique(),
        });
        let _ = app.update(Message::WindowRestored {
            generation: old,
            id: iced::window::Id::unique(),
        });
        let _ = app.update(Message::TargetRestored {
            generation: old,
            result: Err("old error".into()),
        });
        assert!(app.quick.is_some());
        assert!(!app.window_hidden);
        assert!(app.window_id.is_none());
        assert!(app.state.status_note.is_empty());
        assert!(app.quick.as_ref().unwrap().pending.is_none());
    }

    #[test]
    fn selection_stays_in_bounds_and_tracks_the_selected_sequence() {
        assert_eq!(moved_index(&[], None, 1), None);
        assert_eq!(moved_index(&[9, 6, 3], Some(6), 1), Some(2));
        assert_eq!(moved_index(&[9, 6, 3], Some(9), -1), Some(0));
        assert_eq!(moved_index(&[9, 6, 3], Some(3), 1), Some(2));
        assert_eq!(moved_index(&[9, 6, 3], Some(100), -1), Some(0));
    }

    #[test]
    fn picker_keys_preserve_typing_and_modified_shortcuts() {
        assert!(matches!(
            quick_key(Key::Named(Named::Enter), Modifiers::empty()),
            Some(Message::RecopySelected)
        ));
        assert!(matches!(
            quick_key(Key::Named(Named::Escape), Modifiers::empty()),
            Some(Message::QuickDismiss { restore: true })
        ));
        assert!(matches!(
            quick_key(Key::Named(Named::ArrowDown), Modifiers::empty()),
            Some(Message::QuickMove(1))
        ));
        assert!(quick_key(Key::Character("a".into()), Modifiers::empty()).is_none());
        assert!(quick_key(Key::Named(Named::Enter), Modifiers::CTRL).is_none());
    }

    #[test]
    fn only_the_active_copy_request_can_complete_a_paste() {
        let panel = QuickPanel {
            target: None,
            saved: ManagementState {
                search: String::new(),
                filter: Filter::All,
                pinned_only: false,
                sort_order: SortOrder::Newest,
                history_page: 0,
                selected: None,
                size: iced::Size::new(1080.0, 700.0),
            },
            pending: Some(7),
            sending: false,
            error: None,
            strip: Strip::default(),
            ctrl_down: false,
        };
        assert!(panel.accepts(7));
        assert!(!panel.accepts(6));
        assert!(panel.busy());
    }

    #[test]
    fn ctrl_digits_map_to_the_numbered_rows() {
        let key = |s: &str| Key::Character(s.into());
        let nth = |s: &str, mods| match quick_key(key(s), mods) {
            Some(Message::QuickPasteNth(index)) => Some(index),
            _ => None,
        };
        assert_eq!(nth("1", Modifiers::CTRL), Some(0));
        assert_eq!(nth("9", Modifiers::CTRL), Some(8));
        assert_eq!(nth("0", Modifiers::CTRL), Some(9));
        assert_eq!(nth("a", Modifiers::CTRL), None);
        assert_eq!(nth("12", Modifiers::CTRL), None);
        assert_eq!(nth("1", Modifiers::empty()), None, "plain digits still type");
        assert_eq!(nth("1", Modifiers::CTRL | Modifiers::SHIFT), None);
        assert_eq!(nth("1", Modifiers::ALT), None);
    }

    #[test]
    fn only_a_single_inserted_digit_counts_as_an_echo() {
        assert!(is_single_digit_insertion("ab", "ab1"));
        assert!(is_single_digit_insertion("ab", "a0b"));
        assert!(is_single_digit_insertion("", "9"));
        assert!(is_single_digit_insertion("中文", "中3文"));
        assert!(!is_single_digit_insertion("ab", "abc"));
        assert!(!is_single_digit_insertion("ab", "ab12"));
        assert!(!is_single_digit_insertion("ab", "ac1"));
        assert!(!is_single_digit_insertion("ab1", "ab"));
    }

    #[test]
    fn ctrl_digit_echo_is_dropped_only_while_ctrl_is_held() {
        let mut app = quick_app();
        app.search = "ab".into();
        let _ = app.update(Message::QuickCtrl(true));
        let _ = app.update(Message::SearchChanged("ab1".into()));
        assert_eq!(app.search, "ab");
        let _ = app.update(Message::SearchChanged("abc".into()));
        assert_eq!(app.search, "abc", "letters and pastes still edit the search");
        let _ = app.update(Message::QuickCtrl(false));
        let _ = app.update(Message::SearchChanged("abc1".into()));
        assert_eq!(app.search, "abc1");
    }

    #[test]
    fn numbering_covers_ten_rows_and_ignores_missing_ones() {
        let mut app = quick_app();
        assert_eq!(ROW_NUMBER_LABELS.len(), NUMBERED_ROWS);
        let _ = app.update(Message::QuickPasteNth(9));
        let _ = app.update(Message::QuickPasteNth(10));
        assert!(app.quick.as_ref().is_some_and(|quick| !quick.busy()));
        render_at(&app, iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT));
    }

    fn quick_app() -> App {
        let (mut app, _rx) = ready_text_app();
        let _ = app.show_quick(None);
        app
    }

    fn strip(app: &App) -> &Strip {
        &app.quick.as_ref().expect("quick panel is open").strip
    }

    #[test]
    fn double_press_on_the_strip_toggles_the_pin() {
        let mut app = quick_app();
        let _ = app.update(Message::QuickStripPress);
        assert!(!strip(&app).pinned, "a single press only drags");
        let _ = app.update(Message::QuickStripPress);
        assert!(strip(&app).pinned);
        let _ = app.update(Message::QuickStripPress);
        let _ = app.update(Message::QuickStripPress);
        assert!(!strip(&app).pinned);
    }

    #[test]
    fn slow_presses_never_count_as_a_double_press() {
        let mut app = quick_app();
        let _ = app.update(Message::QuickStripPress);
        app.quick.as_mut().unwrap().strip.last_press =
            Some(std::time::Instant::now() - STRIP_DOUBLE_CLICK * 2);
        let _ = app.update(Message::QuickStripPress);
        assert!(!strip(&app).pinned);
    }

    #[test]
    fn losing_focus_closes_the_panel_unless_it_is_pinned() {
        let mut app = quick_app();
        app.quick.as_mut().unwrap().strip.pinned = true;
        let _ = app.update(Message::QuickUnfocused);
        assert!(app.quick.is_some());
        app.quick.as_mut().unwrap().strip.pinned = false;
        let _ = app.update(Message::QuickUnfocused);
        assert!(app.quick.is_none());
    }

    #[test]
    fn folding_pins_and_unfolding_keeps_the_pin() {
        let mut app = quick_app();
        app.window_size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::QuickToggleFold);
        assert!(strip(&app).folded && strip(&app).pinned);
        assert_eq!(strip(&app).expanded_size.width, QUICK_WIDTH);
        let _ = app.update(Message::QuickToggleFold);
        assert!(!strip(&app).folded && strip(&app).pinned);
    }

    #[test]
    fn a_folded_panel_ignores_double_press_unpinning() {
        let mut app = quick_app();
        let _ = app.update(Message::QuickToggleFold);
        let _ = app.update(Message::QuickStripPress);
        let _ = app.update(Message::QuickStripPress);
        assert!(strip(&app).pinned);
    }

    #[test]
    fn closing_a_pinned_folded_panel_clears_the_session_state() {
        let mut app = quick_app();
        let _ = app.update(Message::QuickToggleFold);
        let _ = app.update(Message::QuickDismiss { restore: true });
        assert!(app.quick.is_none());
        let _ = app.show_quick(None);
        assert!(!strip(&app).pinned && !strip(&app).folded);
    }

    #[test]
    fn opening_the_manager_clears_pin_and_fold() {
        let mut app = quick_app();
        let _ = app.update(Message::QuickToggleFold);
        let _ = app.open_manager();
        assert!(app.quick.is_none());
    }

    #[test]
    fn folded_and_pinned_panel_still_renders() {
        let mut app = quick_app();
        render_at(&app, iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT));
        let _ = app.update(Message::QuickToggleFold);
        render_at(&app, iced::Size::new(STRIP_WIDTH, QUICK_HEIGHT));
    }

    #[test]
    fn hover_expand_unfolds_and_hover_fold_collapses() {
        let mut app = quick_app();
        app.window_size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::QuickToggleFold);
        assert!(strip(&app).folded);
        assert!(!strip(&app).hover_expanded);
        let _ = app.update(Message::QuickHoverExpand);
        assert!(!strip(&app).folded);
        assert!(strip(&app).hover_expanded);
        let _ = app.update(Message::QuickHoverFold);
        assert!(strip(&app).folded);
        assert!(!strip(&app).hover_expanded);
    }

    #[test]
    fn manual_toggle_clears_hover_expanded() {
        let mut app = quick_app();
        app.window_size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::QuickToggleFold);
        let _ = app.update(Message::QuickHoverExpand);
        assert!(strip(&app).hover_expanded);
        let _ = app.update(Message::QuickToggleFold);
        assert!(!strip(&app).hover_expanded);
        assert!(!strip(&app).folded);
    }

    #[test]
    fn hover_expand_is_ignored_when_already_unfolded() {
        let mut app = quick_app();
        app.window_size = iced::Size::new(QUICK_WIDTH, QUICK_HEIGHT);
        let _ = app.update(Message::QuickHoverExpand);
        assert!(!strip(&app).folded);
        assert!(!strip(&app).hover_expanded);
    }
}

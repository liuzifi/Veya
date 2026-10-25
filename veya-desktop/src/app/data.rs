//! Database backup and restore: state machine, background tasks and the settings section.
//!
//! File dialogs and SQLite copies run on a helper thread. Backing up reads the live
//! database through its own connection; restoring goes through the worker, which owns the
//! open store and the in-memory state that must be reloaded afterwards.

use std::path::PathBuf;

use super::*;
use crate::format::byte_size_label;
use crate::worker::RestoreReport;
use iced::widget::column;
use veya_storage::{BackupInfo, Store};
use veya_windows::platform::dialog;

/// Title of the main window; the file dialogs use it as their owner.
const WINDOW_TITLE: &str = "Veya";

#[derive(Debug, Clone, Default)]
pub(super) enum DataJob {
    #[default]
    Idle,
    /// A dialog or copy is in progress; the buttons are disabled.
    Working(&'static str),
    /// The chosen backup was checked and waits for the user's go-ahead.
    ConfirmRestore {
        path: PathBuf,
        info: BackupInfo,
    },
    Done(Result<String, String>),
}

impl DataJob {
    pub(super) fn locked(&self) -> bool {
        matches!(self, Self::Working(_) | Self::ConfirmRestore { .. })
    }
}

/// Run blocking work (file dialogs, SQLite) off the UI thread.
pub(super) async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::Builder::new()
        .name("veya-data".into())
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|error| format!("无法启动后台任务：{error}"))?;
    rx.await.map_err(|_| "后台任务已中断".to_string())
}

fn counts_label(info: &BackupInfo) -> String {
    format!(
        "{} 条记录、{} 个分组，{}",
        info.records,
        info.groups,
        byte_size_label(info.size_bytes as usize)
    )
}

impl App {
    pub(super) fn start_backup(&mut self) -> Task<Message> {
        if self.data_job.locked() {
            return Task::none();
        }
        self.data_job = DataJob::Working("请在对话框中选择备份保存位置…");
        Task::perform(
            run_blocking(|| {
                let name = dialog::default_backup_file_name(&dialog::local_timestamp());
                dialog::pick_save_file("备份 Veya 数据库", &name, WINDOW_TITLE)
            }),
            |picked| Message::BackupPicked(picked.ok().flatten()),
        )
    }

    pub(super) fn backup_picked(&mut self, path: Option<PathBuf>) -> Task<Message> {
        let Some(path) = path else {
            self.data_job = DataJob::Idle;
            return Task::none();
        };
        self.data_job = DataJob::Working("正在备份…");
        Task::perform(
            run_blocking(move || {
                Store::backup_file(crate::worker::database_path(), &path).map(|info| (path, info))
            }),
            |result| Message::BackupFinished(result.and_then(|inner| inner)),
        )
    }

    pub(super) fn backup_finished(&mut self, result: Result<(PathBuf, BackupInfo), String>) {
        self.data_job = DataJob::Done(match result {
            Ok((path, info)) => Ok(format!(
                "备份完成：{}\n保存位置：{}",
                counts_label(&info),
                path.display()
            )),
            Err(error) => Err(format!("备份失败：{error}")),
        });
    }

    pub(super) fn start_restore(&mut self) -> Task<Message> {
        if self.data_job.locked() {
            return Task::none();
        }
        self.data_job = DataJob::Working("请在对话框中选择要还原的备份文件…");
        Task::perform(
            run_blocking(|| dialog::pick_open_file("选择要还原的备份文件", WINDOW_TITLE)),
            |picked| Message::RestorePicked(picked.ok().flatten()),
        )
    }

    pub(super) fn restore_picked(&mut self, path: Option<PathBuf>) -> Task<Message> {
        let Some(path) = path else {
            self.data_job = DataJob::Idle;
            return Task::none();
        };
        self.data_job = DataJob::Working("正在检查备份文件…");
        Task::perform(
            run_blocking(move || Store::inspect_backup(&path).map(|info| (path, info))),
            |result| Message::RestoreInspected(result.and_then(|inner| inner)),
        )
    }

    pub(super) fn restore_inspected(&mut self, result: Result<(PathBuf, BackupInfo), String>) {
        self.data_job = match result {
            Ok((path, info)) => DataJob::ConfirmRestore { path, info },
            Err(error) => DataJob::Done(Err(format!("无法使用所选文件：{error}"))),
        };
    }

    pub(super) fn confirm_restore(&mut self) {
        if !matches!(self.data_job, DataJob::ConfirmRestore { .. }) {
            return;
        }
        let DataJob::ConfirmRestore { path, .. } = std::mem::take(&mut self.data_job) else {
            return;
        };
        self.data_job = if self
            .worker
            .cmd_tx
            .send(WorkerCmd::RestoreDatabase { path })
            .is_ok()
        {
            DataJob::Working("正在还原，请稍候…")
        } else {
            DataJob::Done(Err("后台服务没有响应，未执行还原".into()))
        };
    }

    pub(super) fn cancel_restore(&mut self) {
        if matches!(self.data_job, DataJob::ConfirmRestore { .. }) {
            self.data_job = DataJob::Idle;
        }
    }

    // --- Database storage directory customisation ---------------------------

    /// Open the native folder picker so the user can choose where the database
    /// should live on the next launch. Runs off the UI thread because the
    /// dialog blocks; the result comes back as [`Message::DbFolderPicked`].
    pub(super) fn browse_db_folder(&mut self) -> Task<Message> {
        if self.data_job.locked() {
            return Task::none();
        }
        self.data_job = DataJob::Working("请在对话框中选择数据库存储文件夹…");
        Task::perform(
            run_blocking(|| dialog::pick_folder("选择数据库存储文件夹", WINDOW_TITLE)),
            |picked| Message::DbFolderPicked(picked.ok().flatten()),
        )
    }

    /// Folder picker returned: fill the input box with the chosen path (or do
    /// nothing on cancel). Does not persist yet — the user clicks "应用" first
    /// so they can edit the picked path before committing.
    pub(super) fn db_folder_picked(&mut self, folder: Option<PathBuf>) -> Task<Message> {
        if matches!(self.data_job, DataJob::Working(_)) {
            self.data_job = DataJob::Idle;
        }
        if let Some(path) = folder {
            self.db_path_input = path.display().to_string();
        }
        Task::none()
    }

    /// Persist the current draft path to the sidecar config. An empty draft
    /// clears the override (falls back to default). The change only takes
    /// effect after restarting Veya, so the status note says so.
    pub(super) fn apply_db_path(&mut self) -> Task<Message> {
        if self.data_job.locked() {
            return Task::none();
        }
        let dir = self.db_path_input.trim().to_string();
        let _ = self.worker.cmd_tx.send(WorkerCmd::SetDbPath { dir });
        Task::none()
    }

    /// Clear the override and fall back to the default storage directory.
    /// Resets the input box and sends an empty `SetDbPath` to the worker.
    pub(super) fn reset_db_path(&mut self) -> Task<Message> {
        if self.data_job.locked() {
            return Task::none();
        }
        self.db_path_input.clear();
        let _ = self
            .worker
            .cmd_tx
            .send(WorkerCmd::SetDbPath { dir: String::new() });
        Task::none()
    }

    /// The worker finished (or refused) a restore. On success every piece of view state
    /// that pointed into the old data is dropped and the list is queried again.
    pub(super) fn database_restored(
        &mut self,
        result: Result<RestoreReport, String>,
    ) -> Task<Message> {
        match result {
            Ok(report) => {
                self.data_job = DataJob::Done(Ok(format!(
                    "还原完成：{} 条记录、{} 个分组。\n还原前的数据已另存为：{}",
                    report.records,
                    report.groups,
                    report.safety_copy.display()
                )));
                self.search.clear();
                self.history_page = 0;
                self.state.selected = None;
                self.quick_group_sel = None;
                self.card_menu = None;
                self.content_modal_for = None;
                self.requested_content = None;
                // Settings (limits, theme) were reloaded by the worker; refresh the drafts.
                self.limit_inputs_synced = false;
                // The worker reloaded `quick_settings` from the restored database; let the
                // next snapshot overwrite the UI-owned copy so toggles reflect the backup.
                self.quick_settings_owned = false;
                self.request_history_query(false)
            }
            Err(error) => {
                self.data_job = DataJob::Done(Err(error));
                Task::none()
            }
        }
    }

    /// The "backup and restore" row of the data section, with its status line.
    pub(super) fn backup_section(&self) -> Element<'_, Message> {
        let locked = self.data_job.locked();
        let action = |label: &'static str, message: Message| {
            let base = button(text(label).size(12))
                .style(theme::action_button)
                .padding(pad(7.0, 14.0, 7.0, 14.0));
            if locked {
                base
            } else {
                base.on_press(message)
            }
        };
        let head =
            row![
                icons::icon(Icon::External, theme::ACCENT(), 18.0),
                column![
                body("备份与还原").size(13).font(crate::font::name_font()),
                meta("备份包含全部历史、分组和设置；还原会替换当前全部数据，还原前自动保留一份副本")
                    .size(11)
                    .color(theme::MUTED()),
            ]
                .spacing(2)
                .width(Length::Fill),
                action("备份…", Message::BackupDatabase),
                action("还原…", Message::RestoreDatabase),
            ]
            .spacing(12)
            .align_y(Alignment::Center);

        let status: Option<Element<'_, Message>> = match &self.data_job {
            DataJob::Idle => None,
            DataJob::Working(note) => Some(meta(*note).size(11).color(theme::MUTED()).into()),
            DataJob::Done(Ok(note)) => Some(meta(note.clone()).size(11).color(theme::OK()).into()),
            DataJob::Done(Err(note)) => {
                Some(meta(note.clone()).size(11).color(theme::DANGER()).into())
            }
            DataJob::ConfirmRestore { path, info } => Some(
                container(
                    column![
                        body("确认还原？").size(13).font(crate::font::name_font()),
                        meta(format!("备份内容：{}", counts_label(info)))
                            .size(11)
                            .color(theme::INK()),
                        meta(format!("文件：{}", path.display()))
                            .size(11)
                            .color(theme::MUTED()),
                        meta(
                            "当前全部历史、分组和设置将被替换（快捷键设置保持不变）。\
                             还原前会把现有数据另存到数据目录的 backups 文件夹，最多保留最近 5 份。"
                        )
                        .size(11)
                        .color(theme::DANGER()),
                        row![
                            button(text("确认还原").size(12).color(theme::DANGER()))
                                .style(theme::danger_button)
                                .padding(pad(7.0, 14.0, 7.0, 14.0))
                                .on_press(Message::ConfirmRestoreDatabase),
                            button(text("取消").size(12))
                                .style(theme::action_button)
                                .padding(pad(7.0, 14.0, 7.0, 14.0))
                                .on_press(Message::CancelRestoreDatabase),
                        ]
                        .spacing(8),
                    ]
                    .spacing(6),
                )
                .padding(14)
                .width(Length::Fill)
                .style(theme::inset_style)
                .into(),
            ),
        };

        match status {
            Some(status) => column![head, Space::with_height(10.0), status].into(),
            None => head.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::quick::tests::isolated_app;
    use super::*;

    fn info() -> BackupInfo {
        BackupInfo {
            records: 3,
            groups: 1,
            size_bytes: 4096,
        }
    }

    #[test]
    fn cancelling_a_dialog_returns_to_idle() {
        let mut app = isolated_app();
        let _ = app.update(Message::BackupDatabase);
        assert!(matches!(app.data_job, DataJob::Working(_)));
        let _ = app.update(Message::BackupPicked(None));
        assert!(matches!(app.data_job, DataJob::Idle));
        let _ = app.update(Message::RestoreDatabase);
        let _ = app.update(Message::RestorePicked(None));
        assert!(matches!(app.data_job, DataJob::Idle));
    }

    #[test]
    fn buttons_are_ignored_while_a_job_is_running() {
        let mut app = isolated_app();
        let _ = app.update(Message::BackupDatabase);
        let _ = app.update(Message::RestoreDatabase);
        assert!(
            matches!(app.data_job, DataJob::Working(note) if note.contains("保存位置")),
            "the second request must not replace the running backup"
        );
    }

    #[test]
    fn restore_needs_an_explicit_confirmation_before_the_worker_is_told() {
        let mut app = isolated_app();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        app.worker.cmd_tx = cmd_tx;
        let path = PathBuf::from("C:/backups/saved.db");

        let _ = app.update(Message::RestoreInspected(Ok((path.clone(), info()))));
        assert!(matches!(app.data_job, DataJob::ConfirmRestore { .. }));
        assert!(
            cmd_rx.try_recv().is_err(),
            "nothing is sent before confirming"
        );

        let _ = app.update(Message::CancelRestoreDatabase);
        assert!(matches!(app.data_job, DataJob::Idle));
        let _ = app.update(Message::ConfirmRestoreDatabase);
        assert!(cmd_rx.try_recv().is_err(), "cancel really cancelled");

        let _ = app.update(Message::RestoreInspected(Ok((path.clone(), info()))));
        let _ = app.update(Message::ConfirmRestoreDatabase);
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(WorkerCmd::RestoreDatabase { path: sent }) if sent == path
        ));
        assert!(matches!(app.data_job, DataJob::Working(_)));
    }

    #[test]
    fn an_unusable_file_is_reported_and_never_reaches_the_worker() {
        let mut app = isolated_app();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        app.worker.cmd_tx = cmd_tx;
        let _ = app.update(Message::RestoreInspected(
            Err("不是 Veya 的备份文件".into()),
        ));
        assert!(matches!(&app.data_job, DataJob::Done(Err(note)) if note.contains("无法使用")));
        let _ = app.update(Message::ConfirmRestoreDatabase);
        assert!(cmd_rx.try_recv().is_err());
    }

    #[test]
    fn a_finished_restore_drops_view_state_that_pointed_into_the_old_data() {
        let mut app = isolated_app();
        app.search = "old".into();
        app.history_page = 4;
        app.quick_group_sel = Some(9);
        app.content_modal_for = Some(7);
        app.limit_inputs_synced = true;
        let report = RestoreReport {
            records: 12,
            groups: 2,
            safety_copy: PathBuf::from("C:/Veya/backups/veya-before-restore-1.db"),
        };
        let _ = app.database_restored(Ok(report));
        assert!(app.search.is_empty());
        assert_eq!(app.history_page, 0);
        assert_eq!(app.quick_group_sel, None);
        assert_eq!(app.content_modal_for, None);
        assert!(!app.limit_inputs_synced);
        assert!(matches!(&app.data_job, DataJob::Done(Ok(note)) if note.contains("12 条记录")));

        let _ = app.database_restored(Err("还原失败".into()));
        assert!(matches!(app.data_job, DataJob::Done(Err(_))));
    }

    #[test]
    fn browsing_a_db_folder_locks_then_fills_the_input() {
        let mut app = isolated_app();
        let _ = app.update(Message::BrowseDbFolder);
        assert!(
            matches!(app.data_job, DataJob::Working(note) if note.contains("数据库存储文件夹")),
            "the picker runs as a busy job so further clicks are ignored"
        );
        let _ = app.update(Message::BackupDatabase);
        assert!(
            matches!(app.data_job, DataJob::Working(note) if note.contains("数据库存储文件夹")),
            "a second request must not replace the running picker"
        );

        let picked = PathBuf::from("C:/VeyaData");
        let _ = app.update(Message::DbFolderPicked(Some(picked.clone())));
        assert!(matches!(app.data_job, DataJob::Idle));
        assert_eq!(app.db_path_input, picked.display().to_string());

        // Cancelling the dialog leaves any typed draft untouched.
        app.db_path_input = "C:/typed".into();
        let _ = app.update(Message::DbFolderPicked(None));
        assert_eq!(app.db_path_input, "C:/typed");
    }

    #[test]
    fn applying_a_db_path_sends_the_trimmed_draft_and_reset_sends_empty() {
        let mut app = isolated_app();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        app.worker.cmd_tx = cmd_tx;

        app.db_path_input = "  C:/VeyaData  ".into();
        let _ = app.update(Message::ApplyDbPath);
        assert!(
            matches!(cmd_rx.try_recv(), Ok(WorkerCmd::SetDbPath { dir }) if dir == "C:/VeyaData"),
            "the stored draft is trimmed before it reaches the worker"
        );

        let _ = app.update(Message::ResetDbPath);
        assert_eq!(app.db_path_input, "");
        assert!(matches!(
            cmd_rx.try_recv(),
            Ok(WorkerCmd::SetDbPath { dir }) if dir.is_empty()
        ));
    }

    #[test]
    fn db_path_actions_are_locked_while_a_job_is_running() {
        let mut app = isolated_app();
        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
        app.worker.cmd_tx = cmd_tx;
        app.db_path_input = "C:/VeyaData".into();
        let _ = app.update(Message::RestoreDatabase);
        assert!(matches!(app.data_job, DataJob::Working(_)));

        let _ = app.apply_db_path();
        let _ = app.reset_db_path();
        assert!(
            cmd_rx.try_recv().is_err(),
            "no path change is sent while a restore is being confirmed"
        );
        assert_eq!(
            app.db_path_input, "C:/VeyaData",
            "reset is also blocked, so the draft survives"
        );

        let _ = app.update(Message::DbFolderPicked(None));
        assert!(
            !app.db_path_input.is_empty(),
            "a late picker completion must not unlock mid-job edits"
        );
    }

    #[test]
    fn the_data_section_renders_in_every_state() {
        let mut app = isolated_app();
        app.page = Page::Settings;
        for job in [
            DataJob::Idle,
            DataJob::Working("正在备份…"),
            DataJob::Done(Ok("备份完成".into())),
            DataJob::Done(Err("备份失败".into())),
            DataJob::ConfirmRestore {
                path: PathBuf::from("C:/x.db"),
                info: info(),
            },
        ] {
            app.data_job = job;
            let _element = app.settings_panel();
        }
    }
}

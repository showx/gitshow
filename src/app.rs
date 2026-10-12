use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use eframe::egui::{
    self, Align, Button, Color32, Context, Frame, Label, Layout, Margin, Modifiers, RichText,
    ScrollArea, Sense, TextEdit, TextWrapMode, Theme, ThemePreference, Vec2b,
};
use egui::epaint::text::{FontData, FontInsert, FontPriority, InsertFontFamily};
use serde::{Deserialize, Serialize};

use crate::git::{self, BranchInfo, Change, CommitFile, CommitInfo, Snapshot};

const MAX_RECENT: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Changes,
    History,
}

#[derive(Clone, PartialEq, Eq)]
struct Selection {
    path: String,
    staged: bool,
}

struct Banner {
    error: bool,
    text: String,
}

enum Confirm {
    Discard { path: String },
    Delete { path: String },
}

enum JobKind {
    Pull,
    Push,
    Commit,
}

struct PendingJob {
    rx: Receiver<Result<String, String>>,
    kind: JobKind,
}

enum RowAction {
    None,
    Select { index_side: bool },
    Stage,
    Unstage,
    Discard,
    Delete,
    Resolve,
}

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    recent: Vec<PathBuf>,
}

pub struct GitShow {
    recent: Vec<PathBuf>,
    repo: Option<PathBuf>,
    snap: Option<Snapshot>,
    branches: Vec<BranchInfo>,
    commits: Vec<CommitInfo>,
    page: Page,
    selected: Option<Selection>,
    diff: String,
    commit_msg: String,
    new_branch: String,
    selected_commit: Option<String>,
    commit_files: Vec<CommitFile>,
    selected_file: Option<String>,
    banner: Option<Banner>,
    status: String,
    git_version: String,
    window_title: String,
    job: Option<PendingJob>,
    confirm: Option<Confirm>,
}

impl GitShow {
    pub fn new(ctx: &Context) -> Self {
        install_fonts(ctx);
        apply_theme(ctx);

        let saved = load_state();
        let mut app = Self {
            recent: saved.recent,
            repo: None,
            snap: None,
            branches: Vec::new(),
            commits: Vec::new(),
            page: Page::Changes,
            selected: None,
            diff: String::new(),
            commit_msg: String::new(),
            new_branch: String::new(),
            selected_commit: None,
            commit_files: Vec::new(),
            selected_file: None,
            banner: None,
            status: "打开一个 Git 仓库".into(),
            git_version: String::new(),
            window_title: String::new(),
            job: None,
            confirm: None,
        };

        match git::version() {
            Ok(version) => app.git_version = version,
            Err(err) => app.fail(err),
        }

        if let Some(path) = app.recent.first().cloned()
            && path.exists()
        {
            app.open_repo(path);
        }
        app
    }

    fn busy(&self) -> bool {
        self.job.is_some()
    }

    fn open_repo(&mut self, path: PathBuf) {
        if self.busy() {
            return;
        }
        match git::toplevel(&path) {
            Ok(root) => {
                self.repo = Some(root.clone());
                self.remember(root);
                self.selected = None;
                self.diff.clear();
                self.selected_commit = None;
                self.commit_files.clear();
                self.selected_file = None;
                self.confirm = None;
                self.page = Page::Changes;
                self.banner = None;
                if let Err(err) = self.reload() {
                    self.fail(err);
                } else {
                    self.status = "已打开仓库".into();
                }
            }
            Err(err) => self.fail(err),
        }
    }

    fn remember(&mut self, path: PathBuf) {
        self.recent.retain(|item| item != &path);
        self.recent.insert(0, path);
        self.recent.truncate(MAX_RECENT);
        save_state(&self.recent);
    }

    fn forget(&mut self, path: &Path) {
        self.recent.retain(|item| item != path);
        save_state(&self.recent);
    }

    fn reload(&mut self) -> Result<(), String> {
        let Some(repo) = self.repo.clone() else {
            return Ok(());
        };
        let snap = git::snapshot(&repo)?;
        let branches = git::branches(&repo).unwrap_or_default();
        let commits = git::log(&repo, 80)?;
        self.snap = Some(snap);
        self.branches = branches;
        self.commits = commits;
        self.keep_valid_selection();
        self.load_diff();
        self.load_commit_detail();
        Ok(())
    }

    fn reload_report(&mut self) {
        if let Err(err) = self.reload() {
            self.fail(err);
        }
    }

    fn keep_valid_selection(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let still = self.snap.as_ref().is_some_and(|snap| {
            snap.changes.iter().any(|change| {
                change.path == selected.path
                    && if selected.staged {
                        change.in_index() && !change.is_conflict()
                    } else {
                        change.in_worktree() || change.is_conflict()
                    }
            })
        });
        if !still {
            self.selected = None;
            self.diff.clear();
        }
    }

    fn load_diff(&mut self) {
        let (Some(repo), Some(selected)) = (self.repo.clone(), self.selected.clone()) else {
            return;
        };
        let Some(change) = self.snap.as_ref().and_then(|snap| {
            snap.changes
                .iter()
                .find(|change| change.path == selected.path)
                .cloned()
        }) else {
            return;
        };
        self.diff = git::diff_change(&repo, &change, selected.staged)
            .unwrap_or_else(|err| format!("无法读取差异：{err}"));
    }

    fn load_commit_detail(&mut self) {
        let (Some(repo), Some(id)) = (self.repo.clone(), self.selected_commit.clone()) else {
            return;
        };
        if !self.commits.iter().any(|commit| commit.id == id) {
            self.selected_commit = None;
            self.commit_files.clear();
            self.selected_file = None;
            self.diff.clear();
            return;
        }
        match git::commit_files(&repo, &id) {
            Ok(files) => {
                if self
                    .selected_file
                    .as_ref()
                    .is_some_and(|path| !files.iter().any(|file| &file.path == path))
                {
                    self.selected_file = None;
                    if self.page == Page::History {
                        self.diff.clear();
                    }
                }
                self.commit_files = files;
            }
            Err(err) => {
                self.commit_files.clear();
                self.diff = format!("无法读取提交：{err}");
            }
        }
        if self.page == Page::History {
            self.load_history_diff();
        }
    }

    fn load_history_diff(&mut self) {
        let (Some(repo), Some(id), Some(path)) = (
            self.repo.clone(),
            self.selected_commit.clone(),
            self.selected_file.clone(),
        ) else {
            return;
        };
        let Some(file) = self
            .commit_files
            .iter()
            .find(|file| file.path == path)
            .cloned()
        else {
            return;
        };
        self.diff = git::diff_commit_file(&repo, &id, &file)
            .unwrap_or_else(|err| format!("无法读取差异：{err}"));
    }

    fn handle_drop(&mut self, ctx: &Context) -> bool {
        let mut hovering = false;
        let mut dropped = None;
        ctx.input(|input| {
            hovering = input
                .raw
                .hovered_files
                .iter()
                .any(|file| file.path.is_some());
            dropped = repo_from_drop(&input.raw.dropped_files);
        });
        if let Some(path) = dropped {
            if self.busy() {
                self.status = "请等当前操作结束后再打开仓库".into();
            } else {
                self.open_repo(path);
            }
        }
        hovering && !self.busy()
    }

    fn pick_repo(&mut self) {
        if self.busy() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new().set_title("打开 Git 仓库").pick_folder() {
            self.open_repo(path);
        }
    }

    fn run_git<T>(&mut self, result: Result<T, String>, ok: &str) {
        match result {
            Ok(_) => {
                self.status = ok.into();
                self.banner = None;
                self.reload_report();
            }
            Err(err) => self.fail(err),
        }
    }

    fn stage_path(&mut self, path: String) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match git::stage(&repo, &path) {
            Ok(()) => {
                self.selected = Some(Selection {
                    path,
                    staged: true,
                });
                self.status = "已暂存".into();
                self.reload_report();
            }
            Err(err) => self.fail(err),
        }
    }

    fn unstage_path(&mut self, path: String) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let unborn = self.snap.as_ref().is_some_and(|snap| snap.unborn);
        match git::unstage(&repo, &path, unborn) {
            Ok(()) => {
                self.selected = Some(Selection {
                    path,
                    staged: false,
                });
                self.status = "已取消暂存".into();
                self.reload_report();
            }
            Err(err) => self.fail(err),
        }
    }

    fn start_job<F>(&mut self, ctx: &Context, kind: JobKind, job: F)
    where
        F: FnOnce() -> Result<String, String> + Send + 'static,
    {
        if self.busy() || self.repo.is_none() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
        self.job = Some(PendingJob { rx, kind });
        self.status = match self.job.as_ref().map(|job| &job.kind) {
            Some(JobKind::Pull) => "正在拉取…".into(),
            Some(JobKind::Push) => "正在推送…".into(),
            Some(JobKind::Commit) => "正在提交…".into(),
            None => self.status.clone(),
        };
    }

    fn poll_job(&mut self, ctx: &Context) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        match job.rx.try_recv() {
            Ok(result) => {
                let kind = match self.job.take().map(|job| job.kind) {
                    Some(kind) => kind,
                    None => return,
                };
                match result {
                    Ok(message) => {
                        if matches!(kind, JobKind::Commit) {
                            self.commit_msg.clear();
                        }
                        self.note(message);
                        self.reload_report();
                    }
                    Err(err) => self.fail(err),
                }
            }
            Err(TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(150));
            }
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.fail("后台任务中断".into());
            }
        }
    }

    fn do_commit(&mut self, ctx: &Context) {
        if self.busy() || self.confirm.is_some() {
            return;
        }
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let message = self.commit_msg.trim().to_string();
        if message.is_empty() {
            return;
        }
        let staged = self
            .snap
            .as_ref()
            .is_some_and(|snap| snap.changes.iter().any(|change| change.in_index()));
        if !staged {
            return;
        }
        self.start_job(ctx, JobKind::Commit, move || git::commit(&repo, &message));
    }

    fn do_pull(&mut self, ctx: &Context) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.start_job(ctx, JobKind::Pull, move || git::pull(&repo));
    }

    fn do_push(&mut self, ctx: &Context) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.start_job(ctx, JobKind::Push, move || git::push(&repo));
    }

    fn switch_to(&mut self, name: String) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.run_git(git::switch_branch(&repo, &name), &format!("已切换到 {name}"));
    }

    fn create_branch(&mut self) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let name = self.new_branch.trim().to_string();
        match git::create_branch(&repo, &name) {
            Ok(()) => {
                self.new_branch.clear();
                self.status = format!("已创建并切换到 {name}");
                self.banner = None;
                self.reload_report();
            }
            Err(err) => self.fail(err),
        }
    }

    fn apply_confirm(&mut self) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        match confirm {
            Confirm::Discard { path } => {
                self.selected = None;
                self.diff.clear();
                self.run_git(git::discard(&repo, &path), "已放弃更改");
            }
            Confirm::Delete { path } => {
                self.selected = None;
                self.diff.clear();
                self.run_git(git::delete_untracked(&repo, &path), "已删除未跟踪文件");
            }
        }
    }

    fn fail(&mut self, text: String) {
        self.status = first_line(&text);
        self.banner = Some(Banner { error: true, text });
    }

    fn note(&mut self, text: String) {
        self.status = first_line(&text);
        if text.lines().nth(1).is_some() || text.chars().count() > 80 {
            self.banner = Some(Banner {
                error: false,
                text,
            });
        } else {
            self.banner = None;
        }
    }

    fn update_title(&mut self, ctx: &Context) {
        let title = match &self.repo {
            Some(path) => format!("GitShow — {}", file_name(path)),
            None => "GitShow".into(),
        };
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

impl eframe::App for GitShow {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        self.update_title(&ctx);
        let hovering_drop = self.handle_drop(&ctx);

        let mut refresh = false;
        let mut commit = false;
        ctx.input_mut(|input| {
            refresh = input.consume_key(Modifiers::NONE, egui::Key::F5);
            commit = input.consume_key(Modifiers::COMMAND, egui::Key::Enter);
        });
        if commit {
            self.do_commit(&ctx);
        }

        let page_before = self.page;
        let mut open_dialog = false;
        let mut pull = false;
        let mut push = false;
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("GitShow").strong().size(16.0));
                ui.add_space(8.0);
                if toolbar_button(ui, "打开仓库", !self.busy()).clicked() {
                    open_dialog = true;
                }
                if toolbar_button(ui, "刷新", self.repo.is_some() && !self.busy()).clicked() {
                    refresh = true;
                }
                if toolbar_button(ui, "拉取", self.repo.is_some() && !self.busy()).clicked() {
                    pull = true;
                }
                if toolbar_button(ui, "推送", self.repo.is_some() && !self.busy()).clicked() {
                    push = true;
                }
                if self.busy() {
                    ui.spinner();
                    ui.label(&self.status);
                } else if let Some(snap) = &self.snap {
                    ui.add_space(8.0);
                    branch_chip(ui, snap);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(6.0);
                    page_tab(ui, &mut self.page, Page::History, "历史");
                    let changes = self
                        .snap
                        .as_ref()
                        .map(|snap| snap.changes.len())
                        .unwrap_or(0);
                    let label = if changes == 0 {
                        "更改".to_string()
                    } else {
                        format!("更改 {changes}")
                    };
                    page_tab(ui, &mut self.page, Page::Changes, &label);
                });
            });
        });

        if refresh && !self.busy() {
            self.reload_report();
            if !self.banner.as_ref().is_some_and(|banner| banner.error) {
                self.status = "已刷新".into();
            }
        }
        if self.page != page_before {
            self.on_page_changed();
        }
        if open_dialog {
            self.pick_repo();
        }
        if pull {
            self.do_pull(&ctx);
        }
        if push {
            self.do_push(&ctx);
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let color = if self.banner.as_ref().is_some_and(|banner| banner.error) {
                    Color32::from_rgb(240, 130, 120)
                } else {
                    Color32::from_rgb(160, 164, 172)
                };
                ui.label(RichText::new(&self.status).color(color));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(8.0);
                    if let Some(repo) = &self.repo {
                        ui.label(
                            RichText::new(repo.display().to_string())
                                .size(12.0)
                                .color(Color32::from_rgb(130, 134, 142)),
                        );
                    }
                });
            });
        });

        let mut open_recent = None;
        let mut forget = None;
        let mut switch = None;
        let mut create = false;
        egui::Panel::left("nav")
            .default_size(250.0)
            .min_size(190.0)
            .max_size(360.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("仓库").small().weak());
                ui.add_space(2.0);
                if self.recent.is_empty() {
                    ui.label(RichText::new("还没有打开过仓库").weak());
                }
                ScrollArea::vertical()
                    .id_salt("recent")
                    .max_height((ui.available_height() * 0.42).clamp(80.0, 260.0))
                    .show(ui, |ui| {
                        for path in self.recent.clone() {
                            let selected = self.repo.as_ref() == Some(&path);
                            ui.horizontal(|ui| {
                                let name = file_name(&path);
                                let response = ui.selectable_label(selected, name);
                                response.clone().on_hover_text(path.display().to_string());
                                if response.clicked() && !selected && !self.busy() {
                                    open_recent = Some(path.clone());
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("×").on_hover_text("从列表移除").clicked() {
                                        forget = Some(path);
                                    }
                                });
                            });
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                ui.label(RichText::new("分支").small().weak());
                ui.add_space(2.0);
                let list_height = (ui.available_height() - 72.0).max(48.0);
                ScrollArea::vertical()
                    .id_salt("branches")
                    .max_height(list_height)
                    .show(ui, |ui| {
                        if self.branches.is_empty() {
                            if let Some(name) = self
                                .snap
                                .as_ref()
                                .filter(|snap| !snap.detached && !snap.branch.is_empty())
                                .map(|snap| snap.branch.clone())
                            {
                                let _ = ui.selectable_label(true, name);
                            } else if self.repo.is_some() {
                                ui.label(RichText::new("还没有分支").weak());
                            }
                        }
                        for branch in self.branches.clone() {
                            let response = ui.selectable_label(branch.current, &branch.name);
                            if response.clicked() && !branch.current && !self.busy() {
                                switch = Some(branch.name);
                            }
                        }
                    });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add(
                        TextEdit::singleline(&mut self.new_branch)
                            .hint_text("新分支名")
                            .desired_width(140.0),
                    );
                    let enabled =
                        git::valid_branch_name(&self.new_branch) && self.repo.is_some() && !self.busy();
                    if ui.add_enabled(enabled, Button::new("创建")).clicked() {
                        create = true;
                    }
                });
                if !self.git_version.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(&self.git_version).size(11.0).weak());
                }
            });

        if let Some(path) = open_recent {
            self.open_repo(path);
        }
        if let Some(path) = forget {
            self.forget(&path);
        }
        if let Some(name) = switch {
            self.switch_to(name);
        }
        if create {
            self.create_branch();
        }

        if self.page == Page::Changes {
            let mut commit_now = false;
            egui::Panel::right("commit")
                .default_size(300.0)
                .min_size(240.0)
                .max_size(420.0)
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("提交").strong().size(16.0));
                    ui.label(RichText::new("只提交已暂存的文件").small().weak());
                    ui.add_space(6.0);
                    TextEdit::multiline(&mut self.commit_msg)
                        .hint_text("提交说明")
                        .desired_rows(8)
                        .desired_width(f32::INFINITY)
                        .show(ui);
                    ui.add_space(8.0);
                    let staged = self
                        .snap
                        .as_ref()
                        .map(|snap| snap.changes.iter().filter(|change| change.in_index()).count())
                        .unwrap_or(0);
                    let enabled = staged > 0 && !self.commit_msg.trim().is_empty() && !self.busy();
                    let mut button =
                        Button::new("提交").min_size(egui::vec2(ui.available_width(), 34.0));
                    if enabled {
                        button = button.fill(Color32::from_rgb(37, 110, 210));
                    }
                    if ui.add_enabled(enabled, button).clicked() {
                        commit_now = true;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("已暂存 {staged} 个文件"))
                            .small()
                            .weak(),
                    );
                    ui.label(RichText::new("Ctrl+Enter 提交    F5 刷新").small().weak());
                });
            if commit_now {
                self.do_commit(&ctx);
            }
        }

        egui::CentralPanel::default().show(ui, |ui| {
            self.banner_bar(ui);
            if self.repo.is_none() {
                empty_state(
                    ui,
                    "还没有打开仓库",
                    "点击「打开仓库」，或把文件夹拖进窗口。",
                );
                return;
            }
            match self.page {
                Page::Changes => self.changes_view(ui),
                Page::History => self.history_view(ui),
            }
        });

        self.confirm_window(&ctx);
        if hovering_drop {
            paint_drop_overlay(&ctx);
        }
    }
}

impl GitShow {
    fn banner_bar(&mut self, ui: &mut egui::Ui) {
        let Some(banner) = self.banner.as_ref() else {
            return;
        };
        let error = banner.error;
        let text = banner.text.clone();
        let fill = if error {
            Color32::from_rgb(72, 32, 32)
        } else {
            Color32::from_rgb(28, 48, 72)
        };
        let mut close = false;
        Frame::new()
            .fill(fill)
            .corner_radius(6)
            .inner_margin(Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    if ui.button("关闭").clicked() {
                        close = true;
                    }
                    ui.add(Label::new(text).wrap_mode(TextWrapMode::Wrap));
                });
            });
        ui.add_space(6.0);
        if close {
            self.banner = None;
        }
    }

    fn changes_view(&mut self, ui: &mut egui::Ui) {
        let Some(snap) = self.snap.clone() else {
            return;
        };
        let conflicts: Vec<Change> = snap
            .changes
            .iter()
            .filter(|change| change.is_conflict())
            .cloned()
            .collect();
        let unstaged: Vec<Change> = snap
            .changes
            .iter()
            .filter(|change| change.in_worktree() && !change.is_conflict())
            .cloned()
            .collect();
        let staged: Vec<Change> = snap
            .changes
            .iter()
            .filter(|change| change.in_index() && !change.is_conflict())
            .cloned()
            .collect();

        if conflicts.is_empty() && unstaged.is_empty() && staged.is_empty() {
            empty_state(ui, "工作区是干净的", "没有需要提交的更改。");
            return;
        }

        let mut action: Option<(RowAction, Change)> = None;
        let mut stage_all = false;
        let mut unstage_all = false;
        let list_height = (ui.available_height() * 0.46).clamp(160.0, 340.0);
        ScrollArea::vertical()
            .id_salt("change-list")
            .max_height(list_height)
            .auto_shrink(Vec2b::new(false, true))
            .show(ui, |ui| {
                if !conflicts.is_empty() {
                    section_title(ui, "冲突", conflicts.len(), None);
                    ui.label(RichText::new("解决冲突后，点「已解决」将其暂存。").small().weak());
                    for change in &conflicts {
                        let selected = self.selected.as_ref().is_some_and(|item| {
                            !item.staged && item.path == change.path
                        });
                        let row = file_row(ui, change, 'U', false, selected, "已解决", None);
                        if !matches!(row, RowAction::None) {
                            action = Some((row, change.clone()));
                        }
                    }
                    ui.add_space(8.0);
                }
                let stage_label = if unstaged.is_empty() {
                    None
                } else {
                    Some("全部暂存")
                };
                if section_title(ui, "未暂存", unstaged.len(), stage_label) {
                    stage_all = true;
                }
                if unstaged.is_empty() {
                    ui.label(RichText::new("没有未暂存的更改").weak());
                }
                for change in &unstaged {
                    let selected = self
                        .selected
                        .as_ref()
                        .is_some_and(|item| !item.staged && item.path == change.path);
                    let letter = if change.is_untracked() {
                        '?'
                    } else {
                        change.unstaged
                    };
                    let extra = if change.is_untracked() && !change.path.ends_with('/') {
                        Some("删除")
                    } else if !change.is_untracked() {
                        Some("放弃")
                    } else {
                        None
                    };
                    let row = file_row(ui, change, letter, false, selected, "暂存", extra);
                    if !matches!(row, RowAction::None) {
                        action = Some((row, change.clone()));
                    }
                }
                ui.add_space(8.0);
                let unstage_label = if staged.is_empty() {
                    None
                } else {
                    Some("全部取消")
                };
                if section_title(ui, "已暂存", staged.len(), unstage_label) {
                    unstage_all = true;
                }
                if staged.is_empty() {
                    ui.label(RichText::new("还没有暂存文件").weak());
                }
                for change in &staged {
                    let selected = self
                        .selected
                        .as_ref()
                        .is_some_and(|item| item.staged && item.path == change.path);
                    let row = file_row(ui, change, change.staged, true, selected, "取消", None);
                    if !matches!(row, RowAction::None) {
                        action = Some((row, change.clone()));
                    }
                }
            });

        if stage_all {
            if let Some(repo) = self.repo.clone() {
                self.run_git(git::stage_all(&repo), "已全部暂存");
            }
        }
        if unstage_all {
            if let Some(repo) = self.repo.clone() {
                let unborn = self.snap.as_ref().is_some_and(|snap| snap.unborn);
                self.run_git(git::unstage_all(&repo, unborn), "已全部取消暂存");
            }
        }
        if let Some((row, change)) = action {
            self.apply_row(row, change);
        }

        ui.add_space(6.0);
        self.diff_panel(ui, "选择左侧文件查看差异");
    }

    fn on_page_changed(&mut self) {
        match self.page {
            Page::History => {
                if self.selected_file.is_some() {
                    self.load_history_diff();
                } else {
                    self.diff.clear();
                }
            }
            Page::Changes => {
                if self.selected.is_some() {
                    self.load_diff();
                } else {
                    self.diff.clear();
                }
            }
        }
    }

    fn apply_row(&mut self, action: RowAction, change: Change) {
        if self.busy() {
            return;
        }
        match action {
            RowAction::Select { index_side } => {
                self.selected = Some(Selection {
                    path: change.path,
                    staged: index_side,
                });
                self.load_diff();
            }
            RowAction::Stage | RowAction::Resolve => self.stage_path(change.path),
            RowAction::Unstage => self.unstage_path(change.path),
            RowAction::Discard => {
                self.confirm = Some(Confirm::Discard { path: change.path });
            }
            RowAction::Delete => {
                self.confirm = Some(Confirm::Delete { path: change.path });
            }
            RowAction::None => {}
        }
    }

    fn history_view(&mut self, ui: &mut egui::Ui) {
        if self.commits.is_empty() {
            empty_state(ui, "还没有提交", "第一次提交后，历史会显示在这里。");
            return;
        }
        let list_height = (ui.available_height() * 0.42).clamp(150.0, 320.0);
        let mut pick_commit = None;
        ScrollArea::vertical()
            .id_salt("history")
            .max_height(list_height)
            .show(ui, |ui| {
                for commit in &self.commits {
                    let selected = self.selected_commit.as_ref() == Some(&commit.id);
                    if commit_row(ui, commit, selected) {
                        pick_commit = Some(commit.id.clone());
                    }
                }
            });
        if let Some(id) = pick_commit {
            self.selected_commit = Some(id);
            self.selected_file = None;
            self.diff.clear();
            self.load_commit_detail();
        }

        ui.add_space(6.0);
        if let Some(commit) = self
            .selected_commit
            .as_ref()
            .and_then(|id| self.commits.iter().find(|commit| &commit.id == id))
            .cloned()
        {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&commit.summary).strong());
                if ui.small_button("复制哈希").clicked() {
                    ui.ctx().copy_text(commit.id.clone());
                    self.status = "已复制完整哈希".into();
                }
            });
            ui.label(
                RichText::new(format!("{}   {}   {}", commit.short_id, commit.author, commit.time))
                    .small()
                    .weak(),
            );
            ui.add_space(4.0);
            let mut pick_file = None;
            ScrollArea::vertical()
                .id_salt("commit-files")
                .max_height(120.0)
                .show(ui, |ui| {
                    for file in &self.commit_files {
                        let selected = self.selected_file.as_ref() == Some(&file.path);
                        let label = match &file.old_path {
                            Some(old) => format!("{}  {old} → {}", file.status, file.path),
                            None => format!("{}  {}", file.status, file.path),
                        };
                        if ui.selectable_label(selected, label).clicked() {
                            pick_file = Some(file.path.clone());
                        }
                    }
                });
            if let Some(path) = pick_file {
                self.selected_file = Some(path);
                self.load_history_diff();
            }
        } else {
            ui.label(RichText::new("选择一个提交").weak());
        }
        ui.add_space(6.0);
        self.diff_panel(ui, "选择提交中的文件查看差异");
    }

    fn diff_panel(&mut self, ui: &mut egui::Ui, placeholder: &str) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("差异").strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(!self.diff.is_empty(), Button::new("复制差异").small())
                    .clicked()
                {
                    ui.ctx().copy_text(self.diff.clone());
                    self.status = "已复制差异".into();
                }
            });
        });
        Frame::new()
            .fill(Color32::from_rgb(16, 17, 20))
            .corner_radius(6)
            .inner_margin(8)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .id_salt("diff")
                    .auto_shrink(Vec2b::FALSE)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if self.diff.is_empty() {
                            ui.label(RichText::new(placeholder).weak());
                            return;
                        }
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for line in self.diff.lines() {
                            ui.label(
                                RichText::new(line)
                                    .monospace()
                                    .size(13.0)
                                    .color(line_color(line)),
                            );
                        }
                    });
            });
    }

    fn confirm_window(&mut self, ctx: &Context) {
        let Some(confirm) = self.confirm.as_ref() else {
            return;
        };
        let (title, body) = match confirm {
            Confirm::Discard { path } => (
                "放弃更改",
                format!("确定放弃对 {path} 的未暂存修改？此操作不能撤销。"),
            ),
            Confirm::Delete { path } => (
                "删除文件",
                format!("确定删除未跟踪文件 {path}？此操作不能撤销。"),
            ),
        };
        let mut accept = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(360.0);
                ui.label(body);
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add(Button::new("确定").fill(Color32::from_rgb(170, 60, 55)))
                        .clicked()
                    {
                        accept = true;
                    }
                });
            });
        if accept {
            self.apply_confirm();
        } else if cancel {
            self.confirm = None;
        }
    }
}

fn file_row(
    ui: &mut egui::Ui,
    change: &Change,
    letter: char,
    index_side: bool,
    selected: bool,
    primary: &str,
    extra: Option<&str>,
) -> RowAction {
    let mut action = RowAction::None;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 26.0), Sense::click());
    let fill = if selected {
        Color32::from_rgb(36, 74, 132)
    } else if response.hovered() {
        Color32::from_rgb(42, 46, 54)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 4.0, fill);
    }
    if response.clicked() {
        action = RowAction::Select { index_side };
    }

    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(6.0))
            .layout(Layout::left_to_right(Align::Center)),
    );
    badge(&mut row, letter);
    let text = match &change.old_path {
        Some(old) => format!("{old} → {}", change.path),
        None => change.path.clone(),
    };
    row.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if let Some(extra) = extra
            && ui.small_button(extra).clicked()
        {
            action = if extra == "删除" {
                RowAction::Delete
            } else {
                RowAction::Discard
            };
        }
        if ui.small_button(primary).clicked() {
            action = match primary {
                "取消" => RowAction::Unstage,
                "已解决" => RowAction::Resolve,
                _ => RowAction::Stage,
            };
        }
        let text_width = ui.available_width();
        ui.allocate_ui_with_layout(
            egui::vec2(text_width, 18.0),
            Layout::left_to_right(Align::Center),
            |ui| {
                ui.add(Label::new(text).truncate());
            },
        );
    });
    action
}

fn section_title(ui: &mut egui::Ui, title: &str, count: usize, action: Option<&str>) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{title}  {count}")).strong());
        if let Some(action) = action {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(action).clicked() {
                    clicked = true;
                }
            });
        }
    });
    clicked
}

fn commit_row(ui: &mut egui::Ui, commit: &CommitInfo, selected: bool) -> bool {
    let fill = if selected {
        Color32::from_rgb(36, 74, 132)
    } else {
        Color32::TRANSPARENT
    };
    let mut clicked = false;
    Frame::new()
        .fill(fill)
        .corner_radius(4)
        .inner_margin(Margin::symmetric(8, 4))
        .show(ui, |ui| {
            let response = ui.add(
                Label::new(RichText::new(&commit.summary).strong()).sense(Sense::click()),
            );
            let meta = ui.add(
                Label::new(
                    RichText::new(format!(
                        "{}    {}    {}",
                        commit.short_id, commit.author, commit.time
                    ))
                    .small()
                    .color(Color32::from_rgb(150, 156, 166)),
                )
                .sense(Sense::click()),
            );
            clicked = response.clicked() || meta.clicked();
        });
    clicked
}

fn badge(ui: &mut egui::Ui, letter: char) {
    let (text, color) = match letter {
        'M' => ("M", Color32::from_rgb(214, 166, 68)),
        'A' => ("A", Color32::from_rgb(72, 186, 120)),
        'D' => ("D", Color32::from_rgb(230, 96, 88)),
        'R' | 'C' => ("R", Color32::from_rgb(176, 140, 232)),
        '?' => ("?", Color32::from_rgb(96, 164, 232)),
        'U' => ("U", Color32::from_rgb(232, 96, 88)),
        'T' => ("T", Color32::from_rgb(214, 166, 68)),
        _ => ("·", Color32::from_rgb(160, 164, 172)),
    };
    ui.label(RichText::new(text).monospace().strong().color(color));
}

fn branch_chip(ui: &mut egui::Ui, snap: &Snapshot) {
    let name = if snap.detached {
        "游离 HEAD".to_string()
    } else if snap.unborn {
        format!("{}（尚无提交）", snap.branch)
    } else {
        snap.branch.clone()
    };
    ui.label(
        RichText::new(name)
            .color(Color32::from_rgb(130, 186, 255))
            .strong(),
    );
    if snap.upstream_gone {
        ui.label(RichText::new("上游已删除").small().weak());
    } else if snap.upstream.is_some() {
        ui.label(
            RichText::new(format!("领先 {}   落后 {}", snap.ahead, snap.behind))
                .small()
                .color(Color32::from_rgb(160, 164, 172)),
        );
    } else if !snap.detached && !snap.unborn {
        ui.label(RichText::new("未设置上游").small().weak());
    }
}

fn page_tab(ui: &mut egui::Ui, page: &mut Page, value: Page, label: &str) {
    let selected = *page == value;
    if ui.selectable_label(selected, label).clicked() {
        *page = value;
    }
}

fn toolbar_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, Button::new(label))
}

fn repo_from_drop(files: &[egui::DroppedFileHandle]) -> Option<PathBuf> {
    let mut file_parent = None;
    for file in files {
        let path = file.path();
        if path.as_os_str().is_empty() {
            continue;
        }
        if path.is_dir() {
            return Some(path.to_path_buf());
        }
        if file_parent.is_none() {
            file_parent = path.parent().map(Path::to_path_buf);
        }
    }
    file_parent
}

fn paint_drop_overlay(ctx: &Context) {
    let rect = ctx.viewport_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("repo-drop"),
    ));
    painter.rect_filled(rect, 0, Color32::from_black_alpha(150));
    let frame = rect.shrink(16.0);
    painter.rect_stroke(
        frame,
        8,
        egui::Stroke::new(2.0, Color32::from_rgb(120, 176, 255)),
        egui::StrokeKind::Inside,
    );
    painter.text(
        frame.center(),
        egui::Align2::CENTER_CENTER,
        "松开以打开仓库",
        egui::FontId::proportional(22.0),
        Color32::WHITE,
    );
    painter.text(
        frame.center() + egui::vec2(0.0, 32.0),
        egui::Align2::CENTER_CENTER,
        "可以拖入文件夹，或仓库中的文件",
        egui::FontId::proportional(14.0),
        Color32::from_rgb(210, 216, 224),
    );
}

fn empty_state(ui: &mut egui::Ui, title: &str, body: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(80.0);
        ui.label(RichText::new(title).size(20.0).strong());
        ui.add_space(6.0);
        ui.label(RichText::new(body).weak());
    });
}

fn line_color(line: &str) -> Color32 {
    if line.starts_with("diff ")
        || line.starts_with("index ")
        || line.starts_with("---")
        || line.starts_with("+++")
        || line.starts_with('\\')
    {
        Color32::from_rgb(140, 145, 155)
    } else if line.starts_with("@@") {
        Color32::from_rgb(120, 170, 255)
    } else if line.starts_with('+') {
        Color32::from_rgb(86, 196, 128)
    } else if line.starts_with('-') {
        Color32::from_rgb(236, 112, 104)
    } else {
        Color32::from_rgb(214, 216, 222)
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("仓库")
        .to_string()
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or(text);
    let mut short: String = line.chars().take(120).collect();
    if line.chars().count() > 120 {
        short.push('…');
    }
    short
}

fn install_fonts(ctx: &Context) {
    let ui_font = read_font(&[
        r"C:\Windows\Fonts\msyh.ttc",
        r"C:\Windows\Fonts\msyh.ttf",
        r"C:\Windows\Fonts\simhei.ttf",
    ]);
    if let Some(bytes) = ui_font {
        ctx.add_font(FontInsert::new(
            "gitshow-cjk",
            FontData::from_owned(bytes),
            vec![
                InsertFontFamily {
                    family: egui::FontFamily::Proportional,
                    priority: FontPriority::Highest,
                },
                InsertFontFamily {
                    family: egui::FontFamily::Monospace,
                    priority: FontPriority::Lowest,
                },
            ],
        ));
    }
    if let Some(bytes) = read_font(&[r"C:\Windows\Fonts\consola.ttf"]) {
        ctx.add_font(FontInsert::new(
            "gitshow-mono",
            FontData::from_owned(bytes),
            vec![InsertFontFamily {
                family: egui::FontFamily::Monospace,
                priority: FontPriority::Highest,
            }],
        ));
    }
}

fn read_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|path| std::fs::read(path).ok())
}

fn apply_theme(ctx: &Context) {
    ctx.set_theme(ThemePreference::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = Color32::from_rgb(30, 31, 34);
    visuals.panel_fill = Color32::from_rgb(24, 25, 28);
    visuals.extreme_bg_color = Color32::from_rgb(16, 17, 20);
    visuals.faint_bg_color = Color32::from_rgb(38, 40, 46);
    visuals.code_bg_color = Color32::from_rgb(16, 17, 20);
    visuals.selection.bg_fill = Color32::from_rgb(36, 84, 150);
    visuals.hyperlink_color = Color32::from_rgb(120, 176, 255);
    visuals.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(220, 222, 228);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(46, 49, 56);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(58, 64, 74);
    visuals.widgets.active.bg_fill = Color32::from_rgb(42, 104, 186);
    ctx.set_visuals_of(Theme::Dark, visuals);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(8.0, 5.0);
        style.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}

pub fn state_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("GitShow");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn state_path() -> PathBuf {
    state_dir().join("state.json")
}

fn load_state() -> Saved {
    let bytes = match std::fs::read(state_path()) {
        Ok(bytes) => bytes,
        Err(_) => return Saved::default(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

fn save_state(recent: &[PathBuf]) {
    let saved = Saved {
        recent: recent.to_vec(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&saved) {
        let _ = std::fs::write(state_path(), bytes);
    }
}

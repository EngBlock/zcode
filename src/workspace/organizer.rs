//! Window ▸ Organizer (⌥⇧⌘O): the archives in Xcode's Archives folder,
//! grouped by app as Xcode's Organizer lists them, with the selected
//! archive's details, Distribute App (`distribute_sheet.rs`), Show in
//! Finder and Delete. A tab of its own, one per window; it lists the folder
//! when it opens and after anything zcode does to it.

use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Disableable as _, Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::{Value, json};

use super::Workspace;
use crate::build::archive::{self, Archive};

pub struct Organizer {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    pub dir: PathBuf,
    pub archives: Vec<Archive>,
    pub loaded: bool,
    /// The app whose archives are listed (`Archive::group`).
    pub group: Option<String>,
    pub selected: Option<PathBuf>,
    /// Selected once the folder has been listed (a new archive).
    pending: Option<PathBuf>,
    /// The scheme whose app is shown first.
    scheme: Option<String>,
    error: Option<String>,
    _task: Option<Task<()>>,
}

impl Focusable for Organizer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Organizer {
    pub fn new(workspace: WeakEntity<Workspace>, scheme: Option<String>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            workspace,
            dir: crate::paths::archives_dir(),
            archives: vec![],
            loaded: false,
            group: None,
            selected: None,
            pending: None,
            scheme,
            error: None,
            _task: None,
        };
        this.refresh(cx);
        this
    }

    /// Lists the folder again (in the background).
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let dir = self.dir.clone();
        self._task = Some(cx.spawn(async move |this, cx| {
            let archives = cx.background_spawn(async move { archive::archives::list(&dir) }).await;
            this.update(cx, |this, cx| {
                this.set_archives(archives);
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_archives(&mut self, archives: Vec<Archive>) {
        self.archives = archives;
        self.loaded = true;
        let wanted = self.pending.take().or(self.selected.clone());
        let selected = wanted.and_then(|path| self.archives.iter().find(|a| a.path == path)).cloned();
        match selected {
            Some(archive) => {
                self.group = Some(archive.group());
                self.selected = Some(archive.path);
            }
            None => {
                // The scheme's app first, as Xcode opens on the project's.
                let groups = self.groups();
                let scheme = self.scheme.clone();
                let mine = self.archives.iter().find(|a| scheme.is_some() && a.scheme == scheme).map(Archive::group);
                let keep = self.group.clone().filter(|g| groups.iter().any(|(name, _)| name == g));
                self.group = keep.or(mine).or_else(|| groups.first().map(|(g, _)| g.clone()));
                self.selected = self.listed().first().map(|a| a.path.clone());
            }
        }
    }

    /// The apps (groups) and how many archives each has, in the order of
    /// their newest archive.
    pub fn groups(&self) -> Vec<(String, usize)> {
        let mut groups: Vec<(String, usize)> = vec![];
        for archive in &self.archives {
            let group = archive.group();
            match groups.iter_mut().find(|(g, _)| *g == group) {
                Some((_, count)) => *count += 1,
                None => groups.push((group, 1)),
            }
        }
        groups
    }

    /// The selected app's archives, the newest first.
    pub fn listed(&self) -> Vec<&Archive> {
        self.archives.iter().filter(|a| Some(a.group()) == self.group).collect()
    }

    pub fn selected_archive(&self) -> Option<&Archive> {
        self.selected.as_ref().and_then(|path| self.archives.iter().find(|a| a.path == *path))
    }

    /// Shows `path` once listed (a new archive), refreshing the list.
    pub fn reveal(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.pending = Some(path);
        self.refresh(cx);
    }

    pub fn select_group(&mut self, group: String, cx: &mut Context<Self>) {
        self.group = Some(group);
        self.selected = self.listed().first().map(|a| a.path.clone());
        cx.notify();
    }

    pub fn select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if let Some(archive) = self.archives.iter().find(|a| a.path == path) {
            self.group = Some(archive.group());
            self.selected = Some(path);
        }
        cx.notify();
    }

    /// Selects an archive by path, file name or (its newest) name.
    pub fn select_named(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let found = self
            .archives
            .iter()
            .find(|a| a.path == Path::new(name) || a.path.file_name().is_some_and(|f| f == name))
            .or_else(|| self.archives.iter().find(|a| a.name == name))
            .map(|a| a.path.clone())
            .ok_or_else(|| format!("no archive {name}"))?;
        self.select(found, cx);
        Ok(())
    }

    /// Distribute App for the selected archive.
    pub fn distribute(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Result<(), String> {
        let archive = self.selected_archive().cloned().ok_or("no archive is selected")?;
        if archive.app.is_none() {
            return Err("Only app archives can be distributed".into());
        }
        let workspace = self.workspace.clone();
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |ws, cx| ws.show_distribute_sheet(archive, window, cx)).ok();
        });
        Ok(())
    }

    /// Show in Finder.
    pub fn show_in_finder(&self, cx: &mut Context<Self>) {
        if let Some(archive) = self.selected_archive() {
            cx.reveal_path(&archive.path);
        }
    }

    /// Delete Archive: moves it to the Trash, as Xcode does.
    pub fn delete_selected(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let path = self.selected.clone().ok_or("no archive is selected")?;
        let listed: Vec<PathBuf> = self.listed().iter().map(|a| a.path.clone()).collect();
        let next = listed.iter().position(|p| *p == path).and_then(|ix| listed.get(ix + 1).or(ix.checked_sub(1).and_then(|i| listed.get(i)))).cloned();
        if let Err(err) = move_to_trash(&path) {
            self.error = Some(format!("Couldn’t move the archive to the Trash: {err}"));
            cx.notify();
            return Err(err.to_string());
        }
        self.error = None;
        self.archives.retain(|a| a.path != path);
        self.selected = next;
        if self.selected.is_none() {
            let archives = std::mem::take(&mut self.archives);
            self.set_archives(archives);
        }
        cx.notify();
        Ok(())
    }

    /// For `dump`'s `archives`.
    pub fn summary(&self) -> Value {
        json!({
            "dir": self.dir,
            "loaded": self.loaded,
            "groups": self.groups().into_iter().map(|(g, n)| json!({ "name": g, "count": n })).collect::<Vec<_>>(),
            "group": self.group,
            "selected": self.selected,
            "listed": self.listed().iter().map(|a| archive_json(a)).collect::<Vec<_>>(),
            "error": self.error,
        })
    }
}

/// An archive as the control channel and MCP report it.
pub fn archive_json(archive: &Archive) -> Value {
    json!({
        "path": archive.path,
        "name": archive.name,
        "scheme": archive.scheme,
        "created": archive.created_label(),
        "kind": archive.kind(),
        "comment": archive.comment,
        "app": archive.app.as_ref().map(|app| json!({
            "path": app.path,
            "bundle_id": app.bundle_id,
            "version": app.version,
            "build": app.build,
            "team": app.team,
            "signing_identity": app.signing_identity,
            "architectures": app.architectures,
            "platform": app.platform,
        })),
    })
}

/// `~/.Trash`, Finder-style names on collision.
fn move_to_trash(path: &Path) -> std::io::Result<()> {
    let trash = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".Trash")).unwrap_or_else(|| PathBuf::from("/tmp"));
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut target = trash.join(&name);
    let mut n = 2;
    while target.exists() {
        target = trash.join(format!("{name} {n}"));
        n += 1;
    }
    std::fs::rename(path, target)
}

fn platform_icon(platform: &str) -> IconName {
    match platform {
        "macOS" => IconName::Laptop,
        "tvOS" => IconName::Tv,
        "watchOS" => IconName::Watch,
        "visionOS" => IconName::Glasses,
        "iOS" => IconName::Smartphone,
        _ => IconName::Package,
    }
}

impl Organizer {
    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut list = v_flex()
            .id("organizer-apps")
            .w(px(220.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(div().px_3().py_2().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child("ARCHIVES"));
        for (ix, (group, count)) in self.groups().into_iter().enumerate() {
            let selected = self.group.as_ref() == Some(&group);
            let platform = self.archives.iter().find(|a| a.group() == group).and_then(|a| a.app.as_ref()).map(|a| a.platform.clone()).unwrap_or_default();
            let value = group.clone();
            list = list.child(
                h_flex()
                    .id(("organizer-app", ix))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .when(selected, |el| el.bg(theme.sidebar_accent))
                    .hover(|el| el.bg(theme.sidebar_accent))
                    .child(Icon::new(platform_icon(&platform)).small().text_color(theme.muted_foreground))
                    .child(div().flex_1().truncate().child(group))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(count.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_group(value.clone(), cx))),
            );
        }
        list
    }

    fn render_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let header = h_flex()
            .px_3()
            .py_1()
            .gap_2()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted_foreground)
            .border_b_1()
            .border_color(theme.border)
            .child(div().flex_1().child("Name"))
            .child(div().w(px(130.)).child("Creation Date"))
            .child(div().w(px(90.)).child("Version"));
        let mut list = v_flex().id("organizer-archives").flex_1().min_h_0().overflow_y_scroll();
        if !self.loaded {
            list = list.child(div().p_4().text_sm().text_color(theme.muted_foreground).child("Reading archives…"));
        } else if self.archives.is_empty() {
            list = list.child(
                v_flex()
                    .p_4()
                    .gap_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No archives.")
                    .child("Product ▸ Archive builds the scheme for distribution and lists it here."),
            );
        }
        for (ix, archive) in self.listed().into_iter().enumerate() {
            let selected = self.selected.as_ref() == Some(&archive.path);
            let path = archive.path.clone();
            let version = archive.app.as_ref().map(|a| a.version_label()).unwrap_or_default();
            list = list.child(
                h_flex()
                    .id(("organizer-archive", ix))
                    .px_3()
                    .py_1()
                    .gap_2()
                    .text_sm()
                    .when(selected, |el| el.bg(theme.list_active).text_color(theme.foreground))
                    .hover(|el| el.bg(theme.list_hover))
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(Icon::new(IconName::Package).small().text_color(theme.muted_foreground))
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(div().truncate().child(archive.name.clone()))
                                    .when(!archive.comment.is_empty(), |el| {
                                        el.child(div().text_xs().truncate().text_color(theme.muted_foreground).child(archive.comment.clone()))
                                    }),
                            ),
                    )
                    .child(div().w(px(130.)).child(archive.created_label()))
                    .child(div().w(px(90.)).truncate().child(version))
                    .on_click(cx.listener(move |this, _, _, cx| this.select(path.clone(), cx))),
            );
        }
        v_flex().flex_1().min_w_0().h_full().child(header).child(list)
    }

    fn render_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let panel = v_flex().w(px(300.)).h_full().flex_none().p_3().gap_2().border_l_1().border_color(theme.border).text_sm();
        let Some(archive) = self.selected_archive().cloned() else {
            return panel.child(div().text_color(theme.muted_foreground).child("No archive selected"));
        };
        let row = |label: &str, value: String| {
            v_flex()
                .gap_0p5()
                .child(div().text_xs().text_color(theme.muted_foreground).child(label.to_string()))
                .child(div().text_sm().child(if value.is_empty() { "—".to_string() } else { value }))
        };
        let distributing = self.workspace.upgrade().is_some_and(|ws| ws.read(cx).build.read(cx).distributing.is_some());
        let last = self
            .workspace
            .upgrade()
            .and_then(|ws| ws.read(cx).build.read(cx).last_distribution.clone())
            .filter(|d| d.archive == archive.path);
        let mut panel = panel
            .child(div().text_base().font_weight(FontWeight::SEMIBOLD).child(archive.name.clone()))
            .child(
                Button::new("organizer-distribute")
                    .label("Distribute App")
                    .small()
                    .primary()
                    .disabled(archive.app.is_none() || distributing)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.distribute(window, cx).ok();
                    })),
            );
        if let Some(app) = &archive.app {
            panel = panel
                .child(row("Version", app.version_label()))
                .child(row("Identifier", app.bundle_id.clone()))
                .child(row("Type", archive.kind()))
                .child(row("Team", if app.team.is_empty() { "None (Sign to Run Locally)".into() } else { app.team.clone() }))
                .child(row("Signing", app.signing_identity.clone()))
                .child(row("Architectures", app.architectures.join(", ")));
        } else {
            panel = panel.child(row("Type", archive.kind()));
        }
        panel = panel
            .child(row("Created", archive.created_label()))
            .child(row("Scheme", archive.scheme.clone().unwrap_or_default()))
            .when(!archive.comment.is_empty(), |el| el.child(row("Description", archive.comment.clone())));
        if let Some(last) = last {
            let (icon, color, text) = match &last.error {
                None => (
                    IconName::CircleCheck,
                    theme.success,
                    match (last.uploaded, last.stapled) {
                        (true, _) => format!("Uploaded to App Store Connect ({})", last.method.title()),
                        (false, true) => format!("Exported, notarized and stapled ({})", last.method.title()),
                        (false, false) => format!("Exported ({})", last.method.title()),
                    },
                ),
                Some(error) => (IconName::CircleX, theme.danger, error.clone()),
            };
            let folder = last.export_path.clone();
            panel = panel.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded(theme.radius)
                    .bg(theme.secondary)
                    .child(h_flex().gap_1p5().child(Icon::new(icon).small().text_color(color)).child(div().flex_1().text_xs().child(text)))
                    .when(!last.uploaded && last.error.is_none(), |el| {
                        el.child(Button::new("organizer-show-export").label("Show Export in Finder").xsmall().ghost().on_click(cx.listener(
                            move |_, _, _, cx| cx.reveal_path(&folder),
                        )))
                    }),
            );
        }
        panel.child(div().flex_1()).child(
            h_flex()
                .gap_2()
                .child(Button::new("organizer-finder").label("Show in Finder").xsmall().on_click(cx.listener(|this, _, _, cx| this.show_in_finder(cx))))
                .child(
                    Button::new("organizer-delete")
                        .label("Delete Archive")
                        .xsmall()
                        .danger()
                        .disabled(distributing)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.delete_selected(cx).ok();
                        })),
                ),
        )
    }
}

impl Render for Organizer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let count = self.archives.len();
        v_flex()
            .id("organizer")
            .key_context("Organizer")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(
                h_flex()
                    .px_3()
                    .py_1p5()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Archives"))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(match count {
                        1 => "1 archive".to_string(),
                        n => format!("{n} archives"),
                    }))
                    .child(div().flex_1().truncate().text_xs().text_color(theme.muted_foreground).child(crate::paths::display_path(&self.dir)))
                    .child(
                        Button::new("organizer-refresh")
                            .icon(IconName::RefreshCw)
                            .ghost()
                            .xsmall()
                            .tooltip("Refresh")
                            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(div().px_3().py_1().text_sm().bg(theme.danger.opacity(0.12)).text_color(theme.danger).child(error))
            })
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(self.render_list(cx))
                    .child(self.render_details(cx)),
            )
    }
}

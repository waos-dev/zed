//! Zed's workspace in WaOS: a folder picked in the system's dialog, opened as a local project
//! with Zed's project, git, outline and terminal panels, editors, buffer & project search,
//! diagnostics, symbol/outline/tab/theme pickers, file finder and command palette. What a project
//! runs (language servers, git, the terminal's shell) runs on the machine its user picked.
//!
//! Started as Zed's `main` starts it, with what this port leaves out: the network (the client's
//! HTTP is blocked, so no sign-in, downloads or telemetry), extensions, collaboration, the agent
//! and the crash handler.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use anyhow::Context as _;
use assets::Assets;
use breadcrumbs::Breadcrumbs;
use client::{Client, UserStore};
use git_ui::{
    branch_diff::BranchDiffToolbar,
    commit_view::CommitViewToolbar,
    git_panel::GitPanel,
    project_diff::ProjectDiffToolbar,
    solo_diff_view::{SoloDiffGitToolbar, SoloDiffStyleToolbar},
    staged_diff::StagedDiffToolbar,
    unstaged_diff::UnstagedDiffToolbar,
};
use git_ui_core::file_diff_view::FileDiffStyleToolbar;
use gpui::{
    App, AppContext as _, AsyncApp, AsyncWindowContext, Context, Entity, KeyBinding,
    PathPromptOptions, Task, TaskExt as _, UpdateGlobal as _, WeakEntity, Window, WindowOptions,
    actions,
};
use http_client::{BlockedHttpClient, HttpClientWithUrl};
use language::LanguageRegistry;
use node_runtime::NodeRuntime;
use outline_panel::OutlinePanel;
use project_panel::ProjectPanel;
use rope::Rope;
use search::project_search::ProjectSearchBar;
use session::{AppSession, Session};
use settings::{DefaultOpenBehavior, Settings as _, SettingsStore};
use terminal_view::terminal_panel::TerminalPanel;
use theme::{ActiveTheme as _, GlobalTheme};
use util::ResultExt as _;
use workspace::{
    AppState, OpenOptions, OpenPathPromptMode, Pane, Panel, Workspace, WorkspaceSettings,
    WorkspaceStore, create_and_open_local_file, with_active_or_new_workspace,
};

actions!(zed_waos, [Quit]);

#[wasdk::main]
async fn main() {
    use_the_temporary_folder();
    wasdk_adapter_gpui::run_with(|app| app.with_assets(Assets), launch).await;
}

/// `tempfile` makes its files in WaOS's `/tmp`, which `TMPDIR` names: std's `env::temp_dir`,
/// which it asks otherwise, panics on WASI whatever `TMPDIR` says.
fn use_the_temporary_folder() {
    let Some(folder) = std::env::var_os("TMPDIR") else {
        log::error!("WaOS gave no TMPDIR");
        return;
    };
    tempfile::env::override_temp_dir(std::path::Path::new(&folder)).ok();
}

fn launch(cx: &mut App) {
    init_look(cx);
    let app_database = db::AppDatabase::new();
    let key_value_store = db::kvp::KeyValueStore::from_app_db(&app_database);
    cx.set_global(app_database);
    cx.spawn(async move |cx| {
        let session = Session::new(uuid::Uuid::new_v4().to_string(), key_value_store).await;
        let app_state = cx.update(|cx| app_state(session, cx));
        cx.update(|cx| init_workspace(app_state.clone(), cx));
        open_on_the_machine(app_state, cx).await;
    })
    .detach();
}

/// Zed's fonts, settings, themes and keymap.
fn init_look(cx: &mut App) {
    if let Err(error) = Assets.load_fonts(cx) {
        log::error!("Zed's fonts: {error:#}");
    }
    settings::init(cx);
    theme_settings::init(theme::LoadThemes::All(Box::new(Assets)), cx);
    release_channel::init(semver::Version::new(0, 1, 0), cx);
    cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
    cx.on_action(|_: &Quit, cx| cx.quit());
}

/// What every workspace shares: the file system, languages, client and session.
fn app_state(session: Session, cx: &mut App) -> Arc<AppState> {
    let fs = fs::RealFs::new(None, cx.background_executor().clone());
    <dyn fs::Fs>::set_global(fs.clone(), cx);
    git::GitHostingProviderRegistry::set_global(
        Arc::new(git::GitHostingProviderRegistry::new()),
        cx,
    );
    language_model::init(cx);
    std::fs::create_dir_all(paths::config_dir()).ok();
    SettingsStore::update_global(cx, |store, cx| {
        store.watch_settings_files(fs.clone(), cx, |_, result, _| {
            if let Err(error) = result.result() {
                log::error!("settings: {error:#}");
            }
        });
    });
    let http = Arc::new(HttpClientWithUrl::new_url(
        Arc::new(BlockedHttpClient::new()),
        "https://zed.dev",
        None,
    ));
    let client = Client::new(Arc::new(clock::RealSystemClock), http, cx);
    let languages = Arc::new(LanguageRegistry::new(cx.background_executor().clone()));
    let node_runtime = NodeRuntime::unavailable();
    languages::init(languages.clone(), fs.clone(), node_runtime.clone(), cx);
    highlight_with_the_theme(&languages, cx);
    Client::set_global(client.clone(), cx);
    project::Project::init(&client, cx);
    client::init(&client, cx);
    let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
    let workspace_store = cx.new(|cx| WorkspaceStore::new(client.clone(), cx));
    let session = cx.new(|cx| AppSession::new(session, cx));
    let app_state = Arc::new(AppState {
        languages,
        client,
        user_store,
        workspace_store,
        fs,
        build_window_options: |_, _| WindowOptions::default(),
        node_runtime,
        session,
    });
    AppState::set_global(app_state.clone(), cx);
    app_state
}

/// The languages' highlights follow the theme, as Zed's `main` has them.
fn highlight_with_the_theme(languages: &Arc<LanguageRegistry>, cx: &mut App) {
    languages.set_theme(cx.theme().clone());
    let languages = languages.clone();
    cx.observe_global::<GlobalTheme>(move |cx| languages.set_theme(cx.theme().clone()))
        .detach();
}

/// The workspace's parts this port has, and Zed's default keymap for the system WaOS runs on.
fn init_workspace(app_state: Arc<AppState>, cx: &mut App) {
    editor::init(cx);
    diagnostics::init(cx);
    workspace::init(app_state, cx);
    go_to_line::init(cx);
    file_finder::init(cx);
    tab_switcher::init(cx);
    outline::init(cx);
    project_symbols::init(cx);
    project_panel::init(cx);
    outline_panel::init(cx);
    search::init(cx);
    cx.set_global(workspace::PaneSearchBarCallbacks {
        setup_search_bar: |languages, toolbar, window, cx| {
            let search_bar = cx.new(|cx| search::BufferSearchBar::new(languages, window, cx));
            toolbar.update(cx, |toolbar, cx| {
                toolbar.add_item(search_bar, window, cx);
            });
        },
        wrap_div_with_search_actions: search::buffer_search::register_pane_search_actions,
    });
    terminal_view::init(cx);
    theme_selector::init(cx);
    command_palette::init(cx);
    git_ui::init(cx);

    cx.on_action(|_: &zed_actions::OpenSettings, cx| {
        with_active_or_new_workspace(cx, |_, window, cx| {
            open_settings_file(
                paths::settings_file(),
                || settings::initial_user_settings_content().as_ref().into(),
                window,
                cx,
            );
        });
    });
    cx.on_action(|_: &zed_actions::OpenSettingsFile, cx| {
        with_active_or_new_workspace(cx, |_, window, cx| {
            open_settings_file(
                paths::settings_file(),
                || settings::initial_user_settings_content().as_ref().into(),
                window,
                cx,
            );
        });
    });
    cx.on_action(|_: &zed_actions::OpenKeymapFile, cx| {
        with_active_or_new_workspace(cx, |_, window, cx| {
            open_settings_file(
                paths::keymap_file(),
                || settings::initial_keymap_content().as_ref().into(),
                window,
                cx,
            );
        });
    });

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else { return };

        let workspace_handle = cx.entity();
        let center_pane = workspace.active_pane().clone();
        initialize_pane(workspace, &center_pane, window, cx);

        cx.subscribe_in(&workspace_handle, window, |workspace, _, event, window, cx| {
            if let workspace::Event::PaneAdded(pane) = event {
                initialize_pane(workspace, pane, window, cx);
            }
        })
        .detach();

        let search_button = cx.new(|_| search::search_status_button::SearchButton::new());
        let diagnostic_summary =
            cx.new(|cx| diagnostics::items::DiagnosticIndicator::new(workspace, cx));
        let active_file_name = cx.new(|_| workspace::active_file_name::ActiveFileName::new());
        let git_blame_status = cx.new(|_| git_ui::GitBlameStatus::default());
        let merge_conflict_indicator =
            cx.new(|cx| git_ui::MergeConflictIndicator::new(workspace, cx));
        let cursor_position =
            cx.new(|_| go_to_line::cursor_position::CursorPosition::new(workspace));
        workspace.status_bar().update(cx, |status_bar, cx| {
            status_bar.add_left_item(search_button, window, cx);
            status_bar.add_left_item(diagnostic_summary, window, cx);
            status_bar.add_left_item(active_file_name, window, cx);
            status_bar.add_left_item(git_blame_status, window, cx);
            status_bar.add_left_item(merge_conflict_indicator, window, cx);
            status_bar.add_right_item(cursor_position, window, cx);
        });

        let panels_task = initialize_panels(window, cx);
        workspace.set_panels_task(panels_task);

        workspace
            .register_action(|workspace, action: &workspace::Open, window, cx| {
                workspace::prompt_for_open_path_and_open(
                    workspace,
                    workspace.app_state().clone(),
                    PathPromptOptions {
                        files: true,
                        directories: true,
                        multiple: true,
                        prompt: None,
                    },
                    if action.create_new_window.unwrap_or_else(|| {
                        WorkspaceSettings::get_global(cx).default_open_behavior
                            == DefaultOpenBehavior::NewWindow
                    }) {
                        OpenPathPromptMode::NewWindow
                    } else {
                        OpenPathPromptMode::CurrentWindow
                    },
                    window,
                    cx,
                );
            })
            .register_action(|workspace, _: &workspace::OpenFiles, window, cx| {
                let directories = cx.can_select_mixed_files_and_dirs();
                workspace::prompt_for_open_path_and_open(
                    workspace,
                    workspace.app_state().clone(),
                    PathPromptOptions {
                        files: true,
                        directories,
                        multiple: true,
                        prompt: None,
                    },
                    if WorkspaceSettings::get_global(cx).default_open_behavior
                        == DefaultOpenBehavior::NewWindow
                    {
                        OpenPathPromptMode::NewWindow
                    } else {
                        OpenPathPromptMode::CurrentWindow
                    },
                    window,
                    cx,
                );
            });
    })
    .detach();
    load_keymap(cx);
}

fn initialize_pane(
    workspace: &Workspace,
    pane: &Entity<Pane>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    pane.update(cx, |pane, cx| {
        pane.toolbar().update(cx, |toolbar, cx| {
            let solo_diff_style_toolbar = cx.new(SoloDiffStyleToolbar::new);
            toolbar.add_item(solo_diff_style_toolbar, window, cx);
            let file_diff_style_toolbar = cx.new(FileDiffStyleToolbar::new);
            toolbar.add_item(file_diff_style_toolbar, window, cx);
            let breadcrumbs = cx.new(|_| Breadcrumbs::new());
            toolbar.add_item(breadcrumbs, window, cx);
            let buffer_search_bar = cx.new(|cx| {
                search::BufferSearchBar::new(
                    Some(workspace.project().read(cx).languages().clone()),
                    window,
                    cx,
                )
            });
            toolbar.add_item(buffer_search_bar, window, cx);
            let diagnostic_editor_controls = cx.new(|_| diagnostics::ToolbarControls::new());
            toolbar.add_item(diagnostic_editor_controls, window, cx);
            let project_search_bar = cx.new(|_| ProjectSearchBar::new());
            toolbar.add_item(project_search_bar, window, cx);
            let project_diff_toolbar = cx.new(|cx| ProjectDiffToolbar::new(workspace, cx));
            toolbar.add_item(project_diff_toolbar, window, cx);
            let staged_diff_toolbar = cx.new(|cx| StagedDiffToolbar::new(workspace, cx));
            toolbar.add_item(staged_diff_toolbar, window, cx);
            let unstaged_diff_toolbar = cx.new(|cx| UnstagedDiffToolbar::new(workspace, cx));
            toolbar.add_item(unstaged_diff_toolbar, window, cx);
            let branch_diff_toolbar = cx.new(BranchDiffToolbar::new);
            toolbar.add_item(branch_diff_toolbar, window, cx);
            let solo_diff_git_toolbar = cx.new(SoloDiffGitToolbar::new);
            toolbar.add_item(solo_diff_git_toolbar, window, cx);
            let commit_view_toolbar = cx.new(|_| CommitViewToolbar::new());
            toolbar.add_item(commit_view_toolbar, window, cx);
        });
    });
}

fn initialize_panels(window: &mut Window, cx: &mut Context<Workspace>) -> Task<anyhow::Result<()>> {
    cx.spawn_in(window, async move |workspace_handle, cx| {
        let project_panel = ProjectPanel::load(workspace_handle.clone(), cx.clone());
        let outline_panel = OutlinePanel::load(workspace_handle.clone(), cx.clone());
        let terminal_panel = TerminalPanel::load(workspace_handle.clone(), cx.clone());
        let git_panel = GitPanel::load(workspace_handle.clone(), cx.clone());

        async fn add_panel_when_ready(
            panel_task: impl Future<Output = anyhow::Result<Entity<impl Panel>>> + 'static,
            workspace_handle: WeakEntity<Workspace>,
            mut cx: AsyncWindowContext,
        ) {
            if let Some(panel) = panel_task.await.context("failed to load panel").log_err() {
                workspace_handle
                    .update_in(&mut cx, |workspace, window, cx| {
                        workspace.add_panel(panel, window, cx);
                    })
                    .log_err();
            }
        }

        futures::join!(
            add_panel_when_ready(project_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(outline_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(terminal_panel, workspace_handle.clone(), cx.clone()),
            add_panel_when_ready(git_panel, workspace_handle.clone(), cx.clone()),
        );

        workspace_handle.update(cx, |workspace, cx| {
            workspace.finish_dock_restoration(cx);
        })?;

        anyhow::Ok(())
    })
}

fn open_settings_file(
    abs_path: &'static Path,
    default_content: impl FnOnce() -> Rope + Send + 'static,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.spawn_in(window, async move |workspace, cx| {
        let fs = workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone())?;
        fs.create_dir(paths::config_dir()).await.ok();
        let project = workspace.read_with(cx, |workspace, _| workspace.project().clone())?;
        let (_worktree, _) = project
            .update(cx, |project, cx| {
                project.find_or_create_worktree(paths::config_dir(), false, cx)
            })
            .await?;
        workspace
            .update_in(cx, |_, window, cx| {
                create_and_open_local_file(abs_path, window, cx, default_content)
            })?
            .await?;
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

/// A component is built for no OS of its own, so the keymap is the one of the OS WaOS runs on.
fn load_keymap(cx: &mut App) {
    let keymap = match wasdk::system::os() {
        wasdk::system::OperatingSystem::Macos => "keymaps/default-macos.json",
        wasdk::system::OperatingSystem::Windows => "keymaps/default-windows.json",
        _ => "keymaps/default-linux.json",
    };
    match settings::KeymapFile::load_asset_allow_partial_failure(keymap, cx) {
        Ok(bindings) => {
            log::info!("{} bindings from {keymap}", bindings.len());
            cx.bind_keys(bindings);
        }
        Err(error) => log::error!("{keymap}: {error:#}"),
    }
}

/// The machine the project lives on, connected before the project opens, its root mounted at `/`
/// so its files are read at the paths its commands name them by, as Zed works on an SSH host
/// (`util::machine`); then a window with an empty workspace, whose welcome page opens a folder of
/// the machine through Zed's own folder prompt, so no dialog is shown before Zed has a window.
async fn open_on_the_machine(app_state: Arc<AppState>, cx: &mut AsyncApp) {
    util::machine::connect().await.log_err();
    let opening = cx.update(|cx| {
        workspace::open_new(OpenOptions::default(), app_state, cx, |_workspace, _window, _cx| {})
    });
    opening.await.log_err();
}

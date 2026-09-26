//! The command palette (`Ctrl+K`, `Cmd+K` on macOS): every command of the
//! window by name, with its shortcut, plus the open tabs, the scope's tags,
//! recent workspaces and the query language's qualifiers, found by fuzzy
//! matching as you type.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::app::{AppCommand, SearchApp};
use super::render::{short_dir, tag_label};
use super::table::ResultsView;
use super::windows::Windows;
use super::*;
use crate::fuzzy;
use dowse::engine::repo;
use dowse::engine::table::ExportFormat;
use dowse::engine::workspace;

/// Commands listed when nothing is typed, and at most when something is.
const MAX_MATCHES: usize = 60;

struct PaletteCommand {
    group: &'static str,
    label: String,
    icon: Icon,
    /// Shows the command's shortcut; running it calls `run` directly, since
    /// the palette's dialog is outside the window's key context.
    action: Option<Box<dyn Action>>,
    keywords: Vec<&'static str>,
    checked: bool,
    run: AppCommand,
}

fn command(
    group: &'static str,
    label: impl Into<String>,
    icon: impl Into<Icon>,
    run: impl Fn(&mut SearchApp, &mut Window, &mut Context<SearchApp>) + 'static,
) -> PaletteCommand {
    PaletteCommand {
        group,
        label: label.into(),
        icon: icon.into(),
        action: None,
        keywords: Vec::new(),
        checked: false,
        run: Rc::new(run),
    }
}

impl PaletteCommand {
    fn action(mut self, action: impl Action) -> Self {
        self.action = Some(Box::new(action));
        self
    }

    fn keywords(mut self, keywords: &[&'static str]) -> Self {
        self.keywords = keywords.to_vec();
        self
    }

    fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// How well `query` matches the label, or less well a keyword or group.
    fn score(&self, query: &str) -> Option<i32> {
        let label = fuzzy::score(query, &self.label);
        let others = self
            .keywords
            .iter()
            .copied()
            .chain([self.group])
            .filter_map(|word| fuzzy::score(query, word))
            .max()
            .map(|score| score - 40);
        label.max(others)
    }
}

impl SearchApp {
    pub(super) fn on_command_palette(
        &mut self,
        _: &CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_palette(window, cx);
    }

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let commands: Rc<Vec<PaletteCommand>> = Rc::new(self.palette_commands(cx));
        let state = cx.new(|cx| CommandState::new(window, cx));
        // The commands behind each row of the latest render, as the palette
        // reports confirmed rows by position.
        let layout: Rc<RefCell<Vec<Vec<usize>>>> = Rc::default();
        let app = cx.entity().downgrade();

        let palette_state = state.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            let query = palette_state.read(cx).query(cx).to_string();
            let sections = arrange(&commands, &query);
            *layout.borrow_mut() = sections.iter().map(|(_, rows)| rows.clone()).collect();
            let flat = !query.trim().is_empty();

            let mut palette = Command::new(&palette_state)
                .filterable(false)
                .bordered(false)
                .max_h(px(440.))
                .placeholder("Type a command, a tab, a tag or a qualifier…")
                // The rows are chosen here, so draw again as the query changes.
                .on_query(|_, window, _| window.refresh())
                .on_confirm({
                    let (layout, commands, app) = (layout.clone(), commands.clone(), app.clone());
                    move |path, window, cx| {
                        let index = layout
                            .borrow()
                            .get(path.section)
                            .and_then(|rows| rows.get(path.row))
                            .copied();
                        let Some(index) = index else {
                            return;
                        };
                        window.close_dialog(cx);
                        let run = commands[index].run.clone();
                        app.update(cx, |this, cx| run(this, window, cx)).ok();
                    }
                })
                .empty(|_, _, cx| {
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("No matching command")
                });
            for (heading, rows) in sections {
                let items = rows.iter().map(|&index| item(&commands, index, flat));
                let group = CommandGroup::new().items(items);
                palette = palette.group(match heading {
                    Some(heading) => group.label(heading),
                    None => group,
                });
            }
            dialog
                .w(px(640.))
                .margin_top(px(72.))
                .close_button(false)
                .p_2()
                .child(palette)
        });
        state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Everything the palette offers in this window right now.
    fn palette_commands(&self, cx: &App) -> Vec<PaletteCommand> {
        let tab = self.tab();
        let results_view = tab.view;
        let mut commands = vec![
            command(
                "Search",
                "Focus Search Box",
                IconName::Search,
                |this, w, cx| this.on_focus_search(&FocusSearch, w, cx),
            )
            .action(FocusSearch),
            command("Search", "Filter Paths", Lucide::Funnel, |this, w, cx| {
                this.on_focus_path_filter(&FocusPathFilter, w, cx)
            })
            .action(FocusPathFilter),
            command(
                "Search",
                "Match Case",
                IconName::CaseSensitive,
                |this, w, cx| this.on_toggle_case_sensitive(&ToggleCaseSensitive, w, cx),
            )
            .action(ToggleCaseSensitive)
            .checked(tab.case_sensitive),
            command(
                "Search",
                "Match Whole Word",
                Lucide::WholeWord,
                |this, w, cx| this.on_toggle_whole_word(&ToggleWholeWord, w, cx),
            )
            .action(ToggleWholeWord)
            .checked(tab.whole_word),
            command(
                "Search",
                "Whole Query as Regular Expression",
                Lucide::Regex,
                |this, w, cx| this.on_toggle_regex(&ToggleRegex, w, cx),
            )
            .action(ToggleRegex)
            .keywords(&["regex"])
            .checked(tab.regex),
            command(
                "Results",
                if results_view == ResultsView::Table {
                    "Show Results as Snippets"
                } else {
                    "Show Results as Table"
                },
                if results_view == ResultsView::Table {
                    Lucide::Rows3
                } else {
                    Lucide::Table
                },
                |this, w, cx| this.toggle_view(w, cx),
            )
            .action(ToggleResultsView)
            .keywords(&["view", "table", "snippets", "grid"]),
        ];
        for format in ExportFormat::ALL {
            let mut export = command(
                "Results",
                format!("Export Results as {}…", format.label()),
                Lucide::Download,
                move |this, w, cx| this.export_results(format, w, cx),
            )
            .keywords(&["save", "download", "file"]);
            if format == ExportFormat::Csv {
                export = export.action(ExportResults).keywords(&["excel", "save"]);
            }
            commands.push(export);
        }
        commands.extend([
            command(
                "Results",
                "Copy Results as TSV",
                IconName::Copy,
                |this, w, cx| this.copy_results(ExportFormat::Tsv, w, cx),
            )
            .action(CopyResultsAsTsv)
            .keywords(&["clipboard", "spreadsheet", "excel"]),
            command(
                "Results",
                "Copy Results as Markdown",
                IconName::Copy,
                |this, w, cx| this.copy_results(ExportFormat::Markdown, w, cx),
            )
            .action(CopyResultsAsMarkdown)
            .keywords(&["clipboard"]),
            command(
                "Results",
                "Next Match",
                IconName::ChevronDown,
                |this, w, cx| this.on_next_match(&NextMatch, w, cx),
            )
            .action(NextMatch),
            command(
                "Results",
                "Previous Match",
                IconName::ChevronUp,
                |this, w, cx| this.on_previous_match(&PreviousMatch, w, cx),
            )
            .action(PreviousMatch),
        ]);
        if tab.preview.is_some() {
            commands.push(
                command(
                    "Results",
                    "Close Preview",
                    IconName::Close,
                    |this, w, cx| this.on_close_preview(&ClosePreview, w, cx),
                )
                .action(ClosePreview),
            );
        }
        if !tab.facet_filter.is_empty() {
            commands.push(
                command("Results", "Clear Filters", Lucide::Funnel, |this, _, cx| {
                    let tab = this.tab_mut();
                    tab.facet_filter = Default::default();
                    tab.refresh_visible();
                    cx.notify();
                })
                .keywords(&["facets"]),
            );
        }

        // Qualifiers, added to the query, so the syntax is discoverable.
        let qualifiers: [(&str, &str); 9] = [
            ("path:", "Files whose path matches"),
            ("-path:", "Files whose path doesn't match"),
            ("lang:", "Files in a language"),
            ("repo:", "Files from a repository"),
            ("branch:", "Repositories on a branch"),
            ("tag:", "Repositories with a tag"),
            ("content:", "Text that looks like a qualifier"),
            ("OR", "Either side matches"),
            ("NOT", "The next term doesn't match"),
        ];
        for (text, detail) in qualifiers {
            commands.push(
                command(
                    "Query",
                    format!("{text}  {detail}"),
                    Lucide::TextSearch,
                    move |this, w, cx| this.append_to_query(text, w, cx),
                )
                .keywords(&["syntax", "qualifier", "insert"]),
            );
        }

        commands.extend([
            command("Tabs", "New Tab", IconName::Plus, |this, w, cx| {
                this.on_new_tab(&NewTab, w, cx)
            })
            .action(NewTab),
            command("Tabs", "Close Tab", IconName::Close, |this, w, cx| {
                this.on_close_tab(&CloseTab, w, cx)
            })
            .action(CloseTab),
            command("Tabs", "Next Tab", IconName::ChevronRight, |this, w, cx| {
                this.on_next_tab(&NextTab, w, cx)
            })
            .action(NextTab),
            command(
                "Tabs",
                "Previous Tab",
                IconName::ChevronLeft,
                |this, w, cx| this.on_previous_tab(&PreviousTab, w, cx),
            )
            .action(PreviousTab),
        ]);
        for (index, other) in self.tabs.iter().enumerate() {
            if index == self.active_tab {
                continue;
            }
            commands.push(
                command(
                    "Tabs",
                    format!("Go to Tab: {}", other.label(cx)),
                    IconName::Search,
                    move |this, w, cx| this.activate_tab(index, w, cx),
                )
                .keywords(&["switch"]),
            );
        }

        let repos = self.repo_views(cx);
        for (_, tags) in repo::tag_catalog(repos.iter().map(|repo| &*repo.info)) {
            for (tag, _) in tags {
                let checked = self.scope.contains(&tag);
                let label = format!("Scope: {}", tag_label(&tag));
                commands.push(
                    command("Scope", label, Lucide::Tag, move |this, _, cx| {
                        this.toggle_scope_tag(&tag, cx)
                    })
                    .keywords(&["tag", "narrow"])
                    .checked(checked),
                );
            }
        }
        if !self.scope.is_empty() {
            commands.push(
                command("Scope", "Clear Scope", Lucide::Tag, |this, _, cx| {
                    this.clear_scope(cx)
                })
                .keywords(&["all repositories"]),
            );
        }

        commands.extend([
            command(
                "Workspace",
                "Add Repositories…",
                Lucide::FolderGit2,
                |this, w, cx| this.on_add_repository(&AddRepository, w, cx),
            )
            .action(AddRepository)
            .keywords(&["folder", "open"]),
            command(
                "Workspace",
                "Show Repositories",
                Lucide::FolderGit2,
                |this, w, cx| this.on_show_repositories(&ShowRepositories, w, cx),
            )
            .action(ShowRepositories)
            .keywords(&["tags", "settings"]),
            command(
                "Workspace",
                "Rebuild Indexes in Scope",
                Lucide::RefreshCw,
                |this, w, cx| this.on_rebuild_index(&RebuildIndex, w, cx),
            )
            .action(RebuildIndex)
            .keywords(&["reindex"]),
            command("Workspace", "New Window", Lucide::AppWindow, |_, _, cx| {
                Windows::new_window(cx);
            })
            .action(NewWindow),
            command(
                "Workspace",
                "New Workspace",
                Lucide::FilePlus,
                |this, w, cx| this.on_new_workspace(&NewWorkspace, w, cx),
            ),
            command(
                "Workspace",
                "Open Workspace…",
                IconName::FolderOpen,
                |this, w, cx| this.on_open_workspace(&OpenWorkspace, w, cx),
            )
            .action(OpenWorkspace),
            command(
                "Workspace",
                "Save Workspace As…",
                Lucide::Save,
                |this, w, cx| this.on_save_workspace_as(&SaveWorkspaceAs, w, cx),
            )
            .action(SaveWorkspaceAs),
        ]);
        let current = self.workspace.clone();
        for file in Windows::recent(cx) {
            if Some(&file) == current.as_ref() {
                continue;
            }
            let label = format!(
                "Open Recent: {}  ·  {}",
                workspace::name(&file),
                short_dir(&file)
            );
            let target: PathBuf = file.clone();
            commands.push(
                command("Workspace", label, Lucide::Layers, move |this, w, cx| {
                    this.open_workspace_file(target.clone(), w, cx)
                })
                .keywords(&["workspace"]),
            );
        }
        commands.push(
            command("View", "Developer Tools", Lucide::Bug, |_, _, cx| {
                super::devtools::open(cx);
            })
            .action(ToggleDevTools)
            .keywords(&["logs", "metrics", "debug", "devtools"]),
        );
        commands.push(
            command(
                "View",
                "Toggle Light and Dark Theme",
                IconName::Moon,
                |this, w, cx| this.on_toggle_theme(&ToggleTheme, w, cx),
            )
            .keywords(&["dark mode", "appearance"]),
        );
        commands
    }

    /// Add `text` to the end of the query, as a new term, and put the cursor
    /// after it.
    fn append_to_query(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.tab().search_input.clone();
        let value = input.read(cx).value().to_string();
        let joined = if value.is_empty() || value.ends_with(' ') {
            format!("{value}{text}")
        } else {
            format!("{value} {text}")
        };
        // An operator needs something after it.
        let joined = if text.ends_with(':') {
            joined
        } else {
            format!("{joined} ")
        };
        input.update(cx, |input, cx| input.set_value(joined, window, cx));
        self.show_search(window, cx);
        self.run_search(true, cx);
    }
}

/// The palette's sections: by group when nothing is typed, otherwise one
/// list, best match first.
fn arrange(commands: &[PaletteCommand], query: &str) -> Vec<(Option<&'static str>, Vec<usize>)> {
    if query.trim().is_empty() {
        let mut sections: Vec<(Option<&'static str>, Vec<usize>)> = Vec::new();
        for (index, command) in commands.iter().enumerate() {
            match sections.last_mut() {
                Some((Some(group), rows)) if *group == command.group => rows.push(index),
                _ => sections.push((Some(command.group), vec![index])),
            }
        }
        return sections;
    }
    let mut ranked: Vec<(i32, usize)> = commands
        .iter()
        .enumerate()
        .filter_map(|(index, command)| Some((command.score(query)?, index)))
        .collect();
    // Best first; equal scores keep the palette's order.
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let rows: Vec<usize> = ranked
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(_, index)| index)
        .collect();
    if rows.is_empty() {
        Vec::new()
    } else {
        vec![(None, rows)]
    }
}

/// One row: icon, label, the group when ranked, a check, and the shortcut.
fn item(commands: &Rc<Vec<PaletteCommand>>, index: usize, show_group: bool) -> CommandItem {
    let label = SharedString::from(commands[index].label.clone());
    let commands = commands.clone();
    CommandItem::new()
        .label(label.clone())
        .child(move |window, cx| {
            let command = &commands[index];
            let muted = cx.theme().muted_foreground;
            // The window's bindings are scoped to the search view, not the dialog.
            let binding = command
                .action
                .as_ref()
                .and_then(|action| Kbd::binding_for_action(action.as_ref(), Some(CONTEXT), window));
            h_flex()
                .w_full()
                .gap_2()
                .child(command.icon.clone().small().text_color(muted))
                .child(div().flex_1().min_w_0().truncate().child(label.clone()))
                .when(show_group, |row| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(muted)
                            .child(command.group),
                    )
                })
                .when(command.checked, |row| {
                    row.child(Icon::new(IconName::Check).small())
                })
                .children(binding)
        })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI's `test` attribute would shadow the built-in one.
    use super::{PaletteCommand, arrange, command};
    use gpui_kit::component::IconName;

    fn commands(labels: &[(&'static str, &str)]) -> Vec<PaletteCommand> {
        labels
            .iter()
            .map(|(group, label)| command(group, *label, IconName::Search, |_, _, _| {}))
            .collect()
    }

    #[test]
    fn groups_show_until_something_is_typed() {
        let commands = commands(&[
            ("Tabs", "New Tab"),
            ("Tabs", "Close Tab"),
            ("View", "Theme"),
        ]);
        assert_eq!(
            arrange(&commands, " "),
            vec![(Some("Tabs"), vec![0, 1]), (Some("View"), vec![2])]
        );
    }

    #[test]
    fn a_query_ranks_matches_into_one_list() {
        let commands = commands(&[
            ("Results", "Next Match"),
            ("Tabs", "New Tab"),
            ("View", "Toggle Theme"),
        ]);
        assert_eq!(arrange(&commands, "nt"), vec![(None, vec![1, 0])]);
        assert!(arrange(&commands, "zzz").is_empty());
        // A group's name finds its commands too, after label matches.
        assert_eq!(arrange(&commands, "view"), vec![(None, vec![2])]);
    }
}

//! The table view of a tab's results: one row per matching line, sortable by
//! any column, and the exports and copies made from it.

use std::path::PathBuf;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::table::{Column, ColumnSort, TableDelegate, TableEvent, TableState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::*;

use super::app::SearchApp;
use super::render::match_style;
use super::{CopyResultsAsMarkdown, CopyResultsAsTsv, ExportResults, ToggleResultsView};
use crate::format;
use dowse::engine::table::{ColumnKind, ExportFormat, ResultTable, TableRow};

/// How a tab shows its results.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ResultsView {
    #[default]
    Snippets,
    Table,
}

/// The table's rows in their current order.
pub(super) struct ResultsTable {
    table: ResultTable,
    /// Indexes into `table.rows`, as sorted.
    order: Vec<usize>,
    /// Indexes into `table.columns` shown, in order. Exports keep them all.
    shown: Vec<usize>,
}

impl ResultsTable {
    fn new(table: ResultTable) -> Self {
        let order = (0..table.rows.len()).collect();
        // With every row from one repository, its name and branch say nothing.
        let single_repository = table
            .columns
            .iter()
            .position(|column| column.kind == ColumnKind::Repository)
            .is_some_and(|repository| {
                let mut names = table.rows.iter().map(|row| &row.cells[repository]);
                names
                    .next()
                    .is_none_or(|first| names.all(|name| name == first))
            });
        let shown = (0..table.columns.len())
            .filter(|&index| {
                let kind = table.columns[index].kind;
                !(single_repository && matches!(kind, ColumnKind::Repository | ColumnKind::Branch))
            })
            .collect();
        Self {
            table,
            order,
            shown,
        }
    }

    /// The row shown at `index`.
    fn row(&self, index: usize) -> Option<&TableRow> {
        self.table.rows.get(*self.order.get(index)?)
    }

    /// The text of the cell shown at `row` and `column`.
    fn cell(&self, row: usize, column: usize) -> &str {
        let Some(&column) = self.shown.get(column) else {
            return "";
        };
        self.row(row)
            .and_then(|row| row.cells.get(column))
            .map_or("", String::as_str)
    }

    fn kind(&self, column: usize) -> ColumnKind {
        self.table.columns[self.shown[column]].kind
    }
}

/// A tab's table and the subscription to its clicks.
pub(super) struct TableView {
    pub(super) state: Entity<TableState<ResultsTable>>,
    _subscription: Subscription,
}

fn width(kind: ColumnKind) -> f32 {
    match kind {
        ColumnKind::Repository => 140.,
        ColumnKind::Branch => 110.,
        ColumnKind::Path => 320.,
        ColumnKind::Line | ColumnKind::Column => 72.,
        ColumnKind::Language => 100.,
        ColumnKind::Match => 180.,
        ColumnKind::Group(_) => 150.,
        ColumnKind::Text => 640.,
    }
}

impl TableDelegate for ResultsTable {
    fn columns_count(&self, _: &App) -> usize {
        self.shown.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.order.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        let column = &self.table.columns[self.shown[col_ix]];
        let base = Column::new(format!("column-{col_ix}"), column.name.clone())
            .sortable()
            .width(px(width(column.kind)));
        if column.kind.is_number() {
            base.text_right()
        } else {
            base
        }
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let column = self.shown[col_ix];
        self.order = match sort {
            ColumnSort::Ascending => self.table.sorted(column, false),
            ColumnSort::Descending => self.table.sorted(column, true),
            ColumnSort::Default => (0..self.table.rows.len()).collect(),
        };
        cx.notify();
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let kind = self.kind(col_ix);
        let text = SharedString::from(self.cell(row_ix, col_ix).to_string());
        let theme = cx.theme();
        let cell = div().w_full().truncate();
        let cell = match kind {
            ColumnKind::Match | ColumnKind::Group(_) | ColumnKind::Text => cell
                .font_family(theme.mono_font_family.clone())
                .text_size(theme.mono_font_size),
            ColumnKind::Line | ColumnKind::Column | ColumnKind::Branch => {
                cell.text_color(theme.muted_foreground)
            }
            _ => cell,
        };
        if kind == ColumnKind::Match && !text.is_empty() {
            // The matched text, marked as in the snippets.
            let length = text.len();
            return cell
                .child(StyledText::new(text).with_highlights([(0..length, match_style(cx))]));
        }
        cell.child(text)
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, _: &App) -> String {
        self.cell(row_ix, col_ix).to_string()
    }
}

impl SearchApp {
    pub(super) fn set_view(
        &mut self,
        view: ResultsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.tab_mut().view = view;
        if view == ResultsView::Table {
            self.ensure_table(window, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = match self.tab().view {
            ResultsView::Snippets => ResultsView::Table,
            ResultsView::Table => ResultsView::Snippets,
        };
        self.set_view(view, window, cx);
    }

    /// The current tab's table, built from its results when first needed.
    pub(super) fn ensure_table(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TableState<ResultsTable>>> {
        let tab = &self.tabs[self.active_tab];
        if let Some(view) = &tab.table {
            return Some(view.state.clone());
        }
        let results = tab.results.as_ref()?;
        let table = ResultTable::new(&results.outcome.files, &results.visible, &results.matcher);
        let state = cx.new(|cx| {
            TableState::new(ResultsTable::new(table), window, cx)
                .col_movable(false)
                .col_selectable(false)
        });
        let subscription = cx.subscribe_in(&state, window, Self::on_table_event);
        self.tab_mut().table = Some(TableView {
            state: state.clone(),
            _subscription: subscription,
        });
        Some(state)
    }

    /// Selecting a row previews its line; double-clicking opens the editor.
    fn on_table_event(
        &mut self,
        state: &Entity<TableState<ResultsTable>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (row, open) = match event {
            TableEvent::SelectRow(row) => (*row, false),
            TableEvent::DoubleClickedRow(row) => (*row, true),
            _ => return,
        };
        let Some((file, line)) = state
            .read(cx)
            .delegate()
            .row(row)
            .map(|row| (row.file, row.line))
        else {
            return;
        };
        let Some(file) = self
            .tab()
            .results
            .as_ref()
            .and_then(|results| results.outcome.files.get(file))
            .cloned()
        else {
            return;
        };
        if open {
            self.open_hit(&file.repo.root, &file.path, line, window, cx);
        } else {
            self.preview_hit(
                file.repo.clone(),
                file.path.clone(),
                file.language,
                line,
                cx,
            );
        }
    }

    /// The current tab's results as `format`, in the table's order, with the
    /// number of rows; `None` without results.
    fn export_text(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(String, usize)> {
        let state = self.ensure_table(window, cx)?;
        let table = state.read(cx).delegate();
        Some((table.table.export(&table.order, format), table.order.len()))
    }

    pub(super) fn copy_results(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((text, rows)) = self.export_text(format, window, cx) else {
            window.push_notification("Nothing to copy yet: search first.", cx);
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        window.push_notification(
            format!(
                "Copied {} as {}",
                format::plural(rows, "row", "rows"),
                format.label()
            ),
            cx,
        );
    }

    /// Ask where to save the results, then write them there. The file's
    /// extension picks the format; without one, `format`'s is added.
    pub(super) fn export_results(
        &mut self,
        format: ExportFormat,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab().results.is_none() {
            window.push_notification("Nothing to export yet: search first.", cx);
            return;
        }
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(std::env::temp_dir);
        let suggested = format!("dowse-results.{}", format.extension());
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(file))) = chosen.await else {
                return;
            };
            let (file, format) = match file.extension() {
                Some(_) => {
                    let format = ExportFormat::for_path(&file);
                    (file, format)
                }
                None => {
                    let mut named = file.into_os_string();
                    named.push(format!(".{}", format.extension()));
                    (PathBuf::from(named), format)
                }
            };
            this.update_in(cx, |this, window, cx| {
                let Some((text, rows)) = this.export_text(format, window, cx) else {
                    return;
                };
                let name = file
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match std::fs::write(&file, text) {
                    Ok(()) => window.push_notification(
                        format!("Exported {} to {name}", format::plural(rows, "row", "rows")),
                        cx,
                    ),
                    Err(error) => window.push_notification(
                        Notification::error(format!("Could not write {name}: {error}")),
                        cx,
                    ),
                }
            })
            .ok();
        })
        .detach();
    }
}

impl SearchApp {
    pub(super) fn on_toggle_results_view(
        &mut self,
        _: &ToggleResultsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_view(window, cx);
    }

    pub(super) fn on_export_results(
        &mut self,
        _: &ExportResults,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.export_results(ExportFormat::Csv, window, cx);
    }

    pub(super) fn on_copy_results_as_tsv(
        &mut self,
        _: &CopyResultsAsTsv,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_results(ExportFormat::Tsv, window, cx);
    }

    pub(super) fn on_copy_results_as_markdown(
        &mut self,
        _: &CopyResultsAsMarkdown,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.copy_results(ExportFormat::Markdown, window, cx);
    }
}

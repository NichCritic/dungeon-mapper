//! The session notes drawer: a collapsible strip under the map that shows the
//! selected room's Markdown note, plus the vault browser window.
//!
//! The drawer follows the current selection. Following a `[[wikilink]]` pins the
//! drawer to that note and grows a back trail; changing the selection resets it.

use egui::RichText;

use crate::notes::markdown::{self, NoteAction};
use crate::notes::{NoteBind, NoteVault};
use crate::ui::window_dock::{self, DockWindow};

/// What the rest of the app currently has selected, so the drawer knows which
/// note to show and what to call one it has to create.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NoteContext {
    pub bind: Option<NoteBind>,
    /// Human-readable name of the selected entity, used as the note's filename.
    pub title: String,
    /// Map name, used to group notes into per-map folders.
    pub map_folder: Option<String>,
}

pub struct NotesPanelState {
    pub open: bool,
    pub expanded: bool,
    pub height: f32,
    /// Note pinned by following a link; `None` means "follow the selection".
    pinned: Option<String>,
    /// Back trail of note ids from following links.
    trail: Vec<String>,
    editing: bool,
    /// Live editor buffer, committed to the vault as it changes.
    draft: String,
    /// Note the draft belongs to, so switching notes cannot cross-write.
    draft_of: Option<String>,
    /// Selection the drawer last rendered, to detect a change. `None` until the
    /// first frame, so opening a note before the drawer has ever drawn survives.
    last_context: Option<NoteContext>,
    /// Pending `[[` autocomplete: byte range of the partial link in the draft.
    completion: Option<(usize, usize)>,
    completion_pick: usize,
    pub browser_query: String,
    /// Set when a link points at a note that does not exist yet.
    missing_link: Option<String>,
}

impl Default for NotesPanelState {
    fn default() -> Self {
        Self {
            open: true,
            expanded: true,
            height: 190.0,
            pinned: None,
            trail: Vec::new(),
            editing: false,
            draft: String::new(),
            draft_of: None,
            last_context: None,
            completion: None,
            completion_pick: 0,
            browser_query: String::new(),
            missing_link: None,
        }
    }
}

impl NotesPanelState {
    /// Cycle the drawer: expanded -> collapsed to its title strip -> hidden.
    /// Collapsed is the useful middle stop while navigating — the note's first
    /// line stays on screen and the map gets its height back.
    pub fn toggle(&mut self) {
        match (self.open, self.expanded) {
            (true, true) => self.expanded = false,
            (true, false) => self.open = false,
            (false, _) => {
                self.open = true;
                self.expanded = true;
            }
        }
    }

    /// Show a specific note, remembering where we came from.
    pub fn open_note(&mut self, id: &str, current: Option<&str>) {
        if let Some(cur) = current {
            if cur != id {
                self.trail.push(cur.to_string());
            }
        }
        self.pinned = Some(id.to_string());
        self.open = true;
        self.expanded = true;
        self.stop_editing();
    }

    fn stop_editing(&mut self) {
        self.editing = false;
        self.draft.clear();
        self.draft_of = None;
        self.completion = None;
    }
}

/// Width the header's right-hand button strip needs, so the collapsed summary
/// can be truncated instead of sliding underneath it.
const BUTTON_STRIP_WIDTH: f32 = 210.0;

/// Draw the notes drawer as a bottom panel. Returns nothing; all edits land in the vault.
pub fn notes_drawer(
    ctx: &egui::Context,
    vault: &mut NoteVault,
    state: &mut NotesPanelState,
    context: &NoteContext,
) {
    if !state.open {
        return;
    }

    // A new selection resets link navigation so the drawer tracks the map again.
    // First sight of a selection is not a change: a note opened from the browser
    // before the drawer's first frame must not be thrown away.
    if state.last_context.as_ref().is_some_and(|prev| prev != context) {
        state.pinned = None;
        state.trail.clear();
        state.missing_link = None;
        state.stop_editing();
    }
    state.last_context = Some(context.clone());

    let current_id = current_note_id(vault, state, context);

    let mut panel = egui::TopBottomPanel::bottom("notes_drawer").resizable(state.expanded);
    if state.expanded {
        panel = panel.default_height(state.height).min_height(90.0);
    } else {
        panel = panel.exact_height(26.0);
    }

    let response = panel.show(ctx, |ui| {
        header_row(ui, vault, state, context, current_id.as_deref());
        if !state.expanded {
            return;
        }
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("notes_drawer_scroll")
            .show(ui, |ui| {
                body_ui(ui, vault, state, context, current_id.as_deref());
            });
    });

    if state.expanded {
        state.height = response.response.rect.height();
    }
}

/// The note the drawer should be showing right now.
fn current_note_id(vault: &NoteVault, state: &NotesPanelState, context: &NoteContext) -> Option<String> {
    if let Some(pinned) = &state.pinned {
        if vault.get(pinned).is_some() {
            return Some(pinned.clone());
        }
    }
    let bind = context.bind.as_ref()?;
    vault.note_id_for(bind).map(str::to_string)
}

fn header_row(
    ui: &mut egui::Ui,
    vault: &mut NoteVault,
    state: &mut NotesPanelState,
    context: &NoteContext,
    current_id: Option<&str>,
) {
    ui.horizontal(|ui| {
        let arrow = if state.expanded { "\u{25be}" } else { "\u{25b8}" };
        if ui
            .small_button(arrow)
            .on_hover_text(if state.expanded { "Collapse notes (F9)" } else { "Expand notes (F9)" })
            .clicked()
        {
            state.expanded = !state.expanded;
        }

        if !state.trail.is_empty() {
            if ui.small_button("\u{2190}").on_hover_text("Back").clicked() {
                state.pinned = state.trail.pop();
                state.stop_editing();
            }
        }

        let title = current_id
            .and_then(|id| vault.get(id))
            .map(|n| n.title.clone())
            .unwrap_or_else(|| {
                if context.title.is_empty() { "Notes".to_string() } else { context.title.clone() }
            });
        ui.label(RichText::new(title).strong());

        if state.pinned.is_some() && !context.title.is_empty() {
            ui.weak(format!("\u{2022} linked from {}", context.title));
        }

        // Collapsed drawer shows the first line so the strip is still worth its height.
        // It is truncated short of the buttons on the right, which are laid out after it.
        if !state.expanded {
            if let Some(note) = current_id.and_then(|id| vault.get(id)) {
                let summary = markdown::summary_line(&note.body);
                if !summary.is_empty() {
                    let room = ui.available_width() - BUTTON_STRIP_WIDTH;
                    if room > 40.0 {
                        ui.scope(|ui| {
                            ui.set_max_width(room);
                            ui.add(
                                egui::Label::new(RichText::new(summary).weak().italics())
                                    .truncate(),
                            );
                        });
                    }
                }
            }
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("\u{2715}").on_hover_text("Hide notes (F9)").clicked() {
                state.open = false;
            }
            if ui.small_button("Browse").on_hover_text("All notes in this campaign").clicked() {
                window_dock::WindowDock::open(ui.ctx(), DockWindow::NoteBrowser);
            }
            if let Some(id) = current_id {
                let label = if state.editing { "Done" } else { "Edit" };
                if ui.small_button(label).clicked() {
                    if state.editing {
                        commit_draft(vault, state);
                        vault.flush();
                    } else {
                        state.draft = vault.get(id).map(|n| n.body.clone()).unwrap_or_default();
                        state.draft_of = Some(id.to_string());
                        state.editing = true;
                    }
                }
                if !state.editing {
                    if let Some(root) = vault.root.clone() {
                        let path = vault.get(id).map(|n| n.path.clone());
                        if ui
                            .small_button("Reveal")
                            .on_hover_text(format!("Open the .md file ({})", root.display()))
                            .clicked()
                        {
                            if let Some(p) = path {
                                let _ = open::that_detached(&p);
                            }
                        }
                    }
                }
            }
        });
    });
}

fn body_ui(
    ui: &mut egui::Ui,
    vault: &mut NoteVault,
    state: &mut NotesPanelState,
    context: &NoteContext,
    current_id: Option<&str>,
) {
    if let Some(err) = &vault.last_error {
        ui.colored_label(ui.visuals().error_fg_color, format!("Notes: {}", err));
    }

    if let Some(missing) = state.missing_link.clone() {
        ui.horizontal(|ui| {
            ui.label(format!("No note called \u{201c}{}\u{201d}.", missing));
            if ui.button("Create it").clicked() {
                let id = vault.create_page(&missing);
                let from = current_id.map(str::to_string);
                state.open_note(&id, from.as_deref());
                state.missing_link = None;
                state.draft = String::new();
                state.draft_of = Some(id);
                state.editing = true;
            }
            if ui.small_button("Dismiss").clicked() {
                state.missing_link = None;
            }
        });
        ui.separator();
    }

    let Some(id) = current_id.map(str::to_string) else {
        empty_state(ui, vault, state, context);
        return;
    };

    if state.editing {
        editor_ui(ui, vault, state, &id);
        return;
    }

    let action = {
        let Some(note) = vault.get_mut(&id) else { return };
        if note.is_empty() {
            ui.weak("This note is empty. Press Edit to write in it.");
            None
        } else {
            let blocks = note.blocks().to_vec();
            markdown::render(ui, &blocks, 13.5)
        }
    };

    if let Some(action) = action {
        handle_action(vault, state, &id, action);
    }
}

fn empty_state(ui: &mut egui::Ui, vault: &mut NoteVault, state: &mut NotesPanelState, context: &NoteContext) {
    let Some(bind) = context.bind.clone() else {
        ui.weak("Select a room to see its notes.");
        return;
    };
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.weak(format!("No notes yet for {}.", context.title));
        if ui.button("Create note").clicked() {
            let id = vault.ensure(bind, &context.title, context.map_folder.as_deref());
            state.draft = String::new();
            state.draft_of = Some(id);
            state.editing = true;
            state.expanded = true;
        }
    });
    if vault.root.is_none() {
        ui.add_space(4.0);
        ui.weak("Save the campaign to write notes as .md files on disk.");
    }
}

fn editor_ui(ui: &mut egui::Ui, vault: &mut NoteVault, state: &mut NotesPanelState, id: &str) {
    let output = egui::TextEdit::multiline(&mut state.draft)
        .desired_width(f32::INFINITY)
        .desired_rows(6)
        .hint_text("Markdown. Use [[Note Name]] to link another note.")
        .id_salt("note_editor")
        .show(ui);

    if output.response.changed() {
        vault.set_body(id, state.draft.clone());
    }

    // Offer completions when the caret sits inside an unclosed `[[`.
    let caret = output
        .cursor_range
        .map(|r| byte_index(&state.draft, r.primary.ccursor.index));
    state.completion = caret.and_then(|c| open_wikilink(&state.draft, c));

    if let Some((start, end)) = state.completion {
        let partial = state.draft[start..end].to_string();
        let matches: Vec<String> = vault.complete(&partial).into_iter().map(str::to_string).collect();
        if !matches.is_empty() {
            state.completion_pick = state.completion_pick.min(matches.len() - 1);
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_max_width(280.0);
                ui.weak("Link to:");
                for (i, title) in matches.iter().enumerate() {
                    if ui.selectable_label(i == state.completion_pick, title).clicked() {
                        state.draft.replace_range(start..end, title);
                        vault.set_body(id, state.draft.clone());
                        state.completion = None;
                    }
                }
            });
        }
    }

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.weak("**bold**  *italic*  # heading  - [ ] task  > read-aloud  [[link]]");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("Delete note").clicked() {
                vault.delete(id);
                vault.flush();
                state.pinned = state.trail.pop();
                state.stop_editing();
            }
        });
    });
}

fn handle_action(vault: &mut NoteVault, state: &mut NotesPanelState, current: &str, action: NoteAction) {
    match action {
        NoteAction::OpenWiki(target) => match vault.resolve_wiki(&target) {
            Some(id) => {
                let id = id.to_string();
                state.open_note(&id, Some(current));
            }
            None => state.missing_link = Some(target),
        },
        NoteAction::OpenUrl(url) => {
            let _ = open::that_detached(&url);
        }
        NoteAction::ToggleTask(line) => {
            if let Some(note) = vault.get(current) {
                let updated = markdown::toggle_task_line(&note.body, line);
                vault.set_body(current, updated);
                vault.flush();
            }
        }
    }
}

fn commit_draft(vault: &mut NoteVault, state: &mut NotesPanelState) {
    if let Some(id) = state.draft_of.clone() {
        vault.set_body(&id, state.draft.clone());
    }
    state.stop_editing();
}

/// Byte offset for a character index into `s`.
fn byte_index(s: &str, char_index: usize) -> usize {
    s.char_indices().nth(char_index).map(|(b, _)| b).unwrap_or(s.len())
}

/// If the caret sits inside an unclosed `[[`, the byte range of the partial name.
fn open_wikilink(text: &str, caret: usize) -> Option<(usize, usize)> {
    let before = text.get(..caret)?;
    let start = before.rfind("[[")? + 2;
    if before[start..].contains("]]") || before[start..].contains('\n') {
        return None;
    }
    Some((start, caret))
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('\u{2026}');
    out
}

// ------------------------------------------------------------------- browser

/// Searchable listing of every note in the vault.
pub fn note_browser_window(ctx: &egui::Context, vault: &mut NoteVault, state: &mut NotesPanelState) {
    if !window_dock::WindowDock::is_visible(ctx, DockWindow::NoteBrowser) {
        return;
    }
    let mut open_id: Option<String> = None;
    let mut new_page: Option<String> = None;

    window_dock::dock_window(
        ctx,
        DockWindow::NoteBrowser,
        "All Notes",
        |w| w.default_width(380.0).default_height(420.0).resizable(true),
        |ui| {
        ui.horizontal(|ui| {
            ui.label("Search:");
            ui.add(
                egui::TextEdit::singleline(&mut state.browser_query)
                    .desired_width(200.0)
                    .hint_text("title or text"),
            );
            if ui.small_button("Clear").clicked() {
                state.browser_query.clear();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("New page").clicked() {
                let base = if state.browser_query.trim().is_empty() {
                    "Untitled".to_string()
                } else {
                    state.browser_query.trim().to_string()
                };
                new_page = Some(base);
            }
            if let Some(root) = &vault.root {
                if ui.small_button("Open folder").on_hover_text(root.display().to_string()).clicked() {
                    let _ = open::that_detached(root);
                }
            } else {
                ui.weak("Save the campaign to store notes on disk.");
            }
        });
        ui.separator();

        let hits: Vec<(String, String, String)> = vault
            .search(&state.browser_query)
            .into_iter()
            .filter_map(|(id, snippet)| {
                vault.get(id).map(|n| (id.to_string(), n.title.clone(), snippet))
            })
            .collect();

        if hits.is_empty() {
            ui.weak("No notes match.");
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let mut last_folder = String::new();
            for (id, title, snippet) in hits {
                let folder = id.rsplit_once('/').map(|(f, _)| f.to_string()).unwrap_or_default();
                if folder != last_folder {
                    ui.add_space(4.0);
                    ui.weak(if folder.is_empty() { "campaign".to_string() } else { folder.clone() });
                    last_folder = folder;
                }
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    if ui.selectable_label(false, RichText::new(&title).strong()).clicked() {
                        open_id = Some(id.clone());
                    }
                    if !snippet.is_empty() {
                        ui.weak(truncate(&snippet, 60));
                    }
                });
            }
        });
        },
    );

    if let Some(title) = new_page {
        let id = vault.create_page(&title);
        state.open_note(&id, None);
        state.draft = String::new();
        state.draft_of = Some(id);
        state.editing = true;
    }
    if let Some(id) = open_id {
        state.open_note(&id, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_with_drawer(vault: &mut NoteVault, state: &mut NotesPanelState, context: &NoteContext) {
        let ctx = egui::Context::default();
        // Two frames, so panel sizing and any stored widget state round-trip.
        for _ in 0..2 {
            let _ = ctx.run(Default::default(), |ctx| {
                notes_drawer(ctx, vault, state, context);
            });
        }
    }

    fn room_context() -> NoteContext {
        NoteContext {
            bind: Some(NoteBind::Room("r1".into())),
            title: "Throne Room".into(),
            map_folder: Some("Sunken Keep".into()),
        }
    }

    #[test]
    fn drawer_lays_out_with_no_note_for_the_selection() {
        let mut vault = NoteVault::default();
        let mut state = NotesPanelState::default();
        ctx_with_drawer(&mut vault, &mut state, &room_context());
    }

    #[test]
    fn drawer_lays_out_a_note_with_every_block_kind() {
        let mut vault = NoteVault::default();
        let id = vault.ensure(NoteBind::Room("r1".into()), "Throne Room", Some("Sunken Keep"));
        vault.set_body(
            &id,
            "# Throne Room\n\nThe **dais** is [[NPC - Vex]].\n\n- [ ] task\n\n> aloud\n\n| A | B |\n|---|---|\n| 1 | 2 |"
                .to_string(),
        );
        let mut state = NotesPanelState::default();
        ctx_with_drawer(&mut vault, &mut state, &room_context());
    }

    #[test]
    fn drawer_lays_out_while_collapsed_and_while_hidden() {
        let mut vault = NoteVault::default();
        let mut state = NotesPanelState::default();
        state.expanded = false;
        ctx_with_drawer(&mut vault, &mut state, &room_context());
        state.open = false;
        ctx_with_drawer(&mut vault, &mut state, &room_context());
    }

    #[test]
    fn toggle_cycles_expanded_then_collapsed_then_hidden() {
        let mut state = NotesPanelState::default();
        assert!(state.open && state.expanded);
        state.toggle();
        assert!(state.open && !state.expanded, "expanded -> collapsed");
        state.toggle();
        assert!(!state.open, "collapsed -> hidden");
        state.toggle();
        assert!(state.open && state.expanded, "hidden -> expanded");
    }

    #[test]
    fn changing_the_selection_drops_link_navigation() {
        let mut vault = NoteVault::default();
        let page = vault.create_page("NPC - Vex");
        let mut state = NotesPanelState::default();
        state.open_note(&page, None);
        assert_eq!(state.pinned.as_deref(), Some(page.as_str()));

        ctx_with_drawer(&mut vault, &mut state, &room_context());
        assert_eq!(state.pinned.as_deref(), Some(page.as_str()), "same selection keeps the link");

        let other = NoteContext {
            bind: Some(NoteBind::Room("r2".into())),
            title: "Entry Hall".into(),
            map_folder: Some("Sunken Keep".into()),
        };
        ctx_with_drawer(&mut vault, &mut state, &other);
        assert!(state.pinned.is_none(), "a new selection returns the drawer to the map");
        assert!(state.trail.is_empty());
    }

    #[test]
    fn following_a_link_leaves_a_back_trail() {
        let mut state = NotesPanelState::default();
        state.open_note("pages/A", Some("rooms/Throne Room"));
        state.open_note("pages/B", Some("pages/A"));
        assert_eq!(state.trail, vec!["rooms/Throne Room", "pages/A"]);
        assert_eq!(state.pinned.as_deref(), Some("pages/B"));
    }

    #[test]
    fn wikilink_completion_spots_an_unclosed_link() {
        assert_eq!(open_wikilink("see [[Ve", 8), Some((6, 8)));
        assert_eq!(open_wikilink("see [[Vex]] done", 16), None, "closed link");
        assert_eq!(open_wikilink("no link here", 12), None);
        assert_eq!(open_wikilink("[[a\nb", 5), None, "links do not span lines");
    }

    #[test]
    fn byte_index_handles_multibyte_text() {
        let s = "sw\u{f6}rd [[";
        assert_eq!(byte_index(s, 0), 0);
        assert_eq!(byte_index(s, 3), 4, "past the two-byte o-umlaut");
        assert_eq!(byte_index(s, 999), s.len());
    }
}

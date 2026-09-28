//! Managed floating windows that can be minimized to a bottom tab bar and restored from it.
//!
//! Every standalone tool window goes through [`dock_window`]. The window's title-bar collapse
//! arrow is repurposed to "minimize to the bar"; the X closes the window. State lives in egui
//! temp memory so it is reachable from any sidebar or window function.

use egui::collapsing_header::CollapsingState;

/// Every standalone window the bar knows about.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DockWindow {
    CombatTracker,
    CombatPrep,
    DistanceChecks,
    MassSave,
    StatBlock,
    EncounterEditor,
    MonsterBrowser,
    MonsterWorkshop,
    CustomMonsterEditor,
    MonteCarlo,
    NoteBrowser,
}

impl DockWindow {
    /// Text shown on the window's tab in the bar.
    pub fn label(self) -> &'static str {
        match self {
            DockWindow::CombatTracker => "Combat Tracker",
            DockWindow::CombatPrep => "Combat Prep",
            DockWindow::DistanceChecks => "Distance Checks",
            DockWindow::MassSave => "Mass Save",
            DockWindow::StatBlock => "Stat Block",
            DockWindow::EncounterEditor => "Encounter Editor",
            DockWindow::MonsterBrowser => "Monster Browser",
            DockWindow::MonsterWorkshop => "Monster Workshop",
            DockWindow::CustomMonsterEditor => "Creature Editor",
            DockWindow::MonteCarlo => "Monte Carlo",
            DockWindow::NoteBrowser => "All Notes",
        }
    }

    /// Stable egui id for the window's `Area`, independent of its (possibly dynamic) title.
    pub fn id(self) -> egui::Id {
        egui::Id::new(("dock_window", self))
    }

    /// Windows that only make sense while presenting; closed on Stop Presenting.
    pub fn presentation_only(self) -> bool {
        matches!(
            self,
            DockWindow::CombatTracker
                | DockWindow::CombatPrep
                | DockWindow::DistanceChecks
                | DockWindow::MassSave
                | DockWindow::StatBlock
        )
    }
}

/// Open/minimized state of every managed window, in the order they were opened.
#[derive(Clone, Default)]
pub struct WindowDock {
    entries: Vec<(DockWindow, bool)>,
}

const DOCK_ID: &str = "window_dock";

impl WindowDock {
    pub fn load(ctx: &egui::Context) -> Self {
        ctx.memory(|mem| mem.data.get_temp(egui::Id::new(DOCK_ID))).unwrap_or_default()
    }

    fn store(self, ctx: &egui::Context) {
        ctx.memory_mut(|mem| mem.data.insert_temp(egui::Id::new(DOCK_ID), self));
    }

    fn entry(&self, w: DockWindow) -> Option<bool> {
        self.entries.iter().find(|(e, _)| *e == w).map(|(_, min)| *min)
    }

    /// Open (or minimized) windows, in open order.
    pub fn minimized(&self) -> impl Iterator<Item = DockWindow> + '_ {
        self.entries.iter().filter(|(_, min)| *min).map(|(w, _)| *w)
    }

    /// The window is open and not minimized, i.e. it should be drawn this frame.
    pub fn is_visible(ctx: &egui::Context, w: DockWindow) -> bool {
        Self::load(ctx).entry(w) == Some(false)
    }

    /// The window is open, whether visible or minimized.
    pub fn is_registered(ctx: &egui::Context, w: DockWindow) -> bool {
        Self::load(ctx).entry(w).is_some()
    }

    /// Show the window (registering it if needed, restoring it if minimized) and bring it to the front.
    pub fn open(ctx: &egui::Context, w: DockWindow) {
        let mut dock = Self::load(ctx);
        match dock.entries.iter_mut().find(|(e, _)| *e == w) {
            Some(entry) => entry.1 = false,
            None => dock.entries.push((w, false)),
        }
        dock.store(ctx);
        ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, w.id()));
    }

    /// Hide the window and list it in the bar.
    pub fn minimize(ctx: &egui::Context, w: DockWindow) {
        let mut dock = Self::load(ctx);
        if let Some(entry) = dock.entries.iter_mut().find(|(e, _)| *e == w) {
            entry.1 = true;
        }
        dock.store(ctx);
    }

    /// Close the window entirely (removes its tab too).
    pub fn close(ctx: &egui::Context, w: DockWindow) {
        let mut dock = Self::load(ctx);
        dock.entries.retain(|(e, _)| *e != w);
        dock.store(ctx);
    }

    /// Close every presentation-only window; edit-mode windows are left alone.
    pub fn close_presentation_windows(ctx: &egui::Context) {
        let mut dock = Self::load(ctx);
        dock.entries.retain(|(e, _)| !e.presentation_only());
        dock.store(ctx);
    }
}

/// Draw a managed window if it is currently visible.
///
/// `configure` applies per-window settings (default size, resizable...). Collapsing the window
/// with its title-bar arrow minimizes it to the bar instead; closing with the X closes it.
/// Returns the closure's result, or `None` when the window was not drawn.
pub fn dock_window<R>(
    ctx: &egui::Context,
    w: DockWindow,
    title: impl Into<egui::WidgetText>,
    configure: impl FnOnce(egui::Window<'_>) -> egui::Window<'_>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> Option<R> {
    if !WindowDock::is_visible(ctx, w) {
        return None;
    }
    let mut open = true;
    let window = configure(egui::Window::new(title).id(w.id()).open(&mut open));
    let result = window.show(ctx, add_contents).and_then(|r| r.inner);

    // The collapse arrow was clicked: un-collapse for next time and minimize instead.
    if let Some(mut collapsing) = CollapsingState::load(ctx, w.id().with("collapsing")) {
        if !collapsing.is_open() {
            collapsing.set_open(true);
            collapsing.store(ctx);
            WindowDock::minimize(ctx, w);
        }
    }
    if !open {
        WindowDock::close(ctx, w);
    }
    result
}

/// Bottom tab bar listing minimized windows. Draws nothing when none are minimized.
pub fn window_bar(ctx: &egui::Context) {
    let dock = WindowDock::load(ctx);
    let minimized: Vec<DockWindow> = dock.minimized().collect();
    if minimized.is_empty() {
        return;
    }
    egui::TopBottomPanel::bottom("window_bar").show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            for w in minimized {
                let tab = ui.add(egui::Button::new(egui::RichText::new(w.label()).weak()))
                    .on_hover_text("Restore window");
                if tab.clicked() {
                    WindowDock::open(ctx, w);
                }
                if ui.small_button("\u{00d7}").on_hover_text("Close window").clicked() {
                    WindowDock::close(ctx, w);
                }
                ui.add_space(6.0);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_minimize_close_transitions() {
        let ctx = egui::Context::default();
        let w = DockWindow::MonsterBrowser;
        assert!(!WindowDock::is_registered(&ctx, w));

        WindowDock::open(&ctx, w);
        assert!(WindowDock::is_visible(&ctx, w));

        WindowDock::minimize(&ctx, w);
        assert!(WindowDock::is_registered(&ctx, w));
        assert!(!WindowDock::is_visible(&ctx, w));
        assert_eq!(WindowDock::load(&ctx).minimized().collect::<Vec<_>>(), vec![w]);

        WindowDock::open(&ctx, w);
        assert!(WindowDock::is_visible(&ctx, w));

        WindowDock::close(&ctx, w);
        assert!(!WindowDock::is_registered(&ctx, w));
    }

    #[test]
    fn stop_presenting_keeps_edit_windows() {
        let ctx = egui::Context::default();
        WindowDock::open(&ctx, DockWindow::CombatTracker);
        WindowDock::open(&ctx, DockWindow::EncounterEditor);
        WindowDock::minimize(&ctx, DockWindow::EncounterEditor);

        WindowDock::close_presentation_windows(&ctx);
        assert!(!WindowDock::is_registered(&ctx, DockWindow::CombatTracker));
        assert!(WindowDock::is_registered(&ctx, DockWindow::EncounterEditor));
        assert!(!WindowDock::is_visible(&ctx, DockWindow::EncounterEditor));
    }
}

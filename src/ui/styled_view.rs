use crate::model::*;
use crate::render::EguiRenderer;
use crate::ui::canvas_common::{handle_pan_zoom, ViewState};
use crate::util::ViewTransform;

pub struct StyledViewState {
    pub view: ViewState,
    pub show_grid: bool,
    pub show_labels: bool,
    pub show_notes: bool,
    pub show_secrets: bool,
}

impl Default for StyledViewState {
    fn default() -> Self {
        Self {
            view: ViewState::default(),
            show_grid: true,
            show_labels: true,
            show_notes: true,
            show_secrets: true,
        }
    }
}

pub fn styled_view(ui: &mut egui::Ui, dungeon: &Dungeon, state: &mut StyledViewState) {
    let (response, painter) = ui.allocate_painter(
        ui.available_size(),
        egui::Sense::click_and_drag(),
    );
    let rect = response.rect;

    // Fill with theme background
    let bg = dungeon.theme.bg_color;
    painter.rect_filled(rect, 0.0, egui::Color32::from_rgba_unmultiplied(bg[0], bg[1], bg[2], bg[3]));

    handle_pan_zoom(&response, &mut state.view);
    let transform = ViewTransform::new(state.view.offset, state.view.zoom, rect);

    if let Some(layout) = &dungeon.layout {
        let mut renderer = EguiRenderer::new(&painter, &transform);
        crate::render::themed::render_themed(
            &mut renderer,
            &dungeon.graph,
            layout,
            &dungeon.theme,
            state.show_grid,
            state.show_labels,
            state.show_notes,
            state.show_secrets,
        );
    } else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Generate a layout first (Spatial tab).",
            egui::FontId::proportional(16.0),
            egui::Color32::from_rgb(150, 150, 150),
        );
    }
}

pub fn styled_sidebar(ui: &mut egui::Ui, dungeon: &mut Dungeon, state: &mut StyledViewState) {
    ui.heading("Styled View");
    ui.separator();

    ui.label("Theme:");
    ui.label(&dungeon.theme.name);

    ui.add_space(8.0);
    ui.checkbox(&mut state.show_grid, "Grid lines");
    ui.checkbox(&mut state.show_labels, "Room labels");
    ui.checkbox(&mut state.show_notes, "DM notes");
    ui.checkbox(&mut state.show_secrets, "Show secrets");
    ui.checkbox(&mut dungeon.theme.exterior_shading, "Exterior shading");
    if dungeon.theme.exterior_shading {
        ui.add(egui::Slider::new(&mut dungeon.theme.shading_radius, 0.2..=3.0).text("Radius"));
        egui::ComboBox::from_id_salt("shading_style")
            .selected_text(dungeon.theme.shading_style.label())
            .show_ui(ui, |ui| {
                for s in ShadingStyle::ALL {
                    ui.selectable_value(&mut dungeon.theme.shading_style, s, s.label());
                }
            });
        if dungeon.theme.shading_style == ShadingStyle::Hatched
            || dungeon.theme.shading_style == ShadingStyle::Stippled
        {
            ui.add(egui::Slider::new(&mut dungeon.theme.hatching_density, 0.3..=3.0).text("Density"));
        }
    }

    ui.add_space(16.0);
    ui.heading("Export");
    ui.separator();

    if ui.button("Export DM Map (PNG)").clicked() {
        export_png(dungeon, true);
    }
    if ui.button("Export Player Map (PNG)").clicked() {
        export_png(dungeon, false);
    }
}

fn export_png(dungeon: &Dungeon, dm_mode: bool) {
    if dungeon.layout.is_none() {
        return;
    }

    let path = rfd::FileDialog::new()
        .set_title(if dm_mode { "Export DM Map" } else { "Export Player Map" })
        .add_filter("PNG Image", &["png"])
        .save_file();

    if let Some(path) = path {
        if let Err(e) = crate::io::export::export_png(dungeon, &path, dm_mode, 2) {
            eprintln!("Export error: {}", e);
        }
    }
}

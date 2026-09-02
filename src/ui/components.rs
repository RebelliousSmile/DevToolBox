//! Shared UI primitives. They preserve egui semantics and accessibility nodes.

use eframe::egui;

use super::theme;

pub fn page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.vertical(|ui| {
        ui.heading(egui::RichText::new(title).size(24.0).strong());
        if !subtitle.is_empty() {
            ui.colored_label(theme::palette(ui.visuals().dark_mode).text_muted, subtitle);
        }
    });
    ui.add_space(theme::GRID * 2.0);
}

pub fn card<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let colors = theme::palette(ui.visuals().dark_mode);
    egui::Frame::new()
        .fill(colors.surface)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_CARD))
        .inner_margin(egui::Margin::same(16))
        .show(ui, body)
        .inner
}

#[allow(dead_code)]
pub fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let colors = theme::palette(ui.visuals().dark_mode);
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .color(colors.accent_text)
                .strong(),
        )
        .fill(colors.accent)
        .corner_radius(egui::CornerRadius::same(theme::RADIUS_CONTROL)),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MessageKind {
    Info,
    Success,
    Warning,
    Error,
    Unavailable,
}

pub fn status_message(ui: &mut egui::Ui, kind: MessageKind, text: &str) -> egui::Response {
    let colors = theme::palette(ui.visuals().dark_mode);
    let color = match kind {
        MessageKind::Info => colors.accent,
        MessageKind::Success => colors.success,
        MessageKind::Warning => colors.warning,
        MessageKind::Error => colors.danger,
        MessageKind::Unavailable => colors.text_muted,
    };
    ui.colored_label(color, text)
}

pub fn badge(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let colors = theme::palette(ui.visuals().dark_mode);
    egui::Frame::new()
        .fill(colors.surface_raised)
        .stroke(egui::Stroke::new(1.0, colors.border))
        .corner_radius(egui::CornerRadius::same(99))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| ui.small(label))
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::Queryable;
    use egui_kittest::Harness;

    #[test]
    fn primitives_keep_accessible_labels() {
        let mut harness = Harness::new_ui(|ui| {
            page_header(ui, "Titre", "Contexte");
            card(ui, |ui| {
                primary_button(ui, "Continuer");
                badge(ui, "Disponible");
                status_message(ui, MessageKind::Unavailable, "Indisponible");
            });
        });
        harness.run();
        assert!(harness.query_by_label("Continuer").is_some());
        assert!(harness.query_by_label("Indisponible").is_some());
    }
}

impl BenchScopeApp {
    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F11)) {
            self.toggle_fullscreen(ctx);
        }
    }

    fn ui_fullscreen_button(&mut self, ui: &mut egui::Ui) {
        let label = if self.fullscreen {
            "Exit fullscreen"
        } else {
            "Fullscreen"
        };
        let response = ui
            .add_sized(
                [156.0, 38.0],
                egui::Button::new(egui::RichText::new(label).size(17.0)),
            )
            .on_hover_text("Toggle fullscreen (F11)");
        if response.clicked() {
            self.toggle_fullscreen(ui.ctx());
        }
    }
}

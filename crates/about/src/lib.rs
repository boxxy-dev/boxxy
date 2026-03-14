use libadwaita as adw;
use libadwaita::prelude::*;

#[derive(Debug)]
pub struct AboutComponent {
    dialog: adw::AboutDialog,
}

impl Default for AboutComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl AboutComponent {
    pub fn new() -> Self {
        let dialog = adw::AboutDialog::builder()
            .application_name("Boxxy Terminal")
            .application_icon("play.mii.Boxxy")
            .developer_name("Mii")
            .version(env!("CARGO_PKG_VERSION"))
            .copyright("© 2026 Mii")
            .website("https://github.com/miifrommera/boxxy/")
            .issue_url("https://github.com/miifrommera/boxxy/issues")
            .license_type(gtk4::License::MitX11)
            .build();

        dialog.add_credit_section(Some("Developers"), &["Mii <miifrommera@gmail.com>"]);
        dialog.add_credit_section(Some("Artists"), &["Mii "]);

        Self { dialog }
    }

    pub fn show(&self, parent: &gtk4::Window) {
        self.dialog.present(Some(parent));
    }

    pub fn widget(&self) -> &adw::AboutDialog {
        &self.dialog
    }

    pub fn hide(&self) {
        self.dialog.close();
    }
}

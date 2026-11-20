//! Default component selection for the installer window and `install all`.

pub(crate) const RUST_EXPLORER: &str = "rust-explorer";

pub(crate) fn selected_by_default(name: &str, installed: bool) -> bool {
    !matches!(name, RUST_EXPLORER | "menubar") || installed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_install_does_not_replace_explorer_by_default() {
        assert!(!selected_by_default(RUST_EXPLORER, false));
        assert!(selected_by_default("topbar", false));
        assert!(selected_by_default("rust-search", false));
    }

    #[test]
    fn existing_rust_explorer_is_selected_for_gui_updates() {
        assert!(selected_by_default(RUST_EXPLORER, true));
    }

    #[test]
    fn separate_menu_bar_is_opt_in_and_preserved_on_update() {
        assert!(!selected_by_default("menubar", false));
        assert!(selected_by_default("menubar", true));
    }
}

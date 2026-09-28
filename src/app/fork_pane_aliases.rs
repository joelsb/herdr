//! FORK.md F6 - a moved pane's old public id survives a live handoff.
//!
//! `App::state.public_pane_id_aliases` maps an old public pane id (left
//! behind by a cross-workspace `pane.move`) to the pane's current
//! `PaneId`, so an agent still reporting to its baked-in `HERDR_PANE_ID`
//! keeps resolving. A live handoff rebuilds `App` from a `HandoffManifest`,
//! which upstream owns and does not carry that map, so the alias was lost
//! and the old id came back `pane_not_found` after any handoff following a
//! move. This fork-owned file holds the export/import pair so the upstream
//! hook lines in `src/server/headless/lifecycle.rs` and
//! `src/server/headless/bootstrap.rs` stay one line each.

use std::collections::HashMap;

use super::App;

impl App {
    /// Serialise `public_pane_id_aliases` into the manifest before a live
    /// handoff, keyed by each alias's *current* public id rather than its raw
    /// `PaneId`, since handoff import can renumber those. Dead aliases (the
    /// pane closed since the move) are dropped rather than carried forward.
    pub(crate) fn export_public_pane_id_aliases(&self) -> HashMap<String, String> {
        self.state
            .public_pane_id_aliases
            .iter()
            .filter_map(|(old, &pane_id)| {
                let (ws_idx, _) = self.find_pane(pane_id)?;
                let current = self.public_pane_id(ws_idx, pane_id)?;
                Some((old.clone(), current))
            })
            .collect()
    }

    /// Reinstate `public_pane_id_aliases` after `new_from_handoff` rebuilds
    /// workspaces, resolving each exported current id back to a live
    /// `PaneId`. Skips an entry whose `old` id already resolves to a live
    /// pane on its own (never let a stale alias shadow a real id) or whose
    /// `current` id no longer resolves (the pane closed during the handoff).
    pub(crate) fn import_public_pane_id_aliases(&mut self, aliases: &HashMap<String, String>) {
        for (old, current) in aliases {
            if self.parse_pane_id(old).is_some() {
                continue;
            }
            let Some((_, pane_id)) = self.parse_pane_id(current) else {
                continue;
            };
            self.state
                .public_pane_id_aliases
                .insert(old.clone(), pane_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::workspace::Workspace;

    fn test_app() -> App {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            event_hub,
        );
        app.state.workspaces = vec![Workspace::test_new("herd")];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();
        app
    }

    #[test]
    fn export_skips_an_alias_whose_pane_no_longer_exists() {
        let mut app = test_app();
        app.state
            .public_pane_id_aliases
            .insert("dead".to_string(), crate::layout::PaneId::from_raw(999_999));

        assert!(app.export_public_pane_id_aliases().is_empty());
    }

    #[test]
    fn import_skips_an_alias_whose_old_id_already_resolves() {
        let mut app = test_app();
        let real_pane = app.state.workspaces[0]
            .focused_pane_id()
            .expect("test app should start with a pane");
        let real_public_id = app.public_pane_id(0, real_pane).expect("public pane id");

        let mut aliases = HashMap::new();
        // `old` here is a live pane's own current public id, which must never
        // be shadowed by an imported alias.
        aliases.insert(real_public_id.clone(), "w1:p999".to_string());

        app.import_public_pane_id_aliases(&aliases);

        assert!(app.state.public_pane_id_aliases.is_empty());
    }
}

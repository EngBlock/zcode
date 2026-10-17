//! Settings for every window. For now that's Editor ▸ Vim Mode, kept in
//! the session file with the other toggles; roadmap 2.8 moves settings to a
//! file of their own, and this is the one place to change for that.

use gpui_kit::{App, Global};

use crate::session::Session;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    /// Editor ▸ Vim Mode: off by default, as in Xcode and Zed.
    pub vim_mode: bool,
}

impl Global for Settings {}

impl Settings {
    /// The settings from a session already read, the first time a window
    /// opens (nothing more is read at startup).
    pub fn init(session: &Session, cx: &mut App) {
        if !cx.has_global::<Settings>() {
            cx.set_global(Settings { vim_mode: session.vim_mode.unwrap_or(false) });
        }
    }

    pub fn get(cx: &App) -> Settings {
        cx.try_global::<Settings>().copied().unwrap_or_default()
    }

    /// Changes the settings for every window (they observe the global) and
    /// saves them.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
        let mut settings = Self::get(cx);
        f(&mut settings);
        cx.set_global(settings);
        let mut session = Session::load();
        settings.write(&mut session);
        session.save();
    }

    /// Puts the settings into a session about to be saved.
    pub fn write(self, session: &mut Session) {
        session.vim_mode = Some(self.vim_mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vim_mode_is_off_in_older_sessions_and_round_trips() {
        let old: Session = serde_json::from_str(r#"{ "recent": [], "inlay_hints": true }"#).unwrap();
        assert_eq!(old.vim_mode, None);
        let mut session = old;
        Settings { vim_mode: true }.write(&mut session);
        let reread: Session = serde_json::from_str(&serde_json::to_string(&session).unwrap()).unwrap();
        assert_eq!(reread.vim_mode, Some(true));
    }
}

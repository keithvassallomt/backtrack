// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! What the window is currently looking at, and who needs to know when it
//! changes.
//!
//! The design idea the whole app rests on is that the folder stays put while
//! time moves around it. That only works if "which folder" and "which backup"
//! are one shared fact rather than something each pane keeps its own copy of:
//! the sidebar, the file pane, the preview, the density strip and the position
//! label all read the same state and all re-render from it.
//!
//! Changes are announced with what changed, because the panes care about
//! different parts. Stepping through time must not make the breadcrumb rebuild
//! itself, and walking into a folder must not redraw the density strip.

use std::cell::RefCell;
use std::rc::Rc;

use backtrack_core::index::ArchiveSummary;

/// What just changed. A pane subscribes once and decides from this whether it
/// has any work to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The set of backups was loaded or reloaded.
    Archives,
    /// A different backup is being viewed.
    Seq,
    /// A different folder is being viewed.
    Folder,
    /// A different entry within the folder is selected.
    Selection,
}

/// The entry the file pane has selected, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selected {
    /// Archive-relative path of the entry.
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    /// Size in bytes and modification time in epoch microseconds, as the index
    /// records them. Carried here so the preview can show what it knows about a
    /// file the instant it is selected, without waiting for anything.
    pub size: i64,
    pub mtime: i64,
    /// What the daemon said about this path being on the computer now: 0
    /// unknown, 1 absent, 2 present. Comparing with today needs a today to
    /// compare against, and this is how the window knows there is one.
    pub on_disk: u8,
}

/// Everything the panes read, as one value they can be handed a copy of.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// Every archive, newest first, as `archives_overview` returns them.
    pub archives: Rc<Vec<ArchiveSummary>>,
    /// The backup being viewed. `None` before the first load, and when the
    /// index holds no backups at all.
    pub seq: Option<i64>,
    /// The folder being viewed, archive-relative (`home/keith/Documents`).
    pub folder: String,
    pub selected: Option<Selected>,
}

impl View {
    /// The archive being viewed.
    pub fn archive(&self) -> Option<&ArchiveSummary> {
        let seq = self.seq?;
        self.archives.iter().find(|a| a.seq == seq)
    }

    /// Position of the current backup counting from the newest, 1-based, and
    /// the total — the two numbers the position label spells out.
    pub fn position(&self) -> Option<(usize, usize)> {
        let seq = self.seq?;
        let index = self.archives.iter().position(|a| a.seq == seq)?;
        Some((index + 1, self.archives.len()))
    }

    /// The backup one step older, or `None` at the end of the history.
    pub fn older(&self) -> Option<i64> {
        self.step(1)
    }

    /// The backup one step newer, or `None` at the present.
    pub fn newer(&self) -> Option<i64> {
        self.step(-1)
    }

    /// The path that "previous/next change" applies to: the selected entry if
    /// there is one, and otherwise the folder itself.
    ///
    /// Stepping to the next change to *the folder you are looking at* is the
    /// sensible reading when nothing is selected — it skips the hours in which
    /// nothing you can see changed, which is the whole point of the feature.
    pub fn change_target(&self) -> Option<String> {
        match &self.selected {
            Some(selected) => Some(selected.path.clone()),
            None if self.folder.is_empty() => None,
            None => Some(self.folder.clone()),
        }
    }

    /// What that path is called, for the menu item and the toast.
    pub fn change_target_name(&self) -> Option<String> {
        match &self.selected {
            Some(selected) => Some(selected.name.clone()),
            None if self.folder.is_empty() => None,
            None => Some(crate::path::name(&self.folder).to_string()),
        }
    }

    /// Step `by` places through the newest-first list. Positive is older.
    fn step(&self, by: isize) -> Option<i64> {
        let seq = self.seq?;
        let index = self.archives.iter().position(|a| a.seq == seq)? as isize;
        let target = index.checked_add(by)?;
        if target < 0 {
            return None;
        }
        self.archives.get(target as usize).map(|a| a.seq)
    }
}

type Subscriber = Rc<dyn Fn(&View, Change)>;

/// The shared state, and the panes watching it.
pub struct AppState {
    view: RefCell<View>,
    subscribers: RefCell<Vec<Subscriber>>,
}

impl AppState {
    /// A state looking at `folder`, with nothing loaded yet.
    pub fn new(folder: String) -> Rc<AppState> {
        Rc::new(AppState {
            view: RefCell::new(View {
                folder,
                ..View::default()
            }),
            subscribers: RefCell::new(Vec::new()),
        })
    }

    /// A copy of the current view. Callers get a value, not a borrow, so a
    /// subscriber can change the state while reading it without panicking on
    /// the `RefCell`.
    pub fn view(&self) -> View {
        self.view.borrow().clone()
    }

    /// Watch for changes. There is no unsubscribe: every subscriber is a pane
    /// of the one window and lives exactly as long as the state does.
    pub fn subscribe(&self, on_change: impl Fn(&View, Change) + 'static) {
        self.subscribers.borrow_mut().push(Rc::new(on_change));
    }

    /// Replace the list of archives, keeping the current position if it still
    /// exists and falling back to the newest backup if it does not — which is
    /// what a prune does to whatever the user was looking at.
    pub fn set_archives(&self, archives: Vec<ArchiveSummary>) {
        let mut seq_changed = false;
        {
            let mut view = self.view.borrow_mut();
            let still_there = view
                .seq
                .is_some_and(|s| archives.iter().any(|a| a.seq == s));
            if !still_there {
                let newest = archives.iter().find(|a| a.catalogued).map(|a| a.seq);
                seq_changed = view.seq != newest;
                view.seq = newest;
            }
            view.archives = Rc::new(archives);
        }
        self.announce(Change::Archives);
        if seq_changed {
            self.announce(Change::Seq);
        }
    }

    /// View a different backup. Announces nothing if it is the one already open.
    pub fn set_seq(&self, seq: i64) {
        if self.view.borrow().seq == Some(seq) {
            return;
        }
        self.view.borrow_mut().seq = Some(seq);
        self.announce(Change::Seq);
    }

    /// View a different folder, clearing the selection with it.
    pub fn set_folder(&self, folder: impl Into<String>) {
        let folder = folder.into();
        if self.view.borrow().folder == folder {
            return;
        }
        {
            let mut view = self.view.borrow_mut();
            view.folder = folder;
            view.selected = None;
        }
        self.announce(Change::Folder);
        self.announce(Change::Selection);
    }

    /// Select an entry in the current folder, or clear the selection.
    pub fn set_selected(&self, selected: Option<Selected>) {
        if self.view.borrow().selected == selected {
            return;
        }
        self.view.borrow_mut().selected = selected;
        self.announce(Change::Selection);
    }

    /// Tell every subscriber. The list is copied first so that a subscriber
    /// which changes the state in response — the sidebar selecting a row, say —
    /// cannot re-enter a borrow that is still held.
    fn announce(&self, change: Change) {
        let view = self.view();
        let subscribers = self.subscribers.borrow().clone();
        for subscriber in subscribers {
            subscriber(&view, change);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn archive(seq: i64) -> ArchiveSummary {
        ArchiveSummary {
            seq,
            borg_id: None,
            name: format!("snapshot-{seq}"),
            ts: 1_781_438_400 + seq * 3600,
            repo: "primary".to_string(),
            catalogued: true,
        }
    }

    /// Newest first, as the reader returns them.
    fn archives(seqs: &[i64]) -> Vec<ArchiveSummary> {
        seqs.iter().map(|s| archive(*s)).collect()
    }

    #[test]
    fn loading_archives_lands_on_the_newest_one() {
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[3, 2, 1]));
        assert_eq!(state.view().seq, Some(3));
        assert_eq!(state.view().position(), Some((1, 3)));
    }

    #[test]
    fn stepping_runs_out_at_both_ends() {
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[3, 2, 1]));
        assert_eq!(state.view().newer(), None, "already at the newest");
        assert_eq!(state.view().older(), Some(2));
        state.set_seq(1);
        assert_eq!(state.view().older(), None, "already at the oldest");
        assert_eq!(state.view().newer(), Some(2));
    }

    #[test]
    fn a_reload_keeps_the_backup_being_viewed() {
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[3, 2, 1]));
        state.set_seq(2);
        state.set_archives(archives(&[4, 3, 2, 1]));
        assert_eq!(
            state.view().seq,
            Some(2),
            "a new backup must not move the view"
        );
        assert_eq!(state.view().position(), Some((3, 4)));
    }

    #[test]
    fn a_prune_that_removes_the_viewed_backup_falls_back_to_the_newest() {
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[3, 2, 1]));
        state.set_seq(1);
        state.set_archives(archives(&[3, 2]));
        assert_eq!(state.view().seq, Some(3));
    }

    #[test]
    fn an_archive_still_being_catalogued_is_not_chosen_for_you() {
        // Landing on one would show an empty folder and look like data loss.
        let mut pending = archive(4);
        pending.catalogued = false;
        let state = AppState::new("home".to_string());
        state.set_archives(vec![pending, archive(3), archive(2)]);
        assert_eq!(state.view().seq, Some(3));
    }

    #[test]
    fn next_change_follows_the_selection_and_falls_back_to_the_folder() {
        let state = AppState::new("home/keith/Documents".to_string());
        assert_eq!(
            state.view().change_target(),
            Some("home/keith/Documents".to_string()),
            "with nothing selected, the folder is what you are stepping through"
        );
        assert_eq!(
            state.view().change_target_name(),
            Some("Documents".to_string())
        );

        state.set_selected(Some(Selected {
            path: "home/keith/Documents/report.odt".to_string(),
            name: "report.odt".to_string(),
            is_dir: false,
            size: 45_000,
            mtime: 0,
            on_disk: 0,
        }));
        assert_eq!(
            state.view().change_target(),
            Some("home/keith/Documents/report.odt".to_string())
        );
        assert_eq!(
            state.view().change_target_name(),
            Some("report.odt".to_string())
        );
    }

    #[test]
    fn changing_folder_drops_the_selection() {
        let state = AppState::new("home".to_string());
        state.set_selected(Some(Selected {
            path: "home/notes.txt".to_string(),
            name: "notes.txt".to_string(),
            is_dir: false,
            size: 12,
            mtime: 0,
            on_disk: 0,
        }));
        state.set_folder("home/Documents");
        assert_eq!(state.view().selected, None);
        assert_eq!(state.view().folder, "home/Documents");
    }

    #[test]
    fn setting_what_is_already_set_announces_nothing() {
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[2, 1]));
        let seen = Rc::new(Cell::new(0));
        let counter = Rc::clone(&seen);
        state.subscribe(move |_, change| {
            if change == Change::Seq {
                counter.set(counter.get() + 1);
            }
        });
        state.set_seq(2);
        assert_eq!(seen.get(), 0, "already viewing 2");
        state.set_seq(1);
        assert_eq!(seen.get(), 1);
    }

    #[test]
    fn a_subscriber_may_change_the_state_while_being_told_about_it() {
        // The sidebar does exactly this: told the archives changed, it selects
        // a row, which sets the seq. A borrow held across the callback would
        // turn that into a panic.
        let state = AppState::new("home".to_string());
        let inner = Rc::clone(&state);
        state.subscribe(move |view, change| {
            if change == Change::Archives {
                if let Some(oldest) = view.archives.last() {
                    inner.set_seq(oldest.seq);
                }
            }
        });
        state.set_archives(archives(&[3, 2, 1]));
        assert_eq!(state.view().seq, Some(1));
    }
}

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
//! itself, and walking into a folder redraws the density strip only when the
//! folder lists different backups (Stage 15), which `Change::Listed` says.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use backtrack_core::dbus::TrashedItem;
use backtrack_core::index::ArchiveSummary;

use crate::model::trash;

/// What just changed. A pane subscribes once and decides from this whether it
/// has any work to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The set of backups was loaded or reloaded.
    Archives,
    /// The backups listed for the folder being viewed changed: it is a
    /// different folder, or its answer has arrived.
    Listed,
    /// A different backup is being viewed.
    Seq,
    /// A different folder is being viewed.
    Folder,
    /// A different entry within the folder is selected.
    Selection,
    /// What is on this computer changed under the folder being viewed (a
    /// restore, or its undo), so what the pane says about the disk is stale.
    Disk,
    /// What the desktop's Trash holds changed, so a Trash entry being shown
    /// may show something else (Stage 16).
    Trash,
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
    /// For something in the Trash, the archive name its bytes are read
    /// through (`trash:<…>`); None for an entry of a backup's.
    pub archive: Option<String>,
}

/// Which backups the catalogue said to list for a folder
/// (`IndexReader::listed_for`), and which load of the catalogue it said so of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    pub folder: String,
    /// The [`View::generation`] the answer belongs to. A prune renumbers the
    /// backups, so an answer from before a reload names the wrong ones.
    pub generation: u64,
    pub seqs: Rc<HashSet<i64>>,
}

/// Everything the panes read, as one value they can be handed a copy of.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// Every archive, newest first, as `archives_overview` returns them.
    pub archives: Rc<Vec<ArchiveSummary>>,
    /// The backups the time controls offer for the folder being viewed,
    /// newest first: those in which something in it differs (Stage 15), every
    /// backup still being catalogued, and always the one being viewed. Every
    /// backup until the catalogue has said which to list.
    pub listed: Rc<Vec<ArchiveSummary>>,
    /// What [`Self::listed`] was worked out from.
    pub listing: Option<Listing>,
    /// Counts loads of the catalogue, so a late answer can be recognised.
    pub generation: u64,
    /// Every backup is listed, whatever the listing says (Show All Backups).
    pub show_all: bool,
    /// What the desktop's Trash holds from the backed-up folders, as the
    /// daemon lists it (Stage 16).
    pub trash: Rc<Vec<TrashedItem>>,
    /// The Trash's entries listed for the folder being viewed, newest first
    /// (see [`crate::model::trash`]). In [`Self::listed`] too.
    pub trash_entries: Rc<Vec<ArchiveSummary>>,
    /// The backup being viewed. `None` before the first load, and when the
    /// index holds no backups at all.
    pub seq: Option<i64>,
    /// The folder being viewed, archive-relative (`home/keith/Documents`).
    pub folder: String,
    pub selected: Option<Selected>,
}

impl View {
    /// The backup, or the Trash entry, being viewed.
    pub fn archive(&self) -> Option<&ArchiveSummary> {
        let seq = self.seq?;
        self.archives
            .iter()
            .chain(self.trash_entries.iter())
            .find(|a| a.seq == seq)
    }

    /// The backup the catalogue is asked about for what is being viewed: the
    /// backup itself, or, at a Trash entry, the backup before its gap (None
    /// before the first backup).
    pub fn catalogue_seq(&self) -> Option<i64> {
        let seq = self.seq?;
        match trash::base_of(seq) {
            Some(base) => base,
            None => Some(seq),
        }
    }

    /// The archive the selected entry's bytes are read through: the Trash's,
    /// for something in it, and otherwise the backup the catalogue is asked
    /// about (see [`Self::catalogue_seq`]).
    pub fn reading_archive(&self) -> Option<String> {
        if let Some(archive) = self.selected.as_ref().and_then(|s| s.archive.clone()) {
            return Some(archive);
        }
        let backup = self.catalogue_seq()?;
        self.archives
            .iter()
            .find(|a| a.seq == backup)
            .map(|a| a.name.clone())
    }

    /// What restoring the selected entry, or the folder being viewed, takes
    /// and from where: at a Trash entry, the backup before its gap and what
    /// went to the Trash in it (see [`trash::restoring`]). None when there is
    /// nothing to restore it from.
    pub fn restoring(&self) -> Option<trash::Restoring> {
        let target = self.change_target()?;
        let seq = self.seq?;
        if !trash::is_entry(seq) {
            let backup = self.archive()?;
            return Some(trash::Restoring {
                archive: backup.name.clone(),
                trashed: Vec::new(),
                origin: crate::model::restore::Origin::backup(backup.ts),
            });
        }
        let restoring = trash::restoring(&target, seq, &self.archives, &self.trash);
        (!restoring.archive.is_empty() || !restoring.trashed.is_empty()).then_some(restoring)
    }

    /// Whether a Trash entry is being viewed rather than a backup.
    pub fn at_trash_entry(&self) -> bool {
        self.seq.is_some_and(trash::is_entry)
    }

    /// Position of the current backup among those listed, counting from the
    /// newest, 1-based, and how many are listed: the two numbers the position
    /// label spells out.
    pub fn position(&self) -> Option<(usize, usize)> {
        let seq = self.seq?;
        let index = self.listed.iter().position(|a| a.seq == seq)?;
        Some((index + 1, self.listed.len()))
    }

    /// Whether some backups are left out for this folder, which the position
    /// label has to say, or "3 of 7" reads as though there were only seven.
    pub fn is_filtered(&self) -> bool {
        let backups = self
            .listed
            .iter()
            .filter(|a| !trash::is_entry(a.seq))
            .count();
        backups < self.archives.len()
    }

    /// Whether [`Self::listed`] is the catalogue's answer for the folder being
    /// viewed now, rather than the last folder's, still on screen until this
    /// one's arrives.
    pub fn listing_is_current(&self) -> bool {
        self.listing
            .as_ref()
            .is_some_and(|l| l.folder == self.folder && l.generation == self.generation)
    }

    /// Where a request to view `seq` lands: `seq` itself if it is listed, and
    /// otherwise the nearest newer backup that is, which holds every version
    /// of every file `seq` held in this folder (that is why `seq` was left
    /// out). Only once the list is this folder's: until then, the request is
    /// taken as it stands and settled when the list arrives.
    ///
    /// The nearest newer place may be a Trash entry: one for a gap after a
    /// backup from `seq` on shows that backup's folder, which holds all `seq`
    /// held here. A Trash entry that is gone, its Trash emptied, lands as the
    /// backup before its gap would.
    fn landing(&self, seq: i64) -> i64 {
        if trash::is_entry(seq) {
            if self.trash_entries.iter().any(|e| e.seq == seq) {
                return seq;
            }
            let base = trash::base_of(seq).flatten().or_else(|| {
                // Before the first backup: the first backup is what follows.
                self.archives
                    .iter()
                    .rev()
                    .find(|a| a.catalogued)
                    .map(|a| a.seq)
            });
            return base.map_or(seq, |base| self.landing(base));
        }
        if self.show_all || !self.listing_is_current() {
            return seq;
        }
        let Some(listing) = &self.listing else {
            return seq;
        };
        if listing.seqs.contains(&seq) {
            return seq;
        }
        let backups = self
            .archives
            .iter()
            .filter(|a| a.catalogued && a.seq > seq && listing.seqs.contains(&a.seq))
            .map(|a| (a.ts, a.seq));
        let entries = self
            .trash_entries
            .iter()
            .filter(|e| {
                trash::base_of(e.seq)
                    .flatten()
                    .is_some_and(|base| base >= seq)
            })
            .map(|e| (e.ts, e.seq));
        backups
            .chain(entries)
            .min()
            .map(|(_, seq)| seq)
            .or_else(|| {
                self.archives
                    .iter()
                    .find(|a| a.catalogued && listing.seqs.contains(&a.seq))
                    .map(|a| a.seq)
            })
            .unwrap_or(seq)
    }

    /// Work the Trash's entries for the folder out again, land on what is
    /// listed, and list it: what every change of the folder, the backups,
    /// their listing or the Trash comes down to.
    fn settle(&mut self) {
        self.trash_entries = Rc::new(trash::entries_for(
            &self.folder,
            &self.archives,
            &self.trash,
        ));
        if let Some(seq) = self.seq {
            self.seq = Some(self.landing(seq));
        }
        self.relist();
    }

    /// Take `seqs` as the listing for the folder being viewed, and land on
    /// what it lists.
    fn adopt(view: &mut View, seqs: Rc<HashSet<i64>>) {
        view.listing = Some(Listing {
            folder: view.folder.clone(),
            generation: view.generation,
            seqs,
        });
        view.settle();
    }

    /// Work [`Self::listed`] out again from the archives, the listing, the
    /// Trash's entries and the backup being viewed.
    fn relist(&mut self) {
        let mut listed: Vec<ArchiveSummary> = match &self.listing {
            _ if self.show_all => self.archives.as_ref().clone(),
            None => self.archives.as_ref().clone(),
            Some(listing) => self
                .archives
                .iter()
                .filter(|a| {
                    !a.catalogued || listing.seqs.contains(&a.seq) || Some(a.seq) == self.seq
                })
                .cloned()
                .collect(),
        };
        // Among the backups by time, newest first. The sort is stable, and
        // the backups are already in that order.
        listed.extend(self.trash_entries.iter().cloned());
        listed.sort_by_key(|a| std::cmp::Reverse(a.ts));
        self.listed = Rc::new(listed);
    }

    /// What is listed and when, for telling whether a relist changed
    /// anything: a Trash entry's time moves as things join its gap.
    fn listed_seqs(&self) -> Vec<(i64, i64)> {
        self.listed.iter().map(|a| (a.seq, a.ts)).collect()
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

    /// Step `by` places through the newest-first list of backups listed.
    /// Positive is older.
    fn step(&self, by: isize) -> Option<i64> {
        let seq = self.seq?;
        let index = self.listed.iter().position(|a| a.seq == seq)? as isize;
        let target = index.checked_add(by)?;
        if target < 0 {
            return None;
        }
        self.listed.get(target as usize).map(|a| a.seq)
    }
}

type Subscriber = Rc<dyn Fn(&View, Change)>;

/// The shared state, and the panes watching it.
pub struct AppState {
    view: RefCell<View>,
    subscribers: RefCell<Vec<Subscriber>>,
    /// Every folder's listing known for the current load of the catalogue, so
    /// walking back to a folder already seen costs nothing. Emptied when the
    /// catalogue is reloaded, since a prune renumbers what it names.
    listings: RefCell<HashMap<String, Rc<HashSet<i64>>>>,
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
            listings: RefCell::new(HashMap::new()),
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

    /// Replace the list of archives, with nothing yet known about which to
    /// list: every one is, until a listing arrives. See [`Self::set_catalogue`]
    /// for a load that brings its listing with it, which is how the window
    /// loads them.
    #[cfg(test)]
    pub fn set_archives(&self, archives: Vec<ArchiveSummary>) {
        self.load(archives, None);
    }

    /// Replace the list of archives together with the backups to list for
    /// `folder`, asked of the same reading of the catalogue, so the two can
    /// never disagree about what a seq names.
    pub fn set_catalogue(&self, archives: Vec<ArchiveSummary>, folder: &str, listed: Vec<i64>) {
        self.load(archives, Some((folder.to_string(), listed)));
    }

    /// Keep the current position if it still exists, and fall back to the
    /// newest backup if it does not, which is what a prune does to whatever
    /// the user was looking at. Then land as [`View::landing`] says: a new
    /// backup can leave the one being viewed with nothing of its own to show.
    fn load(&self, archives: Vec<ArchiveSummary>, listed: Option<(String, Vec<i64>)>) {
        let seq_before = self.view.borrow().seq;
        {
            let mut listings = self.listings.borrow_mut();
            listings.clear();
            let mut view = self.view.borrow_mut();
            view.generation += 1;
            let still_there = view
                .seq
                .is_some_and(|s| archives.iter().any(|a| a.seq == s));
            if !still_there {
                view.seq = archives.iter().find(|a| a.catalogued).map(|a| a.seq);
            }
            view.archives = Rc::new(archives);
            view.listing = listed.map(|(folder, seqs)| {
                let seqs = Rc::new(seqs.into_iter().collect::<HashSet<i64>>());
                listings.insert(folder.clone(), Rc::clone(&seqs));
                Listing {
                    folder,
                    generation: view.generation,
                    seqs,
                }
            });
            view.settle();
        }
        self.announce(Change::Archives);
        if self.view.borrow().seq != seq_before {
            self.announce(Change::Seq);
        }
    }

    /// The catalogue's answer to which backups to list for `folder`, asked of
    /// load `generation`. Ignored once it is stale: the window has moved to
    /// another folder, or the catalogue has been reloaded since.
    pub fn set_listing(&self, folder: &str, generation: u64, listed: Vec<i64>) {
        {
            let view = self.view.borrow();
            if view.folder != folder || view.generation != generation {
                return;
            }
        }
        let seqs = Rc::new(listed.into_iter().collect::<HashSet<i64>>());
        self.listings
            .borrow_mut()
            .insert(folder.to_string(), Rc::clone(&seqs));
        self.adopt_listing(seqs);
    }

    /// Make `seqs` the current folder's listing, landing and announcing as it
    /// requires.
    fn adopt_listing(&self, seqs: Rc<HashSet<i64>>) {
        let before = self.marks();
        View::adopt(&mut self.view.borrow_mut(), seqs);
        self.announce_moves(before);
    }

    /// What [`Self::announce_moves`] compares against.
    fn marks(&self) -> (Option<i64>, Vec<(i64, i64)>) {
        let view = self.view.borrow();
        (view.seq, view.listed_seqs())
    }

    /// Say what a listing changed: the list, and the backup it landed on.
    fn announce_moves(&self, (seq_before, listed_before): (Option<i64>, Vec<(i64, i64)>)) {
        let (seq_after, listed_after) = self.marks();
        if listed_after != listed_before {
            self.announce(Change::Listed);
        }
        if seq_after != seq_before {
            self.announce(Change::Seq);
        }
    }

    /// List every backup, or only those the folder's listing names. Switching
    /// it off lands as any other listing does: the backup being viewed may be
    /// one this folder leaves out.
    pub fn set_show_all(&self, on: bool) {
        if self.view.borrow().show_all == on {
            return;
        }
        let before = self.marks();
        {
            let mut view = self.view.borrow_mut();
            view.show_all = on;
            view.settle();
        }
        self.announce_moves(before);
    }

    /// What the desktop's Trash holds from the backed-up folders, as the
    /// daemon lists it (Stage 16). Its entries join the timeline, and the one
    /// being viewed may change or go.
    pub fn set_trash(&self, items: Vec<TrashedItem>) {
        if *self.view.borrow().trash == items {
            return;
        }
        let before = self.marks();
        {
            let mut view = self.view.borrow_mut();
            view.trash = Rc::new(items);
            view.settle();
        }
        self.announce(Change::Trash);
        self.announce_moves(before);
    }

    /// View a different backup, or the nearest newer one listed when this one
    /// is not (see [`View::landing`]). Announces nothing if that is the one
    /// already open.
    pub fn set_seq(&self, seq: i64) {
        let listed_before = self.view.borrow().listed_seqs();
        {
            let mut view = self.view.borrow_mut();
            let landing = view.landing(seq);
            if view.seq == Some(landing) {
                return;
            }
            view.seq = Some(landing);
            view.relist();
        }
        if self.view.borrow().listed_seqs() != listed_before {
            self.announce(Change::Listed);
        }
        self.announce(Change::Seq);
    }

    /// View a different folder, clearing the selection with it. A folder
    /// already seen since the catalogue was loaded brings its listing straight
    /// back; for any other, the last folder's stays on screen until the
    /// window has asked for this one's (see [`View::listing_is_current`]).
    pub fn set_folder(&self, folder: impl Into<String>) {
        let folder = folder.into();
        if self.view.borrow().folder == folder {
            return;
        }
        let before = self.marks();
        let known = self.listings.borrow().get(&folder).cloned();
        {
            let mut view = self.view.borrow_mut();
            view.folder = folder;
            view.selected = None;
            // In place before anyone is told, so nobody asks for it again.
            match known {
                Some(seqs) => View::adopt(&mut view, seqs),
                // The Trash's entries are this folder's at once; its backups
                // follow when the catalogue says.
                None => view.settle(),
            }
        }
        self.announce(Change::Folder);
        self.announce(Change::Selection);
        self.announce_moves(before);
    }

    /// Select an entry in the current folder, or clear the selection.
    pub fn set_selected(&self, selected: Option<Selected>) {
        if self.view.borrow().selected == selected {
            return;
        }
        self.view.borrow_mut().selected = selected;
        self.announce(Change::Selection);
    }

    /// Say that what is on this computer changed under the folder being
    /// viewed. Nothing in the state itself changes, which is why this is not
    /// `set_folder` again: setting what is already set announces nothing.
    pub fn disk_changed(&self) {
        self.announce(Change::Disk);
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
            caught_up: None,
            holds_intermediate: false,
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
            archive: None,
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
            archive: None,
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
    fn a_change_on_the_disk_is_announced_and_keeps_the_view() {
        // A restore leaves the folder and the selection as they were; only
        // what the disk holds is different, and the pane must be told.
        let state = AppState::new("home".to_string());
        state.set_archives(archives(&[2, 1]));
        state.set_selected(Some(Selected {
            path: "home/report.txt".to_string(),
            name: "report.txt".to_string(),
            is_dir: false,
            size: 10,
            mtime: 0,
            on_disk: 0,
            archive: None,
        }));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let changes = Rc::clone(&seen);
        state.subscribe(move |_, change| changes.borrow_mut().push(change));
        state.set_folder("home");
        assert!(
            seen.borrow().is_empty(),
            "the same folder again says nothing"
        );
        state.disk_changed();
        assert_eq!(*seen.borrow(), [Change::Disk]);
        assert_eq!(state.view().folder, "home");
        assert!(state.view().selected.is_some(), "the selection is kept");
    }

    /// Nine backups, seq 9 the newest, with "home/Docs" listing only 9 and 4:
    /// Stage 15's worked example.
    fn docs_listing_9_and_4() -> Rc<AppState> {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(
            archives(&[9, 8, 7, 6, 5, 4, 3, 2, 1]),
            "home/Docs",
            vec![9, 4],
        );
        state
    }

    fn changes_seen(state: &AppState) -> Rc<RefCell<Vec<Change>>> {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let changes = Rc::clone(&seen);
        state.subscribe(move |_, change| changes.borrow_mut().push(change));
        seen
    }

    #[test]
    fn stepping_goes_only_through_the_backups_listed() {
        let state = docs_listing_9_and_4();
        let view = state.view();
        assert_eq!(view.seq, Some(9));
        assert_eq!(view.older(), Some(4), "5 to 8 hold nothing 9 does not");
        assert_eq!(view.position(), Some((1, 2)));
        assert!(view.is_filtered(), "the label must say what it counts");
        state.set_seq(4);
        assert_eq!(state.view().older(), None, "1 to 3 are left out too");
        assert_eq!(state.view().newer(), Some(9));
    }

    #[test]
    fn a_backup_not_listed_lands_on_the_nearest_newer_one_that_is() {
        let state = docs_listing_9_and_4();
        state.set_seq(2);
        assert_eq!(state.view().seq, Some(4));
        state.set_seq(6);
        assert_eq!(state.view().seq, Some(9));
        state.set_seq(4);
        assert_eq!(state.view().seq, Some(4), "a listed backup stays put");
    }

    #[test]
    fn walking_into_a_folder_lands_on_what_it_lists_once_its_answer_arrives() {
        let state = AppState::new("home".to_string());
        state.set_catalogue(
            archives(&[9, 8, 7, 6, 5, 4, 3, 2, 1]),
            "home",
            vec![9, 7, 4],
        );
        state.set_seq(7);
        let seen = changes_seen(&state);

        state.set_folder("home/Docs");
        let view = state.view();
        assert!(
            !view.listing_is_current(),
            "home's list is still the one on screen"
        );
        assert_eq!(view.seq, Some(7), "nothing moves before the answer");
        assert_eq!(view.position(), Some((2, 3)));

        let generation = view.generation;
        state.set_listing("home/Docs", generation, vec![9, 4]);
        let view = state.view();
        assert_eq!(
            view.seq,
            Some(9),
            "7 holds nothing in home/Docs that 9 does not"
        );
        assert_eq!(view.position(), Some((1, 2)));
        assert_eq!(
            *seen.borrow(),
            [
                Change::Folder,
                Change::Selection,
                Change::Listed,
                Change::Seq
            ]
        );
    }

    #[test]
    fn an_answer_for_another_folder_or_an_older_catalogue_is_dropped() {
        let state = AppState::new("home".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home", vec![3, 2, 1]);
        let generation = state.view().generation;
        state.set_folder("home/Docs");

        state.set_listing("home", generation, vec![3]);
        assert!(
            !state.view().listing_is_current(),
            "home is not being viewed"
        );

        state.set_catalogue(archives(&[4, 3, 2, 1]), "home/Docs", vec![4, 2]);
        state.set_listing("home/Docs", generation, vec![3]);
        let view = state.view();
        assert_eq!(
            view.listed.len(),
            2,
            "the reload's answer stands: {:?}",
            view.listed
        );
        assert!(view.listing_is_current());
    }

    #[test]
    fn a_folder_seen_since_the_catalogue_was_read_needs_no_second_answer() {
        let state = AppState::new("home".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home", vec![3, 2, 1]);
        state.set_folder("home/Docs");
        state.set_listing("home/Docs", state.view().generation, vec![3]);

        state.set_folder("home");
        assert!(state.view().listing_is_current());
        assert_eq!(state.view().listed.len(), 3);
        state.set_folder("home/Docs");
        assert!(state.view().listing_is_current());
        assert_eq!(state.view().listed.len(), 1);

        // A reload renumbers, so nothing from before it is kept.
        state.set_catalogue(archives(&[4, 3, 2, 1]), "home/Docs", vec![4]);
        state.set_folder("home");
        assert!(!state.view().listing_is_current());
    }

    #[test]
    fn a_backup_still_being_catalogued_stays_listed() {
        let mut pending = archive(10);
        pending.catalogued = false;
        let mut all = vec![pending];
        all.extend(archives(&[9, 8, 7, 6, 5, 4, 3, 2, 1]));
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(all, "home/Docs", vec![9, 4]);
        let listed: Vec<i64> = state.view().listed.iter().map(|a| a.seq).collect();
        assert_eq!(listed, [10, 9, 4]);
        assert_eq!(state.view().seq, Some(9), "and is not landed on");
    }

    #[test]
    fn the_backup_being_viewed_is_listed_while_the_folder_is_awaiting_its_list() {
        let state = AppState::new("home".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home", vec![3, 1]);
        state.set_folder("home/Docs");
        state.set_seq(2);
        let view = state.view();
        assert_eq!(view.seq, Some(2), "not landed against home's list");
        assert!(
            view.listed.iter().any(|a| a.seq == 2),
            "so the sidebar can show it"
        );
    }

    #[test]
    fn a_new_backup_with_nothing_new_here_moves_the_view_onto_it() {
        let state = docs_listing_9_and_4();
        state.set_catalogue(
            archives(&[10, 9, 8, 7, 6, 5, 4, 3, 2, 1]),
            "home/Docs",
            vec![10, 4],
        );
        assert_eq!(
            state.view().seq,
            Some(10),
            "9 is now left out: 10 holds all of it"
        );
    }

    #[test]
    fn showing_all_backups_lists_and_steps_through_every_one() {
        let state = docs_listing_9_and_4();
        state.set_show_all(true);
        let view = state.view();
        assert_eq!(view.listed.len(), 9);
        assert!(!view.is_filtered(), "the label counts every backup again");
        assert_eq!(view.older(), Some(8));
        state.set_seq(6);
        assert_eq!(state.view().seq, Some(6), "nothing is landed elsewhere");
    }

    #[test]
    fn switching_all_backups_off_lands_on_what_the_folder_lists() {
        let state = docs_listing_9_and_4();
        state.set_show_all(true);
        state.set_seq(6);
        let seen = changes_seen(&state);
        state.set_show_all(false);
        let view = state.view();
        assert_eq!(view.seq, Some(9));
        assert_eq!(view.listed.len(), 2);
        assert_eq!(*seen.borrow(), [Change::Listed, Change::Seq]);
    }

    #[test]
    fn showing_all_backups_survives_walking_into_another_folder() {
        let state = docs_listing_9_and_4();
        state.set_show_all(true);
        state.set_folder("home/Docs/tax");
        state.set_listing("home/Docs/tax", state.view().generation, vec![9]);
        assert_eq!(state.view().listed.len(), 9);
    }

    /// Something moved to the Trash half an hour after backup `after`, from
    /// `path`.
    fn trashed_after(after: i64, path: &str, counts: bool) -> TrashedItem {
        TrashedItem {
            archive: format!("trash:/t/info/{}.trashinfo", crate::path::name(path)),
            path: path.to_string(),
            deleted_at: archive(after).ts + 1800,
            kind: "file".to_string(),
            size: 7,
            mtime: 0,
            counts,
        }
    }

    fn listed(state: &AppState) -> Vec<i64> {
        state.view().listed.iter().map(|a| a.seq).collect()
    }

    #[test]
    fn a_trash_entry_takes_its_place_among_the_backups_by_time() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        state.set_trash(vec![trashed_after(1, "home/Docs/C", true)]);
        let entry = trash::entry_seq(Some(1));
        assert_eq!(listed(&state), [3, entry, 1]);
        assert_eq!(state.view().older(), Some(entry), "stepping reaches it");
        state.set_seq(entry);
        let view = state.view();
        assert!(view.at_trash_entry());
        assert_eq!(
            view.catalogue_seq(),
            Some(1),
            "its folder is the backup before it"
        );
        assert!(
            view.is_filtered(),
            "2 of the 3 backups are listed; the entry is not one"
        );
    }

    #[test]
    fn something_in_the_trash_with_nothing_new_makes_no_entry() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        state.set_trash(vec![trashed_after(1, "home/Docs/A", false)]);
        assert_eq!(listed(&state), [3, 1]);
    }

    #[test]
    fn a_backup_left_out_lands_on_a_nearer_trash_entry() {
        // 2 is left out; the Trash entry after it is nearer than 3, and shows
        // 2's folder with the Trash put back.
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        state.set_trash(vec![trashed_after(2, "home/Docs/C", true)]);
        state.set_seq(2);
        assert_eq!(state.view().seq, Some(trash::entry_seq(Some(2))));
    }

    #[test]
    fn emptying_the_trash_lands_where_the_entrys_backup_would() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        state.set_trash(vec![trashed_after(2, "home/Docs/C", true)]);
        state.set_seq(trash::entry_seq(Some(2)));
        let seen = changes_seen(&state);
        state.set_trash(Vec::new());
        assert_eq!(state.view().seq, Some(3), "2 is left out, so on to 3");
        assert_eq!(listed(&state), [3, 1]);
        assert_eq!(*seen.borrow(), [Change::Trash, Change::Listed, Change::Seq]);
    }

    #[test]
    fn trash_entries_follow_the_folder() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 2, 1]);
        state.set_trash(vec![trashed_after(1, "home/Docs/C", true)]);
        assert_eq!(state.view().trash_entries.len(), 1);
        state.set_folder("home/Pictures");
        assert!(
            state.view().trash_entries.is_empty(),
            "nothing went to the Trash from here"
        );
        state.set_folder("home");
        assert_eq!(
            state.view().trash_entries.len(),
            1,
            "from below here, it did"
        );
    }

    #[test]
    fn a_trashed_file_is_read_from_the_trash_and_the_rest_from_the_backup_before() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        let c = trashed_after(1, "home/Docs/C", true);
        state.set_trash(vec![c.clone()]);
        state.set_seq(trash::entry_seq(Some(1)));
        let selected = |name: &str, archive: Option<String>| Selected {
            path: format!("home/Docs/{name}"),
            name: name.to_string(),
            is_dir: false,
            size: 1,
            mtime: 0,
            on_disk: 0,
            archive,
        };
        state.set_selected(Some(selected("C", Some(c.archive.clone()))));
        assert_eq!(state.view().reading_archive(), Some(c.archive));
        state.set_selected(Some(selected("A", None)));
        assert_eq!(
            state.view().reading_archive(),
            Some("snapshot-1".to_string())
        );
    }

    #[test]
    fn a_restore_at_a_trash_entry_takes_the_backup_before_it_and_the_trash() {
        let state = AppState::new("home/Docs".to_string());
        state.set_catalogue(archives(&[3, 2, 1]), "home/Docs", vec![3, 1]);
        let c = trashed_after(1, "home/Docs/C", true);
        state.set_trash(vec![c.clone()]);

        state.set_seq(3);
        let at_a_backup = state.view().restoring().expect("a backup restores");
        assert_eq!(at_a_backup.archive, "snapshot-3");
        assert!(at_a_backup.trashed.is_empty());

        state.set_seq(trash::entry_seq(Some(1)));
        let folder = state.view().restoring().expect("the entry restores");
        assert_eq!(folder.archive, "snapshot-1");
        assert_eq!(folder.trashed, std::slice::from_ref(&c.archive));
        assert!(folder.origin.comes_from_trash("home/Docs/C"));

        // Before the first backup, there is only the Trash to restore from.
        let early = TrashedItem {
            deleted_at: archive(1).ts - 1800,
            ..trashed_after(1, "home/Docs/B", true)
        };
        state.set_trash(vec![early.clone()]);
        state.set_seq(trash::entry_seq(None));
        let before = state.view().restoring().expect("the Trash restores");
        assert_eq!(before.archive, "");
        assert_eq!(before.trashed, [early.archive]);
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

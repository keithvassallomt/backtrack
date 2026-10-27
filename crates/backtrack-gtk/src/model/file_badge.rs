// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Keith Vassallo <keith@vassallo.cloud>

//! The badge in the file pane's Status column, and what hovering it says.
//!
//! The words are brief (S18-T9), as the sidebar's are: a word that fits the
//! column at a glance, and the tooltip for the rest. They are words first and
//! colour second, so the pane still reads in a screenshot printed in black and
//! white.

use super::above;
use super::group::IN_TRASH_TOOLTIP;

/// What the pane knows about a row, as far as its badge is concerned.
#[derive(Debug, Clone, Copy, Default)]
pub struct Facts {
    pub folder: bool,
    /// In the Trash, shown at a Trash entry (Stage 16).
    pub in_trash: bool,
    /// A folder outside the backups that leads down to backed-up ones.
    pub partly: bool,
    /// In this backup, and gone from the newest catalogued one.
    pub deleted_after: bool,
    /// The catalogue has it as current, and the daemon found it is not on the
    /// computer.
    pub absent: bool,
    /// A newer version follows this one; for a folder, something inside it.
    pub changed_since: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileBadge {
    pub text: &'static str,
    /// The style class, which gives it its colour; none for the dash.
    pub class: Option<&'static str>,
    pub tooltip: Option<&'static str>,
}

/// Every class [`of`] can give, for a recycled row to drop before it is bound
/// again.
pub const CLASSES: [&str; 4] = ["in-trash", "partly", "deleted", "changed"];

/// The badge for a row. One at most, the first of these that holds: in the
/// Trash, partly backed up, deleted after this backup, not on the computer,
/// changed since this backup.
pub fn of(facts: Facts) -> FileBadge {
    if facts.in_trash {
        FileBadge {
            text: "trashed",
            class: Some("in-trash"),
            tooltip: Some(IN_TRASH_TOOLTIP),
        }
    } else if facts.partly {
        FileBadge {
            text: above::BADGE,
            class: Some("partly"),
            tooltip: None,
        }
    } else if facts.deleted_after {
        FileBadge {
            text: "deleted",
            class: Some("deleted"),
            tooltip: Some("Deleted after this backup: the newest backup does not have it."),
        }
    } else if facts.absent {
        // The catalogue believes this file is current and the computer
        // disagrees, which is the one question the other badges cannot
        // answer and the one a person usually arrives with. Only ever shown
        // for a *known* absence: an unknown says nothing.
        FileBadge {
            text: "not on your disk",
            class: Some("deleted"),
            tooltip: None,
        }
    } else if facts.changed_since {
        FileBadge {
            text: "changed",
            class: Some("changed"),
            tooltip: Some(if facts.folder {
                "Changed since this backup: something inside it is different, or gone, in the newest backup."
            } else {
                "Changed since this backup: the newest backup has a newer version."
            }),
        }
    } else {
        // An em dash rather than nothing, so the column reads as answered.
        FileBadge {
            text: "—",
            class: None,
            tooltip: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_badges_are_brief_and_their_tooltips_say_the_rest() {
        let deleted = of(Facts {
            deleted_after: true,
            ..Facts::default()
        });
        assert_eq!((deleted.text, deleted.class), ("deleted", Some("deleted")));
        assert_eq!(
            deleted.tooltip,
            Some("Deleted after this backup: the newest backup does not have it.")
        );

        let changed = of(Facts {
            changed_since: true,
            ..Facts::default()
        });
        assert_eq!((changed.text, changed.class), ("changed", Some("changed")));
        assert_eq!(
            changed.tooltip,
            Some("Changed since this backup: the newest backup has a newer version.")
        );

        let trashed = of(Facts {
            in_trash: true,
            ..Facts::default()
        });
        assert_eq!((trashed.text, trashed.class), ("trashed", Some("in-trash")));
        assert_eq!(trashed.tooltip, Some(IN_TRASH_TOOLTIP));
    }

    #[test]
    fn a_changed_folder_says_it_is_something_inside_it() {
        let shown = of(Facts {
            folder: true,
            changed_since: true,
            ..Facts::default()
        });
        assert_eq!(shown.text, "changed");
        assert!(
            shown.tooltip.unwrap().contains("something inside it"),
            "{shown:?}"
        );
    }

    #[test]
    fn a_row_gets_the_first_badge_that_holds() {
        let all = Facts {
            folder: false,
            in_trash: true,
            partly: true,
            deleted_after: true,
            absent: true,
            changed_since: true,
        };
        assert_eq!(of(all).text, "trashed");
        let all = Facts {
            in_trash: false,
            ..all
        };
        assert_eq!(of(all).text, above::BADGE);
        let all = Facts {
            partly: false,
            ..all
        };
        assert_eq!(of(all).text, "deleted");
        let all = Facts {
            deleted_after: false,
            ..all
        };
        assert_eq!(of(all).text, "not on your disk");
        let all = Facts {
            absent: false,
            ..all
        };
        assert_eq!(of(all).text, "changed");
        let none = of(Facts::default());
        assert_eq!((none.text, none.class, none.tooltip), ("—", None, None));
    }

    #[test]
    fn a_recycled_row_can_drop_every_class_a_badge_gives() {
        let rows = [
            Facts {
                in_trash: true,
                ..Facts::default()
            },
            Facts {
                partly: true,
                ..Facts::default()
            },
            Facts {
                deleted_after: true,
                ..Facts::default()
            },
            Facts {
                absent: true,
                ..Facts::default()
            },
            Facts {
                changed_since: true,
                ..Facts::default()
            },
        ];
        for facts in rows {
            let class = of(facts).class.unwrap();
            assert!(CLASSES.contains(&class), "{class}");
        }
    }
}

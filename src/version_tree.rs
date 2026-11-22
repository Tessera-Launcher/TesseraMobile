//! The version list's tree view.
//!
//! Mirrors the desktop `tessera-core::version_tree`: in the "全部" pane with no
//! search, runs of one snapshot family collapse into a single row, because a
//! snapshot cycle is dozens of rows that differ only by a trailing number.
//! Releases and other version kinds keep their own rows.

use mdt::VersionType;

use crate::versions::{kind_label, Version};

/// How deep in the tree a row sits.
pub const CHILD_DEPTH: u8 = 1;

/// What one drawn line stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowKind {
    /// A single version: `index` into the filtered list.
    Version(usize),
    /// A run `first..=last` of snapshots of one family. `first` is the newest
    /// member, because the list is ordered newest first.
    Group {
        key: String,
        first: usize,
        last: usize,
        expanded: bool,
    },
}

/// One line of the version pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub kind: RowKind,
    pub depth: u8,
}

impl Row {
    /// The version this line picks when it is chosen.
    pub fn anchor(&self) -> usize {
        match self.kind {
            RowKind::Version(index) => index,
            RowKind::Group { first, .. } => first,
        }
    }
}

fn is_old(kind: VersionType) -> bool {
    matches!(kind, VersionType::OldBeta | VersionType::OldAlpha)
}

/// The family a snapshot belongs to, or `None` when it stands on its own.
pub fn family(version: &Version) -> Option<String> {
    let id = version.id.as_str();
    if is_old(version.kind) {
        return old_family(kind_label(version.kind), id);
    }
    if version.kind != VersionType::Snapshot {
        return None;
    }

    // A base plus a trailing counter: `1.21.9-pre9`, `1.21.9-rc1`,
    // `26.4-snapshot-7`, and the dashed spelling mirrors hand out.
    let stem = id.trim_end_matches(|c: char| c.is_ascii_digit());
    if stem.len() < id.len() {
        let stem = stem.strip_suffix('-').unwrap_or(stem);
        for suffix in ["-pre", "-rc", "-snapshot"] {
            if stem.len() > suffix.len() && stem.ends_with(suffix) {
                return Some(stem.to_string());
            }
        }
    }

    // Weekly snapshots: 25w37a. Folded per year.
    let bytes = id.as_bytes();
    if bytes.len() == 6
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2] == b'w'
        && bytes[3].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_alphabetic()
    {
        return Some(format!("weekly-{}", &id[..2]));
    }

    None
}

/// The family of a pre-release version, or `None`.
fn old_family(kind: &str, id: &str) -> Option<String> {
    let prefix: String = id
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_lowercase();
    if prefix.is_empty() {
        return None;
    }
    Some(format!("{kind}-{prefix}"))
}

/// The label of a folded row: `newest ~ oldest`.
pub fn title(all: &[Version], visible: &[usize], first: usize, last: usize) -> String {
    let at = |index: usize| visible.get(index).and_then(|&index| all.get(index));
    match (at(first), at(last)) {
        (Some(newest), Some(oldest)) => format!("{} ~ {}", newest.id, oldest.id),
        _ => String::new(),
    }
}

/// Lay the filtered list out as rows.
pub fn build(all: &[Version], visible: &[usize], expanded: &[String], tree: bool) -> Vec<Row> {
    let version = |index: usize| &all[visible[index]];
    let mut rows = Vec::with_capacity(visible.len().max(1));
    let mut index = 0;
    while index < visible.len() {
        let key = if tree { family(version(index)) } else { None };
        if let Some(key) = key {
            let mut last = index;
            while last + 1 < visible.len() && family(version(last + 1)).as_deref() == Some(&key) {
                last += 1;
            }
            if last > index {
                let expanded = expanded.iter().any(|open| open == &key);
                rows.push(Row {
                    kind: RowKind::Group {
                        key,
                        first: index,
                        last,
                        expanded,
                    },
                    depth: 0,
                });
                if expanded {
                    rows.extend((index..=last).map(|child| Row {
                        kind: RowKind::Version(child),
                        depth: CHILD_DEPTH,
                    }));
                }
                index = last + 1;
                continue;
            }
        }
        rows.push(Row {
            kind: RowKind::Version(index),
            depth: 0,
        });
        index += 1;
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(id: &str, kind: VersionType) -> Version {
        Version {
            id: id.to_string(),
            kind,
            date: String::new(),
        }
    }

    fn build_for_test(versions: &[Version], expanded: &[String]) -> Vec<Row> {
        let visible: Vec<usize> = (0..versions.len()).collect();
        build(versions, &visible, expanded, true)
    }

    fn labels(versions: &[Version], rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match &row.kind {
                RowKind::Version(index) => versions[*index].id.clone(),
                RowKind::Group { first, last, .. } => {
                    let visible: Vec<usize> = (0..versions.len()).collect();
                    title(versions, &visible, *first, *last)
                }
            })
            .collect()
    }

    #[test]
    fn families_are_recognised_for_the_shapes_mojang_ships() {
        for (id, expected) in [
            ("1.21.9-pre9", Some("1.21.9-pre")),
            ("26.4-snapshot-7", Some("26.4-snapshot")),
            ("26.3-pre-3", Some("26.3-pre")),
            ("25w37a", Some("weekly-25")),
            ("1.21.8", None),
            ("snapshot-7", None),
        ] {
            assert_eq!(
                family(&version(id, VersionType::Snapshot)).as_deref(),
                expected,
                "{id}"
            );
        }
    }

    #[test]
    fn a_snapshot_run_folds_newest_first() {
        let versions = vec![
            version("1.21.8", VersionType::Release),
            version("1.21.9-pre3", VersionType::Snapshot),
            version("1.21.9-pre2", VersionType::Snapshot),
            version("1.21.9-pre1", VersionType::Snapshot),
            version("1.21.7", VersionType::Release),
        ];
        let rows = build_for_test(&versions, &[]);
        assert_eq!(
            labels(&versions, &rows),
            ["1.21.8", "1.21.9-pre3 ~ 1.21.9-pre1", "1.21.7"]
        );
    }

    #[test]
    fn expanding_a_group_adds_indented_members() {
        let versions = vec![
            version("1.21.9-pre2", VersionType::Snapshot),
            version("1.21.9-pre1", VersionType::Snapshot),
        ];
        let rows = build_for_test(&versions, &["1.21.9-pre".to_string()]);
        assert_eq!(rows.len(), 3);
        assert!(matches!(
            rows[0].kind,
            RowKind::Group { expanded: true, .. }
        ));
        assert_eq!(rows[1].depth, CHILD_DEPTH);
    }

    #[test]
    fn old_versions_fold_by_era() {
        let versions = vec![
            version("b1.7.3", VersionType::OldBeta),
            version("b1.7.2", VersionType::OldBeta),
            version("a1.2.6", VersionType::OldAlpha),
            version("a1.0.16", VersionType::OldAlpha),
        ];
        let rows = build_for_test(&versions, &[]);
        assert_eq!(
            labels(&versions, &rows),
            ["b1.7.3 ~ b1.7.2", "a1.2.6 ~ a1.0.16"]
        );
    }
}

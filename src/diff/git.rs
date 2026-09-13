//! git2 queries: HEAD→workdir diff with per-file stats and hunks.

use std::cell::RefCell;
use std::path::Path;

use super::{DiffFile, DiffHunk, DiffLine, GitDiff, MAX_LINES_PER_FILE};
use git2::{DiffDelta, Repository};

/// Diff the project against HEAD (staged + unstaged + untracked).
pub fn head_diff(path: &Path, limits: &std::collections::HashMap<String, usize>) -> anyhow::Result<GitDiff> {
    let repo = Repository::discover(path)?;
    let branch = repo
        .head()
        .ok()
        .and_then(|head| head.shorthand().map(Into::into));
    let tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());

    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        // Emit untracked file contents as added lines.
        .show_untracked_content(true);

    let diff = repo.diff_tree_to_workdir_with_index(tree.as_ref(), Some(&mut opts))?;

    let files = RefCell::new(Vec::<DiffFile>::new());
    diff.foreach(
        &mut |delta: DiffDelta, _| {
            files.borrow_mut().push(DiffFile {
                path: delta
                    .new_file()
                    .path()
                    .or_else(|| delta.old_file().path())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                ..Default::default()
            });
            true
        },
        None,
        Some(&mut |_, hunk: git2::DiffHunk| {
            if let Some(file) = files.borrow_mut().last_mut() {
                let limit = limits.get(&file.path).copied().unwrap_or(MAX_LINES_PER_FILE);
                if file.lines_total >= limit {
                    return true;
                }
                file.hunks.push(DiffHunk {
                    header: String::from_utf8_lossy(hunk.header()).trim_end().to_owned(),
                    lines: vec![],
                });
            }
            true
        }),
        Some(&mut |_, _hunk, line: git2::DiffLine| {
            let mut guard = files.borrow_mut();
            let Some(file) = guard.last_mut() else {
                return true;
            };
            let origin = line.origin();
            if !matches!(origin, '+' | '-' | ' ') {
                return true;
            }
            match origin {
                '+' => file.added += 1,
                '-' => file.removed += 1,
                _ => {}
            }
            let limit = limits.get(&file.path).copied().unwrap_or(MAX_LINES_PER_FILE);
            if file.lines_total < limit {
                if let Some(hunk) = file.hunks.last_mut() {
                    hunk.lines.push(DiffLine {
                        kind: origin,
                        old_no: line.old_lineno(),
                        new_no: line.new_lineno(),
                        text: String::from_utf8_lossy(line.content())
                            .trim_end_matches(['\n', '\r'])
                            .to_owned(),
                    });
                }
                file.lines_total += 1;
            } else {
                file.truncated = true;
            }
            true
        }),
    )?;

    Ok(GitDiff {
        branch,
        files: files.into_inner(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{RepositoryInitOptions, Signature};

    /// M3 data layer, hermetic: temp repo → commit → modify + untracked
    /// → head_diff must see both, with correct hunk lines and numbers.
    #[test]
    fn head_diff_sees_edits_and_untracked() {
        let dir = std::env::temp_dir().join(format!("ddu-git-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let repo = Repository::init_opts(&dir, RepositoryInitOptions::new().bare(false)).unwrap();
        let mut config = repo.config().unwrap();
        config.set_str("user.name", "ddu").unwrap();
        config.set_str("user.email", "ddu@test").unwrap();

        let file = dir.join("hello.txt");
        std::fs::write(&file, "line one\nline two\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("hello.txt")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = Signature::now("ddu", "ddu@test").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .unwrap();

        std::fs::write(&file, "line one\nline two changed\n").unwrap();
        std::fs::write(dir.join("new.txt"), "fresh\n").unwrap();

        let diff = head_diff(&dir, &Default::default()).expect("head_diff");
        assert_eq!(diff.branch.as_deref(), repo.head().unwrap().shorthand());

        let hello = diff
            .files
            .iter()
            .find(|f| f.path == "hello.txt")
            .expect("hello.txt");
        assert_eq!((hello.added, hello.removed), (1, 1));
        let changed = hello
            .hunks
            .iter()
            .flat_map(|h| &h.lines)
            .find(|l| l.text.contains("line two changed"))
            .expect("+ line for change");
        assert_eq!(changed.kind, '+');
        assert_eq!(changed.new_no, Some(2));

        let new_file = diff
            .files
            .iter()
            .find(|f| f.path == "new.txt")
            .expect("new.txt");
        assert_eq!(new_file.added, 1);
        assert!(
            new_file
                .hunks
                .iter()
                .any(|h| h.lines.iter().any(|l| l.text == "fresh"))
        );

        let nested = dir.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(head_diff(&nested, &Default::default()).unwrap(), diff);
        std::fs::write(
            dir.join("large.txt"),
            "line\n".repeat(MAX_LINES_PER_FILE + 10),
        )
        .unwrap();
        let large = head_diff(&dir, &Default::default())
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.path == "large.txt")
            .unwrap();
        assert!(large.truncated);
        assert_eq!(large.lines_total, MAX_LINES_PER_FILE);
        assert_eq!(large.added, MAX_LINES_PER_FILE + 10);
        assert_eq!(
            large.hunks.iter().map(|h| h.lines.len()).sum::<usize>(),
            MAX_LINES_PER_FILE
        );
        // A raised per-file budget lifts the cap: the same diff parses in
        // full once the pane's scroll-driven expansion kicks in.
        let mut limits = std::collections::HashMap::new();
        limits.insert("large.txt".to_owned(), MAX_LINES_PER_FILE * 4);
        let large = head_diff(&dir, &limits)
            .unwrap()
            .files
            .into_iter()
            .find(|f| f.path == "large.txt")
            .unwrap();
        assert!(!large.truncated);
        assert_eq!(large.lines_total, MAX_LINES_PER_FILE + 10);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

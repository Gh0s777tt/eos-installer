//! Applications a person may decline while installing, and the removal that honours the answer.
//!
//! WHY A REMOVAL AND NOT A SELECTION.  The fast install path clones the live filesystem wholesale
//! (`try_fast_install`), which is what turned a seven-hour install into minutes.  Nothing in that
//! path chooses packages, so "do not install E-OS Sheets" cannot be expressed as an omission --
//! it has to be expressed as a deletion afterwards.  That is also why this module needs an exact
//! file list rather than a package name: guessing would leave the launcher entry behind and
//! orphan the icon.
//!
//! WHERE THE LIST COMES FROM.  `/usr/share/eos/optional-apps.toml`, shipped in the image by the
//! E-OS meta-repository and checked there by integrity check 24 against the recipes that produce
//! the files.  This module does not decide what is optional; it reads that decision.
//!
//! A MISSING FILE IS SUCCESS, NOT FAILURE.  Removing something that is not there reaches exactly
//! the state the person asked for.  An installer that stopped on it would refuse to honour a
//! choice because of a file nobody can see -- and that case is real: until 2026-09-03 not one
//! application icon reached the image at all.

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_derive::Deserialize;

/// Where the image keeps the list. Absolute, and read from the LIVE system the installer runs on.
pub const MANIFEST_PATH: &str = "/usr/share/eos/optional-apps.toml";

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct OptionalApp {
    /// Human-readable name, shown in the prompt.
    pub title: String,
    /// One line explaining what it is, so the choice is informed.
    pub summary: String,
    /// Every path this application owns, absolute, as it appears in the installed system.
    pub files: Vec<String>,
    /// The package name. Filled in from the table key by [`parse`]; not present in the file.
    #[serde(skip)]
    pub name: String,
}

/// What a removal actually did. Kept apart from `Result` because "the file was already gone" is
/// not an error and must not be reported as one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Removal {
    /// Files that existed and were deleted.
    pub removed: Vec<String>,
    /// Files the manifest named that were not there -- already in the desired state.
    pub absent: Vec<String>,
    /// Files that existed and could not be deleted, with the reason.
    pub failed: Vec<(String, String)>,
}

impl Removal {
    pub fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Parse the manifest. The table key becomes [`OptionalApp::name`], so the file stays readable
/// (`[eos-notes]`) without repeating the name inside the table.
pub fn parse(text: &str) -> Result<Vec<OptionalApp>, toml::de::Error> {
    let raw: BTreeMap<String, OptionalApp> = toml::from_str(text)?;
    Ok(raw
        .into_iter()
        .map(|(name, mut app)| {
            app.name = name;
            app
        })
        .collect())
}

/// Read the manifest from the live system. A missing manifest is not an error: an image that
/// ships no optional applications simply offers no choice, and the installer must still work.
pub fn read(path: &Path) -> Option<Vec<OptionalApp>> {
    let text = fs::read_to_string(path).ok()?;
    match parse(&text) {
        Ok(apps) if !apps.is_empty() => Some(apps),
        Ok(_) => None,
        Err(err) => {
            eprintln!(
                "optional apps: {} is not readable ({err}); offering no choice",
                path.display()
            );
            None
        }
    }
}

/// Ask which applications to leave out. Returns the indices of the DECLINED ones.
///
/// The default is to keep everything: a person who presses Enter, or who is driving this over a
/// serial line that ate their input, ends up with the complete system rather than a mutilated one.
pub fn prompt<R: BufRead, W: Write>(
    apps: &[OptionalApp],
    input: &mut R,
    out: &mut W,
) -> io::Result<Vec<usize>> {
    writeln!(
        out,
        "\nOptional applications. All are installed unless you say otherwise."
    )?;
    for (i, app) in apps.iter().enumerate() {
        writeln!(out, "  {}. {} - {}", i + 1, app.title, app.summary)?;
    }
    write!(
        out,
        "Numbers to LEAVE OUT, separated by spaces (empty = keep all): "
    )?;
    out.flush()?;

    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        // End of input: keep everything. Silence must not mean "delete things".
        writeln!(out)?;
        return Ok(Vec::new());
    }

    let mut declined = Vec::new();
    for token in line.split_whitespace() {
        match token.parse::<usize>() {
            Ok(n) if n >= 1 && n <= apps.len() => {
                let idx = n - 1;
                if !declined.contains(&idx) {
                    declined.push(idx);
                }
            }
            _ => {
                writeln!(out, "  ignoring {token:?}: not one of 1..{}", apps.len())?;
            }
        }
    }
    declined.sort_unstable();
    Ok(declined)
}

/// Delete the files of the declined applications from a mounted target filesystem.
///
/// `root` is where the target is mounted; the manifest's paths are absolute in the INSTALLED
/// system, so each is joined onto `root` after stripping its leading separator. A path that
/// escapes `root` is refused rather than followed -- the manifest is trusted input today, and
/// this stays true if that ever stops being the case.
pub fn remove(root: &Path, declined: &[&OptionalApp]) -> Removal {
    let mut out = Removal::default();
    for app in declined {
        for file in &app.files {
            let rel = file.trim_start_matches('/');
            if rel.split('/').any(|c| c == "..") {
                out.failed
                    .push((file.clone(), "path escapes the target root".into()));
                continue;
            }
            let target = root.join(rel);
            match fs::symlink_metadata(&target) {
                Err(_) => out.absent.push(file.clone()),
                Ok(_) => match fs::remove_file(&target) {
                    Ok(()) => out.removed.push(file.clone()),
                    Err(err) => out.failed.push((file.clone(), err.to_string())),
                },
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const SAMPLE: &str = r#"
[eos-notes]
title = "E-OS Notes"
summary = "Notes."
files = ["/usr/bin/eos-notes", "/usr/share/ui/apps/30_eos-notes"]

[eos-store]
title = "E-OS Store"
summary = "Applications."
files = ["/usr/bin/eos-store"]
"#;

    fn apps() -> Vec<OptionalApp> {
        parse(SAMPLE).expect("sample parses")
    }

    #[test]
    fn parse_fills_the_name_from_the_table_key() {
        let a = apps();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].name, "eos-notes");
        assert_eq!(a[0].title, "E-OS Notes");
        assert_eq!(a[1].name, "eos-store");
    }

    #[test]
    fn parse_rejects_a_table_without_files() {
        assert!(parse("[x]\ntitle = \"X\"\nsummary = \"s\"\n").is_err());
    }

    #[test]
    fn empty_answer_keeps_everything() {
        let a = apps();
        let mut out = Vec::new();
        let declined = prompt(&a, &mut Cursor::new(b"\n".to_vec()), &mut out).unwrap();
        assert!(declined.is_empty(), "an empty line must not delete anything");
    }

    #[test]
    fn end_of_input_keeps_everything() {
        // A serial line that closed, or a harness that answered nothing, must not mutilate the
        // system it is installing.
        let a = apps();
        let mut out = Vec::new();
        let declined = prompt(&a, &mut Cursor::new(Vec::new()), &mut out).unwrap();
        assert!(declined.is_empty());
    }

    #[test]
    fn numbers_select_and_are_deduplicated_and_sorted() {
        let a = apps();
        let mut out = Vec::new();
        let declined = prompt(&a, &mut Cursor::new(b"2 1 2\n".to_vec()), &mut out).unwrap();
        assert_eq!(declined, vec![0, 1]);
    }

    #[test]
    fn out_of_range_and_nonsense_are_ignored_not_fatal() {
        let a = apps();
        let mut out = Vec::new();
        let declined = prompt(&a, &mut Cursor::new(b"9 banana 1\n".to_vec()), &mut out).unwrap();
        assert_eq!(declined, vec![0]);
        let shown = String::from_utf8(out).unwrap();
        assert!(
            shown.contains("ignoring"),
            "the person is told what was ignored"
        );
    }

    #[test]
    fn remove_deletes_what_is_there_and_tolerates_what_is_not() {
        let dir = std::env::temp_dir().join(format!("eos-opt-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("usr/bin")).unwrap();
        fs::write(dir.join("usr/bin/eos-notes"), b"x").unwrap();
        // 30_eos-notes is deliberately NOT created: the "already gone" case.
        let a = apps();
        let r = remove(&dir, &[&a[0]]);
        assert_eq!(r.removed, vec!["/usr/bin/eos-notes".to_string()]);
        assert_eq!(r.absent, vec!["/usr/share/ui/apps/30_eos-notes".to_string()]);
        assert!(r.is_clean(), "a missing file is not a failure");
        assert!(!dir.join("usr/bin/eos-notes").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_refuses_a_path_that_escapes_the_root() {
        let dir = std::env::temp_dir().join(format!("eos-opt-esc-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let evil = OptionalApp {
            name: "evil".into(),
            title: "Evil".into(),
            summary: "s".into(),
            files: vec!["/../../etc/passwd".into()],
        };
        let r = remove(&dir, &[&evil]);
        assert!(r.removed.is_empty());
        assert_eq!(r.failed.len(), 1);
        assert!(r.failed[0].1.contains("escapes"));
        let _ = fs::remove_dir_all(&dir);
    }
}

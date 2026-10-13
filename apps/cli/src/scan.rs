//! Finds transcript files and reads only the bytes appended since last sync.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::state::{read_json, write_json_atomic};

#[derive(Clone, Debug, PartialEq)]
pub struct TranscriptFile {
    pub path: PathBuf,
    pub subagent: bool,
    /// Session id implied by the path, used when a line lacks `sessionId`.
    pub session_hint: String,
}

fn name_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn is_jsonl(p: &Path) -> bool {
    p.extension().is_some_and(|e| e == "jsonl")
}

fn fresh(p: &Path, since: Option<SystemTime>) -> bool {
    match since {
        None => true,
        Some(t) => fs::metadata(p).and_then(|m| m.modified()).map(|m| m >= t).unwrap_or(true),
    }
}

fn collect_subagents(dir: &Path, session: &str, since: Option<SystemTime>, out: &mut Vec<TranscriptFile>) {
    let Ok(items) = fs::read_dir(dir) else { return };
    for item in items.flatten() {
        let p = item.path();
        if p.is_dir() {
            collect_subagents(&p, session, since, out);
        } else if is_jsonl(&p) && fresh(&p, since) {
            out.push(TranscriptFile { path: p, subagent: true, session_hint: session.to_string() });
        }
    }
}

/// Lists `projects/*/*.jsonl` and `projects/*/<session>/subagents/**/*.jsonl`
/// modified at or after `modified_since`.
pub fn discover(projects_dir: &Path, modified_since: Option<SystemTime>) -> Vec<TranscriptFile> {
    let mut out = Vec::new();
    let Ok(projects) = fs::read_dir(projects_dir) else { return out };
    for proj in projects.flatten() {
        let Ok(items) = fs::read_dir(proj.path()) else { continue };
        for item in items.flatten() {
            let p = item.path();
            if p.is_dir() {
                let session = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                collect_subagents(&p.join("subagents"), &session, modified_since, &mut out);
            } else if is_jsonl(&p) && fresh(&p, modified_since) {
                out.push(TranscriptFile { session_hint: name_of(&p), path: p, subagent: false });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The given main transcript plus its subagent transcripts.
pub fn session_files(transcript: &Path) -> Vec<TranscriptFile> {
    let session = name_of(transcript);
    let mut out = vec![TranscriptFile { path: transcript.to_path_buf(), subagent: false, session_hint: session.clone() }];
    collect_subagents(&transcript.with_extension("").join("subagents"), &session, None, &mut out);
    out
}

pub fn find_transcript(projects_dir: &Path, session_id: &str) -> Option<PathBuf> {
    fs::read_dir(projects_dir).ok()?.flatten().map(|p| p.path().join(format!("{session_id}.jsonl"))).find(|p| p.is_file())
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Offsets {
    files: BTreeMap<String, u64>,
}

impl Offsets {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_atomic(path, self, None)
    }
    pub fn get(&self, file: &Path) -> u64 {
        self.files.get(&file.to_string_lossy().into_owned()).copied().unwrap_or(0)
    }
    pub fn set(&mut self, file: &Path, offset: u64) {
        self.files.insert(file.to_string_lossy().into_owned(), offset);
    }
}

#[derive(Debug, PartialEq)]
pub struct NewLines {
    pub lines: Vec<String>,
    /// Byte offset just past the last complete line read.
    pub end: u64,
}

/// Reads complete lines after `offset`. A file shorter than `offset` was
/// rewritten, so reading restarts at 0 (the server dedupes re-sent entries).
pub fn read_new_lines(path: &Path, offset: u64) -> io::Result<NewLines> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = if offset > len { 0 } else { offset };
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') else {
        return Ok(NewLines { lines: vec![], end: start });
    };
    let text = String::from_utf8_lossy(&buf[..=last_nl]);
    Ok(NewLines {
        lines: text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect(),
        end: start + last_nl as u64 + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_main_and_subagent_files() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/-home-dev-proj");
        fs::create_dir_all(proj.join("sess-1/subagents/nested")).unwrap();
        fs::write(proj.join("sess-1.jsonl"), "").unwrap();
        fs::write(proj.join("sess-1/subagents/agent-a.jsonl"), "").unwrap();
        fs::write(proj.join("sess-1/subagents/nested/agent-b.jsonl"), "").unwrap();
        fs::write(proj.join("notes.txt"), "").unwrap();

        let files = discover(&dir.path().join("projects"), None);
        assert_eq!(files.len(), 3);
        let main: Vec<_> = files.iter().filter(|f| !f.subagent).collect();
        assert_eq!(main.len(), 1);
        assert_eq!(main[0].session_hint, "sess-1");
        assert!(files.iter().filter(|f| f.subagent).all(|f| f.session_hint == "sess-1"));

        let session = session_files(&proj.join("sess-1.jsonl"));
        assert_eq!(session.len(), 3);
        assert_eq!(find_transcript(&dir.path().join("projects"), "sess-1"), Some(proj.join("sess-1.jsonl")));
        assert_eq!(find_transcript(&dir.path().join("projects"), "nope"), None);
        assert!(discover(&dir.path().join("missing"), None).is_empty());
    }

    #[test]
    fn modified_since_filters_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("projects/p");
        fs::create_dir_all(&proj).unwrap();
        fs::write(proj.join("s.jsonl"), "").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(3600);
        assert!(discover(&dir.path().join("projects"), Some(future)).is_empty());
    }

    #[test]
    fn reads_only_complete_lines_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "one\ntwo\npart").unwrap();
        let r = read_new_lines(&f, 0).unwrap();
        assert_eq!(r.lines, vec!["one", "two"]);
        assert_eq!(r.end, 8);
        let r2 = read_new_lines(&f, r.end).unwrap();
        assert!(r2.lines.is_empty());
        assert_eq!(r2.end, 8, "partial line is not consumed");
        fs::write(&f, "one\ntwo\npartial\n").unwrap();
        let r3 = read_new_lines(&f, 8).unwrap();
        assert_eq!(r3.lines, vec!["partial"]);
    }

    #[test]
    fn shrunk_file_restarts_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "a\n").unwrap();
        let r = read_new_lines(&f, 999).unwrap();
        assert_eq!(r.lines, vec!["a"]);
        assert_eq!(r.end, 2);
    }

    #[test]
    fn offsets_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("state/offsets.json");
        let mut o = Offsets::load(&store);
        assert_eq!(o.get(Path::new("/x")), 0);
        o.set(Path::new("/x"), 42);
        o.save(&store).unwrap();
        assert_eq!(Offsets::load(&store).get(Path::new("/x")), 42);
    }
}

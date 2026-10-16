//! Finds transcript files and reads only the bytes appended since last sync.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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

/// How far into a transcript has been uploaded, and which file that was.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(from = "StoredPosition")]
pub struct Position {
    pub offset: u64,
    /// Device and inode of the file when `offset` was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ino: Option<u64>,
    /// Hash of the bytes just before `offset` (see [`fingerprint_before`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// On-disk forms of [`Position`]: older versions stored a plain offset.
#[derive(Deserialize)]
#[serde(untagged)]
enum StoredPosition {
    Legacy(u64),
    Full {
        offset: u64,
        #[serde(default)]
        dev: Option<u64>,
        #[serde(default)]
        ino: Option<u64>,
        #[serde(default)]
        fingerprint: Option<String>,
    },
}

impl From<StoredPosition> for Position {
    fn from(s: StoredPosition) -> Self {
        match s {
            StoredPosition::Legacy(offset) => Position { offset, ..Default::default() },
            StoredPosition::Full { offset, dev, ino, fingerprint } => Position { offset, dev, ino, fingerprint },
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Offsets {
    files: BTreeMap<String, Position>,
}

impl Offsets {
    pub fn load(path: &Path) -> Self {
        read_json(path).unwrap_or_default()
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        write_json_atomic(path, self, None)
    }
    pub fn get(&self, file: &Path) -> Position {
        self.files.get(&file.to_string_lossy().into_owned()).cloned().unwrap_or_default()
    }
    pub fn set(&mut self, file: &Path, pos: Position) {
        self.files.insert(file.to_string_lossy().into_owned(), pos);
    }
}

#[derive(Debug, PartialEq)]
pub struct NewLines {
    pub lines: Vec<String>,
    /// Where reading started: the stored offset, or 0 after a rewrite.
    pub start: u64,
    /// Position just past the last complete line read.
    pub end: Position,
}

/// How many bytes before an offset are hashed into its fingerprint.
pub const FINGERPRINT_BYTES: u64 = 256;

/// sha256 (first 16 hex chars) of up to [`FINGERPRINT_BYTES`] bytes
/// immediately before `offset`.
pub fn fingerprint_before(f: &mut File, offset: u64) -> io::Result<String> {
    let from = offset.saturating_sub(FINGERPRINT_BYTES);
    f.seek(SeekFrom::Start(from))?;
    let mut buf = vec![0u8; (offset - from) as usize];
    f.read_exact(&mut buf)?;
    Ok(Sha256::digest(&buf)[..8].iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn identity(meta: &fs::Metadata) -> (Option<u64>, Option<u64>) {
    use std::os::unix::fs::MetadataExt;
    (Some(meta.dev()), Some(meta.ino()))
}

#[cfg(not(unix))]
fn identity(_: &fs::Metadata) -> (Option<u64>, Option<u64>) {
    (None, None)
}

/// Reads complete lines after `stored`. Reading restarts at 0 (the server
/// dedupes re-sent entries) when the file was rewritten: it is shorter than
/// the offset, is a different file (device/inode), or the bytes before the
/// offset no longer match the stored fingerprint.
pub fn read_new_lines(path: &Path, stored: &Position) -> io::Result<NewLines> {
    let mut f = File::open(path)?;
    let meta = f.metadata()?;
    let len = meta.len();
    let (dev, ino) = identity(&meta);
    let other_file = (stored.dev.is_some() && stored.dev != dev) || (stored.ino.is_some() && stored.ino != ino);
    let mut start = stored.offset;
    if start > len || other_file {
        start = 0;
    } else if start > 0
        && let Some(fp) = &stored.fingerprint
        && fingerprint_before(&mut f, start)? != *fp
    {
        start = 0;
    }
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let (lines, end) = match buf.iter().rposition(|&b| b == b'\n') {
        None => (vec![], start),
        Some(last_nl) => {
            let text = String::from_utf8_lossy(&buf[..=last_nl]);
            (text.lines().filter(|l| !l.trim().is_empty()).map(String::from).collect(), start + last_nl as u64 + 1)
        }
    };
    let fingerprint = Some(fingerprint_before(&mut f, end)?);
    Ok(NewLines { lines, start, end: Position { offset: end, dev, ino, fingerprint } })
}

/// Calls `f` with each line before `stored`'s offset (the bytes earlier
/// syncs read), and returns true; returns false without reading when the
/// file was rewritten since (see [`read_new_lines`]).
pub fn for_each_line_before(path: &Path, stored: &Position, mut f: impl FnMut(&str)) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let meta = file.metadata()?;
    let (dev, ino) = identity(&meta);
    let other_file = (stored.dev.is_some() && stored.dev != dev) || (stored.ino.is_some() && stored.ino != ino);
    if stored.offset > meta.len() || other_file {
        return Ok(false);
    }
    if let Some(fp) = &stored.fingerprint
        && fingerprint_before(&mut file, stored.offset)? != *fp
    {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut r = BufReader::new(file.take(stored.offset));
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if r.read_until(b'\n', &mut buf)? == 0 {
            return Ok(true);
        }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim_end();
        if !line.is_empty() {
            f(line);
        }
    }
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

    fn at(offset: u64) -> Position {
        Position { offset, ..Default::default() }
    }

    #[test]
    fn reads_only_complete_lines_and_resumes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "one\ntwo\npart").unwrap();
        let r = read_new_lines(&f, &Position::default()).unwrap();
        assert_eq!(r.lines, vec!["one", "two"]);
        assert_eq!(r.end.offset, 8);
        let r2 = read_new_lines(&f, &r.end).unwrap();
        assert!(r2.lines.is_empty());
        assert_eq!(r2.end, r.end, "partial line is not consumed");
        fs::write(&f, "one\ntwo\npartial\n").unwrap();
        let r3 = read_new_lines(&f, &r.end).unwrap();
        assert_eq!(r3.lines, vec!["partial"]);
        assert_eq!(r3.start, 8, "append-only growth resumes at the offset");
    }

    #[test]
    fn shrunk_file_restarts_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "a\n").unwrap();
        let r = read_new_lines(&f, &at(999)).unwrap();
        assert_eq!(r.lines, vec!["a"]);
        assert_eq!(r.end.offset, 2);
    }

    #[test]
    fn same_size_rewrite_is_reread() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "aaa\nbbb\n").unwrap();
        let first = read_new_lines(&f, &Position::default()).unwrap();
        fs::write(&f, "ccc\nddd\n").unwrap(); // same length, same inode
        let r = read_new_lines(&f, &first.end).unwrap();
        assert_eq!((r.start, r.lines), (0, vec!["ccc".to_string(), "ddd".to_string()]));
    }

    #[test]
    fn larger_rewrite_is_reread() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "aaa\nbbb\n").unwrap();
        let first = read_new_lines(&f, &Position::default()).unwrap();
        fs::write(&f, "xxx\nyyy\nzzz\n").unwrap();
        let r = read_new_lines(&f, &first.end).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.lines, vec!["xxx", "yyy", "zzz"]);
    }

    #[test]
    fn replaced_file_with_new_inode_is_reread() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("t.jsonl");
        let tail = "t".repeat(300) + "\n";
        fs::write(&f, format!("a\n{tail}")).unwrap();
        let first = read_new_lines(&f, &Position::default()).unwrap();
        // Replaced by rename (new inode) with identical content: re-read from 0.
        let tmp = dir.path().join("t.tmp");
        fs::write(&tmp, format!("a\n{tail}")).unwrap();
        fs::rename(&tmp, &f).unwrap();
        let r = read_new_lines(&f, &first.end).unwrap();
        assert_eq!(r.start, 0);
        assert_eq!(r.lines.len(), 2);
    }

    #[test]
    fn legacy_plain_offsets_still_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("offsets.json");
        fs::write(&store, r#"{"files":{"/x":42,"/y":{"offset":7,"dev":1,"ino":2,"fingerprint":"abcd"}}}"#).unwrap();
        let o = Offsets::load(&store);
        assert_eq!(o.get(Path::new("/x")), at(42));
        assert_eq!(o.get(Path::new("/y")), Position { offset: 7, dev: Some(1), ino: Some(2), fingerprint: Some("abcd".into()) });

        // A legacy offset is trusted (no fingerprint yet) and upgraded on the next read.
        let f = dir.path().join("t.jsonl");
        fs::write(&f, "aaa\nbbb\n").unwrap();
        let r = read_new_lines(&f, &at(4)).unwrap();
        assert_eq!((r.start, r.lines), (4, vec!["bbb".to_string()]));
        assert!(r.end.fingerprint.is_some() && r.end.ino.is_some());
    }

    #[test]
    fn offsets_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join("state/offsets.json");
        let mut o = Offsets::load(&store);
        assert_eq!(o.get(Path::new("/x")), Position::default());
        let pos = Position { offset: 42, dev: Some(3), ino: Some(9), fingerprint: Some("00ff".into()) };
        o.set(Path::new("/x"), pos.clone());
        o.save(&store).unwrap();
        assert_eq!(Offsets::load(&store).get(Path::new("/x")), pos);
    }
}

//! Command journal (Journal): JSONL frames + replay.
//!
//! Frame format (one JSON per line):
//! ```text
//! {"TxBegin":{"id":1}}
//! {"Cmd":{"MoveNode":{...}}}
//! {"TxCommit":{"id":1}}
//! ```
//!
//! replay applies only up to the last `TxCommit`; trailing uncommitted transactions are dropped, so "killing the process" never
//! produces a half transaction (G3 acceptance item).

use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One journal frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum JournalFrame {
    /// History rebuild: discard all frames before this one and use `commands` as the new applied history.
    ///
    /// undo/redo do not emit inverse commands; instead the whole current undo stack is written as a Rebase. On recovery only
    /// the commands in this frame are replayed, so "undo then crash" cannot resurrect undone edits.
    Rebase {
        /// Applied commands (in apply order).
        commands: Vec<JournalCommand>,
    },
    /// Transaction begin.
    TxBegin {
        /// Transaction ID.
        id: u64,
    },
    /// Command record (flattened: `{"kind":"Cmd","op":"MoveNode",...}`).
    Cmd {
        /// The concrete command.
        #[serde(flatten)]
        command: JournalCommand,
    },
    /// Transaction commit.
    TxCommit {
        /// Transaction ID.
        id: u64,
    },
}

/// Replayable command record (only the minimal information needed for replay).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum JournalCommand {
    /// Add a node.
    AddNode {
        /// ID (negative = newly created locally).
        id: i64,
        /// Latitude in 1e-7 degrees.
        lat: i32,
        /// Longitude in 1e-7 degrees.
        lon: i32,
        /// Tags.
        tags: Vec<(String, String)>,
    },
    /// Add a way.
    AddWay {
        /// ID.
        id: i64,
        /// Node ID sequence.
        nodes: Vec<i64>,
        /// Tags.
        tags: Vec<(String, String)>,
    },
    /// Add a relation.
    AddRelation {
        /// ID.
        id: i64,
        /// Members: (kind 0=node/1=way/2=relation, id, role).
        members: Vec<(u8, i64, String)>,
        /// Tags.
        tags: Vec<(String, String)>,
    },
    /// Modify relation members.
    SetRelationMembers {
        /// Relation ID.
        relation: i64,
        /// Members: (kind, id, role).
        members: Vec<(u8, i64, String)>,
    },
    /// Move a node.
    MoveNode {
        /// Node ID.
        id: i64,
        /// New latitude (1e-7 degrees).
        lat: i32,
        /// New longitude (1e-7 degrees).
        lon: i32,
        /// Latitude before the move (optional; older logs lack this field).
        #[serde(default)]
        from_lat: Option<i32>,
        /// Longitude before the move (optional; older logs lack this field).
        #[serde(default)]
        from_lon: Option<i32>,
    },
    /// Modify tags.
    SetTags {
        /// Kind: 0=node,1=way,2=relation.
        kind: u8,
        /// ID.
        id: i64,
        /// New tags.
        tags: Vec<(String, String)>,
    },
    /// Modify the way's node sequence.
    SetWayNodes {
        /// Way ID.
        way: i64,
        /// New node ID sequence.
        nodes: Vec<i64>,
    },
    /// Delete an object.
    Delete {
        /// Kind.
        kind: u8,
        /// ID.
        id: i64,
    },
    /// Split a way.
    SplitWay {
        /// Original way ID.
        way: i64,
        /// Split index.
        at: u32,
        /// New way ID (0 means allocated by the implementation).
        new_way: i64,
    },
    /// Merge ways.
    MergeWay {
        /// The way to keep.
        a: i64,
        /// The way merged in.
        b: i64,
    },
    /// Reverse a way.
    ReverseWay {
        /// Way ID.
        way: i64,
    },
}

/// Journal output target.
pub trait JournalSink: Send {
    /// Write one line (the implementation handles newline/fsync policy).
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;
    /// Clear and start over (after switching datasets / resetting the baseline after save).
    fn reset(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// File journal (JSONL).
pub struct FileJournal {
    writer: BufWriter<std::fs::File>,
}

impl FileJournal {
    /// Open for append (create if missing).
    ///
    /// If a previous crash left a trailing line without a newline, append one first so new frames do not stick to it and
    /// make the whole line unparsable, losing all subsequent frames as well.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let path = path.as_ref();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(path)?;
        let len = file.metadata()?.len();
        if len > 0 {
            use std::io::{Read, Seek, SeekFrom};
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0u8; 1];
            if file.read_exact(&mut last).is_ok() && last[0] != b'\n' {
                use std::io::Write;
                file.write_all(b"\n")?;
                file.flush()?;
            }
        }
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }
}

impl JournalSink for FileJournal {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        // The crash-recovery promise relies on "committed frames are on disk"; flush only reaches the OS cache.
        self.writer.get_ref().sync_data()
    }

    fn reset(&mut self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom};
        self.writer.flush()?;
        let file = self.writer.get_mut();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.sync_all()
    }
}

/// In-memory journal (for tests/replay).
#[derive(Default)]
pub struct MemoryJournal {
    /// Raw frames.
    pub frames: Vec<JournalFrame>,
}

impl JournalSink for MemoryJournal {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let frame: JournalFrame = serde_json::from_str(line)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        self.frames.push(frame);
        Ok(())
    }

    fn reset(&mut self) -> std::io::Result<()> {
        self.frames.clear();
        Ok(())
    }
}

/// Journal controller (no-op when there is no sink).
pub struct Journal {
    sink: Option<Box<dyn JournalSink>>,
    next_tx: u64,
    enabled: bool,
    /// Reason for the first write failure (stop retrying afterwards to avoid corrupting the file frame by frame).
    failure: Option<String>,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    /// Empty journal.
    pub fn new() -> Self {
        Self {
            sink: None,
            next_tx: 1,
            enabled: false,
            failure: None,
        }
    }

    /// Set the output target and enable it.
    pub fn set_sink(&mut self, sink: Box<dyn JournalSink>) {
        self.sink = Some(sink);
        self.enabled = true;
        self.failure = None;
    }

    /// Disable recording.
    pub fn disable(&mut self) {
        self.enabled = false;
        self.sink = None;
    }

    /// Whether it is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Take and clear the most recent write failure (for a UI notification; the journal has stopped writing by then).
    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// Clear the journal file (after switching datasets / resetting the baseline after save).
    pub fn reset(&mut self) -> std::io::Result<()> {
        self.next_tx = 1;
        self.failure = None;
        match self.sink.as_mut() {
            Some(sink) => sink.reset(),
            None => Ok(()),
        }
    }

    /// Rebuild the journal from the currently applied history (called after undo/redo).
    pub fn rebase(&mut self, commands: &[JournalCommand]) -> std::io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if let Some(sink) = self.sink.as_mut() {
            sink.reset()?;
            let frame = JournalFrame::Rebase {
                commands: commands.to_vec(),
            };
            let line = serde_json::to_string(&frame)
                .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
            sink.write_line(&line)?;
        }
        self.next_tx = 1;
        Ok(())
    }

    /// Begin a transaction and return its ID.
    pub fn begin(&mut self) -> u64 {
        let id = self.next_tx;
        self.next_tx += 1;
        if self.enabled {
            self.emit(&JournalFrame::TxBegin { id });
        }
        id
    }

    /// Record one command.
    pub fn command(&mut self, command: JournalCommand) {
        if self.enabled {
            self.emit(&JournalFrame::Cmd { command });
        }
    }

    /// Commit the transaction.
    pub fn commit(&mut self, id: u64) {
        if self.enabled {
            self.emit(&JournalFrame::TxCommit { id });
        }
    }

    fn emit(&mut self, frame: &JournalFrame) {
        if self.failure.is_some() {
            return;
        }
        let Some(sink) = self.sink.as_mut() else {
            return;
        };
        let result = serde_json::to_string(frame)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))
            .and_then(|line| sink.write_line(&line));
        if let Err(err) = result {
            // Keep editing on disk-full/permission errors, but stop writing the journal and raise a clear UI alert.
            self.failure = Some(err.to_string());
            self.enabled = false;
        }
    }
}

/// Read a JSONL journal (ignore unparsable lines / trailing partial lines).
pub fn read_journal(path: impl AsRef<Path>) -> std::io::Result<Vec<JournalFrame>> {
    Ok(read_journal_report(path)?.0)
}

/// Read a JSONL journal and report corruption.
///
/// **Stop** at the first unparsable line: continuing to read later frames would replay with a command missing,
/// silently drifting the data (e.g. a lost MoveNode while later SetTags are still applied).
pub fn read_journal_report(path: impl AsRef<Path>) -> std::io::Result<(Vec<JournalFrame>, usize)> {
    use std::io::BufRead;
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let mut frames = Vec::new();
    let mut bad = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => {
                bad += 1;
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        // Tolerate the UTF-8 BOM written by Windows tools.
        let line = line.trim_start_matches('\u{feff}');
        match serde_json::from_str::<JournalFrame>(line) {
            Ok(frame) => frames.push(frame),
            Err(_) => {
                bad += 1;
                break;
            }
        }
    }
    Ok((frames, bad))
}

/// Recovery plan: Rebase baseline commands + the complete committed frames after them.
#[derive(Debug, Default)]
pub struct ReplayPlan<'a> {
    /// Baseline commands from the Rebase frame (empty when there is no Rebase).
    pub base: &'a [JournalCommand],
    /// Frames after the Rebase and before the last complete transaction.
    pub frames: &'a [JournalFrame],
    /// Number of trailing frames discarded.
    pub dropped: usize,
}

/// Compute the recovery plan.
///
/// Only accepts a trailing `TxBegin(id) -> Cmd* -> TxCommit(id)` complete pair; after encountering a Rebase,
/// use its commands as the new baseline. Unclosed transactions and commits with mismatched IDs are not replayed.
pub fn replay_plan(frames: &[JournalFrame]) -> ReplayPlan<'_> {
    let start = frames
        .iter()
        .rposition(|frame| matches!(frame, JournalFrame::Rebase { .. }))
        .map(|index| index + 1)
        .unwrap_or(0);
    let base: &[JournalCommand] = frames
        .get(start.wrapping_sub(1))
        .and_then(|frame| match frame {
            JournalFrame::Rebase { commands } => Some(commands.as_slice()),
            _ => None,
        })
        .unwrap_or(&[]);

    let mut active: Option<u64> = None;
    let mut last_commit = start;
    for (index, frame) in frames.iter().enumerate().skip(start) {
        match frame {
            JournalFrame::TxBegin { id } => {
                if active.is_some() {
                    break;
                }
                active = Some(*id);
            }
            JournalFrame::Cmd { .. } => {
                if active.is_none() {
                    break;
                }
            }
            JournalFrame::TxCommit { id } => {
                if active != Some(*id) {
                    break;
                }
                active = None;
                last_commit = index + 1;
            }
            JournalFrame::Rebase { .. } => break,
        }
    }
    ReplayPlan {
        base,
        frames: &frames[start..last_commit],
        dropped: frames.len() - last_commit,
    }
}

/// Truncate the frames at the last committed transaction (drop trailing uncommitted/incomplete transactions).
pub fn committed_prefix(frames: &[JournalFrame]) -> &[JournalFrame] {
    let plan = replay_plan(frames);
    // A Rebase frame is not a Cmd itself, but its baseline commands are applied separately by the caller.
    plan.frames
}

#[cfg(test)]
mod tests {
    use super::*;

    fn move_cmd(id: i64) -> JournalCommand {
        JournalCommand::MoveNode {
            id,
            lat: 1,
            lon: 2,
            from_lat: None,
            from_lon: None,
        }
    }

    #[test]
    fn frames_roundtrip_through_json() {
        // Internally-tagged enums cannot serialize newtype variants; `Cmd` must be a
        // struct variant with `#[serde(flatten)]`, otherwise every command frame is
        // silently dropped and crash recovery replays nothing.
        let frame = JournalFrame::Cmd {
            command: move_cmd(42),
        };
        let text = serde_json::to_string(&frame).unwrap();
        let back: JournalFrame = serde_json::from_str(&text).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn relation_frames_roundtrip_through_json() {
        for frame in [
            JournalFrame::Cmd {
                command: JournalCommand::SetRelationMembers {
                    relation: 7,
                    members: vec![(0, 11, "outer".into()), (1, -3, "inner".into())],
                },
            },
            JournalFrame::Cmd {
                command: JournalCommand::AddRelation {
                    id: -4,
                    members: vec![(0, 11, String::new())],
                    tags: vec![("type".into(), "multipolygon".into())],
                },
            },
        ] {
            let text = serde_json::to_string(&frame).unwrap();
            let back: JournalFrame = serde_json::from_str(&text).unwrap();
            assert_eq!(back, frame);
        }
    }

    #[test]
    fn replay_plan_drops_unfinished_transaction() {
        let frames = vec![
            JournalFrame::TxBegin { id: 1 },
            JournalFrame::Cmd { command: move_cmd(1) },
            JournalFrame::TxCommit { id: 1 },
            JournalFrame::TxBegin { id: 2 },
            JournalFrame::Cmd { command: move_cmd(2) },
        ];
        let plan = replay_plan(&frames);
        assert_eq!(plan.frames.len(), 3);
        assert_eq!(plan.dropped, 2);
    }

    #[test]
    fn replay_plan_uses_last_rebase_as_baseline() {
        let frames = vec![
            JournalFrame::TxBegin { id: 1 },
            JournalFrame::Cmd { command: move_cmd(1) },
            JournalFrame::TxCommit { id: 1 },
            JournalFrame::Rebase {
                commands: vec![move_cmd(7)],
            },
            JournalFrame::TxBegin { id: 2 },
            JournalFrame::Cmd { command: move_cmd(8) },
            JournalFrame::TxCommit { id: 2 },
        ];
        let plan = replay_plan(&frames);
        assert_eq!(plan.base.len(), 1);
        assert_eq!(plan.frames.len(), 3);
    }

    #[test]
    fn journal_rebase_is_accepted_with_memory_sink() {
        let mut journal = Journal::new();
        journal.set_sink(Box::new(MemoryJournal::default()));
        journal.begin();
        journal.command(move_cmd(1));
        journal.commit(1);
        journal.rebase(&[move_cmd(2)]).unwrap();
        assert!(journal.is_enabled());
        assert!(journal.take_failure().is_none());
    }
}

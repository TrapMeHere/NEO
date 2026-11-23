//! 命令日志（Journal）：JSONL 帧 + replay。
//!
//! 帧格式（每行一个 JSON）：
//! ```text
//! {"TxBegin":{"id":1}}
//! {"Cmd":{"MoveNode":{...}}}
//! {"TxCommit":{"id":1}}
//! ```
//!
//! replay 只应用到最后一个 `TxCommit`；尾部未提交的事务被丢弃，因此"杀进程"不会
//! 产生半条事务（G3 验收项）。

use std::io::{BufWriter, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 一条日志帧。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum JournalFrame {
    /// 历史重建：丢弃此帧之前的全部帧，以 `commands` 作为新的已应用历史。
    ///
    /// undo/redo 不产生逆命令，而是把当前 undo 栈整体写成 Rebase；恢复时只重放
    /// 该帧里的命令，因此"撤销后再崩溃"不会让已撤销的编辑复活。
    Rebase {
        /// 已应用命令（按应用顺序）。
        commands: Vec<JournalCommand>,
    },
    /// 事务开始。
    TxBegin {
        /// 事务 ID。
        id: u64,
    },
    /// 命令记录（flatten：`{"kind":"Cmd","op":"MoveNode",...}`）。
    Cmd {
        /// 具体命令。
        #[serde(flatten)]
        command: JournalCommand,
    },
    /// 事务提交。
    TxCommit {
        /// 事务 ID。
        id: u64,
    },
}

/// 可重放的命令记录（只含重放所需的最小信息）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum JournalCommand {
    /// 添加节点。
    AddNode {
        /// ID（负数 = 本地新建）。
        id: i64,
        /// 1e-7 度纬度。
        lat: i32,
        /// 1e-7 度经度。
        lon: i32,
        /// 标签。
        tags: Vec<(String, String)>,
    },
    /// 添加 way。
    AddWay {
        /// ID。
        id: i64,
        /// 节点 ID 序列。
        nodes: Vec<i64>,
        /// 标签。
        tags: Vec<(String, String)>,
    },
    /// 添加 relation。
    AddRelation {
        /// ID。
        id: i64,
        /// 成员：(kind 0=node/1=way/2=relation, id, role)。
        members: Vec<(u8, i64, String)>,
        /// 标签。
        tags: Vec<(String, String)>,
    },
    /// 修改 relation 成员。
    SetRelationMembers {
        /// Relation ID。
        relation: i64,
        /// 成员：(kind, id, role)。
        members: Vec<(u8, i64, String)>,
    },
    /// 移动节点。
    MoveNode {
        /// 节点 ID。
        id: i64,
        /// 新纬度（1e-7 度）。
        lat: i32,
        /// 新经度（1e-7 度）。
        lon: i32,
        /// 移动前纬度（可选；旧日志没有该字段）。
        #[serde(default)]
        from_lat: Option<i32>,
        /// 移动前经度（可选；旧日志没有该字段）。
        #[serde(default)]
        from_lon: Option<i32>,
    },
    /// 修改标签。
    SetTags {
        /// 种类：0=node,1=way,2=relation。
        kind: u8,
        /// ID。
        id: i64,
        /// 新标签。
        tags: Vec<(String, String)>,
    },
    /// 修改 way 节点序列。
    SetWayNodes {
        /// Way ID。
        way: i64,
        /// 新节点 ID 序列。
        nodes: Vec<i64>,
    },
    /// 删除对象。
    Delete {
        /// 种类。
        kind: u8,
        /// ID。
        id: i64,
    },
    /// 分割 way。
    SplitWay {
        /// 原 way ID。
        way: i64,
        /// 分割下标。
        at: u32,
        /// 新 way ID（0 表示由实现分配）。
        new_way: i64,
    },
    /// 合并 way。
    MergeWay {
        /// 保留的 way。
        a: i64,
        /// 被并入的 way。
        b: i64,
    },
    /// 反转 way。
    ReverseWay {
        /// Way ID。
        way: i64,
    },
}

/// 日志输出目标。
pub trait JournalSink: Send {
    /// 写入一行（实现方负责换行/fsync 策略）。
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;
    /// 清空并从头开始（切换数据集 / 保存后重置基线）。
    fn reset(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// 文件日志（JSONL）。
pub struct FileJournal {
    writer: BufWriter<std::fs::File>,
}

impl FileJournal {
    /// 追加打开（不存在则创建）。
    ///
    /// 若上次崩溃留下缺少换行的残行，先补一个换行，避免新帧粘在残行上导致
    /// 整行无法解析、连带丢失后续所有帧。
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
        // 崩溃恢复承诺依赖"已提交帧已落盘"；flush 只到 OS 缓存。
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

/// 内存日志（测试/回放用）。
#[derive(Default)]
pub struct MemoryJournal {
    /// 原始帧。
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

/// Journal 控制器（没有 sink 时是 no-op）。
pub struct Journal {
    sink: Option<Box<dyn JournalSink>>,
    next_tx: u64,
    enabled: bool,
    /// 第一次写入失败的原因（之后停止尝试，避免每帧写坏文件）。
    failure: Option<String>,
}

impl Default for Journal {
    fn default() -> Self {
        Self::new()
    }
}

impl Journal {
    /// 空日志。
    pub fn new() -> Self {
        Self {
            sink: None,
            next_tx: 1,
            enabled: false,
            failure: None,
        }
    }

    /// 设置输出目标并启用。
    pub fn set_sink(&mut self, sink: Box<dyn JournalSink>) {
        self.sink = Some(sink);
        self.enabled = true;
        self.failure = None;
    }

    /// 关闭记录。
    pub fn disable(&mut self) {
        self.enabled = false;
        self.sink = None;
    }

    /// 是否启用。
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 取出并清除最近一次写入失败（供 UI 提示；此时 journal 已停止写入）。
    pub fn take_failure(&mut self) -> Option<String> {
        self.failure.take()
    }

    /// 清空日志文件（切换数据集 / 保存后重置基线）。
    pub fn reset(&mut self) -> std::io::Result<()> {
        self.next_tx = 1;
        self.failure = None;
        match self.sink.as_mut() {
            Some(sink) => sink.reset(),
            None => Ok(()),
        }
    }

    /// 用当前已应用历史重建日志（undo/redo 后调用）。
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

    /// 开始事务，返回 ID。
    pub fn begin(&mut self) -> u64 {
        let id = self.next_tx;
        self.next_tx += 1;
        if self.enabled {
            self.emit(&JournalFrame::TxBegin { id });
        }
        id
    }

    /// 记录一条命令。
    pub fn command(&mut self, command: JournalCommand) {
        if self.enabled {
            self.emit(&JournalFrame::Cmd { command });
        }
    }

    /// 提交事务。
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
            // 磁盘满/权限错误时继续编辑，但停止写日志并在 UI 明确告警。
            self.failure = Some(err.to_string());
            self.enabled = false;
        }
    }
}

/// 读取 JSONL 日志（忽略无法解析的行/尾部半行）。
pub fn read_journal(path: impl AsRef<Path>) -> std::io::Result<Vec<JournalFrame>> {
    Ok(read_journal_report(path)?.0)
}

/// 读取 JSONL 日志并报告损坏情况。
///
/// 遇到第一行无法解析时**停止**：继续读入后续帧会在缺失一条命令的情况下重放，
/// 让数据静默偏离（例如 MoveNode 丢失后后续 SetTags 仍被应用）。
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
        // 容忍 Windows 工具写出的 UTF-8 BOM。
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

/// 恢复计划：Rebase 基线命令 + 其后完整的已提交帧。
#[derive(Debug, Default)]
pub struct ReplayPlan<'a> {
    /// Rebase 帧里的基线命令（没有 Rebase 时为空）。
    pub base: &'a [JournalCommand],
    /// Rebase 之后、最后一个完整事务之前的帧。
    pub frames: &'a [JournalFrame],
    /// 被丢弃的尾部帧数。
    pub dropped: usize,
}

/// 计算恢复计划。
///
/// 只接受 `TxBegin(id) → Cmd* → TxCommit(id)` 完整配对的尾部；遇到 Rebase 后
/// 重新以它的命令为基线。未闭合事务、id 不匹配的提交都不会被重放。
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

/// 把帧截断到最后一个已提交事务（丢弃尾部未提交/不完整事务）。
pub fn committed_prefix(frames: &[JournalFrame]) -> &[JournalFrame] {
    let plan = replay_plan(frames);
    // Rebase 帧本身不是 Cmd，但基线命令由调用方单独应用。
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

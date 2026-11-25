//! PBF **writing** encoder (`osmpbf` is read-only, so the write side is self-built).
//!
//! Implements protobuf wire encoding of the OSM PBF file format directly (docs/08 §2.7):
//!
//! ```text
//! [u32 BE header_len][BlobHeader{type, datasize}][Blob{raw|zlib_data, raw_size}]
//! OSMHeader: HeaderBlock{ required_features = "OsmSchema-V0.6" }
//! OSMData  : PrimitiveBlock{ StringTable, PrimitiveGroup{nodes|ways|relations} }
//! ```
//!
//! Coordinates: NEO's `Coord` is 1e-7 degrees (decimicro), and PBF's `Node.lat/lon` use
//! `granularity=100` nanodegree units → numerically **exactly equal to decimicro**, so no conversion is needed.
//!
//! Blocking: nodes 8k/block, ways 2k/block, relations 512/block; each block has its own string table (simple and streaming).

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

use neo_core::ObjectId;
use neo_store::{DataSet, NodeSlot, WaySlot};

use crate::xml::XmlWriteMode;

/// Write options.
#[derive(Clone, Copy, Debug)]
pub struct PbfWriteOptions {
    /// Whether to zlib-compress (default true; disabling makes files larger but debuggable).
    pub compress: bool,
}

impl Default for PbfWriteOptions {
    fn default() -> Self {
        Self { compress: true }
    }
}

/// Write statistics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PbfWriteReport {
    /// Node count.
    pub nodes: u64,
    /// Way count.
    pub ways: u64,
    /// Relation count.
    pub relations: u64,
    /// Output size in bytes.
    pub bytes: u64,
    /// Number of blobs (header + data).
    pub blobs: u64,
}

impl core::fmt::Display for PbfWriteReport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "PBF written: {} nodes, {} ways, {} relations, {} bytes in {} blobs",
            self.nodes, self.ways, self.relations, self.bytes, self.blobs
        )
    }
}

/// Controls block sizes (not configurable; internal constants).
const NODE_CHUNK: usize = 8192;
const WAY_CHUNK: usize = 2048;
const RELATION_CHUNK: usize = 512;

/// Writes PBF to a writer.
pub fn write_pbf<W: Write>(
    data: &DataSet,
    out: &mut W,
    opts: PbfWriteOptions,
) -> std::io::Result<PbfWriteReport> {
    let mut report = PbfWriteReport::default();

    // --- OSMHeader ---
    let mut header = Vec::new();
    put_bytes_field(&mut header, 4, b"OsmSchema-V0.6");
    // Writing Info.visible=false requires declaring HistoricalInformation (a MUST in osmformat.proto);
    // otherwise readers treat deleted objects as alive.
    let has_history = data.iter_nodes().any(|node| !node.meta.visible)
        || data.iter_ways().any(|way| !way.meta.visible)
        || data.iter_relations().any(|rel| !rel.meta.visible);
    if has_history {
        put_bytes_field(&mut header, 4, b"HistoricalInformation");
    }
    write_blob(out, "OSMHeader", &header, opts.compress)?;
    report.blobs += 1;

    // --- Nodes ---
    let node_refs: Vec<_> = data.iter_nodes().collect();
    for chunk in node_refs.chunks(NODE_CHUNK) {
        let mut block = BlockWriter::new();
        for node in chunk {
            block.push_node(data, *node);
        }
        block.finish(out, opts.compress)?;
        report.blobs += 1;
    }
    report.nodes = node_refs.len() as u64;

    // --- Way ---
    let way_refs: Vec<_> = data.iter_ways().collect();
    for chunk in way_refs.chunks(WAY_CHUNK) {
        let mut block = BlockWriter::new();
        for way in chunk {
            block.push_way(data, way.handle, way.id)?;
        }
        block.finish(out, opts.compress)?;
        report.blobs += 1;
    }
    report.ways = way_refs.len() as u64;

    // --- Relation ---
    let rel_refs: Vec<_> = data.iter_relations().collect();
    for chunk in rel_refs.chunks(RELATION_CHUNK) {
        let mut block = BlockWriter::new();
        for rel in chunk {
            block.push_relation(data, rel.handle)?;
        }
        block.finish(out, opts.compress)?;
        report.blobs += 1;
    }
    report.relations = rel_refs.len() as u64;

    Ok(report)
}

/// Writes a file.
pub fn write_pbf_file(
    data: &DataSet,
    path: impl AsRef<Path>,
    opts: PbfWriteOptions,
) -> std::io::Result<PbfWriteReport> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = crate::xml::temp_path(path);
    let result = (|| -> std::io::Result<PbfWriteReport> {
        let file = std::fs::File::create(&tmp)?;
        let mut counter = CountingWriter {
            inner: std::io::BufWriter::new(file),
            written: 0,
        };
        let mut report = write_pbf(data, &mut counter, opts)?;
        counter.flush()?;
        counter.inner.get_ref().sync_all()?;
        report.bytes = counter.written;
        Ok(report)
    })();
    match result {
        Ok(report) => {
            crate::xml::finish_atomic_rename(&tmp, path)?;
            Ok(report)
        }
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

struct CountingWriter<W: Write> {
    inner: W,
    written: u64,
}

impl<W: Write> Write for CountingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Single-block builder: one `PrimitiveBlock` = string table + one group.
struct BlockWriter {
    strings: Vec<String>,
    index: HashMap<String, u32>,
    group: Vec<u8>,
    count: u32,
}

impl BlockWriter {
    fn new() -> Self {
        let mut index = HashMap::new();
        // Index 0 of the string table must be the empty string
        index.insert(String::new(), 0);
        Self {
            strings: vec![String::new()],
            index,
            group: Vec::new(),
            count: 0,
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&idx) = self.index.get(s) {
            return idx;
        }
        let idx = self.strings.len() as u32;
        self.strings.push(s.to_owned());
        self.index.insert(s.to_owned(), idx);
        idx
    }

    fn intern_pairs(&mut self, data: &DataSet, tags: neo_store::TagSetId) -> (Vec<u32>, Vec<u32>) {
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        for (k, v) in data.tag_pairs(tags) {
            let key = data.resolve(*k).to_owned();
            let val = data.resolve(*v).to_owned();
            keys.push(self.intern(&key));
            vals.push(self.intern(&val));
        }
        (keys, vals)
    }

    fn info_bytes(&mut self, data: &DataSet, meta: &neo_core::Meta) -> Vec<u8> {
        let mut info = Vec::new();
        if meta.version > 0 {
            put_varint_field(&mut info, 1, u64::from(meta.version));
        }
        if meta.timestamp != 0 {
            put_varint_field(&mut info, 2, meta.timestamp as u64);
        }
        if meta.changeset != 0 {
            put_varint_field(&mut info, 3, meta.changeset as u64);
        }
        if meta.uid != 0 {
            put_varint_field(&mut info, 4, u64::from(meta.uid));
        }
        let user = data.resolve(meta.user).to_owned();
        if !user.is_empty() {
            let sid = self.intern(&user);
            put_varint_field(&mut info, 5, u64::from(sid));
        }
        if !meta.visible {
            put_varint_field(&mut info, 6, 0);
        }
        info
    }

    fn push_node(&mut self, data: &DataSet, node: neo_store::NodeRef) {
        let (keys, vals) = self.intern_pairs(data, node.tags);
        let info = self.info_bytes(data, &node.meta);
        let mut msg = Vec::new();
        // Note: osmformat.Node.id is **sint64** (zigzag), while Way/Relation.id is int64;
        // a mistake here makes osmpbf read back negative ids, leaving every way reference dangling.
        put_varint_field(&mut msg, 1, zigzag(node.id.0));
        put_packed_varint(&mut msg, 2, keys.iter().map(|v| u64::from(*v)));
        put_packed_varint(&mut msg, 3, vals.iter().map(|v| u64::from(*v)));
        if !info.is_empty() {
            put_bytes_field(&mut msg, 4, &info);
        }
        put_varint_field(&mut msg, 8, zigzag(i64::from(node.pos.lat.units())));
        put_varint_field(&mut msg, 9, zigzag(i64::from(node.pos.lon.units())));
        // PrimitiveGroup.nodes = field 1 (message)
        put_bytes_field(&mut self.group, 1, &msg);
        self.count += 1;
    }

    fn push_way(
        &mut self,
        data: &DataSet,
        handle: neo_core::Handle<WaySlot>,
        id: ObjectId,
    ) -> std::io::Result<()> {
        let Some(view) = data.ways.get(handle) else {
            return Ok(());
        };
        let (keys, vals) = self.intern_pairs(data, view.tags);
        let info = self.info_bytes(data, &view.meta);
        let node_ids: Vec<i64> = data
            .try_way_node_ids(handle)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?
            .into_iter()
            .map(|id| id.0)
            .collect();
        let mut msg = Vec::new();
        put_varint_field(&mut msg, 1, id.0 as u64);
        put_packed_varint(&mut msg, 2, keys.iter().map(|v| u64::from(*v)));
        put_packed_varint(&mut msg, 3, vals.iter().map(|v| u64::from(*v)));
        if !info.is_empty() {
            put_bytes_field(&mut msg, 4, &info);
        }
        put_packed_sint_delta(&mut msg, 8, &node_ids)?;
        // PrimitiveGroup.ways = field 3
        put_bytes_field(&mut self.group, 3, &msg);
        self.count += 1;
        Ok(())
    }

    fn push_relation(
        &mut self,
        data: &DataSet,
        handle: neo_core::Handle<neo_store::RelationSlot>,
    ) -> std::io::Result<()> {
        let Some(view) = data.relations.get(handle) else {
            return Ok(());
        };
        let (keys, vals) = self.intern_pairs(data, view.tags);
        let info = self.info_bytes(data, &view.meta);
        let mut roles = Vec::new();
        let mut memids = Vec::new();
        let mut types = Vec::new();
        if let Some(members) = data.relations.members_of(handle) {
            for m in members {
                let (kind, id) = match m.target.kind {
                    neo_core::ObjectKind::Node => (
                        0u64,
                        m.target.as_node().and_then(|h| data.nodes.id(h)).map(|id| id.0),
                    ),
                    neo_core::ObjectKind::Way => (
                        1u64,
                        m.target.as_way().and_then(|h| data.ways.id(h)).map(|id| id.0),
                    ),
                    neo_core::ObjectKind::Relation => (
                        2u64,
                        m.target
                            .as_relation()
                            .and_then(|h| data.relations.id(h))
                            .map(|id| id.0),
                    ),
                    neo_core::ObjectKind::Changeset => (0, None),
                };
                let Some(id) = id else {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "relation has a dangling member handle",
                    ));
                };
                let role = data.resolve(m.role).to_owned();
                roles.push(u64::from(self.intern(&role)));
                memids.push(id);
                types.push(kind);
            }
        }
        let mut msg = Vec::new();
        put_varint_field(&mut msg, 1, view.id.0 as u64);
        put_packed_varint(&mut msg, 2, keys.iter().map(|v| u64::from(*v)));
        put_packed_varint(&mut msg, 3, vals.iter().map(|v| u64::from(*v)));
        if !info.is_empty() {
            put_bytes_field(&mut msg, 4, &info);
        }
        put_packed_varint(&mut msg, 8, roles.iter().copied());
        put_packed_sint_delta(&mut msg, 9, &memids)?;
        put_packed_varint(&mut msg, 10, types.iter().copied());
        // PrimitiveGroup.relations = field 4
        put_bytes_field(&mut self.group, 4, &msg);
        self.count += 1;
        Ok(())
    }

    fn finish<W: Write>(self, out: &mut W, compress: bool) -> std::io::Result<()> {
        // StringTable{ s = repeated bytes field 1 }
        let mut stringtable = Vec::new();
        for s in &self.strings {
            put_bytes_field(&mut stringtable, 1, s.as_bytes());
        }
        // PrimitiveBlock{ stringtable=1, primitivegroup=2 }
        let mut block = Vec::new();
        put_bytes_field(&mut block, 1, &stringtable);
        put_bytes_field(&mut block, 2, &self.group);
        write_blob(out, "OSMData", &block, compress)
    }
}

// ---------------------------------------------------------------------------
// protobuf wire helpers
// ---------------------------------------------------------------------------

fn put_varint(buf: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buf.push(byte);
            return;
        }
        buf.push(byte | 0x80);
    }
}

fn put_key(buf: &mut Vec<u8>, field: u64, wire: u64) {
    put_varint(buf, (field << 3) | wire);
}

fn put_varint_field(buf: &mut Vec<u8>, field: u64, value: u64) {
    put_key(buf, field, 0);
    put_varint(buf, value);
}

fn put_bytes_field(buf: &mut Vec<u8>, field: u64, bytes: &[u8]) {
    put_key(buf, field, 2);
    put_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

fn packed_bytes(values: impl Iterator<Item = u64>) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        put_varint(&mut out, v);
    }
    out
}

fn put_packed_varint(buf: &mut Vec<u8>, field: u64, values: impl Iterator<Item = u64>) {
    let packed = packed_bytes(values);
    put_bytes_field(buf, field, &packed);
}

/// sint64 delta encoding (way refs / relation memids).
fn put_packed_sint_delta(buf: &mut Vec<u8>, field: u64, values: &[i64]) -> std::io::Result<()> {
    let mut prev = 0i64;
    let mut packed = Vec::with_capacity(values.len());
    for &v in values {
        // Subtracting near i64::MIN overflows; an error is preferable to writing a wrong reference.
        let delta = v.checked_sub(prev).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "PBF delta overflow: object id out of range",
            )
        })?;
        prev = v;
        put_varint(&mut packed, zigzag(delta));
    }
    put_bytes_field(buf, field, &packed);
    Ok(())
}

fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn write_blob<W: Write>(out: &mut W, blob_type: &str, raw: &[u8], compress: bool) -> std::io::Result<()> {
    // PBF spec limits: uncompressed Blob data ≤ 32 MiB, BlobHeader ≤ 64 KiB.
    const MAX_BLOB: usize = 32 * 1024 * 1024;
    const MAX_HEADER: usize = 64 * 1024;
    if raw.len() > MAX_BLOB {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "PBF blob too large ({} > {MAX_BLOB}); reduce object chunk size",
                raw.len()
            ),
        ));
    }
    let mut blob = Vec::new();
    if compress {
        use flate2::Compression;
        use flate2::write::ZlibEncoder;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(raw)?;
        let compressed = encoder.finish()?;
        put_varint_field(&mut blob, 2, raw.len() as u64);
        put_bytes_field(&mut blob, 3, &compressed);
    } else {
        put_bytes_field(&mut blob, 1, raw);
    }

    let mut header = Vec::new();
    put_bytes_field(&mut header, 1, blob_type.as_bytes());
    put_varint_field(&mut header, 3, blob.len() as u64);
    if header.len() > MAX_HEADER {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PBF blob header too large",
        ));
    }

    out.write_all(&(header.len() as u32).to_be_bytes())?;
    out.write_all(&header)?;
    out.write_all(&blob)?;
    Ok(())
}

#[allow(dead_code)]
fn _assert_unused(_: XmlWriteMode, _: NodeSlot) {}

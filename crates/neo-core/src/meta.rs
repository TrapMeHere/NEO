//! OSM object metadata.
//!
//! Fully replacing JOSM means **we cannot drop** version / timestamp / changeset / uid / user /
//! visible for performance: upload diffs, conflict resolution, and history viewing all depend on them.
//! They are packed into a 32-byte `Copy` struct (about 320 MB for 10M objects, an acceptable upper bound;
//! further compression options are discussed in docs/03).

use crate::sym::Sym;

/// Server metadata of one OSM primitive.
///
/// Locally created objects use `version == 0`, `timestamp == 0`, a placeholder `changeset` for the current
/// editing session, and `visible == true` (deleted local objects are expressed via editor state, not `visible=false`).
///
/// Field order deliberately puts "8-byte fields first" so that under `repr(C)` the size is exactly 32 bytes
/// (the tail padding of version/uid/user/visible is merged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Meta {
    /// Unix timestamp in seconds (OSM's current precision is seconds).
    pub timestamp: i64,
    /// ID of the containing changeset.
    pub changeset: i64,
    /// OSM version number; 0 for newly created objects, +1 after a delete is uploaded.
    pub version: u32,
    /// uid of the last editor; `0` means unknown.
    pub uid: u32,
    /// Username of the last editor (interned symbol).
    pub user: Sym,
    /// Visibility (`false` means deleted on the server).
    pub visible: bool,
}

impl Meta {
    /// Metadata for a locally created object.
    pub const fn new_local() -> Self {
        Self {
            version: 0,
            timestamp: 0,
            changeset: 0,
            uid: 0,
            user: Sym::EMPTY,
            visible: true,
        }
    }

    /// Whether this is an object that already exists on the server (has a version number).
    #[inline]
    pub const fn is_server_versioned(&self) -> bool {
        self.version > 0
    }

    /// Whether this is a locally created object.
    #[inline]
    pub const fn is_new(&self) -> bool {
        self.version == 0
    }

    /// Byte size of the metadata of a single object.
    #[inline]
    pub const fn mem_bytes() -> usize {
        core::mem::size_of::<Self>()
    }
}

impl Default for Meta {
    fn default() -> Self {
        Self::new_local()
    }
}

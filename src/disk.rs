use nix::sys::statfs::statfs;

pub struct DiskUsage {
    pub total: u64,
    pub free: u64,
}

impl DiskUsage {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

/// The data volume holds your files. All APFS volumes share its free space.
pub fn usage() -> Option<DiskUsage> {
    let stat = statfs("/System/Volumes/Data")
        .or_else(|_| statfs("/"))
        .ok()?;
    let block_size = u64::from(stat.block_size());

    Some(DiskUsage {
        total: stat.blocks() * block_size,
        free: stat.blocks_available() * block_size,
    })
}

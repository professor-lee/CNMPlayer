use std::hash::Hash;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrackKey {
    pub path: Option<PathBuf>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: u64,
}

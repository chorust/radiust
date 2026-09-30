pub mod discover;
pub mod download;
pub mod list;

use std::path::Path;

pub(crate) struct Context<'a> {
    pub config_path: Option<&'a Path>,
    pub json: bool,
    pub quiet: bool,
    pub verbose: bool,
    pub progress_enabled: bool,
}

// Stub definitions to make things *compile*.

use std::path::PathBuf;

/// WaOS port: on WASI, `$HOME`, which the program's environment gives, as Linux reads it; none on
/// the web.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(target_os = "wasi")]
    return std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from);
    #[cfg(not(target_os = "wasi"))]
    None
}

pub fn cache_dir()        -> Option<PathBuf> { None }
pub fn config_dir()       -> Option<PathBuf> { None }
pub fn config_local_dir() -> Option<PathBuf> { None }
pub fn data_dir()         -> Option<PathBuf> { None }
pub fn data_local_dir()   -> Option<PathBuf> { None }
pub fn preference_dir()   -> Option<PathBuf> { None }
pub fn runtime_dir()      -> Option<PathBuf> { None }
pub fn executable_dir()   -> Option<PathBuf> { None }
pub fn state_dir()        -> Option<PathBuf> { None }

pub fn audio_dir()        -> Option<PathBuf> { None }
pub fn desktop_dir()      -> Option<PathBuf> { None }
pub fn document_dir()     -> Option<PathBuf> { None }
pub fn download_dir()     -> Option<PathBuf> { None }
pub fn font_dir()         -> Option<PathBuf> { None }
pub fn picture_dir()      -> Option<PathBuf> { None }
pub fn public_dir()       -> Option<PathBuf> { None }
pub fn template_dir()     -> Option<PathBuf> { None }
pub fn video_dir()        -> Option<PathBuf> { None }

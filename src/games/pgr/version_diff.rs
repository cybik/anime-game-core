use std::path::{Path, PathBuf};

use serde::{Serialize, Deserialize};
use thiserror::Error;

use crate::{version::Version, installer::prelude::Downloader};
use crate::traits::version_diff::VersionDiffExt;

#[cfg(feature = "install")]
use crate::installer::{
    downloader::DownloadingError,
    free_space
};

#[derive(Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffDownloadingError {
    /// Your installation is already up to date and not needed to be updated
    #[error("Component version is already latest")]
    AlreadyLatest,

    /// Failed to fetch remove data. Redirected from `Downloader`
    #[error("{0}")]
    DownloadingError(#[from] DownloadingError),

    /// Installation path wasn't specified. This could happen when you
    /// try to call `install` method on `VersionDiff` that was generated
    /// in `VoicePackage::list_latest`. This method couldn't know
    /// your game installation path and thus indicates that it doesn't know
    /// where this package needs to be installed
    #[error("Path to the component's downloading folder is not specified")]
    PathNotSpecified
}

impl From<minreq::Error> for DiffDownloadingError {
    fn from(error: minreq::Error) -> Self {
        DownloadingError::Minreq(error.to_string()).into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Update {
    CheckingFreeSpace(PathBuf),

    DownloadingStarted,

    /// `(downloaded files, total files)`
    DownloadingProgress(usize, usize),

    DownloadingFinished
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VersionDiff {
    /// Latest version
    Latest(Version),
    
    // TODO: Micropatch enum for updates within one game version

    /// Update available
    Outdated {
        current: Version,
        latest: Version,

        unpacked_url: String,
        files: Vec<String>,
        total_size: u64,

        /// Path to the folder this difference should be installed by the `install` method
        /// 
        /// This value can be `None`, so `install` will return `Err(DiffDownloadError::PathNotSpecified)`
        installation_path: Option<PathBuf>,

        /// Optional path to the `.version` file
        version_file_path: Option<PathBuf>
    },

    /// Component is not yet installed
    NotInstalled {
        latest: Version,

        unpacked_url: String,
        files: Vec<String>,
        total_size: u64,

        /// Path to the folder this difference should be installed by the `install` method
        /// 
        /// This value can be `None`, so `install` will return `Err(DiffDownloadError::PathNotSpecified)`
        installation_path: Option<PathBuf>,

        /// Optional path to the `.version` file
        version_file_path: Option<PathBuf>
    }
}

impl VersionDiff {
    /// Get `.version` file path
    pub fn version_file_path(&self) -> Option<PathBuf> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { version_file_path, .. } |
            Self::NotInstalled { version_file_path, .. } => version_file_path.to_owned()
        }
    }

    pub fn files(&self) -> Option<Vec<String>> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { files, .. } |
            Self::NotInstalled { files, .. } => Some(files.clone())
        }
    }
}

impl VersionDiffExt for VersionDiff {
    type Error = DiffDownloadingError;
    type Update = Update;
    type Edition = ();

    #[inline]
    fn edition(&self) -> Self::Edition {
        ()
    }

    fn current(&self) -> Option<Version> {
        match self {
            Self::Latest(current) |
            Self::Outdated { current, .. } => Some(*current),

            Self::NotInstalled { .. } => None
        }
    }

    fn latest(&self) -> Version {
        match self {
            Self::Latest(latest) |
            Self::Outdated { latest, .. } |
            Self::NotInstalled { latest, .. } => *latest
        }
    }

    fn downloaded_size(&self) -> Option<u64> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { total_size, .. } |
            Self::NotInstalled { total_size, .. } => Some(*total_size)
        }
    }

    fn unpacked_size(&self) -> Option<u64> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { total_size, .. } |
            Self::NotInstalled { total_size, .. } => Some(*total_size)
        }
    }

    fn installation_path(&self) -> Option<&Path> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { installation_path, .. } |
            Self::NotInstalled { installation_path, .. } => match installation_path {
                Some(path) => Some(path.as_path()),
                None => None
            }
        }
    }

    /// Returns base url to the unpacked game folder
    fn downloading_uri(&self) -> Option<String> {
        match self {
            // Can't be installed
            Self::Latest(_) => None,

            // Can be installed
            Self::Outdated { unpacked_url, .. } |
            Self::NotInstalled { unpacked_url, .. } => Some(unpacked_url.to_owned())
        }
    }

    /// This function is not compatible with the game updating mechanics
    fn download_as(&mut self, _path: impl AsRef<Path>, _progress: impl Fn(u64, u64) + Send + 'static) -> Result<(), Self::Error> {
        unimplemented!()
    }

    fn install_to(&self, path: impl AsRef<Path>, updater: impl Fn(Self::Update) + Clone + Send + 'static) -> Result<(), Self::Error> {
        tracing::debug!("Installing version difference");

        let path = path.as_ref();

        let url = self.downloading_uri().expect("Failed to retreive downloading url");
        let required = self.unpacked_size().expect("Failed to retreive total size");
        let files = self.files().expect("Failed to retreive list of files for downloading");

        (updater)(Update::CheckingFreeSpace(path.to_path_buf()));

        // Check available free space
        let Some(space) = free_space::available(&path) else {
            tracing::error!("Path is not mounted: {:?}", &path);

            return Err(DownloadingError::PathNotMounted(path.to_path_buf()).into());
        };

        if space < required {
            tracing::error!("No free space available in the installation folder. Required: {required}. Available: {space}");

            return Err(DownloadingError::NoSpaceAvailable(path.to_path_buf(), required, space).into());
        }

        // Download updated files
        let total = files.len();

        (updater)(Update::DownloadingStarted);

        for (i, file) in files.into_iter().enumerate() {
            tracing::info!("Updating {file} ({}/{total})...", i + 1);

            Downloader::new(format!("{url}/{file}"))?
                // Don't check availability of disk space as it was done before
                .with_free_space_check(false)

                // Overwrite outdated file instead of trying to continue its downloading
                .with_continue_downloading(false)

                // Download outdated file
                .download(path.join(file), |_, _| {})?;

            (updater)(Update::DownloadingProgress(i + 1, total));
        }

        // Create `.version` file here even if hdiff patching is failed because
        // it's easier to explain user why he should run files repairer than
        // why he should re-download entire game update because something is failed
        #[allow(unused_must_use)] {
            let version_path = self.version_file_path()
                .unwrap_or_else(|| path.join(".version"));

            std::fs::write(version_path, self.latest().version);
        }

        (updater)(Update::DownloadingFinished);

        Ok(())
    }
}
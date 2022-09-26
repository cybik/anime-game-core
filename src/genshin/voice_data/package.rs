use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use fs_extra::dir::get_size;

use crate::version::Version;

use crate::genshin::{
    voice_data::locale::VoiceLocale,
    json_schemas::versions::VoicePack as RemoteVoicePack,
    consts::get_voice_package_path,
    api
};

#[cfg(feature = "install")]
use crate::installer::diff::{VersionDiff, TryGetDiff};

/// Find voice package with specified locale from list of packages
fn find_voice_pack(list: Vec<RemoteVoicePack>, locale: VoiceLocale) -> RemoteVoicePack {
    for pack in list {
        if pack.language == locale.to_code() {
            return pack;
        }
    }

    // We're sure that all possible voice packages are listed in VoiceLocale... right?
    unreachable!();
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VoicePackage {
    Installed {
        path: PathBuf,
        locale: VoiceLocale
    },
    NotInstalled {
        locale: VoiceLocale,
        version: Version,
        data: RemoteVoicePack,
        game_path: Option<PathBuf>
    }
}

impl VoicePackage {
    /// Voice packages can't be instaled wherever you want.
    /// Thus this method can return `None` in case the path
    /// doesn't point to a real voice package folder
    pub fn new<T: Into<PathBuf>>(path: T) -> Option<Self> {
        let path = path.into();

        if path.exists() && path.is_dir() {
            match path.file_name() {
                Some(name) => match VoiceLocale::from_str(name.to_string_lossy()) {
                    Some(locale) => Some(Self::Installed {
                        path,
                        locale
                    }),
                    None => None
                },
                None => None
            }
        }

        else {
            None
        }
    }

    /// Get latest voice package with specified locale
    /// 
    /// Note that returned object will be `VoicePackage::NotInstalled`, but
    /// technically it can be installed. This method just don't know the game's path
    pub fn with_locale(locale: VoiceLocale) -> anyhow::Result<Self> {
        let response = api::try_fetch_json()?;
        let latest = response.data.game.latest;

        Ok(Self::NotInstalled {
            locale,
            version: Version::from_str(latest.version).unwrap(),
            data: find_voice_pack(latest.voice_packs, locale),
            game_path: None
        })
    }

    // TODO: find_in(game_path: String, locale: VoiceLocale)

    /// Get installation status of this package
    /// 
    /// This method will return `false` if this package is `VoicePackage::NotInstalled` enum value
    /// 
    /// If you want to check it's actually installed - you'd need to use `is_installed_in`
    pub fn is_installed(&self) -> bool {
        match self {
            Self::Installed { .. } => true,
            Self::NotInstalled { .. } => false
        }
    }

    /// Calculate voice package size in bytes
    /// 
    /// (unpacked size, Option(archive size))
    pub fn size(&self) -> (u64, Option<u64>) {
        match self {
            VoicePackage::Installed { path, .. } => (get_size(path).unwrap(), None),
            VoicePackage::NotInstalled { data, .. } => (
                data.package_size.parse::<u64>().unwrap(),
                Some(data.size.parse::<u64>().unwrap())
            ),
        }
    }

    /// This method will return `true` if the package has `VoicePackage::Installed` enum value
    /// 
    /// If it's `VoicePackage::NotInstalled`, then this method will check `game_path`'s voices folder
    pub fn is_installed_in<T: Into<PathBuf>>(&self, game_path: T) -> bool {
        match self {
            Self::Installed { .. } => true,
            Self::NotInstalled { locale, .. } => {
                get_voice_package_path(game_path, locale.to_folder()).exists()
            }
        }
    }

    /// Get list of latest voice packages
    pub fn list_latest() -> anyhow::Result<Vec<VoicePackage>> {
        let response = api::try_fetch_json()?;

        let mut packages = Vec::new();
        let version = Version::from_str(response.data.game.latest.version).unwrap();

        for package in response.data.game.latest.voice_packs {
            packages.push(Self::NotInstalled {
                locale: VoiceLocale::from_str(&package.language).unwrap(),
                version: version.clone(),
                data: package,
                game_path: None
            });
        }

        Ok(packages)
    }

    /// Get voice package locale
    pub fn locale(&self) -> VoiceLocale {
        match self {
            Self::Installed { path: _, locale } => *locale,
            Self::NotInstalled { locale, version: _, data: _, game_path: _ } => *locale
        }
    }

    /// This method can fail to parse this package version.
    /// It also can mean that the corresponding folder doesn't
    /// contain voice package files
    pub fn try_get_version(&self) -> anyhow::Result<Version> {
        match &self {
            Self::NotInstalled { locale: _, version, data: _, game_path: _} => Ok(*version),
            Self::Installed { path, locale } => {
                // self.path is Some(...) if self.version is None
                // this means that this struct was made from some currently installed path

                let package_size = get_size(&path)?;

                // Since anime company changed the way they store voice packages data
                // now to identify its version I want to calculate the actual
                // size of the voice package directory and compare it with all the
                // remotely available voice packages sizes

                let response = api::try_fetch_json()?;

                let latest_voice_pack = find_voice_pack(response.data.game.latest.voice_packs, *locale);

                // This constant found its origin in the change of the voice packages format.
                // When the Anime Company decided that they know better how their game should work
                // and changed voice files names to some random numbers it caused issue when
                // old files aren't being replaced by the new ones, obviously because now they have
                // different names. When you download new voice package - its size will be something like 9 GB.
                // But Company's API returns double of this size, so like 18 GB, because their API also
                // messed folder where they store unpacked voice packages.
                // That's why we have to substract this approximate value from all the packages sizes

                #[allow(non_snake_case)]
                let CONSTANT_OF_STUPIDITY: u64 = match self.locale() {
                    VoiceLocale::English  => 8593687434 + 750 * 1024 * 1024, // 8 GB    (2.8.0)                                 + 750 MB (3.0.0)
                    VoiceLocale::Japanese => 9373182378 + 750 * 1024 * 1024, // 8.72 GB (2.8.0)                                 + 750 MB (3.0.0)
                    VoiceLocale::Korean   => 8804682956 + 750 * 1024 * 1024, // 8.2 GB  (2.8.0, not calculated (approximation)) + 750 MB (3.0.0)
                    VoiceLocale::Chinese  => 8804682956 + 750 * 1024 * 1024, // 8.2 GB  (2.8.0, not calculated (approximation)) + 750 MB (3.0.0)
                };

                // println!("Actual package size: {}", package_size);

                // API works this way:
                // We have [latest] field that contains absolute voice package with its real, absolute size
                // and we have [diff] fields that contains relative incremental changes with relative sizes
                // Since we're approximating packages versions by the real, so absolute folder sizes, we need to calculate
                // absolute folder sizes for differences
                // Since this is not an option in the API we have second approximation: lets say
                // that absolute [2.6.0] version size is [latest (2.8.0)] absolute size - [2.7.0] relative size - [2.6.0] relative size
                // That's being said we need to substract each diff.size from the latest.size

                let mut voice_pack_size = latest_voice_pack.size.parse::<u64>().unwrap() - CONSTANT_OF_STUPIDITY;
                let mut packages = VecDeque::from(vec![(response.data.game.latest.version.clone(), voice_pack_size)]);

                // println!(" 3.0.0 package size: {}", voice_pack_size);

                // List through other versions of the game
                for diff in response.data.game.diffs {
                    let voice_package = find_voice_pack(diff.voice_packs, *locale);

                    // Approximate this diff absolute folder size
                    let relative_size = voice_package.size.parse::<u64>().unwrap();

                    if relative_size < 4 * 1024 * 1024 * 1024 {
                        voice_pack_size -= relative_size;
                    }

                    // For no reason API's size field in the [diff] can contain
                    // its absolute size. Let's say if size is more than 4 GB then it's only
                    // update size, so difference, so relative size. Otherwise it's absolute size
                    // 
                    // Example (Japanese):
                    // 
                    // 2.8.0 size: 18736543170 (latest, so absolute size)
                    // 2.7.0 size: 1989050587  (clearly update size, so relative)
                    // 2.6.0 size: 15531165534 (clearly absolute size)
                    else {
                        voice_pack_size = relative_size;

                        if voice_pack_size > CONSTANT_OF_STUPIDITY {
                            voice_pack_size -= CONSTANT_OF_STUPIDITY;
                        }
                    }

                    // println!(" {} package size: {}", diff.version, voice_pack_size);

                    packages.push_front((diff.version, voice_pack_size));
                }

                // To approximate the version let's say if an actual folder weights less
                // than API says some version should weight - then it's definitely not this version
                let mut package_version = Version::from_str(response.data.game.latest.version).unwrap();

                for (version, size) in packages {
                    // Actual folder size can be +- the same as in API response
                    // Let's say +-512 MB is ok
                    if package_size > size - 512 * 1024 * 1024 {
                        package_version = Version::from_str(version).unwrap();
                    }
                }

                Ok(package_version)
            }
        }
    }

    /// Try to delete voice package
    /// 
    /// FIXME:
    /// ⚠️ May fail on Chinese version due to paths differences
    pub fn delete(&self) -> anyhow::Result<()> {
        match self {
            VoicePackage::Installed { path, .. } => {
                let mut game_path = Path::new(path);

                for _ in 0..6 {
                    game_path = match game_path.parent() {
                        Some(game_path) => game_path,
                        None => return Err(anyhow::anyhow!("Failed to find game directory"))
                    };
                }

                self.delete_in(game_path)
            },
            VoicePackage::NotInstalled { game_path, .. } => {
                match game_path {
                    Some(game_path) => self.delete_in(game_path),
                    None => return Err(anyhow::anyhow!("Failed to find game directory"))
                }
            }
        }
    }

    /// Try to delete voice package from specific game directory
    /// 
    /// FIXME:
    /// ⚠️ May fail on Chinese version due to paths differences
    pub fn delete_in<T: Into<PathBuf>>(&self, game_path: T) -> anyhow::Result<()> {
        let locale = match self {
            VoicePackage::Installed { locale, .. } |
            VoicePackage::NotInstalled { locale, .. } => locale
        };

        let game_path = game_path.into();

        // Audio_<locale folder>_pkg_version
        std::fs::remove_dir_all(get_voice_package_path(&game_path, locale.clone()))?;
        std::fs::remove_file(game_path.join(format!("Audio_{}_pkg_version", locale.to_folder())))?;

        Ok(())
    }
}

#[cfg(feature = "install")]
impl TryGetDiff for VoicePackage {
    fn try_get_diff(&self) -> anyhow::Result<VersionDiff> {
        let response = api::try_fetch_json()?;

        if self.is_installed() {
            let current = self.try_get_version()?;

            if response.data.game.latest.version == current {
                Ok(VersionDiff::Latest(current))
            }

            else {
                for diff in response.data.game.diffs {
                    if diff.version == current {
                        let diff = find_voice_pack(diff.voice_packs, self.locale());

                        return Ok(VersionDiff::Diff {
                            current,
                            latest: Version::from_str(response.data.game.latest.version).unwrap(),
                            url: diff.path,
                            download_size: diff.size.parse::<u64>().unwrap(),
                            unpacked_size: diff.package_size.parse::<u64>().unwrap(),
                            unpacking_path: match self {
                                VoicePackage::Installed { .. } => None,
                                VoicePackage::NotInstalled { game_path, .. } => game_path.clone(),
                            }
                        })
                    }
                }

                Ok(VersionDiff::Outdated {
                    current,
                    latest: Version::from_str(response.data.game.latest.version).unwrap()
                })
            }
        }
        
        else {
            let latest = find_voice_pack(response.data.game.latest.voice_packs, self.locale());

            Ok(VersionDiff::NotInstalled {
                latest: Version::from_str(response.data.game.latest.version).unwrap(),
                url: latest.path,
                download_size: latest.size.parse::<u64>().unwrap(),
                unpacked_size: latest.package_size.parse::<u64>().unwrap(),
                unpacking_path: match self {
                    VoicePackage::Installed { .. } => None,
                    VoicePackage::NotInstalled { game_path, .. } => game_path.clone(),
                }
            })
        }
    }
}
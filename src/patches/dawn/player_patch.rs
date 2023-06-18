use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::io::Write;
use std::env::temp_dir;

use serde::{Serialize, Deserialize};
use md5::{Md5, Digest};

use crate::version::Version;
use crate::genshin::{api, consts::GameEdition};

use super::prelude::*;

/// If this line is commented in the `patch.sh` or `patch_anti_logincrash.sh` file,
/// then it's stable version. Otherwise it's in testing phase
const STABILITY_MARK: &str = "#echo \"If you would like to test this patch, modify this script and remove the line below this one.\"";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerPatch {
    // I don't like these fields to be public
    // but otherwise it breaks main window compatibility in
    // PerformAction event
    pub patch_folder: PathBuf,
    pub status: PatchStatus,
    pub edition: GameEdition
}

// TODO: add tracing

impl PatchExt for PlayerPatch {
    fn from_folder(patch_folder: impl AsRef<Path>, game_edition: GameEdition) -> anyhow::Result<Self> where Self: Sized {
        let patch_folder = patch_folder.as_ref().to_path_buf();

        // Immediately throw error if patch folder doesn't even exist
        // but it actually shouldn't be possible because we get this struct
        // from `Patch` struct which implements `GitRemoteSync` where it's verified
        // but anyway
        if !patch_folder.exists() {
            anyhow::bail!("Given patch folder doesn't exist: {:?}", patch_folder);
        }

        // Prepare vector of probable patch versions
        let mut patch_folders = patch_folder.read_dir()?.flatten()
            // Filter entries with long names (actual folders are: 310, 320, 330, ...)
            .filter(|entry| entry.file_name().len() == 3)

            // Pass only folders
            .filter(|entry| entry.file_type().map_or_else(|_| false, |entry| entry.is_dir()))

            // Get rid of every folder without patch.sh file
            // FIXME: Preparation stage may not include this file
            .filter(|entry| entry.path().join("patch.sh").exists())

            // Collect entries into the vector
            .collect::<Vec<_>>();

        // No patch available (but why?)
        if patch_folders.is_empty() {
            return Ok(Self {
                patch_folder,
                status: PatchStatus::NotAvailable,
                edition: game_edition
            });
        }

        // Sort probable patch versions in descending order
        // we're interested in latest available version right?
        patch_folders.sort_by(|a, b| b.file_name().cmp(&a.file_name()));

        // Get latest available game version
        let latest_version = Version::from_str(api::request(game_edition)?.data.game.latest.version).unwrap();

        let patch_folder = &patch_folders[0];

        // Get patch version from folder name
        // may look not really safe but it pretty much should be...
        let file_name = patch_folder.file_name().to_string_lossy().bytes().collect::<Vec<u8>>();

        let version = Version::new(file_name[0] - b'0', file_name[1] - b'0', file_name[2] - b'0');

        // Return PatchStatus::Outdated if the patch is, well, outdated
        if version < latest_version {
            return Ok(Self {
                patch_folder: patch_folder.path(),
                status: PatchStatus::Outdated {
                    current: version,
                    latest: latest_version
                },
                edition: game_edition
            });
        }

        // Read patch.sh file
        let patch_script = std::fs::read_to_string(patch_folder.path().join("patch.sh"))?;

        // Try to get available player hashes
        let mut hashes = Vec::with_capacity(2);

        for line in patch_script.lines() {
            // if [ "${sum}" == "8c8c3d845b957e4cb84c662bed44d072" ]; then
            // if [ "${sum}" == "<TODO>" ]; then
            if line.len() > 20 && &line[..18] == "if [ \"${sum}\" == \"" {
                let hash = &line[18..line.len() - 9];

                hashes.push(if hash.len() == 32 { Some(hash) } else { None });
            }
        }

        let player_hash = match hashes.len() {
            0 => None,

            1 => {
                if hashes[0] == None {
                    None
                } else {
                    Some(PatchRegions::Global(hashes[0].unwrap().to_string()))
                }
            }

            2 => {
                if hashes[0] == None {
                    Some(PatchRegions::China(hashes[1].unwrap().to_string()))
                }

                else if hashes[1] == None {
                    Some(PatchRegions::Global(hashes[0].unwrap().to_string()))
                }

                else {
                    Some(PatchRegions::Both {
                        global: hashes[0].unwrap().to_string(),
                        china: hashes[1].unwrap().to_string()
                    })
                }
            }

            _ => unreachable!()
        };

        // Return patch status
        match player_hash {
            Some(player_hash) => {
                // If patch.sh contains STABILITY_MARK - then it's stable version
                if patch_script.contains(STABILITY_MARK) {
                    Ok(Self {
                        patch_folder: patch_folder.path(),
                        status: PatchStatus::Available {
                            version,
                            player_hash
                        },
                        edition: game_edition
                    })
                }

                // Otherwise it's in testing
                else {
                    Ok(Self {
                        patch_folder: patch_folder.path(),
                        status: PatchStatus::Testing {
                            version,
                            player_hash
                        },
                        edition: game_edition
                    })
                }
            }

            // Failed to parse UnityPlayer.dll hashes -> likely in preparation state
            // but also could be changed file structure, or something else
            None => Ok(Self {
                patch_folder: patch_folder.path(),
                status: PatchStatus::Preparation {
                    version
                },
                edition: game_edition
            })
        }
    }

    #[inline]
    fn folder(&self) -> &Path {
        self.patch_folder.as_path()
    }

    #[inline]
    fn status(&self) -> &PatchStatus {
        &self.status
    }

    fn is_applied(&self, game_folder: impl AsRef<Path>) -> anyhow::Result<bool> {
        let dll = std::fs::read(game_folder.as_ref().join("UnityPlayer.dll"))?;

        match &self.status {
            PatchStatus::NotAvailable |
            PatchStatus::Outdated { .. } |
            PatchStatus::Preparation { .. } => Ok(false),

            PatchStatus::Testing { player_hash, .. } |
            PatchStatus::Available { player_hash, .. } => Ok(player_hash.is_applied(format!("{:x}", Md5::digest(dll))))
        }
    }

    fn apply(&self, game_folder: impl AsRef<Path>, use_root: bool) -> anyhow::Result<()> {
        tracing::debug!("Applying game patch");

        match &self.status {
            PatchStatus::NotAvailable |
            PatchStatus::Outdated { .. } |
            PatchStatus::Preparation { .. } => anyhow::bail!("Patch can't be applied because it's not available: {:?}", &self.status),

            PatchStatus::Testing { version, .. } |
            PatchStatus::Available { version, .. } => {
                let temp_dir = temp_dir().join(".patch-applying");
                let patch_folder = self.patch_folder.join(version.to_plain_string());

                // Verify that the patch folder exists
                // Kinda irrealistic situation but still
                if !patch_folder.exists() {
                    tracing::error!("Patch folder doesn't exist: {:?}", patch_folder);

                    anyhow::bail!("Patch folder doesn't exist: {:?}", patch_folder);
                }

                // Remove temp folder if it is for some reason already exists
                if temp_dir.exists() {
                    std::fs::remove_dir_all(&temp_dir)?;
                }

                // Create temp folder
                std::fs::create_dir_all(&temp_dir)?;

                // Copy patch files there
                let mut options = fs_extra::dir::CopyOptions::default();

                options.content_only = true; // Don't copy e.g. "270" folder, just its content

                if let Err(err) = fs_extra::dir::copy(patch_folder, &temp_dir, &options) {
                    tracing::error!("Failed to copy patch to the temp folder: {err}");

                    anyhow::bail!("Failed to copy patch to the temp folder: {err}");
                }

                // Remove exit and read commands from the beginning of the patch.sh file
                // These lines are used for test patch restrictions so we don't need them
                let patch_file = temp_dir.join("patch.sh");

                let mut patch_script = std::fs::read_to_string(&patch_file)?;

                patch_script = format!("{}{}", {
                    patch_script[..1200]
                        .replace("exit", "#exit")
                        .replace("read", "#read")
                }, &patch_script[1200..]);

                // Update patch.sh file
                std::fs::write(&patch_file, patch_script)?;

                // Execute patch.sh from the game folder
                let output = if use_root {
                    // pkexec bash -c "cd '<game path>' ; bash '<patch path>/patch.sh'"
                    // We have to use this command as pkexec ignores current working directory
                    Command::new("pkexec")
                        .arg("bash")
                        .arg("-c")
                        .arg(format!("cd '{}' ; bash '{}'", game_folder.as_ref().to_string_lossy(), patch_file.to_string_lossy()))
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?
                }

                else {
                    Command::new("bash")
                        .arg(patch_file)
                        .current_dir(game_folder)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?
                };

                // Input "y" as it's asked in the patch script
                // I could remove it, but who actually cares?
                output.stdin.as_ref().unwrap().write(b"y")?;

                let output = output.wait_with_output()?;

                // Remove temp patch folder
                std::fs::remove_dir_all(temp_dir)?;

                // Return patching status
                let output = String::from_utf8_lossy(&output.stdout);

                if output.contains("Patch applied!") {
                    Ok(())
                }

                else {
                    tracing::error!("Failed to apply patch: {output}");

                    anyhow::bail!("Failed to apply patch: {output}");
                }
            }
        }
    }

    fn revert(&self, game_folder: impl AsRef<Path>, forced: bool) -> anyhow::Result<()> {
        tracing::debug!("Reverting game patch");

        match &self.status {
            PatchStatus::NotAvailable |
            PatchStatus::Outdated { .. } |
            PatchStatus::Preparation { .. } => anyhow::bail!("Patch can't be reverted because it's not available: {:?}", &self.status),

            PatchStatus::Testing { version, .. } |
            PatchStatus::Available { version, .. } => {
                let temp_dir = temp_dir().join(".patch-applying");
                let patch_folder = self.patch_folder.join(version.to_plain_string());

                // Verify that the patch folder exists
                // Kinda irrealistic situation but still
                if !patch_folder.exists() {
                    tracing::error!("Patch folder doesn't exist: {:?}", patch_folder);

                    anyhow::bail!("Patch folder doesn't exist: {:?}", patch_folder);
                }

                // Remove temp folder if it is for some reason already exists
                if temp_dir.exists() {
                    std::fs::remove_dir_all(&temp_dir)?;
                }

                // Create temp folder
                std::fs::create_dir_all(&temp_dir)?;

                // Copy patch files there
                let mut options = fs_extra::dir::CopyOptions::default();

                options.content_only = true; // Don't copy e.g. "270" folder, just its content

                if let Err(err) = fs_extra::dir::copy(patch_folder, &temp_dir, &options) {
                    tracing::error!("Failed to copy patch to the temp folder: {err}");

                    anyhow::bail!("Failed to copy patch to the temp folder: {err}");
                }

                let revert_file = temp_dir.join("patch_revert.sh");

                // Remove files timestamps checks if it's needed
                if forced {
                    // Update patch_revert.sh file
                    std::fs::write(
                        &revert_file,
                        std::fs::read_to_string(&revert_file)?
                            .replace("difftime=$", "difftime=0 #difftime=$")
                    )?;
                }

                // Execute patch_revert.sh from the game folder
                let output = Command::new("bash")
                    .arg(revert_file)
                    .current_dir(game_folder)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .output()?;

                // Remove temp patch folder
                std::fs::remove_dir_all(temp_dir)?;

                // Return reverting status
                let output = String::from_utf8_lossy(&output.stdout);

                if !output.contains("ERROR: ") {
                    Ok(())
                }

                else {
                    tracing::error!("Failed to revert patch: {output}");

                    anyhow::bail!("Failed to revert patch: {output}");
                }
            }
        }
    }
}
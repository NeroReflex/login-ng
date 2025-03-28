/*
    login-ng A greeter written in rust that also supports autologin with systemd-homed
    Copyright (C) 2024  Denis Benato

    This program is free software; you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation; either version 2 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License along
    with this program; if not, write to the Free Software Foundation, Inc.,
    51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA.
*/

use sys_mount::{Mount, Unmount, UnmountDrop, UnmountFlags};

use login_ng::mount::MountPoints;
use tokio::sync::RwLock;

use std::collections::HashMap;
use std::fs::File;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::{fs::create_dir, path::Path};

use std::io::{self, Write};

use serde::{Deserialize, Serialize};
use serde_json;

use crate::result::ServiceOperationResult;
use crate::service::ServiceError;

use zbus::interface;

use tokio::time::{sleep, Duration};

/// Mounts a filesystem at the specified path.
///
/// This function takes a tuple containing information necessary for mounting a filesystem.
/// It checks if the specified mount path exists and is a directory. If the path does not exist,
/// it attempts to create it. Depending on whether the filesystem type is provided, it constructs
/// a mount operation accordingly.
///
/// # Parameters
///
/// - `data`: A tuple of four `String` values:
///   - `data.0`: The filesystem type (e.g., "ext4", "nfs"). If this is an empty string, the mount
///     operation will be performed without specifying a filesystem type.
///   - `data.1`: Additional data required for the mount operation (e.g., options for the mount).
///   - `data.2`: The source of the filesystem to mount (e.g., a device or remote filesystem).
///   - `data.3`: The target directory where the filesystem should be mounted.
///
/// # Returns
///
/// Returns a `Result<Mount, io::Error>`. On success, it returns a `Mount` object representing
/// the mounted filesystem. On failure, it returns an `io::Error` indicating what went wrong,
/// which could include issues with directory creation or mounting the filesystem.
///
/// # Errors
///
/// This function may fail if:
/// - The specified mount path does not exist and cannot be created due to permission issues.
/// - The mount operation fails due to invalid parameters or system errors.
///
fn mount(data: (String, String, String, String)) -> io::Result<Mount> {
    let mount_path = Path::new(data.3.as_str());
    if !mount_path.exists() || !mount_path.is_dir() {
        // if the path is a file this will fail
        create_dir(mount_path)?;
    }

    match data.0.is_empty() {
        true => Mount::builder().mount(data.2.as_str(), mount_path.as_os_str()),
        false => Mount::builder()
            .fstype(data.0.as_str())
            .data(data.1.as_str())
            .mount(data.2.as_str(), data.3.as_str()),
    }
}

pub(crate) fn mount_all(
    mounts: MountPoints,
    username: String,
    homedir: String,
) -> Vec<UnmountDrop<Mount>> {
    let mut mounted_devices = vec![];

    for m in mounts
        .foreach(|a, b| {
            (
                b.fstype().clone(),
                b.flags().join(",").clone(),
                b.device().clone(),
                a.clone(),
            )
        })
        .iter()
    {
        match mount(m.clone()) {
            Ok(mount) => {
                println!(
                    "Mounted device {} into {} for user '{username}'",
                    m.2.as_str(),
                    m.3.as_str(),
                );

                // Make the mount temporary, so that it will be unmounted on drop.
                mounted_devices.push(mount.into_unmount_drop(UnmountFlags::DETACH));
            }
            Err(err) => {
                eprintln!(
                    "failed to mount device {} into {}: {}",
                    m.2.as_str(),
                    m.3.as_str(),
                    err
                );

                return vec![];
            }
        }
    }

    match mount((
        mounts.mount().fstype().clone(),
        mounts.mount().flags().join(","),
        mounts.mount().device().clone(),
        homedir,
    )) {
        Ok(mount) => {
            println!(
                "Mounted device {} on home directory for user '{username}'",
                mounts.mount().device().as_str(),
            );

            // Make the mount temporary, so that it will be unmounted on drop.
            mounted_devices.push(mount.into_unmount_drop(UnmountFlags::DETACH));
        }
        Err(err) => {
            eprintln!("failed to mount user directory: {err}");
            return vec![];
        }
    }

    mounted_devices
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
pub struct MountAuth {
    authorizations: HashMap<String, Vec<u64>>,
}

impl MountAuth {
    pub fn new(json_str: &str) -> Result<Self, ServiceError> {
        let auth: MountAuth = serde_json::from_str(json_str)?;
        Ok(auth)
    }

    pub fn load_from_file(file_path: &str) -> Result<Self, ServiceError> {
        let json_str = std::fs::read_to_string(file_path)?;
        Self::new(&json_str)
    }

    pub fn add_authorization(&mut self, username: String, hash: u64) {
        self.authorizations
            .entry(username)
            .or_insert_with(Vec::new)
            .push(hash);
    }

    pub fn authorized(&self, username: &str, hash: u64) -> bool {
        let Some(values) = self.authorizations.get(&String::from(username)) else {
            return false;
        };

        values.contains(&hash)
    }
}

pub struct MountAuthDBus {
    file_path: PathBuf,
    mounts_auth: Arc<RwLock<MountAuth>>,
}

impl MountAuthDBus {
    pub fn new(file_path: PathBuf, mounts_auth: Arc<RwLock<MountAuth>>) -> Self {
        Self { file_path, mounts_auth }
    }
}

#[interface(
    name = "org.neroreflex.login_ng_mount1",
    proxy(
        default_service = "org.neroreflex.login_ng_mount",
        default_path = "/org/zbus/login_ng_mount"
    )
)]
impl MountAuthDBus {
    async fn authorize(&mut self, username: String, hash: u64) -> u32 {
        let mut lock = self.mounts_auth
            .write()
            .await;

        let prev = lock.clone();

        lock.add_authorization(username, hash);

        let mut file = match File::create(self.file_path.as_path()) {
            Ok(file) => file,
            Err(err) => {
                eprintln!("Error opening mount authorizations file: {err}");

                *lock = prev;

                return ServiceOperationResult::CannotIdentifyUser.into()
            }
        };

        match file.write(serde_json::to_string(lock.deref()).unwrap().as_bytes()) {
            Ok(_written) => (),
            Err(err) => {
                eprintln!("Error writing data to mount authorizations file: {err}");

                *lock = prev;

                return ServiceOperationResult::CannotIdentifyUser.into()
            }
        }

        match file.flush() {
            Ok(res) => res,
            Err(err) => {
                eprintln!("Error finalizing the mount authorizations file: {err}");

                *lock = prev;

                return ServiceOperationResult::CannotIdentifyUser.into()
            }
        }

        drop(file);

        ServiceOperationResult::Ok.into()
    }

    async fn check(&self, username: &str, hash: u64) -> bool {
        // Defeat brute-force searches in an attempt to find an hash collision
        sleep(Duration::from_secs(1)).await;

        self.mounts_auth.read().await.authorized(username, hash)
    }
}

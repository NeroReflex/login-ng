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

extern crate rand;
extern crate sys_mount;
extern crate tokio;

use rand::rngs::OsRng;

use sys_mount::{Mount, MountFlags, SupportedFilesystems, Unmount, UnmountDrop, UnmountFlags};

use login_ng::{
    storage::{load_user_auth_data, load_user_mountpoints},
    users::{self, get_user_by_name, os::unix::UserExt},
};

use thiserror::Error;

use std::sync::Arc;
use std::{collections::HashMap, ffi::OsString, io};
use tokio::sync::Mutex;

use std::future::pending;
use zbus::{connection, interface, Error as ZError};

use rsa::{
    pkcs1::EncodeRsaPublicKey, pkcs8::LineEnding, Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey,
};

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("Permission error: not running as the root user")]
    MissingPrivilegesError,

    #[error("DBus error: {0}")]
    ZbusError(#[from] ZError),
}

struct UserSession {
    mounts: Vec<UnmountDrop<Mount>>,
}

struct Service {
    priv_key: RsaPrivateKey,
    pub_key: RsaPublicKey,
    sessions: Arc<Mutex<HashMap<OsString, UserSession>>>,
}

impl Service {
    pub fn new() -> Self {
        let mut rng = rand::thread_rng();
        let priv_key = RsaPrivateKey::new(&mut rng, 8192).expect("failed to generate a key");
        let pub_key = RsaPublicKey::from(&priv_key);

        let sessions = Arc::new(Mutex::new(HashMap::new()));

        Self {
            priv_key,
            pub_key,
            sessions,
        }
    }
}

fn mount(data: (String, String, String, String)) -> io::Result<Mount> {
    match data.0.is_empty() {
        true => Mount::builder().mount(data.2.as_str(), data.3.as_str()),
        false => Mount::builder()
            .fstype(data.0.as_str())
            .data(data.1.as_str())
            .mount(data.2.as_str(), data.3.as_str()),
    }
}

#[interface(name = "org.zbus.login_ng")]
impl Service {
    async fn get_pubkey(&self) -> String {
        match self.pub_key.to_pkcs1_pem(LineEnding::CRLF) {
            Ok(key) => key,
            Err(err) => {
                println!("failed to serialize the RSA key: {err}");
                String::new()
            }
        }
    }

    async fn open_user_session(&mut self, user: &str, password: Vec<u8>) -> u32 {
        println!("Requested session for user '{user}' to be opened");

        let source = login_ng::storage::StorageSource::Username(String::from(user));

        let password = match self.priv_key.decrypt(Pkcs1v15Encrypt, &password) {
            Ok(password) => {
                // TODO: defeat replay attacks!!!

                password
            }
            Err(err) => {
                eprintln!("Failed to decrypt data: {err}");
                return 2u32;
            }
        };

        let user_mounts = match load_user_mountpoints(&source) {
            Ok(user_cfg) => user_cfg,
            Err(err) => {
                eprintln!("Failed to load user mount data: {err}");
                return 3u32;
            }
        };

        println!("Mountpoints for user '{user}' loaded");

        // TODO: check for the mount to be approved by root
        // otherwise the user might mount everything he wants to
        // with every dmask, potentially compromising the
        // security and integrity of the whole system.

        let Some(user) = get_user_by_name(user) else {
            // cannot identify user
            return 7u32;
        };

        let mut mounted_devices = vec![];

        // mount every directory in order or throw an error
        if let Some(mounts) = user_mounts {
            let staged_mounts = mounts.foreach(|a, b| {
                (
                    b.fstype().clone(),
                    b.flags().join(",").clone(),
                    b.device().clone(),
                    a.clone(),
                )
            });

            for m in staged_mounts.iter() {
                match mount(m.clone()) {
                    Ok(mount) => {
                        println!(
                            "Mounted device {} into {} for user '{}'",
                            m.2.as_str(),
                            m.3.as_str(),
                            user.name().to_string_lossy()
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
                        return 4u32;
                    }
                }
            }

            match mount((
                mounts.mount().fstype().clone(),
                mounts.mount().flags().join(","),
                mounts.mount().device().clone(),
                user.home_dir().as_os_str().to_string_lossy().to_string(),
            )) {
                Ok(mount) => {
                    println!(
                        "Mounted device {} on home directory for user '{}'",
                        mounts.mount().device().as_str(),
                        user.name().to_string_lossy()
                    );

                    // Make the mount temporary, so that it will be unmounted on drop.
                    mounted_devices.push(mount.into_unmount_drop(UnmountFlags::DETACH));
                }
                Err(err) => {
                    eprintln!("failed to mount user directory: {err}");
                    return 4u32;
                }
            }
        }

        println!(
            "Successfilly mounted {} device for user '{}'",
            mounted_devices.len(),
            user.name().to_string_lossy()
        );

        let mut guard = self.sessions.lock().await;
        guard.insert(
            user.name().to_os_string(),
            UserSession {
                mounts: mounted_devices,
            },
        );

        println!(
            "Successfilly opened session for user '{}'",
            user.name().to_string_lossy()
        );

        0u32 // OK
    }

    async fn close_user_session(&mut self, user: &str) -> u32 {
        println!("Requested session for user '{user}' to be closed");

        let Some(user) = get_user_by_name(user) else {
            // cannot identify user
            return 7u32;
        };

        let mut guard = self.sessions.lock().await;
        if !guard.contains_key(user.name()) {
            // session already closed
            return 6u32;
        }

        let session = guard.remove(user.name());
        drop(session);

        println!(
            "Successfilly closed session for user '{}'",
            user.name().to_string_lossy()
        );

        0u32
    }
}

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    println!("Starting pam support service of login_ng...");

    console_subscriber::init();

    if users::get_current_uid() != 0 {
        eprintln!("Application started without root privileges: aborting...");
        return Err(ServiceError::MissingPrivilegesError);
    }

    let service = Service::new();

    println!("Building the dbus object...");

    let _conn = connection::Builder::session()
        .map_err(|err| ServiceError::ZbusError(err))?
        .name("org.zbus.login_ng")
        .map_err(|err| ServiceError::ZbusError(err))?
        .serve_at("/org/zbus/login_ng", service)
        .map_err(|err| ServiceError::ZbusError(err))?
        .build()
        .await
        .map_err(|err| ServiceError::ZbusError(err))?;

    println!("Application running");

    // Do other things or go to wait forever
    pending::<()>().await;

    Ok(())
}

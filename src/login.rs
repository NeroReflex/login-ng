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

use std::path::{Path, PathBuf};

use configparser::ini::Ini;

use thiserror::Error;
use users::os::unix::UserExt;

use crate::{
    command::SessionCommand,
    storage::{load_user_session_command, StorageSource},
};

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum LoginResult {
    Success,
    Failure,
}

#[derive(Debug, Error)]
pub enum LoginError {
    #[cfg(feature = "greetd")]
    #[error("Error with greetd: {0}")]
    GreetdError(#[from] crate::greetd::GreetdLoginError),

    #[error("Error with pam: {0}")]
    PamError(#[from] crate::pam::PamLoginError),

    #[error("Username not recognised")]
    UserDiscoveryError,
}

pub trait LoginUserInteractionHandler {
    fn provide_username(&mut self, username: &String);

    fn prompt_secret(&mut self, msg: &String) -> Option<String>;

    fn prompt_plain(&mut self, msg: &String) -> Option<String>;

    fn print_info(&mut self, msg: &String);

    fn print_error(&mut self, msg: &String);
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionCommandRetrival {
    Defined(SessionCommand),
    AutodedectFromPath(PathBuf),
    AutodetectFromUserHome,
}

/// Interface that allows a user to authenticate and perform actions
pub trait LoginExecutor {
    /// Authenticate the user and execute the given command, or launch shell if one is not being provided.
    fn execute(
        &mut self,
        maybe_username: &Option<String>,
        retrival_strategy: &SessionCommandRetrival,
    ) -> Result<LoginResult, LoginError>;
}

pub(crate) fn load_session_from_conf(content: String) -> SessionCommand {
    let mut config = Ini::new();
    match config.read(content) {
        Ok(_) => match config.get("Session", "command") {
            Some(value) => SessionCommand::new(
                value.clone(),
                match config.get("Session", "arguments") {
                    Some(args) => args.split(" ").map(|arg| String::from(arg)).collect(),
                    None => vec![],
                },
            ),
            None => system_defined_with_crate_fallback(),
        },
        Err(_) => system_defined_with_crate_fallback(),
    }
}

pub(crate) fn system_defined_with_crate_fallback() -> SessionCommand {
    match std::fs::read_to_string(Path::new("/etc/login_ng/default_session.conf")) {
        Ok(content) => load_session_from_conf(content),
        Err(_) => SessionCommand::new(String::from(crate::DEFAULT_CMD), vec![]),
    }
}

pub(crate) fn user_default_command_with_system_fallback(username: &String) -> SessionCommand {
    match users::get_user_by_name(username) {
        Some(logged_user) => match logged_user.shell().to_str() {
            Some(path_str) => SessionCommand::new(String::from(path_str), vec![]),
            None => match logged_user.name().to_str() {
                Some(username_str) => match std::fs::read_to_string(Path::new(
                    format!("/etc/login_ng/{}.conf", username_str).as_str(),
                )) {
                    Ok(content) => load_session_from_conf(content),
                    Err(_) => system_defined_with_crate_fallback(),
                },
                None => system_defined_with_crate_fallback(),
            },
        },
        None => system_defined_with_crate_fallback(),
    }
}

pub(crate) fn retrieve_session_command_for_user(
    username: &String,
    retrival_strategy: &SessionCommandRetrival,
) -> SessionCommand {
    let storage_source = match retrival_strategy {
        SessionCommandRetrival::Defined(cmd) => return cmd.clone(),
        SessionCommandRetrival::AutodedectFromPath(path) => StorageSource::Path(path.clone()),
        SessionCommandRetrival::AutodetectFromUserHome => StorageSource::Username(username.clone()),
    };

    match load_user_session_command(&storage_source) {
        Ok(maybe_command) => match maybe_command {
            Some(session_cmd) => session_cmd,
            None => user_default_command_with_system_fallback(username),
        },
        Err(_err) => user_default_command_with_system_fallback(username),
    }
}

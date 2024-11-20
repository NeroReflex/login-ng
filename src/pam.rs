use std::{os::unix::process::CommandExt, path::Path, process::Command, sync::{Arc, Mutex}};

use pam_client2::{Context, Flag};
use thiserror::Error;

use crate::{conversation::ProxyLoginUserInteractionHandlerConversation, login::*};

use users::{get_user_by_name, os::unix::UserExt};

#[derive(Debug, Error)]
pub enum PamLoginError {
    #[error("Error setting login prompt: {0}")]
    SetPrompt(String),

    #[error("Error authenticating the user: {0}")]
    Authentication(String),

    #[error("Error validating the user: ")]
    Validation(String),

    #[error("Error opening session: {0}")]
    Open(String),

    #[error("Error obtaining the user from PAM: {0}")]
    GetUser(String),

    #[error("Error executing command: ")]
    Execution(String),

    #[error("Unable to find the username")]
    UnknownUsername,
}


pub struct PamLoginExecutor {
    conversation: ProxyLoginUserInteractionHandlerConversation
}

impl PamLoginExecutor {
    pub fn new (conversation: ProxyLoginUserInteractionHandlerConversation) -> Self {
        Self {
            conversation
        }
    }
}

impl LoginExecutor for PamLoginExecutor {

    fn prompt(&self) -> Arc<Mutex<dyn crate::login::LoginUserInteractionHandler>> {
        //Arc::new(Mutex::new(self.conversation.clone()))
        todo!()
    }

    fn execute(&mut self, maybe_username: &Option<String>, cmd: &Option<String>) -> Result<LoginResult, LoginError> {

        let user_prompt = Some("username: ");

        let mut context = Context::new(
            "system-login",
            maybe_username.as_ref().map(|a| a.as_str()),
            self.conversation.clone()
        ).expect("Failed to initialize PAM context");
    
        context.set_user_prompt(user_prompt).map_err(|err| LoginError::PamError(PamLoginError::SetPrompt(err.to_string())))?;
    
        // Authenticate the user (ask for password, 2nd-factor token, fingerprint, etc.)
        context.authenticate(Flag::NONE).map_err(|err| LoginError::PamError(PamLoginError::Authentication(err.to_string())))?;
    
        // Validate the account (is not locked, expired, etc.)
        context.acct_mgmt(Flag::NONE).map_err(|err| LoginError::PamError(PamLoginError::Validation(err.to_string())))?;
        
        // Get resulting user name and map to a user id
        let username = context.user().map_err(|err| LoginError::PamError(PamLoginError::GetUser(err.to_string())))?;
        let logged_user = get_user_by_name(&username).ok_or(LoginError::UserDiscoveryError)?;

        // Open session and initialize credentials
        let session = context.open_session(Flag::NONE).map_err(|err| LoginError::PamError(PamLoginError::Open(err.to_string())))?;

        let command = match &cmd {
            Some(cmd) => cmd.clone(),
            None => format!("{}", logged_user.shell().to_str().map_or(String::from("/bin/sh"), |shell| shell.to_string())),
        };

        // Run a process in the PAM environment
        let _result = Command::new(command)
            .env_clear()
            .envs(session.envlist().iter_tuples())
            .uid(logged_user.uid())
            .gid(logged_user.primary_group_id())
            //.groups(logged_user.groups().unwrap_or(vec![]).iter().map(|g| g.gid()).collect::<Vec<u32>>().as_slice())
            .current_dir(
                match logged_user.home_dir().exists() {
                    true => logged_user.home_dir(),
                    false => Path::new("/")
                }
                
            )
            .status()
            .map_err(|err| LoginError::PamError(PamLoginError::Execution(err.to_string())))?;

        Ok(LoginResult::Success)
    }
    
}
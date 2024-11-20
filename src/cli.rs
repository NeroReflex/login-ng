use std::{ffi::{CStr, CString}, path::Path, sync::{Arc, Mutex}};

use pam_client2::{ConversationHandler, ErrorCode};
use rpassword::prompt_password;

use crate::{conversation::*, login::LoginUserInteractionHandler, prompt_stderr, user::User};

pub struct TrivialCommandLineConversationPrompter {
    plain: Option<String>,
    hidden: Option<String>
}

impl TrivialCommandLineConversationPrompter {
    pub fn new(
        plain: Option<String>,
        hidden: Option<String>
    ) -> Self {
        Self {
            plain,
            hidden
        }
    }
}

impl ConversationPrompter for TrivialCommandLineConversationPrompter {
    fn echo_on_prompt(&mut self, _prompt: &String) -> Option<String> {
        self.plain.clone()
    }

    fn echo_off_prompt(&mut self, _prompt: &String) -> Option<String> {
        self.hidden.clone()
    }
    
    fn display_info(&mut self, prompt: &String) {
        println!("{:?}", prompt)
    }
    
    fn display_error(&mut self, prompt: &String) {
        eprintln!("{:?}", prompt)
    }
}

pub struct CommandLineConversation {
    answerer: Option<Arc<Mutex<dyn ConversationPrompter>>>,
    recorder: Option<Arc<Mutex<dyn ConversationRecorder>>>,
}

impl CommandLineConversation {
    /// Creates a new null conversation handler
	#[must_use]
	pub fn new(
        answerer: Option<Arc<Mutex<dyn ConversationPrompter>>>,
        recorder: Option<Arc<Mutex<dyn ConversationRecorder>>>
    ) -> Self {
		Self {
            answerer,
            recorder
        }
	}

    pub fn attach_recorder(&mut self, recorder: Arc<Mutex<dyn ConversationRecorder>>) {
        self.recorder = Some(recorder)
    }
}

impl Default for CommandLineConversation {
	fn default() -> Self {
		Self::new(None, None)
	}
}

impl ConversationHandler for CommandLineConversation {
	fn prompt_echo_on(&mut self, msg: &CStr) -> Result<CString, ErrorCode> {
        let prompt = format!("{}", msg.to_string_lossy());

		let response: String = match self.answerer {
            Some(ref ans) => match ans.lock() {
                Ok(mut guard) => match guard.echo_on_prompt(&prompt) {
                    Some(answer) => answer,
                    None => prompt_stderr(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
                },
                Err(_) => prompt_stderr(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
            },
            None => prompt_stderr(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
        };

        if let Some(recorder) = &self.recorder {
            if let Ok(mut guard) = recorder.lock() {
                guard.record_echo_on(prompt, response.clone());
            }
        }

        Ok(CString::new(response).map_err(|_err| ErrorCode::CONV_ERR)?)
	}

	fn prompt_echo_off(&mut self, msg: &CStr) -> Result<CString, ErrorCode> {
		let prompt = format!("{}", msg.to_string_lossy());

        let response: String = match self.answerer {
            Some(ref ans) => match ans.lock() {
                Ok(mut guard) => match guard.echo_off_prompt(&prompt) {
                    Some(answer) => answer,
                    None => prompt_password(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
                },
                Err(_) => prompt_password(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
            },
            None => prompt_password(prompt.as_str()).map_err(|_err| ErrorCode::CONV_ERR)?
        };

        if let Some(recorder) = &self.recorder {
            if let Ok(mut guard) = recorder.lock() {
                guard.record_echo_off(prompt, response.clone());
            }
        }

        Ok(CString::new(response).map_err(|_err| ErrorCode::CONV_ERR)?)
	}

	fn text_info(&mut self, msg: &CStr) {
        let string = format!("{}", msg.to_string_lossy());

        match self.answerer {
            Some(ref ans) => match ans.lock() {
                Ok(mut guard) => guard.display_info(&string),
                Err(_) => {}
            },
            None => {}
        };
    }

	fn error_msg(&mut self, msg: &CStr) {
        let string = format!("{}", msg.to_string_lossy());

        match self.answerer {
            Some(ref ans) => match ans.lock() {
                Ok(mut guard) => guard.display_info(&string),
                Err(_) => {}
            },
            None => {}
        };
    }
}

pub struct CommandLineLoginUserInteractionHandler {

    attempt_autologin: bool,

    maybe_username: Option<String>,

    maybe_user: Option<User>,

}

fn attempt_load_user(username: &String) -> Option<User> {
    let file_path = format!("/etc/login-ng/{}.json", &username);
    match Path::new(&file_path).exists() {
        true => match User::load_from_file(&file_path) {
            Ok(user_cfg) => Some(user_cfg),
            Err(_err) => None,
        },
        false => None,
    }
}

impl CommandLineLoginUserInteractionHandler {

    pub fn new(
        attempt_autologin: bool,
        maybe_username: Option<String>
    ) -> Self {
        Self {
            attempt_autologin,
            maybe_username: maybe_username.clone(),
            maybe_user: match &maybe_username {
                Some(username) => attempt_load_user(username),
                None => None
            }
        }
    }

}

impl Default for CommandLineLoginUserInteractionHandler {
    fn default() -> Self {
        Self { attempt_autologin: bool::default(), maybe_username: Default::default(), maybe_user: Default::default() }
    }
}

impl LoginUserInteractionHandler for CommandLineLoginUserInteractionHandler {

    fn provide_username(&mut self, username: &String) {
        self.maybe_user = attempt_load_user(username)
    }

    fn prompt_secret(&mut self, msg: &String) -> Option<String> {
        if self.attempt_autologin {
            if let Some(user_cfg) = &self.maybe_user {
                if let Ok(main_password) = user_cfg.main_by_auth(&Some(String::new())) {
                    return Some(main_password)
                }
            }
        }

        match prompt_password(msg.as_str()) {
            Ok(provided_secret) => match &self.maybe_user {
                Some(user_cfg) => match user_cfg.main_by_auth(&Some(provided_secret.clone())) {
                    Ok(main_password) => Some(main_password),
                    Err(_) => Some(provided_secret)
                },
                None => Some(provided_secret)
            },
            Err(_) => None
        }
    }

    fn prompt_plain(&mut self, msg: &String) -> Option<String> {
        match &self.maybe_username {
            Some(username) => Some(username.clone()),
            None => match prompt_stderr(msg.as_str()) {
                Ok(response) => Some(response),
                Err(_) => None
            },
        }
    }

    fn print_info(&mut self, msg: &String) {
        println!("{:?}", msg)
    }

    fn print_error(&mut self, msg: &String) {
        eprintln!("{:?}", msg)
    }
}
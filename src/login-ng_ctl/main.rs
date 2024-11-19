use std::sync::Arc;
use std::sync::Mutex;

use login_ng::conversation::*;
use login_ng::user::*;
use login_ng::prompt_password;

use pam_client2::{Context, Flag};

use std::path::Path;
use argh::FromArgs;

#[derive(FromArgs, PartialEq, Debug)]
/// Command line tool for managing login-ng authentication methods
struct Args {
    #[argh(option, short = 'u')]
    /// username
    user: Option<String>,

    #[argh(option, short = 'p')]
    /// main password for authentication (the one accepted by PAM)
    password: Option<String>,

    #[argh(subcommand)]
    command: Command,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
/// Subcommands for managing authentication methods
enum Command {
    Add(AddAuthCommand)
}

#[derive(FromArgs, PartialEq, Debug)]
/// Command to add a new authentication method
#[argh(subcommand, name = "add")]
struct AddAuthCommand {
    #[argh(option)]
    /// intermediate key (the key used to unlock the main password)
    intermediate: Option<String>,

    #[argh(subcommand)]
    method: AddAuthMethod,
}

#[derive(FromArgs, PartialEq, Debug)]
#[argh(subcommand)]
/// Subcommands for adding an authentication method
enum AddAuthMethod {
    Password(AddAuthPasswordCommand)
}

#[derive(FromArgs, PartialEq, Debug)]
/// Command to add a new authentication method
#[argh(subcommand, name = "password")]
struct AddAuthPasswordCommand {
    #[argh(option)]
    /// secondary password for authentication
    secondary_pw: Option<String>,
}

fn main() {
    let args: Args = argh::from_env();

    let user_prompt = Some("username: ");

    let answerer = Arc::new(
        Mutex::new(
            SimpleConversationPromptAnswerer::new(
                args.user.clone(),
                args.password.clone(),
            )
        )
    );

    let interaction_recorder = Arc::new(
        Mutex::new(
            SimpleConversationRecorder::new()
        )
    );

    let conversation = Conversation::new(Some(answerer), Some(interaction_recorder.clone()));

    let mut context = Context::new(
        "system-login",
        args.user.as_deref(),
        conversation
    ).expect("Failed to initialize PAM context");

    context.set_user_prompt(user_prompt).unwrap();

    // Authenticate the user (ask for password, 2nd-factor token, fingerprint, etc.)
    context.authenticate(Flag::NONE).expect("Authentication failed");

    // Validate the account (is not locked, expired, etc.)
    context.acct_mgmt(Flag::NONE).expect("Account validation failed");

    let username = args.user.clone().unwrap_or_else(|| {
        interaction_recorder.lock().unwrap().recorded_username(&user_prompt).unwrap()
    });

    let file_path = format!("/etc/login-ng/{}.json", &username);
    let mut user_cfg = match Path::new(&file_path).exists() {
        true => match User::load_from_file(&file_path) {
            Ok(user_cfg) => user_cfg,
            Err(err) => {
                eprintln!("There is a problem loading your configuration file: {}.\nAborting.", err);
                std::process::exit(-1)
            }
        },
        false => {
            User::new()
        }
    };

    match args.command {
        Command::Add(add_cmd) => {
            let intermediate_password = add_cmd.intermediate.clone().unwrap_or_else(|| {
                prompt_password("Intermediate key:").expect("Failed to read intermediate key")
            });

            if user_cfg.has_main() {
                if let Err(err) = user_cfg.main_by_auth(&Some(intermediate_password.clone())) {
                    eprintln!("Could not verify the correctness of the intermediate key: {}", err);
                    std::process::exit(-1)
                }
            }

            // if the main password is accepted update the stored one
            if let Some(main_password) = interaction_recorder.lock().unwrap().recorded_password() {
                user_cfg.set_main(&main_password, &intermediate_password).expect("Error handling main password");
            }

            match add_cmd.method {
                AddAuthMethod::Password(add_auth_password_command) =>  {
                    let secondary_password = match add_auth_password_command.secondary_pw {
                        Some(secondary_password) => secondary_password,
                        None => {
                            let secondary_password = prompt_password("Secondary password:").expect("Failed to read secondary password");
    
                            let repeat = prompt_password("Secondary password (repeat):").expect("Failed to read secondary password (repeat)");
                            if secondary_password != repeat {
                                println!("Passwords do not match.\nAborting.");
                                std::process::exit(-1)
                            }
    
                            secondary_password
                        }
                    };

                    if !user_cfg.has_main() {
                        println!("Cannot add a secondary password for an account with no main password.\nAborting.");
                        std::process::exit(-1);
                    }
    
                    match user_cfg.add_secondary_password(&intermediate_password, &secondary_password) {
                        Ok(_) => {
                            user_cfg.store_to_file(Path::new(&file_path)).expect("Error saving the updated configuration");
                            println!("Secondary password added.");
                        },
                        Err(err) => {
                            println!("Error adding a secondary password: {}.\nAborting.", err);
                        }
                    }
                },
            }
        }
    }
}
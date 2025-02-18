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

pub mod auth;
pub mod command;
pub mod error;
pub mod storage;
pub mod user;

pub extern crate users;

#[cfg(test)]
pub(crate) mod tests;

extern crate bytevec2;

pub const DEFAULT_XATTR_NAME: &str = "user.login-ng";

use std::io::BufRead;

use hkdf::*;
use sha2::Sha256;

pub const LIBRARY_VERSION: &str = env!("CARGO_PKG_VERSION");

pub use rpassword::prompt_password;

pub(crate) fn derive_key(input: &str, salt: &[u8]) -> [u8; 32] {
    // Create an HKDF instance with SHA-256 as the hash function
    let hkdf = Hkdf::<Sha256>::new(Some(salt), input.as_bytes());

    // Prepare a buffer for the derived key
    let mut okm = [0u8; 32]; // Output key material (32 bytes)

    // Extract the key material
    hkdf.expand(&[], &mut okm).expect("Failed to expand key");

    okm
}

pub(crate) fn password_to_vec(password: &String) -> Vec<u8> {
    password.as_str().into()
}

pub(crate) fn vec_to_password(vec: &Vec<u8>) -> String {
    String::from_utf8_lossy(vec.as_slice()).to_string()
}

// this MUST be implemented and used because entering invalid strings can be a security hole (see lossy_utf8)
pub(crate) fn is_valid_password(password: &String) -> bool {
    vec_to_password(password_to_vec(password).as_ref()) == password.clone()
}

/// Reads a password from the TTY
fn read_plain() -> std::io::Result<String> {
    let tty = std::fs::File::open("/dev/tty")?;
    let mut reader = std::io::BufReader::new(tty);

    let mut answer = String::new();
    reader.read_line(&mut answer)?;

    fix_line_issues(answer)
}

/// Normalizes the return of `read_line()` in the context of a CLI application
fn fix_line_issues(mut line: String) -> std::io::Result<String> {
    if !line.ends_with('\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "unexpected end of file",
        ));
    }

    // Remove the \n from the line.
    line.pop();

    // Remove \r and \n from the line if present
    if (line.ends_with('\r')) || (line.ends_with('\n')) {
        line.pop();
    }

    // Ctrl-U should remove the line in terminals
    if line.contains('') {
        line = match line.rfind('') {
            Some(last_ctrl_u_index) => line[last_ctrl_u_index + 1..].to_string(),
            None => line,
        };
    }

    Ok(line)
}

pub fn prompt_stderr(prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    use std::io::Write;

    let mut stream = std::fs::OpenOptions::new().write(true).open("/dev/tty")?;

    Ok(
        stream
            .write_all(prompt.to_string().as_str().as_bytes())
            .and_then(|_| stream.flush())
            .and_then(|_| read_plain())
            .map_err(|err| Box::new(err))?
    )
}

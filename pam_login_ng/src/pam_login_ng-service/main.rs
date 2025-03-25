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

extern crate tokio;

use std::fs::File;
use std::io::Read;

use tokio::signal::unix::{signal, SignalKind};

use pam_login_ng_common::{
    login_ng::users, zbus::connection,
    dbus::{Service, ServiceError},
};

#[tokio::main]
async fn main() -> Result<(), ServiceError> {
    println!("Reading the private key...");
    let file_path = "/etc/login_ng/private_key_pkcs8.pem";
    let mut file = File::open(file_path)?;
    let mut contents = String::new();
    let read = file.read_to_string(&mut contents)?;
    println!("Read private key file of {read} bytes");

    if users::get_current_uid() != 0 {
        eprintln!("Application started without root privileges: aborting...");
        return Err(ServiceError::MissingPrivilegesError);
    }

    match std::env::var("DBUS_SESSION_BUS_ADDRESS") {
        Ok(value) => println!("Starting dbus service on socket {value}"),
        Err(err) => {
            eprintln!("Couldn't read dbus socket address: {err} - using default...");
            std::env::set_var(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/run/dbus/system_bus_socket",
            );
        }
    }

    println!("Building the dbus object...");

    let dbus_conn = connection::Builder::session()
        .map_err(|err| ServiceError::ZbusError(err))?
        .name("org.zbus.login_ng")
        .map_err(|err| ServiceError::ZbusError(err))?
        .serve_at("/org/zbus/login_ng", Service::new(contents.as_str()))
        .map_err(|err| ServiceError::ZbusError(err))?
        .build()
        .await
        .map_err(|err| ServiceError::ZbusError(err))?;

    println!("Application running");

    // Create a signal listener for SIGTERM
    let mut sigterm =
        signal(SignalKind::terminate()).expect("Failed to create SIGTERM signal handler");

    // Wait for a SIGTERM signal
    sigterm.recv().await;

    Ok(drop(dbus_conn))
}

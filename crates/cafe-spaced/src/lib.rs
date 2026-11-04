pub mod api;
pub mod host;
mod migrations;
pub mod runtime;
mod sessions;
pub mod state;
pub mod update;

use cafe_space_ipc::{BuildFlavor, local};
use std::io;

pub fn run(flavor: BuildFlavor) -> io::Result<std::process::ExitCode> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let check_state = match arguments.as_slice() {
        [] => false,
        [flag] if flag == "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return Ok(std::process::ExitCode::SUCCESS);
        }
        [flag] if flag == "--check-state" => true,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "start without flags; only --version and the read-only --check-state are supported",
            ));
        }
    };
    let root = flavor.data_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user data directory is unavailable",
        )
    })?;
    if check_state {
        return state::State::check_compatible(&root, flavor)
            .map(|()| std::process::ExitCode::SUCCESS);
    }
    let endpoint = local::endpoint(flavor)?;
    if let Some(parent) = root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(runtime::serve(
            flavor,
            &root,
            &endpoint,
            runtime::shutdown_signal(),
        ))
        .map(|reason| match reason {
            runtime::StopReason::Signal => std::process::ExitCode::SUCCESS,
            runtime::StopReason::Update => std::process::ExitCode::from(42),
        })
}

pub mod api;
pub mod runtime;
pub mod state;

use cafe_space_ipc::{BuildFlavor, local};
use std::io;

pub fn run(flavor: BuildFlavor) -> io::Result<()> {
    if std::env::args_os().len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "daemon takes no arguments; start it without flags",
        ));
    }
    let root = flavor.data_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user data directory is unavailable",
        )
    })?;
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
}

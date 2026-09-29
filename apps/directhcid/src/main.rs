#[cfg(windows)]
mod ipc;
#[cfg(windows)]
mod runtime;
#[cfg(windows)]
mod service;

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    match service::dispatch(std::env::args().skip(1).collect()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("directhcid: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn main() -> std::process::ExitCode {
    eprintln!("directhcid is only available on Windows");
    std::process::ExitCode::FAILURE
}

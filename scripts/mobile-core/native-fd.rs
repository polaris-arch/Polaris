// Node owns the descriptors and admission. These operations act on the
// explicitly delegated Unix file description, which remains open in Node.
use std::fs::{File, TryLockError};
use std::io::{self, Read, Write};
use std::os::fd::FromRawFd;
use std::process::ExitCode;

unsafe extern "C" {
    fn fcntl(fd: i32, command: i32, ...) -> i32;
}

fn operation(file: &mut File, command: &str) -> Result<(), TryLockError> {
    match command {
        "exclusive" => file.try_lock(),
        "shared" => file.try_lock_shared(),
        "progress" => progress(file).map_err(TryLockError::Error),
        _ => Err(TryLockError::Error(io::Error::other(
            "unknown native descriptor operation",
        ))),
    }
}

fn progress(file: &mut File) -> io::Result<()> {
    let mut row = Vec::new();
    io::stdin().take(4097).read_to_end(&mut row)?;
    if row.is_empty() || row.len() > 4096 {
        return Err(io::Error::other("invalid progress size"));
    }
    file.write_all(&row)
}

fn main() -> ExitCode {
    let mut args = std::env::args();
    let Some(command) = args.nth(1) else {
        return ExitCode::FAILURE;
    };
    if args.next().is_some() {
        return ExitCode::FAILURE;
    }
    // F_GETFD (1 on Linux and Darwin) verifies the explicitly delegated
    // descriptor before creating its Rust owner.
    if unsafe { fcntl(3, 1) } < 0 {
        return ExitCode::FAILURE;
    }
    let mut file = unsafe { File::from_raw_fd(3) };
    match operation(&mut file, &command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(TryLockError::WouldBlock) => ExitCode::from(2),
        Err(TryLockError::Error(error)) => {
            eprintln!("native descriptor operation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().len() != 1 {
        return Err("packaging custody admits requests only through the owned pipe".into());
    }
    #[cfg(windows)]
    {
        polaris_windows_cleaner::package_custody::serve()?;
        Ok(())
    }
    #[cfg(not(windows))]
    Err("native Windows packaging custody required".into())
}

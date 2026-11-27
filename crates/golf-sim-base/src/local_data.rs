use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const VARIABLE: &str = "GOLF_SIM_LOCAL_DATA";

pub fn workspace_testdata() -> PathBuf {
    let testdata = Path::new("testdata");
    if testdata.join("validation/targets/LICENCES.toml").is_file() {
        testdata.to_path_buf()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
    }
}

pub fn resolve() -> io::Result<Option<PathBuf>> {
    let configured = env::var_os(VARIABLE);
    let path = match &configured {
        Some(value) if value.is_empty() => return Ok(None),
        Some(value) => PathBuf::from(value),
        None => workspace_testdata().join("local"),
    };
    match fs::metadata(&path) {
        Ok(info) if info.is_dir() => Ok(Some(path)),
        Ok(_) => Err(io::Error::other(format!(
            "{VARIABLE}: {} is not a directory",
            path.display()
        ))),
        Err(error)
            if configured.is_none()
                && error.kind() == io::ErrorKind::NotFound
                && fs::symlink_metadata(&path)
                    .is_err_and(|link| link.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{VARIABLE}: {}: {error}", path.display()),
        )),
    }
}

use std::{env, fs, io, path::Path};

fn main() -> Result<(), io::Error> {
    let mut arguments = env::args_os().skip(1);
    let path = arguments
        .next()
        .filter(|_| arguments.next().is_none())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "one artifact is required"))?;
    let path = Path::new(&path);
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact is not a regular file",
        ));
    }
    let bytes = fs::read(path)?;
    println!("{}", blake3::hash(&bytes).to_hex());
    Ok(())
}

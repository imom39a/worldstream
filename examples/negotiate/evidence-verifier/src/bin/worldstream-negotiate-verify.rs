use std::{env, error::Error, fs, io, path::PathBuf};

use worldstream_negotiate_evidence::{VerifierTrustV1, verify_package};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<String> = env::args().skip(1).collect();
    if arguments.is_empty() {
        return Err(io::Error::other(
            "usage: worldstream-negotiate-verify PROOF.json [--pack BUNDLE.wspack] [--trust TRUST.json]",
        )
        .into());
    }
    let proof_path = PathBuf::from(&arguments[0]);
    let mut pack_path = None;
    let mut trust_path = None;
    let mut index = 1;
    while index < arguments.len() {
        let flag = &arguments[index];
        let Some(value) = arguments.get(index + 1) else {
            return Err(io::Error::other(format!("missing value for {flag}")).into());
        };
        match flag.as_str() {
            "--pack" => pack_path = Some(PathBuf::from(value)),
            "--trust" => trust_path = Some(PathBuf::from(value)),
            _ => return Err(io::Error::other(format!("unknown argument {flag}")).into()),
        }
        index += 2;
    }

    let proof = fs::read(proof_path)?;
    let pack = pack_path.map(fs::read).transpose()?;
    let trust = match trust_path {
        Some(path) => serde_json::from_slice(&fs::read(path)?)?,
        None => VerifierTrustV1::default(),
    };
    let report = verify_package(&proof, &trust, pack.as_deref())?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

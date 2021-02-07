use std::{error::Error, fs, process};

fn main() {
    const FILENAME: &str = "Hocfile.default.yaml";

    let data = fs::read(FILENAME).unwrap_or_else(|err| {
        eprintln!("- {}", err);
        process::exit(1);
    });

    match hocfile::Hocfile::from_slice(&data) {
        Ok(_) => (),
        Err(errors) => {
            for error in errors {
                if error.source().is_none() {
                    eprintln!("- {}", error);
                } else {
                    eprintln!("- 1: {}", error);
                    let mut err_chain = error.source();
                    let mut i = 2;
                    while let Some(err) = err_chain {
                        eprintln!("  {}: {}", i, err);
                        err_chain = err.source();
                        i += 1;
                    }
                }
            }
            process::exit(1);
        }
    }
}

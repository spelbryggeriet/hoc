use std::{fs::File, io::Write, os::unix::prelude::OpenOptionsExt};

use hoc_core::kv::{ReadStore, WriteStore};
use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use hoc_log::{hidden_input, info, status, LogErr, Result};
use hoc_macros::{Procedure, ProcedureState};

#[derive(Procedure, StructOpt)]
pub struct CreateUser {
    #[procedure(attribute)]
    username: String,

    #[structopt(skip)]
    password: Option<String>,
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum CreateUserState {
    #[state(transient)]
    ChoosePassword,

    #[state(finish)]
    GenerateSshKeyPair,
}

impl Run for CreateUserState {
    fn choose_password(
        proc: &mut CreateUser,
        _proc_registry: &impl ReadStore,
        _global_registry: &impl ReadStore,
    ) -> Result<Self> {
        proc.password = Some(hidden_input!("Choose a password").verify().get()?);

        Ok(GenerateSshKeyPair)
    }

    fn generate_ssh_key_pair(
        proc: &mut CreateUser,
        proc_registry: &impl WriteStore,
        _global_registry: &impl ReadStore,
    ) -> Result<()> {
        let (pub_key, priv_key) = status!("Generate SSH keypair").on(|| {
            let mut key_pair = KeyPair::generate(KeyType::ED25519, 256).log_err()?;
            *key_pair.comment_mut() = proc.username.clone();

            let password = proc.password.clone().unwrap();
            let pub_key = key_pair.serialize_publickey().log_err()?;
            let priv_key = key_pair
                .serialize_openssh(Some(&password), Cipher::Aes256_Ctr)
                .log_err()?;

            let randomart = key_pair
                .fingerprint_randomart(FingerprintHash::SHA256)
                .log_err()?;

            info!("Fingerprint randomart:\n{}", randomart);

            hoc_log::Result::Ok((pub_key, priv_key))
        })?;

        status!("Store SSH keypair").on(|| {
            let pub_ref = proc_registry.create_file(format!("ssh/id_ed25519.pub"))?;
            let priv_ref = proc_registry.create_file(format!("ssh/id_ed25519"))?;
            let mut pub_file = File::options()
                .write(true)
                .create(true)
                .mode(0o600)
                .open(pub_ref.path())?;
            let mut priv_file = File::options()
                .write(true)
                .create(true)
                .mode(0o600)
                .open(priv_ref.path())?;
            pub_file.write_all(pub_key.as_bytes())?;
            priv_file.write_all(priv_key.as_bytes())?;

            info!("Key stored in {}", priv_ref.path().to_string_lossy());

            hoc_log::Result::Ok(())
        })?;

        Ok(())
    }
}

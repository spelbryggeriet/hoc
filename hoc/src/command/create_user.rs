use std::{
    fs::{self, File},
    io::Write,
    os::unix::prelude::OpenOptionsExt,
};

use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};
use structopt::StructOpt;

use hoclib::DirState;
use hoclog::{hidden_input, info, status, LogErr, Result};
use hocproc::procedure;

procedure! {
    #[derive(StructOpt)]
    pub struct CreateUser {
        #[procedure(attribute)]
        username: String,

        #[structopt(skip)]
        password: Option<String>,
    }

    pub enum CreateUserState {
        #[procedure(transient)]
        ChoosePassword,

        #[procedure(finish)]
        GenerateSshKeyPair,
    }
}

impl Run for CreateUserState {
    fn choose_password(proc: &mut CreateUser, _work_dir_state: &DirState) -> Result<Self> {
        proc.password = Some(hidden_input!("Choose a password").verify().get()?);

        Ok(GenerateSshKeyPair)
    }

    fn generate_ssh_key_pair(proc: &mut CreateUser, work_dir_state: &mut DirState) -> Result<()> {
        info!("Storing username: {}", &proc.username);
        let username_file_path = work_dir_state.track_file(format!("username.txt"));
        fs::write(username_file_path, &proc.username)?;

        let (pub_key, priv_key) = status!("Generate SSH keypair" => {
            let mut key_pair = KeyPair::generate(KeyType::ED25519, 256).log_err()?;
            *key_pair.comment_mut() = proc.username.clone();

            let password = proc.password.clone().unwrap();
            let pub_key = key_pair.serialize_publickey().log_err()?;
            let priv_key = key_pair.serialize_openssh(Some(&password), Cipher::Aes256_Ctr).log_err()?;

            let randomart = key_pair.fingerprint_randomart(
                FingerprintHash::SHA256,
            ).log_err()?;

            info!("Fingerprint randomart:\n{}", randomart);

            (pub_key, priv_key)
        });

        status!("Store SSH keypair" => {
            let ssh_dir = work_dir_state.track_file("ssh");
            fs::create_dir_all(ssh_dir)?;

            let username = &proc.username;
            let pub_path = work_dir_state.track_file(format!("ssh/id_{username}_ed25519.pub"));
            let priv_path = work_dir_state.track_file(format!("ssh/id_{username}_ed25519"));
            let mut pub_file = File::options().write(true).create(true).mode(0o600).open(&pub_path)?;
            let mut priv_file = File::options().write(true).create(true).mode(0o600).open(&priv_path)?;
            pub_file.write_all(pub_key.as_bytes())?;
            priv_file.write_all(priv_key.as_bytes())?;

            info!("Key stored in {}", priv_path.to_string_lossy());
        });

        Ok(())
    }
}

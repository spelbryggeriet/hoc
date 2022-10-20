use std::net::IpAddr;

use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};

use crate::{cidr::Cidr, prelude::*, util::Secret};

#[throws(anyhow::Error)]
pub async fn run(
    node_addresses: Cidr,
    gateway: IpAddr,
    admin_username: String,
    admin_password: Secret<String>,
) {
    {
        progress_scoped!("Storing network information");

        put!(node_addresses.ip_addr.to_string() => "network/start_address").await?;
        put!(node_addresses.prefix_len => "network/prefix_len").await?;
        put!(gateway.to_string() => "network/gateway").await?;
    }

    {
        progress_scoped!("Storing administrator user");

        put!(admin_username.to_string() => "admin/username").await?;
    }

    let ssh_progress = progress!("Generating SSH key pair");

    let mut key_pair = KeyPair::generate(KeyType::ED25519, 256)?;
    *key_pair.comment_mut() = admin_username.clone();

    let pub_key = key_pair.serialize_publickey()?;
    let priv_key = key_pair.serialize_openssh(Some(&*admin_password), Cipher::Aes256_Ctr)?;

    let randomart = key_pair.fingerprint_randomart(FingerprintHash::SHA256)?;
    info!("Fingerprint randomart:\n{randomart}");

    ssh_progress.finish();
}

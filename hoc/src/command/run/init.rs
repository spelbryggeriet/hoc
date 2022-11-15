use std::net::IpAddr;

use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};
use tokio::{io::AsyncWriteExt, join};

use crate::{cidr::Cidr, prelude::*};

#[throws(anyhow::Error)]
pub async fn run(
    node_addresses: Cidr,
    gateway: IpAddr,
    admin_username: String,
    admin_password: Secret<String>,
) {
    store_network_info(node_addresses, gateway).await?;
    store_admin_user(admin_username.clone()).await?;
    let (pub_key, priv_key) = generate_ssh_keys(admin_username, admin_password.as_deref())?;
    store_ssh_keys(&pub_key, &priv_key).await?;
}

#[throws(anyhow::Error)]
async fn store_network_info(node_addresses: Cidr, gateway: IpAddr) {
    progress!("Storing network information");

    put!(node_addresses.ip_addr.to_string() => "network/start_address").await?;
    put!(node_addresses.prefix_len => "network/prefix_len").await?;
    put!(gateway.to_string() => "network/gateway").await?;
}

#[throws(anyhow::Error)]
async fn store_admin_user(admin_username: String) {
    progress!("Storing administrator user");

    put!(admin_username => "admin/username").await?;
}

#[throws(anyhow::Error)]
fn generate_ssh_keys(admin_username: String, admin_password: Secret<&str>) -> (String, String) {
    progress!("Generating SSH key pair");

    let mut key_pair = KeyPair::generate(KeyType::ED25519, 256)?;
    *key_pair.comment_mut() = admin_username;

    let pub_key = key_pair.serialize_publickey()?;
    let priv_key = key_pair.serialize_openssh(Some(&*admin_password), Cipher::Aes256_Ctr)?;

    let randomart = key_pair.fingerprint_randomart(FingerprintHash::SHA256)?;
    info!("Fingerprint randomart:\n{randomart}");

    (pub_key, priv_key)
}

#[throws(anyhow::Error)]
async fn store_ssh_keys(pub_key: &str, priv_key: &str) {
    progress!("Storing SSH key pair");

    let (pub_res, priv_res) = join!(
        create_file_with_content("admin/ssh/pub", pub_key),
        create_file_with_content("admin/ssh/priv", priv_key),
    );

    pub_res?;
    priv_res?;
}

#[throws(anyhow::Error)]
async fn create_file_with_content(key: &'static str, content: &str) {
    let (mut file, _) = context_file!("{key}").create().await?;
    file.write_all(content.as_bytes()).await?;
}

use std::{io::Write, net::IpAddr};

use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};

use crate::{cidr::Cidr, prelude::*};

#[throws(anyhow::Error)]
pub fn run(
    node_addresses: Cidr,
    gateway: IpAddr,
    admin_username: String,
    admin_password: Secret<String>,
    container_registry: String,
) {
    store_network_info(node_addresses, gateway)?;
    store_admin_user(admin_username.clone())?;
    let (pub_key, priv_key) = generate_ssh_keys(admin_username, admin_password.as_deref())?;
    store_ssh_keys(&pub_key, &priv_key)?;
    store_registry_info(container_registry)?;
}

#[throws(anyhow::Error)]
fn store_network_info(node_addresses: Cidr, gateway: IpAddr) {
    progress!("Storing network information");

    kv!("network/start_address").put(node_addresses.ip_addr.to_string())?;
    kv!("network/prefix_len").put(node_addresses.prefix_len)?;
    kv!("network/gateway").put(gateway.to_string())?;
}

#[throws(anyhow::Error)]
fn store_admin_user(admin_username: String) {
    progress!("Storing administrator user");

    kv!("admin/username").put(admin_username)?;
}

#[throws(anyhow::Error)]
fn generate_ssh_keys(admin_username: String, admin_password: Secret<&str>) -> (String, String) {
    progress!("Generating SSH key pair");

    let mut key_pair = KeyPair::generate(KeyType::ED25519, 256)?;
    *key_pair.comment_mut() = admin_username;

    let pub_key = key_pair.serialize_publickey()?;
    let priv_key = key_pair.serialize_openssh(Some(*admin_password), Cipher::Aes256_Ctr)?;

    let randomart = key_pair.fingerprint_randomart(FingerprintHash::SHA256)?;
    info!("Fingerprint randomart:\n{randomart}");

    (pub_key, priv_key)
}

#[throws(anyhow::Error)]
fn store_ssh_keys(pub_key: &str, priv_key: &str) {
    progress!("Storing SSH key pair");

    create_file_with_content("admin/ssh/pub", pub_key)?;
    create_file_with_content("admin/ssh/priv", priv_key)?;
}

#[throws(anyhow::Error)]
fn create_file_with_content(key: &'static str, content: &str) {
    let mut file = files!("{key}").create()?;
    file.write_all(content.as_bytes())?;
}

#[throws(anyhow::Error)]
fn store_registry_info(container_registry: String) {
    progress!("Storing container information");

    kv!("registry/prefix").put(container_registry)?;
}

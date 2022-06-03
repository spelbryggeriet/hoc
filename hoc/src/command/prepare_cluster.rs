use std::{fs::File, io::Write, net::IpAddr};

use hoc_core::kv;
use hoc_log::{bail, hidden_input, info, LogErr, Result};
use hoc_macros::{doc_status, Procedure, ProcedureState};
use osshkeys::{cipher::Cipher, keys::FingerprintHash, KeyPair, KeyType, PublicParts};
use serde::{Deserialize, Serialize};
use structopt::StructOpt;

use super::util::cidr::Cidr;

#[derive(Procedure, StructOpt)]
pub struct PrepareCluster {
    /// The name of the cluster.
    #[procedure(attribute)]
    cluster: String,

    /// The addresses to use for the cluster, where the IP address denote the starting address of
    /// the allocation range, and the prefix length denote the network subnet mask.
    #[structopt(long, required_if("os", "ubuntu"))]
    node_addresses: Cidr,

    /// The default gateway for the cluster.
    #[structopt(long)]
    gateway: IpAddr,

    /// The username of the administrator.
    #[structopt(long)]
    admin_username: String,
}

#[derive(ProcedureState, Serialize, Deserialize)]
pub enum PrepareClusterState {
    NetworkInfo,

    #[state(finish)]
    ClusterAdministrator,
}

#[doc_status]
impl Run for PrepareClusterState {
    fn network_info(proc: &mut PrepareCluster, registry: &impl kv::WriteStore) -> Result<Self> {
        let cluster = &proc.cluster;
        let addresses = &proc.node_addresses;
        let gateway = proc.gateway;

        if !addresses
            .contains(gateway)
            .log_context("validating gateway")?
        {
            bail!("gateway is outside of the subnet");
        }

        {
            //! Store network info

            info!("Start address: {}", addresses.ip_addr);
            registry.put(
                format!("clusters/{cluster}/network/start_address"),
                addresses.ip_addr.to_string(),
            )?;

            info!("Prefix length: {}", addresses.prefix_len);
            registry.put(
                format!("clusters/{cluster}/network/prefix_len"),
                addresses.prefix_len,
            )?;

            info!("Gateway: {gateway}");
            registry.put(
                format!("clusters/{cluster}/network/gateway"),
                gateway.to_string(),
            )?;
        }

        Ok(ClusterAdministrator)
    }

    fn cluster_administrator(
        proc: &mut PrepareCluster,
        registry: &impl kv::WriteStore,
    ) -> Result<()> {
        let cluster = &proc.cluster;
        let username = &proc.admin_username;
        let password = hidden_input!("Choose a password for the cluster administrator")
            .verify()
            .get();

        {
            //! Store administrator user

            info!("Username: {username}");
            registry.put(format!("clusters/{cluster}/admin/username"), username)?;
        }

        let (pub_key, priv_key) = {
            //! Generate SSH keypair

            let mut key_pair = KeyPair::generate(KeyType::ED25519, 256).log_err()?;
            *key_pair.comment_mut() = username.clone();

            let pub_key = key_pair.serialize_publickey().log_err()?;
            let priv_key = key_pair
                .serialize_openssh(Some(&*password), Cipher::Aes256_Ctr)
                .log_err()?;

            let randomart = key_pair
                .fingerprint_randomart(FingerprintHash::SHA256)
                .log_err()?;

            info!("Fingerprint randomart:\n{}", randomart);

            (pub_key, priv_key)
        };

        {
            //! Store SSH keypair

            let pub_ref = registry.create_file(format!("clusters/{cluster}/admin/ssh/pub"))?;
            let priv_ref = registry.create_file(format!("clusters/{cluster}/admin/ssh/priv"))?;
            let mut pub_file = File::options()
                .write(true)
                .create(true)
                .open(pub_ref.path())?;
            let mut priv_file = File::options()
                .write(true)
                .create(true)
                .open(priv_ref.path())?;
            pub_file.write_all(pub_key.as_bytes())?;
            priv_file.write_all(priv_key.as_bytes())?;
        }

        Ok(())
    }
}

use std::{fs, path::PathBuf};

use anyhow::{bail, Context};
use rcgen::{BasicConstraints, CertificateParams, DistinguishedName, IsCa, KeyPair};

const CA_CERT_FILE: &str = "ca.crt";
const CA_KEY_FILE: &str = "ca.key";

#[derive(Debug, Clone)]
pub struct CaStore {
    pub directory: PathBuf,
}

impl CaStore {
    pub fn default() -> anyhow::Result<Self> {
        let base = dirs::data_local_dir().context("cannot determine local data directory")?;
        Ok(Self {
            directory: base.join("httphunter"),
        })
    }

    pub fn cert_path(&self) -> PathBuf {
        self.directory.join(CA_CERT_FILE)
    }

    pub fn key_path(&self) -> PathBuf {
        self.directory.join(CA_KEY_FILE)
    }

    pub fn generate(&self, force: bool) -> anyhow::Result<()> {
        fs::create_dir_all(&self.directory)
            .with_context(|| format!("failed to create {}", self.directory.display()))?;
        if !force && (self.cert_path().exists() || self.key_path().exists()) {
            bail!(
                "CA files already exist in {}; use --force to replace them",
                self.directory.display()
            );
        }

        let mut params = CertificateParams::new(Vec::<String>::new())?;
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "httphunter local CA");
        let key_pair = KeyPair::generate()?;
        let certificate = params.self_signed(&key_pair)?;

        fs::write(self.cert_path(), certificate.pem())?;
        fs::write(self.key_path(), key_pair.serialize_pem())?;
        Ok(())
    }

    pub fn leaf_for_host(&self, host: &str) -> anyhow::Result<(Vec<u8>, Vec<u8>)> {
        let ca_cert_pem = fs::read_to_string(self.cert_path()).with_context(|| {
            format!(
                "failed to read CA certificate {}",
                self.cert_path().display()
            )
        })?;
        let ca_key_pem = fs::read_to_string(self.key_path()).with_context(|| {
            format!(
                "failed to read CA private key {}",
                self.key_path().display()
            )
        })?;
        let ca_key = KeyPair::from_pem(&ca_key_pem)?;
        let ca_params = CertificateParams::from_ca_cert_pem(&ca_cert_pem)?;
        let ca_cert = ca_params.self_signed(&ca_key)?;

        let mut leaf_params = CertificateParams::new(vec![host.to_owned()])?;
        leaf_params
            .distinguished_name
            .push(rcgen::DnType::CommonName, host);
        let leaf_key = KeyPair::generate()?;
        let leaf_cert = leaf_params.signed_by(&leaf_key, &ca_cert, &ca_key)?;
        Ok((leaf_cert.der().to_vec(), leaf_key.serialize_der()))
    }
}

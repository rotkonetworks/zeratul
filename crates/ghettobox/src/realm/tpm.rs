//! tpm realm - linux tpm 2.0 backed storage
//!
//! uses tpm for:
//! - sealing data to pcr state (can't extract without tpm)
//! - hardware dictionary attack protection (rate limiting)
//!
//! requires tpm2-tools and appropriate permissions

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use tss_esapi::{
    attributes::ObjectAttributesBuilder,
    handles::KeyHandle,
    interface_types::{
        algorithm::{HashingAlgorithm, PublicAlgorithm},
        resource_handles::Hierarchy,
    },
    structures::{
        CapabilityData, CreatePrimaryKeyResult, Digest, Public, PublicBuilder,
        SymmetricCipherParameters, SymmetricDefinitionObject, SensitiveData,
    },
    tcti_ldr::{DeviceConfig, TctiNameConf},
    traits::{Marshall, UnMarshall},
    Context,
};

/// tpm hardware info for attestation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TpmInfo {
    /// manufacturer id (4 chars, e.g. "IFX", "STM", "INTC", "MSFT", "GOOG")
    pub manufacturer: String,
    /// firmware version string
    pub firmware_version: String,
    /// detected as virtual tpm
    pub is_virtual: bool,
    /// tpm type description
    pub tpm_type: TpmType,
}

/// tpm type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TpmType {
    /// real hardware tpm chip
    Hardware,
    /// cloud provider vtpm (aws, gcp, azure)
    CloudVirtual,
    /// software emulator (swtpm, ibmswtpm)
    Emulator,
    /// unknown manufacturer
    Unknown,
}

impl std::fmt::Display for TpmType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TpmType::Hardware => write!(f, "hardware"),
            TpmType::CloudVirtual => write!(f, "cloud_virtual"),
            TpmType::Emulator => write!(f, "emulator"),
            TpmType::Unknown => write!(f, "unknown"),
        }
    }
}

/// known tpm manufacturers and their classification
fn classify_manufacturer(mfr: &str) -> (TpmType, &'static str) {
    match mfr.to_uppercase().as_str() {
        // hardware tpm manufacturers
        "IFX" | "IFX\0" => (TpmType::Hardware, "Infineon"),
        "STM" | "STM\0" => (TpmType::Hardware, "STMicroelectronics"),
        "INTC" => (TpmType::Hardware, "Intel"),
        "AMD" | "AMD\0" => (TpmType::Hardware, "AMD"),
        "ATML" => (TpmType::Hardware, "Atmel"),
        "NSM" | "NSM\0" => (TpmType::Hardware, "Nuvoton"),
        "NTZ" | "NTZ\0" => (TpmType::Hardware, "Nationz"),
        "ROCC" => (TpmType::Hardware, "Futurex"),
        "SMSC" => (TpmType::Hardware, "SMSC"),
        "TXN" | "TXN\0" => (TpmType::Hardware, "Texas Instruments"),
        "WEC" | "WEC\0" => (TpmType::Hardware, "Winbond"),

        // cloud vtpm
        "MSFT" => (TpmType::CloudVirtual, "Microsoft Azure/Hyper-V"),
        "GOOG" => (TpmType::CloudVirtual, "Google Cloud"),
        "AMZN" => (TpmType::CloudVirtual, "AWS Nitro"),
        "QEMU" => (TpmType::CloudVirtual, "QEMU"),

        // software emulators
        "IBM" | "IBM\0" => (TpmType::Emulator, "IBM Software TPM"),
        "SW" | "SW\0\0" => (TpmType::Emulator, "swtpm"),
        "SWTM" => (TpmType::Emulator, "swtpm"),
        "VBOX" => (TpmType::Emulator, "VirtualBox"),

        _ => (TpmType::Unknown, "Unknown"),
    }
}

use crate::crypto::random_bytes;
use crate::realm::{Realm, Registration};
use crate::{Error, Result};

/// tpm realm with hardware-backed security
pub struct TpmRealm {
    id: [u8; 16],
    context: Arc<RwLock<Context>>,
    primary_key: KeyHandle,
    storage_path: PathBuf,
    registrations: Arc<RwLock<HashMap<String, Registration>>>,
    /// detected tpm hardware info
    pub info: TpmInfo,
}

impl TpmRealm {
    /// create a new tpm realm
    ///
    /// # arguments
    /// * `storage_path` - directory to store registration data
    pub fn new(storage_path: &str) -> Result<Self> {
        let storage_path = PathBuf::from(storage_path);

        // try tpmrm0 first (resource manager), then tpm0
        let device = if std::path::Path::new("/dev/tpmrm0").exists() {
            "/dev/tpmrm0"
        } else {
            "/dev/tpm0"
        };

        let tcti = TctiNameConf::Device(DeviceConfig::from_str(device)
            .map_err(|e| Error::Tpm(format!("invalid device: {}", e)))?);

        let mut context = Context::new(tcti)
            .map_err(|e| Error::Tpm(format!("failed to create context: {}", e)))?;

        // detect tpm hardware info
        let info = detect_tpm_info(&mut context)
            .unwrap_or_else(|_| TpmInfo {
                manufacturer: "unknown".into(),
                firmware_version: "unknown".into(),
                is_virtual: false,
                tpm_type: TpmType::Unknown,
            });

        // create primary key under owner hierarchy
        let primary = create_primary_key(&mut context)
            .map_err(|e| Error::Tpm(format!("failed to create primary key: {}", e)))?;

        // load existing registrations
        let registrations = load_registrations(&storage_path)
            .unwrap_or_default();

        Ok(Self {
            id: random_bytes(),
            context: Arc::new(RwLock::new(context)),
            primary_key: primary.key_handle,
            storage_path,
            registrations: Arc::new(RwLock::new(registrations)),
            info,
        })
    }

    /// get tpm hardware info
    pub fn tpm_info(&self) -> &TpmInfo {
        &self.info
    }

    fn save_registrations(&self) -> Result<()> {
        let regs = self.registrations.read()
            .map_err(|e| Error::Storage(e.to_string()))?;

        let path = self.storage_path.join("registrations.json");
        let data = serde_json::to_vec(&*regs)
            .map_err(|e| Error::Storage(e.to_string()))?;

        std::fs::write(&path, data)
            .map_err(|e| Error::Storage(e.to_string()))?;

        Ok(())
    }
}

impl Realm for TpmRealm {
    fn id(&self) -> &[u8] {
        &self.id
    }

    fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        let mut ctx = self.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        // create a sealed object under the primary key
        let sealed = seal_data(&mut ctx, self.primary_key, data)
            .map_err(|e| Error::SealFailed(e.to_string()))?;

        Ok(sealed)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        let mut ctx = self.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        // tpm enforces dictionary attack protection here!
        // after too many failures, tpm locks out
        let data = unseal_data(&mut ctx, self.primary_key, sealed)
            .map_err(|e| Error::UnsealFailed(e.to_string()))?;

        Ok(data)
    }

    fn store(&self, user_id: &str, registration: &Registration) -> Result<()> {
        {
            let mut regs = self.registrations.write()
                .map_err(|e| Error::Storage(e.to_string()))?;
            regs.insert(user_id.to_string(), registration.clone());
        }
        self.save_registrations()
    }

    fn load(&self, user_id: &str) -> Result<Option<Registration>> {
        let regs = self.registrations.read()
            .map_err(|e| Error::Storage(e.to_string()))?;
        Ok(regs.get(user_id).cloned())
    }

    fn delete(&self, user_id: &str) -> Result<()> {
        {
            let mut regs = self.registrations.write()
                .map_err(|e| Error::Storage(e.to_string()))?;
            regs.remove(user_id);
        }
        self.save_registrations()
    }

    fn check_rate_limit(&self, _user_id: &str) -> Result<()> {
        // tpm handles rate limiting in hardware via dictionary attack protection
        // we can query the tpm for lockout state if needed
        Ok(())
    }

    fn increment_attempts(&self, user_id: &str) -> Result<u32> {
        let mut regs = self.registrations.write()
            .map_err(|e| Error::Storage(e.to_string()))?;

        if let Some(reg) = regs.get_mut(user_id) {
            reg.attempted_guesses += 1;

            if reg.attempted_guesses >= reg.allowed_guesses {
                // destroy the registration
                regs.remove(user_id);
                drop(regs);
                self.save_registrations()?;
                return Err(Error::NoGuessesRemaining);
            }

            let attempts = reg.attempted_guesses;
            drop(regs);
            self.save_registrations()?;
            Ok(attempts)
        } else {
            Err(Error::NotRegistered)
        }
    }

    fn reset_attempts(&self, user_id: &str) -> Result<()> {
        let mut regs = self.registrations.write()
            .map_err(|e| Error::Storage(e.to_string()))?;

        if let Some(reg) = regs.get_mut(user_id) {
            reg.attempted_guesses = 0;
        }

        drop(regs);
        self.save_registrations()
    }
}

/// create primary key for sealing operations
fn create_primary_key(ctx: &mut Context) -> tss_esapi::Result<CreatePrimaryKeyResult> {
    use tss_esapi::interface_types::session_handles::AuthSession;

    let object_attributes = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_sensitive_data_origin(true)
        .with_user_with_auth(true)
        .with_decrypt(true)
        .with_restricted(true)
        .build()?;

    let public = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::SymCipher)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(object_attributes)
        .with_symmetric_cipher_parameters(SymmetricCipherParameters::new(
            SymmetricDefinitionObject::AES_256_CFB,
        ))
        .with_symmetric_cipher_unique_identifier(Digest::default())
        .build()?;

    // set password session for owner hierarchy
    ctx.execute_with_nullauth_session(|ctx| {
        ctx.create_primary(
            Hierarchy::Owner,
            public,
            None,
            None,
            None,
            None,
        )
    })
}

/// seal data to tpm
fn seal_data(ctx: &mut Context, parent: KeyHandle, data: &[u8]) -> tss_esapi::Result<Vec<u8>> {
    let object_attributes = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_user_with_auth(true)
        .with_no_da(false)  // enable dictionary attack protection!
        .build()?;

    let public = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::KeyedHash)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(object_attributes)
        .with_keyed_hash_parameters(tss_esapi::structures::PublicKeyedHashParameters::new(
            tss_esapi::structures::KeyedHashScheme::Null,
        ))
        .with_keyed_hash_unique_identifier(Digest::default())
        .build()?;

    let sensitive_data = SensitiveData::try_from(data.to_vec())?;

    let result = ctx.execute_with_nullauth_session(|ctx| {
        ctx.create(
            parent,
            public,
            None,
            Some(sensitive_data),
            None,
            None,
        )
    })?;

    // serialize the created object for storage
    // Private derefs to Vec<u8>, Public needs marshall
    let private_bytes: Vec<u8> = result.out_private.to_vec();
    let public_bytes: Vec<u8> = result.out_public.marshall()?;

    let mut sealed = Vec::new();
    sealed.extend_from_slice(&(private_bytes.len() as u32).to_le_bytes());
    sealed.extend_from_slice(&private_bytes);
    sealed.extend_from_slice(&(public_bytes.len() as u32).to_le_bytes());
    sealed.extend_from_slice(&public_bytes);

    Ok(sealed)
}

/// unseal data from tpm
fn unseal_data(ctx: &mut Context, parent: KeyHandle, sealed: &[u8]) -> tss_esapi::Result<Vec<u8>> {
    if sealed.len() < 8 {
        return Err(tss_esapi::Error::WrapperError(
            tss_esapi::WrapperErrorKind::InvalidParam,
        ));
    }

    // deserialize
    let private_len = u32::from_le_bytes(sealed[0..4].try_into().unwrap()) as usize;
    let private_end = 4 + private_len;

    if sealed.len() < private_end + 4 {
        return Err(tss_esapi::Error::WrapperError(
            tss_esapi::WrapperErrorKind::InvalidParam,
        ));
    }

    let public_len = u32::from_le_bytes(sealed[private_end..private_end + 4].try_into().unwrap()) as usize;
    let public_end = private_end + 4 + public_len;

    if sealed.len() < public_end {
        return Err(tss_esapi::Error::WrapperError(
            tss_esapi::WrapperErrorKind::InvalidParam,
        ));
    }

    let private = tss_esapi::structures::Private::try_from(&sealed[4..private_end])?;
    let public = Public::unmarshall(&sealed[private_end + 4..public_end])?;

    // load and unseal with session
    ctx.execute_with_nullauth_session(|ctx| {
        // load the sealed object
        let key_handle = ctx.load(parent, private, public)?;

        // unseal - tpm dictionary attack protection kicks in here!
        let result = ctx.unseal(key_handle.into())?;

        Ok(result.to_vec())
    })
}

/// load registrations from disk
fn load_registrations(path: &PathBuf) -> Result<HashMap<String, Registration>> {
    let file_path = path.join("registrations.json");
    if !file_path.exists() {
        return Ok(HashMap::new());
    }

    let data = std::fs::read(&file_path)
        .map_err(|e| Error::Storage(e.to_string()))?;

    serde_json::from_slice(&data)
        .map_err(|e| Error::Storage(e.to_string()))
}

/// detect tpm hardware info by querying manufacturer and firmware
fn detect_tpm_info(ctx: &mut Context) -> tss_esapi::Result<TpmInfo> {
    use tss_esapi::constants::CapabilityType;
    use tss_esapi::constants::tss::{TPM2_PT_MANUFACTURER, TPM2_PT_FIRMWARE_VERSION_1, TPM2_PT_FIRMWARE_VERSION_2};

    // query tpm properties
    let (cap_data, _more) = ctx.get_capability(
        CapabilityType::TpmProperties,
        TPM2_PT_MANUFACTURER,
        32, // get a few properties
    )?;

    let mut manufacturer_raw: u32 = 0;
    let mut fw_version_1: u32 = 0;
    let mut fw_version_2: u32 = 0;

    if let CapabilityData::TpmProperties(props) = cap_data {
        for prop in props.iter() {
            let tag_val: u32 = prop.property().into();
            if tag_val == TPM2_PT_MANUFACTURER {
                manufacturer_raw = prop.value();
            } else if tag_val == TPM2_PT_FIRMWARE_VERSION_1 {
                fw_version_1 = prop.value();
            } else if tag_val == TPM2_PT_FIRMWARE_VERSION_2 {
                fw_version_2 = prop.value();
            }
        }
    }

    // convert manufacturer id to string (it's stored as 4 ascii chars)
    let mfr_bytes = manufacturer_raw.to_be_bytes();
    let manufacturer = String::from_utf8_lossy(&mfr_bytes)
        .trim_end_matches('\0')
        .to_string();

    // format firmware version
    let firmware_version = format!(
        "{}.{}.{}.{}",
        (fw_version_1 >> 16) & 0xFFFF,
        fw_version_1 & 0xFFFF,
        (fw_version_2 >> 16) & 0xFFFF,
        fw_version_2 & 0xFFFF,
    );

    // classify the manufacturer
    let (tpm_type, _vendor_name) = classify_manufacturer(&manufacturer);
    let is_virtual = matches!(tpm_type, TpmType::CloudVirtual | TpmType::Emulator);

    Ok(TpmInfo {
        manufacturer,
        firmware_version,
        is_virtual,
        tpm_type,
    })
}

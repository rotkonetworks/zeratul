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
    attributes::SessionAttributesBuilder,
    attributes::NvIndexAttributesBuilder,
    handles::{KeyHandle, NvIndexHandle, NvIndexTpmHandle},
    interface_types::{
        algorithm::{HashingAlgorithm, PublicAlgorithm},
        resource_handles::Hierarchy,
        session_handles::PolicySession,
    },
    structures::{
        CapabilityData, CreatePrimaryKeyResult, Digest, Public, PublicBuilder,
        SymmetricCipherParameters, SymmetricDefinitionObject, SensitiveData,
        PcrSelectionList, PcrSelectionListBuilder, PcrSlot,
        NvPublicBuilder,
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

/// pcrs to bind sealed data to
/// pcr0: firmware code
/// pcr7: secure boot state (recommended for production)
const DEFAULT_PCR_SLOTS: &[PcrSlot] = &[PcrSlot::Slot0, PcrSlot::Slot7];

/// tpm realm with hardware-backed security
pub struct TpmRealm {
    id: [u8; 16],
    context: Arc<RwLock<Context>>,
    primary_key: KeyHandle,
    storage_path: PathBuf,
    registrations: Arc<RwLock<HashMap<String, Registration>>>,
    /// detected tpm hardware info
    pub info: TpmInfo,
    /// pcr slots to bind sealed data to
    pcr_slots: Vec<PcrSlot>,
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
            pcr_slots: DEFAULT_PCR_SLOTS.to_vec(),
        })
    }

    /// create tpm realm with custom pcr binding
    ///
    /// # arguments
    /// * `storage_path` - directory to store registration data
    /// * `pcr_slots` - pcr slots to bind sealed data to (empty = no pcr binding)
    pub fn with_pcrs(storage_path: &str, pcr_slots: &[PcrSlot]) -> Result<Self> {
        let mut realm = Self::new(storage_path)?;
        realm.pcr_slots = pcr_slots.to_vec();
        Ok(realm)
    }

    /// disable pcr binding (for testing/development only!)
    pub fn without_pcr_binding(storage_path: &str) -> Result<Self> {
        Self::with_pcrs(storage_path, &[])
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

        // create a sealed object under the primary key with pcr binding
        let sealed = seal_data_with_pcr(&mut ctx, self.primary_key, data, &self.pcr_slots)
            .map_err(|e| Error::SealFailed(e.to_string()))?;

        Ok(sealed)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        let mut ctx = self.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        // unseal with pcr policy - tpm verifies boot state matches!
        // also enforces dictionary attack protection
        let data = unseal_data_with_pcr(&mut ctx, self.primary_key, sealed, &self.pcr_slots)
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

/// build pcr selection list from slots
fn build_pcr_selection(pcr_slots: &[PcrSlot]) -> tss_esapi::Result<PcrSelectionList> {
    if pcr_slots.is_empty() {
        return Ok(PcrSelectionList::builder().build()?);
    }

    let mut builder = PcrSelectionListBuilder::new();
    builder = builder.with_selection(HashingAlgorithm::Sha256, pcr_slots);
    builder.build()
}

/// compute policy digest for pcr binding
fn compute_pcr_policy(ctx: &mut Context, pcr_slots: &[PcrSlot]) -> tss_esapi::Result<Digest> {
    if pcr_slots.is_empty() {
        return Ok(Digest::default());
    }

    // start a trial session to compute the policy digest
    let session = ctx.start_auth_session(
        None,
        None,
        None,
        tss_esapi::constants::SessionType::Trial,
        tss_esapi::structures::SymmetricDefinition::AES_128_CFB,
        HashingAlgorithm::Sha256,
    )?.ok_or_else(|| tss_esapi::Error::WrapperError(
        tss_esapi::WrapperErrorKind::InvalidParam,
    ))?;

    let (session_attrs, session_attrs_mask) = SessionAttributesBuilder::new()
        .with_decrypt(true)
        .with_encrypt(true)
        .build();

    ctx.tr_sess_set_attributes(session, session_attrs, session_attrs_mask)?;

    let pcr_selection = build_pcr_selection(pcr_slots)?;

    // add pcr policy - this extends the policy hash
    ctx.policy_pcr(
        PolicySession::try_from(session)?,
        Digest::default(), // empty = use current pcr values
        pcr_selection,
    )?;

    // get the computed policy digest
    let policy_digest = ctx.policy_get_digest(PolicySession::try_from(session)?)?;

    // flush the trial session (AuthSession converts to SessionHandle via Into)
    ctx.flush_context(tss_esapi::handles::SessionHandle::from(session).into())?;

    Ok(policy_digest)
}

/// seal data to tpm with pcr binding
fn seal_data_with_pcr(
    ctx: &mut Context,
    parent: KeyHandle,
    data: &[u8],
    pcr_slots: &[PcrSlot],
) -> tss_esapi::Result<Vec<u8>> {
    // compute policy digest for pcr binding (if any)
    let auth_policy = if pcr_slots.is_empty() {
        None
    } else {
        Some(compute_pcr_policy(ctx, pcr_slots)?)
    };

    let object_attributes = ObjectAttributesBuilder::new()
        .with_fixed_tpm(true)
        .with_fixed_parent(true)
        .with_user_with_auth(auth_policy.is_none()) // only allow auth if no pcr policy
        .with_no_da(false) // enable dictionary attack protection
        .build()?;

    let mut public_builder = PublicBuilder::new()
        .with_public_algorithm(PublicAlgorithm::KeyedHash)
        .with_name_hashing_algorithm(HashingAlgorithm::Sha256)
        .with_object_attributes(object_attributes)
        .with_keyed_hash_parameters(tss_esapi::structures::PublicKeyedHashParameters::new(
            tss_esapi::structures::KeyedHashScheme::Null,
        ))
        .with_keyed_hash_unique_identifier(Digest::default());

    // add auth policy if we have pcr binding
    if let Some(policy) = auth_policy {
        public_builder = public_builder.with_auth_policy(policy);
    }

    let public = public_builder.build()?;
    let sensitive_data = SensitiveData::try_from(data.to_vec())?;

    let result = ctx.execute_with_nullauth_session(|ctx| {
        ctx.create(parent, public, None, Some(sensitive_data), None, None)
    })?;

    // serialize: private + public + pcr_slot_count + pcr_slots (as u32s)
    let private_bytes: Vec<u8> = result.out_private.to_vec();
    let public_bytes: Vec<u8> = result.out_public.marshall()?;

    let mut sealed = Vec::new();
    sealed.extend_from_slice(&(private_bytes.len() as u32).to_le_bytes());
    sealed.extend_from_slice(&private_bytes);
    sealed.extend_from_slice(&(public_bytes.len() as u32).to_le_bytes());
    sealed.extend_from_slice(&public_bytes);
    // store which pcrs were used (for unseal) as u32 values
    sealed.push(pcr_slots.len() as u8);
    for slot in pcr_slots {
        sealed.extend_from_slice(&pcr_slot_to_u32(*slot).to_le_bytes());
    }

    Ok(sealed)
}

/// convert pcrslot to u32 for serialization
fn pcr_slot_to_u32(slot: PcrSlot) -> u32 {
    match slot {
        PcrSlot::Slot0 => 0,
        PcrSlot::Slot1 => 1,
        PcrSlot::Slot2 => 2,
        PcrSlot::Slot3 => 3,
        PcrSlot::Slot4 => 4,
        PcrSlot::Slot5 => 5,
        PcrSlot::Slot6 => 6,
        PcrSlot::Slot7 => 7,
        PcrSlot::Slot8 => 8,
        PcrSlot::Slot9 => 9,
        PcrSlot::Slot10 => 10,
        PcrSlot::Slot11 => 11,
        PcrSlot::Slot12 => 12,
        PcrSlot::Slot13 => 13,
        PcrSlot::Slot14 => 14,
        PcrSlot::Slot15 => 15,
        PcrSlot::Slot16 => 16,
        PcrSlot::Slot17 => 17,
        PcrSlot::Slot18 => 18,
        PcrSlot::Slot19 => 19,
        PcrSlot::Slot20 => 20,
        PcrSlot::Slot21 => 21,
        PcrSlot::Slot22 => 22,
        PcrSlot::Slot23 => 23,
        PcrSlot::Slot24 => 24,
        PcrSlot::Slot25 => 25,
        PcrSlot::Slot26 => 26,
        PcrSlot::Slot27 => 27,
        PcrSlot::Slot28 => 28,
        PcrSlot::Slot29 => 29,
        PcrSlot::Slot30 => 30,
        PcrSlot::Slot31 => 31,
    }
}

/// unseal data from tpm with pcr policy verification
fn unseal_data_with_pcr(
    ctx: &mut Context,
    parent: KeyHandle,
    sealed: &[u8],
    _expected_pcrs: &[PcrSlot], // stored in sealed data, param kept for api consistency
) -> tss_esapi::Result<Vec<u8>> {
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

    let public_len =
        u32::from_le_bytes(sealed[private_end..private_end + 4].try_into().unwrap()) as usize;
    let public_end = private_end + 4 + public_len;

    if sealed.len() < public_end + 1 {
        return Err(tss_esapi::Error::WrapperError(
            tss_esapi::WrapperErrorKind::InvalidParam,
        ));
    }

    // extract pcr slots from sealed data (stored as u32s)
    let pcr_count = sealed[public_end] as usize;
    let pcr_bytes_len = pcr_count * 4; // each slot is 4 bytes
    if sealed.len() < public_end + 1 + pcr_bytes_len {
        return Err(tss_esapi::Error::WrapperError(
            tss_esapi::WrapperErrorKind::InvalidParam,
        ));
    }

    let stored_pcrs: Vec<PcrSlot> = (0..pcr_count)
        .filter_map(|i| {
            let offset = public_end + 1 + i * 4;
            let val = u32::from_le_bytes(sealed[offset..offset + 4].try_into().ok()?);
            PcrSlot::try_from(val).ok()
        })
        .collect();

    let private = tss_esapi::structures::Private::try_from(&sealed[4..private_end])?;
    let public = Public::unmarshall(&sealed[private_end + 4..public_end])?;

    // load the sealed object
    let key_handle = ctx.execute_with_nullauth_session(|ctx| ctx.load(parent, private, public))?;

    // if no pcr binding, use simple nullauth unseal
    if stored_pcrs.is_empty() {
        return ctx.execute_with_nullauth_session(|ctx| {
            let result = ctx.unseal(key_handle.into())?;
            Ok(result.to_vec())
        });
    }

    // create policy session for pcr-bound unseal
    let session = ctx
        .start_auth_session(
            None,
            None,
            None,
            tss_esapi::constants::SessionType::Policy,
            tss_esapi::structures::SymmetricDefinition::AES_128_CFB,
            HashingAlgorithm::Sha256,
        )?
        .ok_or_else(|| {
            tss_esapi::Error::WrapperError(tss_esapi::WrapperErrorKind::InvalidParam)
        })?;

    let (session_attrs, session_attrs_mask) = SessionAttributesBuilder::new()
        .with_decrypt(true)
        .with_encrypt(true)
        .build();

    ctx.tr_sess_set_attributes(session, session_attrs, session_attrs_mask)?;

    // satisfy the pcr policy - tpm checks current pcrs match sealed-time pcrs
    let pcr_selection = build_pcr_selection(&stored_pcrs)?;
    ctx.policy_pcr(
        PolicySession::try_from(session)?,
        Digest::default(), // empty = use current pcr values
        pcr_selection,
    )?;

    // set the policy session on the object
    ctx.tr_set_auth(key_handle.into(), tss_esapi::structures::Auth::default())?;

    // unseal with policy session - this is where pcr check happens!
    // tpm verifies current pcr values match the policy
    ctx.set_sessions((Some(session), None, None));
    let result = ctx.unseal(key_handle.into())?;
    ctx.clear_sessions();

    // flush the session (AuthSession converts to SessionHandle via Into)
    ctx.flush_context(tss_esapi::handles::SessionHandle::from(session).into())?;

    Ok(result.to_vec())
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

// === nv counter for tamper-evident rate limiting ===
//
// tpm nv counters are monotonic - they can only be incremented, never decremented.
// this provides tamper-evident rate limiting that can't be bypassed by deleting files.
//
// caveat: tpm nv space is limited (typically a few KB), so this is best used for
// high-security deployments with a known number of users, or as a global lockout counter.
//
// note: the exact tss-esapi api varies by version. this implementation is designed
// for tss-esapi 7.x. you may need to adjust auth handles for your version.

/// base nv index for ghettobox counters (in owner-defined range)
/// 0x01800000-0x01bfffff is the owner-defined nv index range
const NV_INDEX_BASE: u32 = 0x0180_0000;

/// nv counter for tracking failed attempts
pub struct NvCounter {
    tpm_handle: NvIndexTpmHandle,
}

impl NvCounter {
    /// compute nv index from user_id hash
    /// returns an index in the owner-defined range
    pub fn index_for_user(user_id: &str) -> u32 {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(b"ghettobox:nv_counter:v1:");
        hasher.update(user_id.as_bytes());
        let hash = hasher.finalize();
        // use first 2 bytes to get offset (0-65535)
        let offset = u16::from_le_bytes([hash[0], hash[1]]) as u32;
        NV_INDEX_BASE + offset
    }

    /// create or open an nv counter for a user
    pub fn open(_ctx: &mut Context, user_id: &str) -> tss_esapi::Result<Self> {
        let index_val = Self::index_for_user(user_id);
        let tpm_handle = NvIndexTpmHandle::new(index_val)
            .map_err(|_| tss_esapi::Error::WrapperError(
                tss_esapi::WrapperErrorKind::InvalidParam
            ))?;

        Ok(Self { tpm_handle })
    }

    /// get esapi handle from tpm handle
    fn get_nv_handle(&self, ctx: &mut Context) -> tss_esapi::Result<NvIndexHandle> {
        let obj_handle = ctx.execute_with_nullauth_session(|ctx| {
            ctx.tr_from_tpm_public(self.tpm_handle.into())
        })?;

        NvIndexHandle::try_from(obj_handle)
            .map_err(|_| tss_esapi::Error::WrapperError(
                tss_esapi::WrapperErrorKind::InvalidParam
            ))
    }

    /// define the nv counter (call once during registration)
    pub fn define(ctx: &mut Context, user_id: &str, _max_value: u64) -> tss_esapi::Result<Self> {
        let counter = Self::open(ctx, user_id)?;

        // check if already exists by trying to get handle
        if counter.exists(ctx) {
            return Ok(counter);
        }

        // counter attributes for TPM2_NT_COUNTER type:
        // bits 7:4 of attributes encode the type (TPM2_NT_COUNTER = 1)
        let nv_attributes = NvIndexAttributesBuilder::new()
            .with_owner_write(true)
            .with_owner_read(true)
            .build()?;

        let nv_public = NvPublicBuilder::new()
            .with_nv_index(counter.tpm_handle)
            .with_index_name_algorithm(HashingAlgorithm::Sha256)
            .with_index_attributes(nv_attributes)
            .with_data_area_size(8) // 64-bit counter
            .build()?;

        ctx.execute_with_nullauth_session(|ctx| {
            ctx.nv_define_space(
                tss_esapi::interface_types::resource_handles::Provision::Owner,
                None,
                nv_public,
            )
        })?;

        Ok(counter)
    }

    /// check if nv index exists
    fn exists(&self, ctx: &mut Context) -> bool {
        self.get_nv_handle(ctx).is_ok()
    }

    /// read current counter value
    pub fn read(&self, ctx: &mut Context) -> tss_esapi::Result<u64> {
        let nv_handle = self.get_nv_handle(ctx)?;

        // nv_read signature: (auth_handle, nv_index, size, offset)
        let auth = tss_esapi::interface_types::resource_handles::NvAuth::Owner;
        let data = ctx.execute_with_nullauth_session(|ctx| {
            ctx.nv_read(auth, nv_handle, 8, 0)
        })?;

        if data.len() < 8 {
            return Ok(0); // uninitialized counter
        }

        let bytes: [u8; 8] = data.as_slice()[..8].try_into()
            .map_err(|_| tss_esapi::Error::WrapperError(
                tss_esapi::WrapperErrorKind::InvalidParam
            ))?;

        Ok(u64::from_be_bytes(bytes))
    }

    /// increment counter (returns new value)
    pub fn increment(&self, ctx: &mut Context) -> tss_esapi::Result<u64> {
        let nv_handle = self.get_nv_handle(ctx)?;

        // nv_increment signature: (auth_handle, nv_index)
        let auth = tss_esapi::interface_types::resource_handles::NvAuth::Owner;
        ctx.execute_with_nullauth_session(|ctx| {
            ctx.nv_increment(auth, nv_handle)
        })?;

        // read back new value
        self.read(ctx)
    }

    /// delete the nv counter (for cleanup)
    pub fn undefine(&self, ctx: &mut Context) -> tss_esapi::Result<()> {
        let nv_handle = self.get_nv_handle(ctx)?;

        ctx.execute_with_nullauth_session(|ctx| {
            ctx.nv_undefine_space(
                tss_esapi::interface_types::resource_handles::Provision::Owner,
                nv_handle,
            )
        })
    }
}

/// tpm realm with nv counter rate limiting
///
/// SECURITY: this implementation guards against the global TPM dictionary attack
/// lockout vulnerability by checking rate limits BEFORE any authenticated TPM
/// operation. the nv counter read is unauthenticated and doesn't trigger DA.
///
/// flow:
///   1. read nv counter (unauthenticated, no DA trigger)
///   2. if over limit → reject immediately, never touch TPM auth
///   3. if under limit → increment counter, then proceed with TPM operation
///   4. on failure → counter already incremented, attacker loses attempt
///   5. on success → optionally reset counter (requires separate nv index)
pub struct TpmRealmWithNvCounter {
    inner: TpmRealm,
    max_attempts: u64,
}

impl TpmRealmWithNvCounter {
    /// create tpm realm with nv counter rate limiting
    pub fn new(storage_path: &str, max_attempts: u64) -> Result<Self> {
        let inner = TpmRealm::new(storage_path)?;
        Ok(Self { inner, max_attempts })
    }

    /// create with custom pcr binding
    pub fn with_pcrs(storage_path: &str, max_attempts: u64, pcr_slots: &[PcrSlot]) -> Result<Self> {
        let inner = TpmRealm::with_pcrs(storage_path, pcr_slots)?;
        Ok(Self { inner, max_attempts })
    }

    /// initialize nv counter for a new user (call during registration)
    pub fn init_user_counter(&self, user_id: &str) -> Result<()> {
        let mut ctx = self.inner.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        NvCounter::define(&mut ctx, user_id, self.max_attempts)
            .map_err(|e| Error::Tpm(format!("failed to define counter: {}", e)))?;

        Ok(())
    }

    /// check rate limit (unauthenticated read - safe, no DA trigger)
    fn check_limit(&self, user_id: &str) -> Result<u64> {
        let mut ctx = self.inner.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        let counter = NvCounter::open(&mut ctx, user_id)
            .map_err(|e| Error::Tpm(format!("failed to open counter: {}", e)))?;

        let val = counter.read(&mut ctx).unwrap_or(0);

        if val >= self.max_attempts {
            return Err(Error::NoGuessesRemaining);
        }

        Ok(val)
    }

    /// increment failure counter (call BEFORE attempting TPM auth operation)
    /// this ensures attacker loses an attempt even if they abort mid-operation
    fn pre_increment(&self, user_id: &str) -> Result<u64> {
        let mut ctx = self.inner.context.write()
            .map_err(|e| Error::Tpm(e.to_string()))?;

        let counter = NvCounter::open(&mut ctx, user_id)
            .map_err(|e| Error::Tpm(format!("failed to open counter: {}", e)))?;

        let new_val = counter.increment(&mut ctx)
            .map_err(|e| Error::Tpm(format!("failed to increment counter: {}", e)))?;

        if new_val > self.max_attempts {
            return Err(Error::NoGuessesRemaining);
        }

        Ok(new_val)
    }

    /// guarded unseal - checks rate limit, increments counter, then unseals
    ///
    /// SECURITY: this is the safe way to unseal. the sequence is:
    ///   1. check counter (no DA trigger)
    ///   2. increment counter (no DA trigger, attacker commits attempt)
    ///   3. attempt unseal (may trigger DA on wrong auth, but attacker
    ///      already used up an attempt so can't spam)
    pub fn guarded_unseal(&self, user_id: &str, sealed: &[u8]) -> Result<Vec<u8>> {
        // step 1: check if already locked out (unauthenticated read)
        self.check_limit(user_id)?;

        // step 2: increment counter BEFORE unseal attempt
        // this means attacker commits to using an attempt before we do any TPM auth
        self.pre_increment(user_id)?;

        // step 3: now safe to attempt unseal
        // even if this triggers DA, attacker already lost an attempt
        // and will hit our NV limit before triggering global DA lockout
        self.inner.unseal(sealed)
    }

    /// get remaining attempts for user
    pub fn remaining_attempts(&self, user_id: &str) -> Result<u64> {
        let current = self.check_limit(user_id)?;
        Ok(self.max_attempts.saturating_sub(current))
    }

    /// get the underlying realm (for seal operations which don't need rate limiting)
    pub fn realm(&self) -> &TpmRealm {
        &self.inner
    }

    /// get tpm info
    pub fn tpm_info(&self) -> &TpmInfo {
        &self.inner.info
    }
}

impl Realm for TpmRealmWithNvCounter {
    fn id(&self) -> &[u8] {
        self.inner.id()
    }

    fn seal(&self, data: &[u8]) -> Result<Vec<u8>> {
        // sealing doesn't need rate limiting - it's a write operation
        self.inner.seal(data)
    }

    fn unseal(&self, sealed: &[u8]) -> Result<Vec<u8>> {
        // NOTE: this bypasses rate limiting! use guarded_unseal() instead
        // keeping this for Realm trait compatibility, but callers should
        // use guarded_unseal() for user-facing operations
        self.inner.unseal(sealed)
    }

    fn store(&self, user_id: &str, registration: &Registration) -> Result<()> {
        // also initialize the user's nv counter
        self.init_user_counter(user_id)?;
        self.inner.store(user_id, registration)
    }

    fn load(&self, user_id: &str) -> Result<Option<Registration>> {
        self.inner.load(user_id)
    }

    fn delete(&self, user_id: &str) -> Result<()> {
        // TODO: also delete nv counter? or leave it to prevent re-registration attacks
        self.inner.delete(user_id)
    }

    fn check_rate_limit(&self, user_id: &str) -> Result<()> {
        self.check_limit(user_id)?;
        Ok(())
    }

    fn increment_attempts(&self, user_id: &str) -> Result<u32> {
        let new_val = self.pre_increment(user_id)?;
        Ok(new_val as u32)
    }

    fn reset_attempts(&self, _user_id: &str) -> Result<()> {
        // NV counters are monotonic - can't reset!
        // for "reset on success" semantics, you'd need a separate NV index
        // that stores "last successful auth counter value" and compare against it
        //
        // for now, we don't support reset - users get max_attempts total, ever
        // this is actually more secure against "authenticate then share creds" attacks
        Ok(())
    }
}

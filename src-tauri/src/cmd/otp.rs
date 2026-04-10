use crate::config;
use crate::config::OtpEntry;
use crate::error::AppResult;
use crate::utils::crypto;
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OtpCodeResult {
    pub code: String,
    pub remaining_seconds: u64,
}

#[tauri::command]
pub fn get_otp_entries(app: tauri::AppHandle) -> AppResult<Vec<OtpEntry>> {
    let mut cfg = config::load_otp_entries(&app)?;
    for entry in &mut cfg.entries {
        entry.secret = None;
    }
    Ok(cfg.entries)
}

#[tauri::command]
pub fn get_otp_secret_value(app: tauri::AppHandle, id: String) -> AppResult<Option<String>> {
    Ok(config::load_otp_entry_by_id(&app, &id)?.secret)
}

#[tauri::command]
pub fn save_otp_entry(app: tauri::AppHandle, mut entry: OtpEntry) -> AppResult<String> {
    let mut cfg = config::load_otp_entries(&app)?;

    if entry.id.is_empty() {
        entry.id = uuid::Uuid::new_v4().to_string();
    }
    let target_id = entry.id.clone();
    let existing = cfg.entries.iter().find(|e| e.id == target_id);

    entry.secret = match entry.secret.as_deref() {
        Some(plain) if !plain.is_empty() => Some(crypto::encrypt(plain)?),
        _ => existing.and_then(|e| e.secret.clone()),
    };

    if let Some(ex) = cfg.entries.iter_mut().find(|e| e.id == target_id) {
        *ex = entry;
    } else {
        cfg.entries.push(entry);
    }
    config::save_otp_entries(&app, &cfg)?;
    Ok(target_id)
}

#[tauri::command]
pub fn delete_otp_entry(app: tauri::AppHandle, id: String) -> AppResult<()> {
    let mut cfg = config::load_otp_entries(&app)?;
    cfg.entries.retain(|e| e.id != id);
    config::save_otp_entries(&app, &cfg)
}

#[tauri::command]
pub fn generate_otp_code(app: tauri::AppHandle, id: String) -> AppResult<OtpCodeResult> {
    generate_otp_for_entry(&app, &id)
}

#[tauri::command]
pub fn import_otp_from_qr(path: String) -> AppResult<OtpEntry> {
    use crate::error::AppError;

    let img = image::open(&path)
        .map_err(|e| AppError::Config(format!("Failed to open image: {e}")))?;
    let gray = img.to_luma8();

    let mut prepared = rqrr::PreparedImage::prepare(gray);
    let grids = prepared.detect_grids();

    let grid = grids
        .first()
        .ok_or_else(|| AppError::Config("No QR code found in the image".to_string()))?;

    let (_, uri) = grid
        .decode()
        .map_err(|e| AppError::Config(format!("Failed to decode QR code: {e}")))?;

    if uri.starts_with("otpauth://totp/") {
        let totp = dragonfly_otp::Totp::from_uri(&uri)
            .map_err(|e| AppError::Config(format!("Invalid TOTP URI: {e}")))?;
        Ok(OtpEntry {
            id: String::new(),
            otp_type: "totp".to_string(),
            issuer: totp.issuer().to_string(),
            username: totp.label().to_string(),
            secret: Some(totp.secret().into_base32()),
            algorithm: totp.alg().to_string(),
            digits: totp.digits(),
            period: totp.period(),
            counter: 0,
            has_secret: false,
        })
    } else if uri.starts_with("otpauth://hotp/") {
        let hotp = dragonfly_otp::Hotp::from_uri(&uri)
            .map_err(|e| AppError::Config(format!("Invalid HOTP URI: {e}")))?;
        Ok(OtpEntry {
            id: String::new(),
            otp_type: "hotp".to_string(),
            issuer: hotp.issuer().to_string(),
            username: hotp.label().to_string(),
            secret: Some(hotp.secret().into_base32()),
            algorithm: hotp.alg().to_string(),
            digits: hotp.digits(),
            period: 30,
            counter: hotp.counter(),
            has_secret: false,
        })
    } else {
        Err(AppError::Config(format!(
            "QR code does not contain a valid otpauth:// URI: {uri}"
        )))
    }
}

/// Shared logic for generating an OTP code from a stored entry.
pub(crate) fn generate_otp_for_entry(
    app: &tauri::AppHandle,
    id: &str,
) -> AppResult<OtpCodeResult> {
    let entry = config::load_otp_entry_by_id(app, id)?;
    let secret_str = entry
        .secret
        .as_deref()
        .ok_or_else(|| crate::error::AppError::Config("OTP entry has no secret".to_string()))?;

    let alg = match entry.algorithm.as_str() {
        "SHA256" => dragonfly_otp::Algorithm::SHA256,
        "SHA512" => dragonfly_otp::Algorithm::SHA512,
        _ => dragonfly_otp::Algorithm::SHA1,
    };

    let secret = dragonfly_otp::Secret::from_base32(secret_str)
        .map_err(|e| crate::error::AppError::Config(format!("Invalid base32 secret: {e:?}")))?;

    match entry.otp_type.as_str() {
        "hotp" => {
            let mut hotp = dragonfly_otp::Hotp::new(
                alg,
                entry.issuer.clone(),
                entry.username.clone(),
                entry.digits,
                entry.counter,
                secret,
            );
            let raw = hotp.generate();
            let code = format!("{:0>width$}", raw, width = entry.digits as usize);

            // Increment the counter and persist
            let mut cfg = config::load_otp_entries(app)?;
            if let Some(e) = cfg.entries.iter_mut().find(|e| e.id == id) {
                e.counter += 1;
            }
            config::save_otp_entries(app, &cfg)?;

            Ok(OtpCodeResult {
                code,
                remaining_seconds: 0,
            })
        }
        _ => {
            let period = if entry.period > 0 { entry.period } else { 30 };
            let totp = dragonfly_otp::Totp::new(
                alg,
                entry.issuer.clone(),
                entry.username.clone(),
                entry.digits,
                period,
                secret,
            );

            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before UNIX epoch")
                .as_secs();

            let raw = totp.generate_at(now);
            let code = format!("{:0>width$}", raw, width = entry.digits as usize);
            let remaining = period - (now % period);

            Ok(OtpCodeResult {
                code,
                remaining_seconds: remaining,
            })
        }
    }
}

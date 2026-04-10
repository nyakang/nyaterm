use crate::{Algorithm, Secret, encoding, hotp::Hotp};

pub struct Totp {
    period: u64,
    hotp: Hotp,
}

impl Default for Totp {
    fn default() -> Self {
        Self {
            hotp: Hotp::default(),
            period: 30,
        }
    }
}

impl Totp {
    /// Creates a new [`Totp`] instance with the specified configuration.
    ///
    /// Internally, this wraps an [`Hotp`] instance and uses time-based counter
    /// calculations according to the specified period.
    ///
    /// # Arguments
    ///
    /// * `alg` - The hashing algorithm to use (e.g., [`Algorithm::SHA1`], [`Algorithm::SHA256`], or [`Algorithm::SHA512`]).
    /// * `issuer` - The name of the service or provider (e.g., `"GitHub"` or `"example.com"`).
    /// * `label` - An identifier for the user account (e.g., `"alice@example.com"`).
    /// * `digits` - Number of digits in the generated OTP (typically 6 or 8).
    /// * `period` - Time step duration in seconds (usually 30).
    /// * `secret` - The shared secret key used to generate the HMAC.
    ///
    /// # Returns
    ///
    /// Returns a new instance of [`Totp`] configured with the provided parameters.
    ///
    /// # Example
    ///
    /// ```rust
    /// use otp::{Totp, Algorithm, Secret};
    ///
    /// let totp = Totp::new(
    ///     Algorithm::SHA1,
    ///     "example".into(),
    ///     "alice@example.com".into(),
    ///     6,
    ///     30,
    ///     Secret::from_bytes(b"supersecret"),
    /// );
    /// ```
    pub fn new(
        alg: Algorithm,
        issuer: String,
        label: String,
        digits: u8,
        period: u64,
        secret: Secret,
    ) -> Self {
        Self {
            period,
            hotp: Hotp::new(alg, issuer, label, digits, Default::default(), secret),
        }
    }

    /// Generates a TOTP code for the current system time using the configured algorithm and secret.
    ///
    /// Internally, this method computes the number of time steps (counters) since the Unix epoch,
    /// and uses that to derive the OTP value.
    ///
    /// # Returns
    /// A numeric TOTP code as a `u32`.
    ///
    /// # Panics
    /// Panics if system time is before the Unix epoch.
    ///
    /// # Example
    /// ```rust
    /// let totp = otp::Totp::default();
    /// let otp = totp.generate();
    /// println!("OTP: {}", otp);
    /// ```
    ///
    /// # References
    /// - [RFC 6238](https://datatracker.ietf.org/doc/html/rfc6238#section-4)
    pub fn generate(&self) -> u32 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("Clock may have gone backwards")
            .as_secs();

        self.generate_at(now)
    }

    /// Generates a TOTP code for a specific timestamp (in seconds since Unix epoch).
    ///
    /// This method is useful when simulating or verifying TOTP behavior
    /// for a given point in time.
    ///
    /// # Arguments
    /// * `timestamp_secs` - The Unix timestamp in seconds
    ///
    /// # Returns
    /// A numeric TOTP code as a `u32`.
    ///
    /// # Example
    /// ```rust
    /// let totp = otp::Totp::default();
    /// let otp = totp.generate_at(1_600_000_000); // fixed timestamp
    /// ```
    ///
    /// # References
    /// - [RFC 6238](https://datatracker.ietf.org/doc/html/rfc6238#section-4)
    pub fn generate_at(&self, timestamp_secs: u64) -> u32 {
        let counter = timestamp_secs / self.period;
        self.hotp.generate_at(counter)
    }

    /// Verifies whether a given OTP is valid for a timestamp, within a configurable window.
    ///
    /// This method accounts for small clock skews by checking OTP values generated
    /// before and after the given timestamp by a number of time steps defined by `window`.
    ///
    /// # Arguments
    /// * `otp`            - The OTP value to check
    /// * `timestamp_secs` - The Unix timestamp (in seconds) to check against
    /// * `window`         - The allowed time-step drift (in units of `period`)
    ///
    /// # Returns
    /// `true` if the OTP is valid within the given window; otherwise, `false`.
    ///
    /// # Example
    /// ```rust
    /// let totp = otp::Totp::default();
    /// let timestamp = 1_600_000_000;
    /// let otp = totp.generate_at(timestamp);
    /// assert!(totp.verify(otp, timestamp + 20, 1)); // within window
    /// ```
    ///
    /// # References
    /// - [RFC 6238](https://datatracker.ietf.org/doc/html/rfc6238#section-5.2)
    pub fn verify(&self, otp: u32, timestamp_secs: u64, window: u64) -> bool {
        let counter = timestamp_secs / self.period;
        self.hotp.verify(otp, counter, window)
    }

    /// Generates a Key URI string in the format compatible with Google Authenticator and other TOTP/HOTP apps.
    ///
    /// This URI can be encoded as a QR code and scanned by authenticator apps (e.g., Google Authenticator, Authy)
    /// to configure the OTP settings automatically.
    ///
    /// The URI format follows the [Key URI Format] specification:
    ///
    /// ```text
    /// otpauth://TYPE/LABEL?PARAMETERS
    /// ```
    ///
    /// For example, a TOTP URI might look like:
    ///
    /// ```text
    /// otpauth://totp/Example%3Aalice%40example.com?secret=JBSWY3DPEHPK3PXP&issuer=Example&algorithm=SHA1&digits=6&period=30
    /// ```
    ///
    /// # Format Details
    /// - `TYPE`: Either `totp` or `hotp`
    /// - `LABEL`: Usually `issuer:account`, URL-encoded
    /// - `secret`: Base32-encoded secret key
    /// - `issuer`: The provider or service name (optional, but recommended)
    /// - `algorithm`: Hash function used (e.g., SHA1, SHA256, SHA512)
    /// - `digits`: Number of digits in the OTP (typically 6 or 8)
    /// - `period` (TOTP only): Time step in seconds (e.g., 30)
    /// - `counter` (HOTP only): Current counter value
    ///
    /// # Returns
    /// A `String` containing the `otpauth://` URI.
    ///
    /// # Example
    /// ```rust
    /// use otp::{Totp, Algorithm, Secret};
    ///
    /// let totp = Totp::new(
    ///     Algorithm::SHA256,
    ///     "Example".into(),
    ///     "alice@example.com".into(),
    ///     6,
    ///     30,
    ///     Secret::from_bytes(b"supersecretkey")
    /// );
    ///
    /// let uri = totp.to_uri();
    /// assert!(uri.starts_with("otpauth://totp/"));
    /// ```
    ///
    /// [Key URI Format]: https://github.com/google/google-authenticator/wiki/Key-Uri-Format
    pub fn to_uri(&self) -> String {
        let secret = self.secret().into_base32();
        let label = if self.issuer().is_empty() {
            encoding::url::encode(self.label().as_bytes())
        } else {
            encoding::url::encode(format!("{}:{}", &self.issuer(), &self.label()).as_bytes())
        };
        let issuer = if !self.issuer().is_empty() {
            format!(
                "&issuer={}",
                encoding::url::encode(self.issuer().as_bytes())
            )
        } else {
            String::new()
        };
        let digits = self.digits();
        let period = self.period;
        let alg = self.alg().to_string();

        format!(
            "otpauth://totp/{label}?secret={secret}{issuer}&algorithm={alg}&digits={digits}&period={period}"
        )
    }

    #[inline]
    pub fn alg(&self) -> Algorithm {
        self.hotp.alg()
    }

    #[inline]
    pub fn issuer(&self) -> &str {
        self.hotp.issuer()
    }

    #[inline]
    pub fn label(&self) -> &str {
        self.hotp.label()
    }

    #[inline]
    pub fn digits(&self) -> u8 {
        self.hotp.digits()
    }

    #[inline]
    pub fn secret(&self) -> &Secret {
        self.hotp.secret()
    }

    #[inline]
    pub fn period(&self) -> u64 {
        self.period
    }

    /// Parses a TOTP configuration from a URI string in the [Key URI Format].
    ///
    /// This function supports URIs of the form:
    /// `otpauth://totp/{label}?secret={secret}&issuer={issuer}&algorithm={algorithm}&digits={digits}&period={period}`
    ///
    /// # Arguments
    ///
    /// * `uri` - A string slice containing the TOTP URI.
    ///
    /// # Returns
    ///
    /// Returns `Ok(Totp)` if the URI is valid and can be parsed. Otherwise returns `Err(Error)`
    /// indicating the reason for failure.
    ///
    /// # Errors
    ///
    /// This method returns an error in the following cases:
    ///
    /// - URI does not start with the `otpauth://totp/` scheme.
    /// - Missing or empty label in the URI.
    /// - Missing or invalid query parameters (e.g., `secret`).
    /// - Unsupported or invalid algorithm name.
    /// - Base32 decoding of the secret fails.
    /// - Convert string errors (e.g., `period`, `digits`).
    /// - Invalid percent-encoding in the label or issuer.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use otp::Totp;
    ///
    /// let uri = "otpauth://totp/example:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=example&algorithm=SHA1&digits=6&period=30";
    /// let totp = Totp::from_uri(uri).unwrap();
    /// assert_eq!(totp.issuer(), "example");
    /// assert_eq!(totp.label(), "alice@example.com");
    /// ```
    ///
    /// [Key URI Format]: https://github.com/google/google-authenticator/wiki/Key-Uri-Format
    pub fn from_uri(uri: &str) -> Result<Self, ParseUriError> {
        let rest = uri
            .strip_prefix("otpauth://totp/")
            .ok_or(ParseUriError::InvalidPrefix)?;

        let (label_encoded, queries) = rest.split_once('?').ok_or(ParseUriError::InvalidFormat)?;
        if label_encoded.is_empty() {
            return Err(ParseUriError::InvalidLabel);
        }

        let label_decoded =
            encoding::url::decode(label_encoded).map_err(|_| ParseUriError::InvalidLabel)?;

        let (issuer_from_label, label) =
            if let Some((issuer, label)) = label_decoded.split_once(':') {
                (Some(issuer), label.to_string())
            } else {
                (None, label_decoded)
            };

        let params: std::collections::HashMap<&str, &str> = queries
            .split('&')
            .map(|param| match param.split_once('=') {
                Some((key, val)) => (key, val),
                None => (param, ""),
            })
            .collect();

        let digits = params.get("digits").map_or(Ok(6), |val| {
            val.parse::<u8>().map_err(|_| ParseUriError::InvalidDigits)
        })?;

        let period = params.get("period").map_or(Ok(30), |val| {
            val.parse::<u64>().map_err(|_| ParseUriError::InvalidPeriod)
        })?;

        let secret = params
            .get("secret")
            .ok_or(ParseUriError::MissingSecret)
            .and_then(|raw_secret| {
                Secret::from_base32(raw_secret).map_err(|_| ParseUriError::InvalidSecret)
            })?;

        let issuer_from_param = params
            .get("issuer")
            .map(|iss| encoding::url::decode(iss).map_err(|_| ParseUriError::InvalidIssuer))
            .transpose()?;

        let issuer = match (issuer_from_label, issuer_from_param) {
            (None, None) => Ok(String::new()),
            (None, Some(from_param)) => Ok(from_param),
            (Some(from_label), None) => Ok(from_label.to_string()),
            (Some(from_label), Some(from_param)) => {
                if from_label != from_param {
                    Err(ParseUriError::IssuerMismatch)
                } else {
                    Ok(from_param)
                }
            }
        }?;

        let alg = params
            .get("algorithm")
            .map(|alg| {
                let alg = alg.to_uppercase();
                match alg.as_str() {
                    "SHA1" => Ok(Algorithm::SHA1),
                    "SHA256" => Ok(Algorithm::SHA256),
                    "SHA512" => Ok(Algorithm::SHA512),
                    _ => Err(ParseUriError::InvalidAlgorithm),
                }
            })
            .transpose()?;

        Ok(Self::new(
            alg.unwrap_or(Algorithm::SHA1),
            issuer,
            label,
            digits,
            period,
            secret,
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseUriError {
    InvalidPrefix,
    InvalidFormat,
    InvalidLabel,
    InvalidIssuer,
    InvalidDigits,
    InvalidPeriod,
    InvalidSecret,
    InvalidAlgorithm,
    IssuerMismatch,
    MissingSecret,
}

impl std::fmt::Display for ParseUriError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseUriError::InvalidPrefix => {
                f.write_str("URI must start with 'otpauth://totp/'. Missing or incorrect prefix.")
            }
            ParseUriError::InvalidFormat => {
                f.write_str("URI has an incorrect general format. Ensure it follows 'otpauth://type/label?parameters'.")
            }
            ParseUriError::InvalidLabel => {
                f.write_str("The label (account name) in the URI is invalid or missing. Ensure it's properly encoded.")
            }
            ParseUriError::InvalidIssuer => {
                f.write_str("The 'issuer' parameter is invalid or missing a value. Ensure it's present and correctly encoded.")
            }
            ParseUriError::InvalidDigits => {
                f.write_str("The 'digits' parameter is invalid. It must be a positive integer, typically 6 or 8.")
            }
            ParseUriError::InvalidPeriod => {
                f.write_str("The 'period' parameter is invalid. It must be a positive integer, typically 30 or 60.")
            }
            ParseUriError::InvalidSecret => {
                f.write_str("The 'secret' parameter is invalid or not properly base32 encoded.")
            }
            ParseUriError::InvalidAlgorithm => {
                f.write_str("The 'algorithm' parameter is invalid. Expected 'SHA1', 'SHA256', or 'SHA512'.")
            }
            ParseUriError::IssuerMismatch => {
                f.write_str("The issuer specified in the label does not match the 'issuer' parameter.")
            }
            ParseUriError::MissingSecret => {
                f.write_str("The 'secret' parameter is required but missing from the URI.")
            }
        }
    }
}

impl std::error::Error for ParseUriError {}

#[cfg(test)]
impl Eq for Totp {}

#[cfg(test)]
impl PartialEq for Totp {
    fn eq(&self, other: &Self) -> bool {
        self.hotp == other.hotp && self.period == other.period
    }
}

#[cfg(test)]
impl std::fmt::Debug for Totp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Totp")
            .field("hotp", &self.hotp)
            .field("period", &self.period)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_uri() {
        let alg = Algorithm::SHA512;
        let issuer = String::from("");
        let label = String::from("alice@example.com");
        let digits = 6;
        let period = 30;
        let secret = Secret::from_bytes(b"The quick brown fox jumps over the lazy dog");

        let totp = Totp::new(alg, issuer, label, digits, period, secret);
        let totp_uri = totp.to_uri();

        let totp_from_uri = Totp::from_uri(&totp_uri).expect("should parse");

        assert_eq!(totp_uri, totp_from_uri.to_uri(), "should generate same uri");
        assert_eq!(totp, totp_from_uri, "should be equal");
    }

    #[test]
    fn test_from_uri_with_invalid_prefix() {
        let uri =
            "otpauth://hotp/issuer:alice@example.com?secret=JBSWY3DPEHPK3PXP&algorithm=SHA1024";
        let result = Totp::from_uri(uri);
        assert!(
            matches!(result, Err(ParseUriError::InvalidPrefix)),
            "should be invalid prefix"
        );
    }
}

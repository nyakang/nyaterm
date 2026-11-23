use std::ffi::{CStr, c_char, c_int};
use std::ptr;

use zeroize::Zeroizing;

use nyaterm_core::secret::SecretString;
use nyaterm_core::spice::settings::{SpiceEndpoint, SpiceSettings};

use crate::native::abi::Object;
use crate::native::{NativeError, NativeRuntime};

/// Owns the session on the GLib thread; borrowing the runtime keeps its context
/// alive until all native session objects have been released.
pub struct NativeSession<'runtime> {
    pub(super) runtime: &'runtime NativeRuntime,
    pub(super) object: Object,
}

impl<'runtime> NativeSession<'runtime> {
    pub fn new(runtime: &'runtime NativeRuntime) -> Result<Self, NativeError> {
        // SAFETY: these are public GObject construction properties and promoted
        // C int arguments. Construct directly to avoid spice_session_new's
        // unconditional USB manager initialization on clients without USB use.
        let object = unsafe {
            (runtime.api.g_object_new)(
                (runtime.api.spice_session_get_type)(),
                c"enable-usbredir".as_ptr(),
                0 as c_int,
                c"enable-smartcard".as_ptr(),
                0 as c_int,
                c"enable-audio".as_ptr(),
                0 as c_int,
                ptr::null::<c_char>(),
            )
        };
        if object.is_null() {
            return Err(NativeError::Allocation);
        }
        let session = Self { runtime, object };
        #[cfg(unix)]
        session.set_bool(c"gl-scanout", false);
        Ok(session)
    }

    pub fn configure(
        &self,
        endpoint: &SpiceEndpoint,
        settings: &SpiceSettings,
        username: &SecretString,
        password: &SecretString,
    ) -> Result<(), NativeError> {
        settings.security.validate(endpoint)?;
        self.set_string(c"host", endpoint.host.expose_secret())?;
        self.set_string(c"username", username.expose_secret())?;
        self.set_string(c"password", password.expose_secret())?;
        // Omitting the plaintext port is enforced in the helper, in addition
        // to requesting secure channels. TLS failures cannot change policy.
        if !settings.security.require_tls
            && let Some(port) = endpoint.port
        {
            self.set_string(c"port", &port.to_string())?;
        } else {
            self.clear_string(c"port");
        }
        if let Some(port) = endpoint.tls_port {
            self.set_string(c"tls-port", &port.to_string())?;
        } else {
            self.clear_string(c"tls-port");
        }
        if let Some(proxy) = &endpoint.proxy {
            self.set_string(c"proxy", proxy.expose_secret())?;
        } else {
            self.clear_string(c"proxy");
        }
        if let Some(subject) = &settings.security.certificate_subject {
            self.set_string(c"cert-subject", subject)?;
        } else {
            self.clear_string(c"cert-subject");
        }
        if let Some(ciphers) = &settings.security.tls_ciphers {
            self.set_string(c"ciphers", ciphers)?;
        } else {
            self.clear_string(c"ciphers");
        }
        // An opaque proxy ticket cannot be matched as a DNS name. Proxmox's
        // supplied subject is still checked against the CA-validated peer.
        let verify: u32 = if settings.security.certificate_subject.is_some() {
            4
        } else {
            2
        };
        unsafe {
            (self.runtime.api.g_object_set)(
                self.object,
                c"verify".as_ptr(),
                verify,
                ptr::null::<c_char>(),
            );
        }
        let channels: Vec<_> = if settings.security.require_tls {
            vec!["all"]
        } else {
            settings
                .security
                .secure_channels
                .iter()
                .map(|c| c.as_str())
                .collect()
        };
        self.set_string_list(c"secure-channels", &channels)?;
        self.clear_string(c"ca-file");
        if settings.security.ca_pem.is_none() && endpoint.tls_port.is_some() {
            // Bundled OpenSSL's build-machine trust path is not portable. The
            // release bundle carries an explicit Mozilla CA snapshot. Imported
            // inline CA data keeps its existing, exclusive trust semantics.
            let bundle = self.runtime.directory.join("ca-bundle.pem");
            let ca = read_ca_bundle(&bundle)?;
            // Use bytes rather than OpenSSL's narrow filename API so an
            // installation under a non-ASCII Windows path has identical trust.
            self.set_ca(Some(&ca))?;
        } else {
            self.set_ca(settings.security.ca_pem.as_deref())?;
        }
        self.set_bool(c"read-only", settings.view_only);
        self.set_bool(c"enable-audio", settings.audio_playback);
        // Device access is never authorized by an imported connection file.
        self.set_bool(c"enable-usbredir", false);
        self.set_bool(c"enable-smartcard", false);
        Ok(())
    }

    pub fn device_redirection_disabled(&self) -> bool {
        !self.get_bool(c"enable-usbredir") && !self.get_bool(c"enable-smartcard")
    }

    pub(super) fn set_bool(&self, name: &CStr, value: bool) {
        // SAFETY: all callers supply a documented boolean property.
        unsafe {
            (self.runtime.api.g_object_set)(
                self.object,
                name.as_ptr(),
                c_int::from(value),
                ptr::null::<c_char>(),
            );
        }
    }

    fn get_bool(&self, name: &CStr) -> bool {
        let mut value: c_int = 0;
        // SAFETY: value has the required gboolean layout and remains alive.
        unsafe {
            (self.runtime.api.g_object_get)(
                self.object,
                name.as_ptr(),
                &mut value as *mut c_int,
                ptr::null::<c_char>(),
            );
        }
        value != 0
    }

    fn clear_string(&self, name: &CStr) {
        unsafe {
            (self.runtime.api.g_object_set)(
                self.object,
                name.as_ptr(),
                ptr::null::<c_char>(),
                ptr::null::<c_char>(),
            );
        }
    }

    fn set_string(&self, name: &'static CStr, value: &str) -> Result<(), NativeError> {
        if value.contains('\0') {
            return Err(NativeError::InvalidField(name.to_str().unwrap_or("string")));
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(value.len() + 1));
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
        // SAFETY: GObject copies the NUL-terminated property before returning.
        unsafe {
            (self.runtime.api.g_object_set)(
                self.object,
                name.as_ptr(),
                bytes.as_ptr().cast::<c_char>(),
                ptr::null::<c_char>(),
            );
        }
        Ok(())
    }

    fn set_string_list(&self, name: &'static CStr, values: &[&str]) -> Result<(), NativeError> {
        let strings: Vec<_> = values
            .iter()
            .map(|value| std::ffi::CString::new(*value))
            .collect::<Result<_, _>>()
            .map_err(|_| NativeError::InvalidField("channels"))?;
        let pointers: Vec<_> = strings
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(ptr::null()))
            .collect();
        // SAFETY: the property is G_TYPE_STRV, which copies the terminated list.
        unsafe {
            (self.runtime.api.g_object_set)(
                self.object,
                name.as_ptr(),
                pointers.as_ptr(),
                ptr::null::<c_char>(),
            );
        }
        Ok(())
    }

    fn set_ca(&self, ca: Option<&str>) -> Result<(), NativeError> {
        // SAFETY: SpiceSession:ca takes a GByteArray boxed reference. The
        // session retains it; our temporary reference is released below.
        unsafe {
            let array = if let Some(ca) = ca {
                let array = (self.runtime.api.g_byte_array_new)();
                if array.is_null() {
                    return Err(NativeError::Allocation);
                }
                (self.runtime.api.g_byte_array_append)(array, ca.as_ptr(), ca.len() as u32);
                array
            } else {
                ptr::null_mut()
            };
            (self.runtime.api.g_object_set)(
                self.object,
                c"ca".as_ptr(),
                array,
                ptr::null::<c_char>(),
            );
            if !array.is_null() {
                (self.runtime.api.g_byte_array_unref)(array);
            }
        }
        Ok(())
    }
}

fn read_ca_bundle(path: &std::path::Path) -> Result<String, NativeError> {
    use std::io::Read;
    const MAX_BUNDLE_BYTES: u64 = 4 * 1024 * 1024;
    let read = || -> std::io::Result<String> {
        let file = std::fs::File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(std::io::Error::other("CA bundle is not a regular file"));
        }
        let mut content = String::new();
        file.take(MAX_BUNDLE_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.is_empty() || content.len() as u64 > MAX_BUNDLE_BYTES {
            return Err(std::io::Error::other("CA bundle size is invalid"));
        }
        Ok(content)
    };
    read().map_err(|_| NativeError::InvalidField("packaged CA bundle"))
}

impl Drop for NativeSession<'_> {
    fn drop(&mut self) {
        // Clear the library's credential copies as soon as the session closes.
        self.clear_string(c"password");
        self.clear_string(c"username");
        unsafe {
            (self.runtime.api.spice_session_disconnect)(self.object);
            // spice_session_disconnect schedules an idle callback holding a
            // session ref. Drive cleanup before dropping the owning context.
            for _ in 0..64 {
                if (self.runtime.api.g_main_context_iteration)(self.runtime.context, 0) == 0 {
                    break;
                }
            }
            (self.runtime.api.g_object_unref)(self.object);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::native::session::read_ca_bundle;

    #[test]
    fn packaged_ca_reads_unicode_paths_and_rejects_oversized_or_invalid_text() {
        let root = nyaterm_core::test_support::TestTempDir::new("nyaterm-spice-ca-unicode");
        std::fs::create_dir_all(root.path()).unwrap();
        let path = root.path().join("证书-é.pem");
        std::fs::write(&path, "fixture PEM bytes").unwrap();
        assert_eq!(read_ca_bundle(&path).unwrap(), "fixture PEM bytes");
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_ca_bundle(&path).is_err());
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(4 * 1024 * 1024 + 1).unwrap();
        assert!(read_ca_bundle(&path).is_err());
    }
}

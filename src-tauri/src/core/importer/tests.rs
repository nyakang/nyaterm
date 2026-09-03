
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_JSON: &str = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "prod-root-password", "name": "Prod root password", "password": "replace-me" }
  ],
  "ssh_keys": [
    {
      "ref": "ops-ed25519",
      "name": "Ops ED25519",
      "private_key": "-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----",
      "passphrase": "optional-passphrase"
    }
  ],
  "groups": [
    { "path": ["Production"] },
    { "path": ["Production", "Web"] },
    { "path": ["Lab"] }
  ],
  "sessions": [
    {
      "name": "Prod web direct password",
      "type": "ssh",
      "group_path": ["Production", "Web"],
      "host": "web-01.example.com",
      "port": 22,
      "username": "deploy",
      "dynamic_tab_title": true,
      "auth": { "mode": "password", "password": "replace-me" }
    },
    {
      "name": "Prod db saved password",
      "type": "ssh",
      "group_path": ["Production", "Database"],
      "host": "db-01.example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "prod-root-password" }
    },
    {
      "name": "Bastion saved key",
      "type": "ssh",
      "group_path": ["Production"],
      "host": "bastion.example.com",
      "username": "ops",
      "auth": { "mode": "key", "key_ref": "ops-ed25519" }
    },
    {
      "name": "Lab router",
      "type": "telnet",
      "group_path": ["Lab"],
      "host": "192.168.10.1",
      "port": 23,
      "backspace_mode": "del"
    },
    {
      "name": "USB console",
      "type": "serial",
      "group_path": ["Lab"],
      "port_name": "COM3",
      "baud_rate": 115200,
      "data_bits": 8,
      "parity": "none",
      "stop_bits": "1",
      "backspace_mode": "ctrl_h"
    },
    {
      "name": "Local PowerShell",
      "type": "local_terminal",
      "shell_path": "pwsh.exe",
      "shell_args": "-NoLogo",
      "working_dir": "C:\\Users\\me",
      "dynamic_tab_title": true
    }
  ]
}
"#;

    const ELECTERM_SAMPLE_JSON: &str = r##"
{
  "bookmarkGroups": [
    {
      "id": "duQ8j9f",
      "title": "dev",
      "bookmarkIds": [
        "PfSRKWV"
      ],
      "color": "#e99695",
      "level": 1
    },
    {
      "id": "default",
      "title": "default",
      "bookmarkIds": [
        "3czUiXi"
      ],
      "bookmarkGroupIds": []
    }
  ],
  "bookmarks": [
    {
      "id": "PfSRKWV",
      "title": "77",
      "host": "192.168.142.77",
      "username": "root",
      "authType": "password",
      "port": 22,
      "useSshAgent": true,
      "sshAgent": "",
      "runScripts": [
        {
          "delay": 500,
          "script": ""
        }
      ],
      "envLang": "en_US.UTF-8",
      "encode": "utf-8",
      "type": "ssh",
      "enableSsh": true,
      "enableSftp": true,
      "term": "xterm-256color",
      "displayRaw": false,
      "cipher": [],
      "compress": [],
      "serverHostKey": [],
      "sshTunnels": [],
      "connectionHoppings": [],
      "color": "#ffab4a",
      "quickCommands": []
    },
    {
      "id": "3czUiXi",
      "title": "56",
      "host": "192.168.142.56",
      "username": "root",
      "authType": "password",
      "port": 22,
      "useSshAgent": true,
      "sshAgent": "",
      "runScripts": [
        {
          "delay": 500,
          "script": ""
        }
      ],
      "envLang": "en_US.UTF-8",
      "encode": "utf-8",
      "type": "ssh",
      "enableSsh": true,
      "enableSftp": true,
      "term": "xterm-256color",
      "displayRaw": false,
      "cipher": [],
      "compress": [],
      "serverHostKey": [],
      "sshTunnels": [],
      "connectionHoppings": [],
      "color": "#24292e",
      "quickCommands": []
    }
  ]
}
"##;

    #[test]
    fn windterm_import_splits_user_at_host_targets() {
        let prepared = parse_windterm_content(
            r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "deploy@192.168.1.10",
    "session.label": "Prod web",
    "session.port": 2222
  }
]
"#,
        )
        .expect("parse windterm sessions");

        assert_eq!(prepared.connections.len(), 1);
        assert_eq!(prepared.connections[0].name, "Prod web");
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh {
                host,
                username,
                port,
                ..
            } if host == "192.168.1.10" && username == "deploy" && *port == 2222
        ));
    }

    #[test]
    fn windterm_import_defaults_username_when_target_has_no_user() {
        let prepared = parse_windterm_content(
            r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "192.168.1.10"
  }
]
"#,
        )
        .expect("parse windterm sessions");

        assert_eq!(prepared.connections.len(), 1);
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh {
                host, username, ..
            } if host == "192.168.1.10" && username == "root"
        ));
    }

    #[test]
    fn windterm_decrypts_encrypted_auto_login() {
        let crypto = derive_windterm_crypto("fingerprint-1", "master-secret");
        let encrypted = encrypt_windterm_auto_login_for_test(
            r#"{"Password":"secret","PasswordEnabled":true,"session.user":"deploy"}"#,
            &crypto,
        );
        let content = format!(
            r#"
[
  {{
    "session.protocol": "SSH",
    "session.target": "192.168.1.10",
    "session.autoLogin": "{encrypted}"
  }}
]
"#
        );

        let prepared =
            parse_windterm_content_with_crypto(&content, Some(&crypto), None).expect("parse");

        assert_eq!(prepared.connections.len(), 1);
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh { username, .. } if username == "deploy"
        ));
        let auth = prepared.connections[0].auth.as_ref().expect("auth");
        assert_eq!(auth.mode, "password");
        assert_eq!(auth.password.as_deref(), Some("test-encrypted:secret"));
    }

    #[test]
    fn windterm_requires_master_password_when_enabled() {
        let root = importer_test_dir("windterm-master-password-required");
        let terminal = root.join("terminal");
        std::fs::create_dir_all(&terminal).expect("create windterm profile");
        let sessions_path = terminal.join("user.sessions");
        std::fs::write(&sessions_path, "[]").expect("write sessions");
        std::fs::write(
            root.join("user.config"),
            r#"{"application.fingerprint":"fingerprint-1","application.masterPassword":true}"#,
        )
        .expect("write config");

        let error = parse_windterm(sessions_path.to_str().expect("utf8 path"), None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("WindTerm master password is required")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn windterm_password_auth_uses_inline_encrypted_password() {
        let prepared = parse_windterm_content(
            r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "fallback@example.com",
    "session.autoLogin": "{\"Password\":\"secret\",\"PasswordEnabled\":true,\"session.user\":\"deploy\"}"
  }
]
"#,
        )
        .expect("parse windterm sessions");

        assert!(prepared.passwords.is_empty());
        assert!(prepared.ssh_keys.is_empty());
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh { username, .. } if username == "deploy"
        ));
        let auth = prepared.connections[0].auth.as_ref().expect("auth");
        assert_eq!(auth.mode, "password");
        assert!(auth.password_id.is_none());
        assert_eq!(auth.password.as_deref(), Some("test-encrypted:secret"));
    }

    #[test]
    fn windterm_key_auth_reads_key_file_and_reuses_matching_keys() {
        let root = importer_test_dir("windterm-key-auth");
        let terminal = root.join("terminal");
        std::fs::create_dir_all(&terminal).expect("create windterm profile");
        let key_path = root.join("id_ed25519");
        std::fs::write(&key_path, "private key material").expect("write key");
        let escaped_key_path =
            serde_json::to_string(&key_path.to_string_lossy()).expect("escape key path");
        let auto_login = serde_json::json!({
            "Public Key": {
                "windows.pass": "passphrase",
                "windows.path": key_path.to_string_lossy(),
            },
            "session.user": "root",
        })
        .to_string();
        let escaped_auto_login = serde_json::to_string(&auto_login).expect("escape auto login");
        let content = format!(
            r#"
[
  {{
    "session.protocol": "SSH",
    "session.target": "one.example.com",
    "session.label": "One",
    "session.autoLogin": {escaped_auto_login}
  }},
  {{
    "session.protocol": "SSH",
    "session.target": "two.example.com",
    "session.label": "Two",
    "ssh.identityFilePath.windows": {escaped_key_path}
  }}
]
"#
        );
        let sessions_path = terminal.join("user.sessions");

        let prepared = parse_windterm_content_with_crypto(&content, None, Some(&sessions_path))
            .expect("parse windterm sessions");

        assert_eq!(prepared.connections.len(), 2);
        assert_eq!(prepared.ssh_keys.len(), 1);
        assert_eq!(
            prepared.ssh_keys[0].key.as_deref(),
            Some("test-encrypted:private key material")
        );
        assert_eq!(
            prepared.ssh_keys[0].passphrase.as_deref(),
            Some("test-encrypted:passphrase")
        );
        let first_auth = prepared.connections[0].auth.as_ref().expect("first auth");
        let second_auth = prepared.connections[1].auth.as_ref().expect("second auth");
        assert_eq!(first_auth.mode, "key");
        assert_eq!(second_auth.mode, "key");
        assert_eq!(first_auth.key_id, second_auth.key_id);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn windterm_unreadable_key_path_falls_back_to_none_auth() {
        let prepared = parse_windterm_content(
            r#"
[
  {
    "session.protocol": "SSH",
    "session.target": "example.com",
    "ssh.identityFilePath.windows": "Z:/definitely/missing/key"
  }
]
"#,
        )
        .expect("parse windterm sessions");

        assert!(prepared.ssh_keys.is_empty());
        let auth = prepared.connections[0].auth.as_ref().expect("auth");
        assert_eq!(auth.mode, "none");
        assert!(auth.key_id.is_none());
    }

    #[test]
    fn windterm_target_rejects_empty_user_or_host_splits() {
        assert_eq!(
            parse_windterm_target("@192.168.1.10"),
            ("@192.168.1.10".to_string(), "root".to_string())
        );
        assert_eq!(
            parse_windterm_target("deploy@"),
            ("deploy@".to_string(), "root".to_string())
        );
    }

    #[test]
    fn windterm_target_splits_on_last_at_symbol() {
        assert_eq!(
            parse_windterm_target("ops@team@example.com"),
            ("example.com".to_string(), "ops@team".to_string())
        );
    }

    #[test]
    fn securecrt_imports_nested_ssh_sessions() {
        let sessions = parse_securecrt_content(
            r#"
<VanDyke version="3.0">
  <key name="Sessions">
    <key name="dev">
      <key name="New">
        <dword name="[SSH2] Port">2222</dword>
        <string name="Hostname">192.168.1.20</string>
        <string name="Protocol Name">SSH2</string>
        <string name="Username">deploy</string>
      </key>
    </key>
  </key>
</VanDyke>
"#,
        )
        .expect("parse securecrt sessions");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "New");
        assert_eq!(sessions[0].host, "192.168.1.20");
        assert_eq!(sessions[0].port, 2222);
        assert_eq!(sessions[0].username, "deploy");
        assert_eq!(sessions[0].group_path, Some(vec!["dev".to_string()]));
    }

    #[test]
    fn securecrt_skips_blank_host_and_non_ssh_sessions() {
        let sessions = parse_securecrt_content(
            r#"
<VanDyke version="3.0">
  <key name="Sessions">
    <key name="Default">
      <string name="Hostname"/>
      <string name="Protocol Name">SSH2</string>
      <string name="Username"/>
    </key>
    <key name="Remote Desktop">
      <string name="Hostname">192.168.1.30</string>
      <dword name="Port">3389</dword>
      <string name="Protocol Name">RDP</string>
    </key>
    <key name="Valid">
      <string name="Hostname">192.168.1.31</string>
      <string name="Protocol Name">SSH2</string>
    </key>
  </key>
</VanDyke>
"#,
        )
        .expect("parse securecrt sessions");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "Valid");
        assert_eq!(sessions[0].username, "root");
        assert_eq!(sessions[0].port, 22);
    }

    #[test]
    fn finalshell_imports_root_and_nested_connections() {
        let root = importer_test_dir("finalshell-imports-root-and-nested");
        let nested = root.join("folder-1");
        std::fs::create_dir_all(&nested).expect("create finalshell test dir");
        std::fs::write(
            nested.join("folder.json"),
            r#"{"id":"folder-1","name":"Prod","parent_id":"root","delete_time":0}"#,
        )
        .expect("write folder");
        std::fs::write(
            root.join("root_connect_config.json"),
            r#"{"name":"Root Host","host":"10.0.0.1","port":22,"user_name":"root","parent_id":"root","conection_type":100,"description":"root desc","delete_time":0}"#,
        )
        .expect("write root conn");
        std::fs::write(
            nested.join("nested_connect_config.json"),
            r#"{"name":"Nested Host","host":"10.0.0.2","port":2222,"user_name":"deploy","parent_id":"folder-1","conection_type":100,"description":"nested desc","delete_time":0}"#,
        )
        .expect("write nested conn");

        let mut sessions = parse_finalshell(root.to_str().expect("utf8 path")).expect("parse dir");
        sessions.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "Nested Host");
        assert_eq!(sessions[0].host, "10.0.0.2");
        assert_eq!(sessions[0].port, 2222);
        assert_eq!(sessions[0].username, "deploy");
        assert_eq!(sessions[0].group_path, Some(vec!["Prod".to_string()]));
        assert_eq!(sessions[0].description, Some("nested desc".to_string()));
        assert_eq!(sessions[1].name, "Root Host");
        assert_eq!(sessions[1].group_path, None);
        assert_eq!(sessions[1].description, Some("root desc".to_string()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn finalshell_skips_non_ssh_and_empty_host_connections() {
        let root = importer_test_dir("finalshell-skips-invalid");
        std::fs::create_dir_all(&root).expect("create finalshell test dir");
        std::fs::write(
            root.join("rdp_connect_config.json"),
            r#"{"name":"RDP","host":"10.0.0.3","port":3389,"user_name":"root","parent_id":"root","conection_type":101,"delete_time":0}"#,
        )
        .expect("write rdp conn");
        std::fs::write(
            root.join("empty_connect_config.json"),
            r#"{"name":"Empty","host":"","port":22,"user_name":"root","parent_id":"root","conection_type":100,"delete_time":0}"#,
        )
        .expect("write empty conn");
        std::fs::write(
            root.join("valid_connect_config.json"),
            r#"{"name":"Valid","host":"10.0.0.4","port":0,"user_name":"","parent_id":"root","conection_type":100,"delete_time":0}"#,
        )
        .expect("write valid conn");

        let sessions = parse_finalshell(root.to_str().expect("utf8 path")).expect("parse dir");

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "Valid");
        assert_eq!(sessions[0].port, 22);
        assert_eq!(sessions[0].username, "root");

        let _ = std::fs::remove_dir_all(root);
    }

    fn importer_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        std::env::temp_dir().join(format!("nyaterm-importer-{name}-{nanos}"))
    }

    fn encrypt_windterm_auto_login_for_test(plaintext: &str, crypto: &WindtermCrypto) -> String {
        use cbc::cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};

        let mut buffer = vec![0_u8; plaintext.len() + WINDTERM_AES_IV_LENGTH];
        buffer[..plaintext.len()].copy_from_slice(plaintext.as_bytes());
        let ciphertext =
            cbc::Encryptor::<aes::Aes256>::new(&crypto.key.into(), &crypto.iv.into())
                .encrypt_padded_mut::<Pkcs7>(&mut buffer, plaintext.len())
                .expect("encrypt windterm payload");
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, ciphertext)
    }

    #[test]
    fn nyaterm_json_sample_import_prepares_supported_shapes() {
        crate::utils::crypto::set_master_password(None);

        let prepared = parse_json_import_content(SAMPLE_JSON).expect("parse sample");

        assert_eq!(prepared.groups.len(), 3);
        assert_eq!(prepared.passwords.len(), 1);
        assert_eq!(prepared.ssh_keys.len(), 1);
        assert_eq!(prepared.connections.len(), 6);
        assert_ne!(
            prepared.passwords[0].password.as_deref(),
            Some("replace-me")
        );
        assert_ne!(
            prepared.ssh_keys[0].key.as_deref(),
            Some("-----BEGIN OPENSSH PRIVATE KEY-----\n...\n-----END OPENSSH PRIVATE KEY-----")
        );

        let direct_auth = prepared.connections[0].auth.as_ref().expect("direct auth");
        assert_eq!(direct_auth.mode, "password");
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh {
                dynamic_tab_title: true,
                ..
            }
        ));
        assert!(direct_auth.password_id.is_none());
        assert_ne!(direct_auth.password.as_deref(), Some("replace-me"));

        let saved_password_auth = prepared.connections[1]
            .auth
            .as_ref()
            .expect("saved password auth");
        assert_eq!(saved_password_auth.mode, "password");
        assert!(saved_password_auth.password_id.is_some());
        assert!(saved_password_auth.password.is_none());

        let key_auth = prepared.connections[2].auth.as_ref().expect("key auth");
        assert_eq!(key_auth.mode, "key");
        assert!(key_auth.key_id.is_some());

        let local_config = &prepared.connections[5].config;
        assert!(matches!(
            local_config,
            ConnectionType::LocalTerminal {
                shell_path,
                shell_args,
                dynamic_tab_title: true,
                ..
            } if shell_path == "pwsh.exe" && shell_args == "-NoLogo"
        ));
    }

    #[test]
    fn nyaterm_json_rejects_duplicate_password_refs() {
        let json = r#"
{
  "version": 1,
  "passwords": [
    { "ref": "dup", "name": "One", "password": "a" },
    { "ref": "dup", "name": "Two", "password": "b" }
  ],
  "sessions": []
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(error.to_string().contains("Duplicate password ref"));
    }

    #[test]
    fn nyaterm_json_rejects_missing_password_refs() {
        let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Missing password",
      "type": "ssh",
      "host": "example.com",
      "username": "root",
      "auth": { "mode": "password", "password_ref": "missing" }
    }
  ]
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("password_ref 'missing' was not found")
        );
    }

    #[test]
    fn nyaterm_json_rejects_invalid_ports() {
        let json = r#"
{
  "version": 1,
  "sessions": [
    {
      "name": "Bad port",
      "type": "ssh",
      "host": "example.com",
      "port": 0,
      "username": "root",
      "auth": { "mode": "none" }
    }
  ]
}
"#;

        let error = parse_nyaterm_json_content(json).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("port must be between 1 and 65535")
        );
    }

    #[test]
    fn termius_normalizes_local_key_variants() {
        use base64::Engine as _;

        let raw = "12345678901234567890123456789012";
        let key = normalize_termius_local_key(raw).expect("raw key");
        assert_eq!(key.as_ref(), raw.as_bytes());

        let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        let key = normalize_termius_local_key(&encoded).expect("base64 key");
        assert_eq!(key.as_ref(), raw.as_bytes());

        let key = normalize_termius_local_key_bytes(raw.as_bytes()).expect("raw bytes key");
        assert_eq!(key.as_ref(), raw.as_bytes());

        let utf16le: Vec<u8> = raw
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let key = normalize_termius_local_key_bytes(&utf16le).expect("utf16 key");
        assert_eq!(key.as_ref(), raw.as_bytes());

        let json = format!(r#"{{"localKey":"{encoded}"}}"#);
        let key = normalize_termius_local_key(&json).expect("json key");
        assert_eq!(key.as_ref(), raw.as_bytes());
    }

    #[test]
    fn termius_decrypts_secretbox_values_and_rejects_corruption() {
        let key = *b"12345678901234567890123456789012";
        let nonce = *b"abcdefghijklmnopqrstuvwx";
        let encrypted = encrypt_termius_secret_for_test("hello", &key, &nonce);

        let decrypted = decrypt_termius_secret(&encrypted, &key).expect("decrypt value");
        assert_eq!(decrypted.as_str(), "hello");

        let mut corrupted = encrypted;
        corrupted.replace_range(corrupted.len() - 2.., "AA");
        let error = decrypt_termius_secret(&corrupted, &key).unwrap_err();
        assert!(error.to_string().contains("Cannot decrypt Termius"));
    }

    #[test]
    fn termius_tagged_string_parser_handles_multibyte_lengths() {
        let long = "x".repeat(300);
        let mut bytes = Vec::new();
        tagged_string("private_key", &mut bytes);
        tagged_string(&long, &mut bytes);

        let strings = parse_tagged_strings(&bytes);
        assert_eq!(strings, vec!["private_key".to_string(), long]);
    }

    #[test]
    fn termius_maps_host_ports_from_linked_ssh_configs() {
        let mut bytes = Vec::new();
        tagged_string("id", &mut bytes);
        tagged_integer(20_265_758, &mut bytes);
        tagged_string("updated_at", &mut bytes);
        tagged_string("2024-04-30T03:36:37", &mut bytes);
        tagged_string("address", &mut bytes);
        tagged_string("ignored.example.com", &mut bytes);
        tagged_string("status", &mut bytes);
        tagged_string("SYNCHRONIZED", &mut bytes);
        tagged_integer(0, &mut bytes);
        tagged_integer(0, &mut bytes);

        tagged_string("id", &mut bytes);
        tagged_integer(20_265_758, &mut bytes);
        tagged_string("updated_at", &mut bytes);
        tagged_string("2024-04-30T03:36:37", &mut bytes);
        tagged_string("identity", &mut bytes);
        tagged_string("id", &mut bytes);
        tagged_integer(13_239_297, &mut bytes);
        tagged_string("port", &mut bytes);
        tagged_integer(22123, &mut bytes);
        tagged_string("resource_uri", &mut bytes);
        tagged_string("/api/v3/terminal/ssh/config/20265758/", &mut bytes);
        tagged_string("status", &mut bytes);
        tagged_string("SYNCHRONIZED", &mut bytes);
        tagged_integer(0, &mut bytes);
        tagged_integer(0, &mut bytes);

        tagged_string("id", &mut bytes);
        tagged_integer(19_657_566, &mut bytes);
        tagged_string("updated_at", &mut bytes);
        tagged_string("2024-04-30T03:36:37", &mut bytes);
        tagged_string("ssh_config", &mut bytes);
        tagged_string("id", &mut bytes);
        tagged_integer(20_265_758, &mut bytes);
        tagged_string("address", &mut bytes);
        tagged_string("example.com", &mut bytes);
        tagged_string("label", &mut bytes);
        tagged_string("Example", &mut bytes);
        tagged_string("resource_uri", &mut bytes);
        tagged_string("/api/v3/terminal/host/19657566/", &mut bytes);
        tagged_string("status", &mut bytes);
        tagged_string("SYNCHRONIZED", &mut bytes);

        let values = parse_tagged_values(&bytes);
        assert!(values.contains(&TermiusTaggedValue::Integer(22123)));
        let store = collect_termius_store(&values);
        assert_eq!(store.ssh_configs.len(), 1);
        assert_eq!(store.hosts.len(), 1);
        assert_eq!(store.hosts[0].ssh_config_id, Some("20265758".to_string()));
        assert_eq!(store.ssh_configs[0].port, Some(22123));

        let prepared = prepare_termius_import(store).expect("prepare import");
        assert_eq!(prepared.connections.len(), 1);
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh { port: 22123, .. }
        ));
    }

    #[test]
    fn electerm_imports_sample_bookmarks_with_groups() {
        let prepared = parse_json_import_content(ELECTERM_SAMPLE_JSON).expect("parse electerm");

        assert_eq!(prepared.passwords.len(), 0);
        assert_eq!(prepared.ssh_keys.len(), 0);
        assert_eq!(prepared.connections.len(), 2);
        assert_eq!(prepared.groups.len(), 2);

        let first = &prepared.connections[0];
        assert_eq!(first.name, "77");
        assert_eq!(first.group_path, Some(vec!["dev".to_string()]));
        assert!(first.icon.is_none());
        assert_eq!(first.auth.as_ref().expect("auth").mode, "password");
        assert!(matches!(
            &first.config,
            ConnectionType::Ssh {
                host,
                port: 22,
                username,
                ..
            } if host == "192.168.142.77" && username == "root"
        ));

        let second = &prepared.connections[1];
        assert_eq!(second.name, "56");
        assert_eq!(second.group_path, Some(vec!["default".to_string()]));
        assert!(matches!(
            &second.config,
            ConnectionType::Ssh {
                host,
                port: 22,
                username,
                ..
            } if host == "192.168.142.56" && username == "root"
        ));
    }

    #[test]
    fn electerm_imports_nested_bookmark_groups() {
        let json = r#"
{
  "bookmarkGroups": [
    { "id": "prod", "title": "Prod", "bookmarkGroupIds": ["web"] },
    { "id": "web", "title": "Web", "bookmarkIds": ["host-1"] }
  ],
  "bookmarks": [
    {
      "id": "host-1",
      "title": "App",
      "host": "app.example.com",
      "username": "deploy",
      "authType": "password",
      "port": 2222,
      "type": "ssh"
    }
  ]
}
"#;

        let prepared = parse_json_import_content(json).expect("parse electerm");

        assert_eq!(prepared.groups, vec![vec!["Prod".to_string(), "Web".to_string()]]);
        assert_eq!(
            prepared.connections[0].group_path,
            Some(vec!["Prod".to_string(), "Web".to_string()])
        );
        assert!(matches!(
            &prepared.connections[0].config,
            ConnectionType::Ssh { port: 2222, .. }
        ));
    }

    #[test]
    fn electerm_skips_unsupported_bookmarks_and_defaults_missing_fields() {
        let json = r#"
{
  "bookmarkGroups": [
    { "id": "default", "title": "default", "bookmarkIds": ["disabled", "non-ssh", "empty", "bad-port", "valid"] }
  ],
  "bookmarks": [
    { "id": "disabled", "title": "Disabled", "host": "disabled.example.com", "type": "ssh", "enableSsh": false },
    { "id": "non-ssh", "title": "Telnet", "host": "telnet.example.com", "type": "telnet" },
    { "id": "empty", "title": "Empty", "host": "", "type": "ssh" },
    { "id": "bad-port", "title": "Bad Port", "host": "bad.example.com", "type": "ssh", "port": 70000 },
    { "id": "valid", "title": "", "host": "valid.example.com", "type": "ssh", "authType": "publickey" }
  ]
}
"#;

        let prepared = parse_json_import_content(json).expect("parse electerm");

        assert_eq!(prepared.connections.len(), 1);
        let connection = &prepared.connections[0];
        assert_eq!(connection.name, "valid.example.com");
        assert_eq!(connection.group_path, Some(vec!["default".to_string()]));
        assert_eq!(connection.auth.as_ref().expect("auth").mode, "none");
        assert!(matches!(
            &connection.config,
            ConnectionType::Ssh {
                host,
                port: 22,
                username,
                ..
            } if host == "valid.example.com" && username == "root"
        ));
    }

    #[test]
    fn json_import_rejects_unknown_json_shapes() {
        let error = parse_json_import_content(r#"{"bookmarks":[]}"#).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Expected NyaTerm JSON or Electerm bookmarks JSON")
        );
    }

    #[test]
    fn termius_maps_hosts_groups_passwords_and_keys() {
        crate::utils::crypto::set_master_password(None);

        let store = TermiusRawStore {
            ssh_configs: Vec::new(),
            groups: vec![TermiusRawGroup {
                id: "group-1".to_string(),
                local_id: Some("group-1".to_string()),
                label: Some("Production".to_string()),
                parent_id: None,
                updated_at: Some("2024-01-01T00:00:00".to_string()),
            }],
            ssh_keys: vec![TermiusRawSshKey {
                id: "key-1".to_string(),
                local_id: Some("key-1".to_string()),
                label: Some("Ops key".to_string()),
                passphrase: Some("passphrase".to_string()),
                private_key: Some(
                    "-----BEGIN OPENSSH PRIVATE KEY-----\nkey\n-----END OPENSSH PRIVATE KEY-----"
                        .to_string(),
                ),
                updated_at: Some("2024-01-01T00:00:00".to_string()),
            }],
            identities: vec![TermiusRawIdentity {
                id: "identity-1".to_string(),
                local_id: Some("identity-1".to_string()),
                label: Some("Deploy".to_string()),
                username: Some("deploy".to_string()),
                password: Some("secret".to_string()),
                ssh_key_id: Some("key-1".to_string()),
                updated_at: Some("2024-01-01T00:00:00".to_string()),
            }],
            hosts: vec![
                TermiusRawHost {
                    id: "host-1".to_string(),
                    local_id: Some("host-1".to_string()),
                    label: Some("Web".to_string()),
                    address: Some("web.example.com".to_string()),
                    username: None,
                    password: None,
                    ssh_config_id: None,
                    identity_id: Some("identity-1".to_string()),
                    group_id: Some("group-1".to_string()),
                    port: Some(2222),
                    updated_at: Some("2024-01-01T00:00:00".to_string()),
                },
                TermiusRawHost {
                    id: "host-2".to_string(),
                    local_id: Some("host-2".to_string()),
                    label: Some("Db".to_string()),
                    address: Some("db.example.com".to_string()),
                    username: Some("root".to_string()),
                    password: Some("db-pass".to_string()),
                    ssh_config_id: None,
                    identity_id: None,
                    group_id: None,
                    port: None,
                    updated_at: Some("2024-01-01T00:00:00".to_string()),
                },
            ],
        };

        let prepared = prepare_termius_import(store).expect("prepare termius import");

        assert_eq!(prepared.groups, vec![vec!["Production".to_string()]]);
        assert_eq!(prepared.ssh_keys.len(), 1);
        assert_eq!(prepared.passwords.len(), 2);
        assert_eq!(prepared.connections.len(), 2);

        let key_auth = prepared.connections[0].auth.as_ref().expect("key auth");
        assert_eq!(key_auth.mode, "key");
        assert!(key_auth.key_id.is_some());
        assert_eq!(
            prepared.connections[0].group_path,
            Some(vec!["Production".to_string()])
        );

        let password_auth = prepared.connections[1].auth.as_ref().expect("password auth");
        assert_eq!(password_auth.mode, "password");
        assert!(password_auth.password_id.is_some());
    }
}

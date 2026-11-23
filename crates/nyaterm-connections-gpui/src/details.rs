use nyaterm_core::SavedConnection;
use rust_i18n::t;
use std::collections::HashMap;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionDetailRow {
    pub label: String,
    pub value: String,
    pub copy_value: Option<String>,
    pub multiline: bool,
}

impl ConnectionDetailRow {
    fn new(label: String, value: String) -> Self {
        Self {
            label,
            value,
            copy_value: None,
            multiline: false,
        }
    }

    fn copyable(mut self, copy_value: Option<String>) -> Self {
        self.copy_value = copy_value;
        self
    }

    fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
}

pub fn connection_detail_rows(
    connection: &SavedConnection,
    all_connections: &HashMap<String, SavedConnection>,
) -> Vec<ConnectionDetailRow> {
    let description = connection
        .description
        .as_deref()
        .and_then(non_empty_detail_value)
        .unwrap_or_else(|| t!("savedConnections.noDescription").to_string());

    let mut rows = match &connection.config {
        nyaterm_core::ConnectionType::Spice {
            endpoint, username, ..
        } => vec![
            copyable_text_detail_row(
                t!("savedConnections.host").to_string(),
                endpoint.host.expose_secret(),
            ),
            copyable_value_detail_row(
                t!("spice.tcpPort").to_string(),
                endpoint.port.map(|p| p.to_string()).unwrap_or_default(),
            ),
            copyable_value_detail_row(
                t!("spice.tlsPort").to_string(),
                endpoint.tls_port.map(|p| p.to_string()).unwrap_or_default(),
            ),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Adb {
            device, directory, ..
        } => vec![
            copyable_value_detail_row("Android".into(), device.serial()),
            copyable_text_detail_row(t!("android.directory").into_owned(), directory),
        ],
        nyaterm_core::ConnectionType::Ssh {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::LocalTerminal {
            shell_path,
            shell_args,
            working_dir,
            ..
        } => {
            let mut rows = vec![ConnectionDetailRow::new(
                t!("savedConnections.terminalPath").to_string(),
                required_detail_value(shell_path),
            )];
            if let Some(shell_args) = non_empty_detail_value(shell_args) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.shellArgs").to_string(),
                    shell_args,
                ));
            }
            rows.push(ConnectionDetailRow::new(
                t!("savedConnections.workingDir").to_string(),
                working_dir
                    .as_deref()
                    .map(required_detail_value)
                    .unwrap_or_else(|| t!("savedConnections.notSet").to_string()),
            ));
            rows
        }
        nyaterm_core::ConnectionType::Telnet {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Serial {
            port_name,
            baud_rate,
            data_bits,
            parity,
            stop_bits,
            ..
        } => {
            let mut rows = vec![
                ConnectionDetailRow::new(
                    t!("savedConnections.serialPort").to_string(),
                    required_detail_value(port_name),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.baudRate").to_string(),
                    baud_rate.to_string(),
                ),
                ConnectionDetailRow::new(
                    t!("savedConnections.dataBits").to_string(),
                    data_bits.to_string(),
                ),
            ];
            if let Some(parity) = non_empty_detail_value(parity) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.parity").to_string(),
                    parity,
                ));
            }
            if let Some(stop_bits) = non_empty_detail_value(stop_bits) {
                rows.push(ConnectionDetailRow::new(
                    t!("savedConnections.stopBits").to_string(),
                    stop_bits,
                ));
            }
            rows
        }
        nyaterm_core::ConnectionType::Rdp {
            host,
            port,
            username,
            ..
        } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
        ],
        nyaterm_core::ConnectionType::Vnc { host, port, .. } => vec![
            copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
            copyable_value_detail_row(t!("savedConnections.port").to_string(), port.to_string()),
            ConnectionDetailRow::new(
                t!("savedConnections.user").to_string(),
                t!("savedConnections.notSet").to_string(),
            ),
        ],
        nyaterm_core::ConnectionType::Ftp {
            host,
            port,
            username,
            initial_directory,
            ..
        } => {
            let mut rows = vec![
                copyable_text_detail_row(t!("savedConnections.host").to_string(), host),
                copyable_value_detail_row(
                    t!("savedConnections.port").to_string(),
                    port.to_string(),
                ),
                copyable_text_detail_row(t!("savedConnections.user").to_string(), username),
            ];
            if let Some(directory) = non_empty_detail_value(initial_directory) {
                rows.push(copyable_text_detail_row(
                    t!("savedConnections.workingDir").to_string(),
                    &directory,
                ));
            }
            rows
        }
    };

    if matches!(&connection.config, nyaterm_core::ConnectionType::Ssh { .. })
        && connection
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.as_deref())
            .is_some()
    {
        rows.push(
            ConnectionDetailRow::new(
                t!("savedConnections.jumpHostChain").to_string(),
                format_jump_host_chain(connection, all_connections),
            )
            .multiline(),
        );
    }
    rows.push(ConnectionDetailRow::new(
        t!("savedConnections.description").to_string(),
        description,
    ));
    rows
}

/// Share the displayed summary without serializing credentials or extensions.
pub fn connection_share_text(connection: &SavedConnection, rows: &[ConnectionDetailRow]) -> String {
    let mut lines = vec![
        format!(
            "{}: {}",
            t!("savedConnections.connectionName"),
            connection.name
        ),
        format!(
            "{}: {}",
            t!("savedConnections.connectionType"),
            connection.kind_label()
        ),
    ];
    lines.extend(
        rows.iter()
            .map(|row| format!("{}: {}", row.label, row.value)),
    );
    lines.join("\n")
}

fn required_detail_value(value: &str) -> String {
    non_empty_detail_value(value).unwrap_or_else(|| t!("savedConnections.notSet").to_string())
}

fn copyable_text_detail_row(label: String, value: &str) -> ConnectionDetailRow {
    let copy_value = non_empty_detail_value(value);
    ConnectionDetailRow::new(label, required_detail_value(value)).copyable(copy_value)
}

fn copyable_value_detail_row(label: String, value: String) -> ConnectionDetailRow {
    ConnectionDetailRow::new(label, value.clone()).copyable(Some(value))
}

fn non_empty_detail_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub fn format_jump_host_chain(
    connection: &SavedConnection,
    by_id: &HashMap<String, SavedConnection>,
) -> String {
    let Some(mut jump_id) = connection
        .network
        .as_ref()
        .and_then(|network| network.proxy_jump_id.clone())
    else {
        return t!("savedConnections.notSet").to_string();
    };
    let mut seen = std::collections::HashSet::new();
    seen.insert(connection.id.clone());
    let mut labels = Vec::new();
    loop {
        if !seen.insert(jump_id.clone()) {
            labels.push(t!("savedConnections.jumpHostCycle").to_string());
            break;
        }
        let Some(jump) = by_id.get(jump_id.as_str()) else {
            labels.push(t!("savedConnections.jumpHostMissing").to_string());
            break;
        };
        labels.push(jump.name.clone());
        match jump
            .network
            .as_ref()
            .and_then(|network| network.proxy_jump_id.clone())
        {
            Some(next) => jump_id = next,
            None => break,
        }
    }
    if labels.is_empty() {
        t!("savedConnections.notSet").to_string()
    } else {
        labels.join(" -> ")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use nyaterm_core::SavedConnection;

    #[test]
    fn saved_connection_tooltip_and_share_use_protocol_fields_for_every_session_type() {
        let cases = [
            (
                serde_json::json!({
                    "type": "adb",
                    "device": { "kind": "usb", "serial": "test-device" },
                    "directory": "/sdcard"
                }),
                vec![
                    "Android",
                    "android.directory",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "ssh",
                    "host": " ssh.example.com ",
                    "port": 2222,
                    "username": " root "
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "local_terminal",
                    "shell_path": " pwsh.exe ",
                    "shell_args": " -NoLogo ",
                    "working_dir": " C:\\\\Work "
                }),
                vec![
                    "savedConnections.terminalPath",
                    "savedConnections.shellArgs",
                    "savedConnections.workingDir",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "telnet",
                    "host": "switch.example.com",
                    "port": 2323,
                    "username": "operator"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "serial",
                    "port_name": "COM3",
                    "baud_rate": 115200,
                    "data_bits": 8,
                    "parity": "none",
                    "stop_bits": "1"
                }),
                vec![
                    "savedConnections.serialPort",
                    "savedConnections.baudRate",
                    "savedConnections.dataBits",
                    "savedConnections.parity",
                    "savedConnections.stopBits",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "rdp",
                    "host": "desktop.example.com",
                    "port": 3390,
                    "username": "alice"
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
            (
                serde_json::json!({
                    "type": "vnc",
                    "host": "screen.example.com",
                    "port": 5901
                }),
                vec![
                    "savedConnections.host",
                    "savedConnections.port",
                    "savedConnections.user",
                    "savedConnections.description",
                ],
            ),
        ];

        for (config, expected_keys) in cases {
            let mut connection = connection("test", "Test", None, 0);
            connection.config = serde_json::from_value(config).expect("valid connection type");
            connection.description = Some(" production host ".to_string());

            let rows = super::connection_detail_rows(&connection, &HashMap::new());
            let labels = rows
                .iter()
                .map(|row| row.label.as_str())
                .collect::<Vec<_>>();
            let expected = expected_keys
                .into_iter()
                .map(|key| rust_i18n::t!(key).into_owned())
                .collect::<Vec<_>>();

            assert_eq!(
                labels,
                expected,
                "unexpected fields for {}",
                connection.kind_label()
            );
            assert_eq!(
                rows.last().map(|row| row.value.as_str()),
                Some("production host")
            );
            assert!(
                rows.last().expect("description row").copy_value.is_none(),
                "the description section shares the full summary instead of copying a field"
            );
            let expected_copy_count = match connection.kind_label() {
                "SSH" | "Telnet" | "RDP" => 3,
                "VNC" | "ADB" => 2,
                _ => 0,
            };
            assert_eq!(
                rows.iter().filter(|row| row.copy_value.is_some()).count(),
                expected_copy_count,
                "unexpected copy buttons for {}",
                connection.kind_label()
            );
            let summary = super::connection_share_text(&connection, &rows);
            assert!(summary.starts_with(&format!(
                "{}: Test\n{}: {}\n",
                rust_i18n::t!("savedConnections.connectionName"),
                rust_i18n::t!("savedConnections.connectionType"),
                connection.kind_label(),
            )));
            for row in &rows {
                assert!(summary.contains(&format!("{}: {}", row.label, row.value)));
            }
        }
    }

    #[test]
    fn saved_connection_tooltip_uses_tauri_missing_value_rules() {
        let mut local = connection("local", "Local", None, 0);
        local.config = serde_json::from_value(serde_json::json!({
            "type": "local_terminal",
            "shell_path": " ",
            "shell_args": " ",
            "working_dir": null
        }))
        .expect("valid local terminal");

        let local_rows = super::connection_detail_rows(&local, &HashMap::new());
        assert_eq!(local_rows.len(), 3, "blank shell args must be omitted");
        assert_eq!(
            local_rows[0].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[1].value,
            rust_i18n::t!("savedConnections.notSet").to_string()
        );
        assert_eq!(
            local_rows[2].value,
            rust_i18n::t!("savedConnections.noDescription").to_string()
        );

        assert!(
            local_rows.iter().all(|row| row.copy_value.is_none()),
            "Tauri does not offer copy buttons for local terminal fields"
        );

        let mut vnc = connection("vnc", "VNC", None, 0);
        vnc.config = serde_json::from_value(serde_json::json!({
            "type": "vnc",
            "host": "screen.example.com"
        }))
        .expect("valid VNC connection");
        let vnc_rows = super::connection_detail_rows(&vnc, &HashMap::new());
        assert_eq!(
            vnc_rows[2].value,
            rust_i18n::t!("savedConnections.notSet").to_string(),
            "Tauri renders the unavailable VNC username as not set"
        );
        assert_eq!(
            vnc_rows[0].copy_value.as_deref(),
            Some("screen.example.com")
        );
        assert_eq!(vnc_rows[1].copy_value.as_deref(), Some("5900"));
        assert_eq!(vnc_rows[2].copy_value, None);

        let mut ssh = connection("ssh", "SSH", None, 0);
        ssh.config = serde_json::from_value(serde_json::json!({
            "type": "ssh",
            "host": " ",
            "port": 22,
            "username": " "
        }))
        .expect("valid SSH connection");
        let ssh_rows = super::connection_detail_rows(&ssh, &HashMap::new());
        assert_eq!(ssh_rows[0].copy_value, None);
        assert_eq!(ssh_rows[1].copy_value.as_deref(), Some("22"));
        assert_eq!(ssh_rows[2].copy_value, None);
        let summary = super::connection_share_text(&local, &local_rows);
        assert!(summary.contains(&format!(
            "{}: {}",
            rust_i18n::t!("savedConnections.terminalPath"),
            rust_i18n::t!("savedConnections.notSet")
        )));
        assert!(summary.ends_with(&format!(
            "{}: {}",
            rust_i18n::t!("savedConnections.description"),
            rust_i18n::t!("savedConnections.noDescription")
        )));
    }

    #[test]
    fn saved_connection_share_preserves_full_values_and_multiline_description() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "name": "Production host",
            "type": "ssh",
            "host": " a-very-long-hostname-for-the-production-server.example.com ",
            "port": 2222,
            "username": " operator ",
            "description": " First line\nSecond line "
        }))
        .expect("valid connection");
        let rows = super::connection_detail_rows(&connection, &HashMap::new());
        assert_eq!(
            super::connection_share_text(&connection, &rows),
            format!(
                "{}: Production host\n{}: SSH\n{}: a-very-long-hostname-for-the-production-server.example.com\n{}: 2222\n{}: operator\n{}: First line\nSecond line",
                rust_i18n::t!("savedConnections.connectionName"),
                rust_i18n::t!("savedConnections.connectionType"),
                rust_i18n::t!("savedConnections.host"),
                rust_i18n::t!("savedConnections.port"),
                rust_i18n::t!("savedConnections.user"),
                rust_i18n::t!("savedConnections.description"),
            )
        );
    }

    #[test]
    fn saved_connection_share_excludes_credentials_and_opaque_fields() {
        let connection: SavedConnection = serde_json::from_value(serde_json::json!({
            "name": "Production host",
            "type": "ssh",
            "host": "example.com",
            "auth": {
                "password": "test-only-password",
                "account_id": "test-only-account",
                "key_id": "test-only-key",
                "otp_id": "test-only-otp",
                "future_secret": "test-only-auth-extension"
            },
            "network": { "proxy_id": "test-only-proxy" },
            "post_login": { "command": "test-only-command" },
            "future_secret": "test-only-top-level-extension"
        }))
        .expect("valid connection with opaque fields");
        let rows = super::connection_detail_rows(&connection, &HashMap::new());
        let summary = super::connection_share_text(&connection, &rows);
        assert!(!summary.contains("test-only-"));
        assert!(summary.contains("example.com"));
    }

    #[test]
    fn saved_connection_share_includes_jump_chain_and_missing_or_cyclic_markers() {
        let mut target: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "target", "name": "Target", "type": "ssh", "host": "target.example.com",
            "network": { "proxy_jump_id": "jump" }
        }))
        .expect("valid connection");
        let jump: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "jump", "name": "Bastion", "type": "ssh", "host": "jump.example.com",
            "network": { "proxy_jump_id": "outer" }
        }))
        .expect("valid jump");
        let outer: SavedConnection = serde_json::from_value(serde_json::json!({
            "id": "outer", "name": "Outer bastion", "type": "ssh", "host": "outer.example.com"
        }))
        .expect("valid outer jump");
        let mut by_id = HashMap::from([("jump".into(), jump), ("outer".into(), outer)]);
        let rows = super::connection_detail_rows(&target, &by_id);
        assert!(
            super::connection_share_text(&target, &rows).contains(&format!(
                "{}: Bastion -> Outer bastion",
                rust_i18n::t!("savedConnections.jumpHostChain")
            ))
        );

        by_id.remove("outer");
        let rows = super::connection_detail_rows(&target, &by_id);
        assert!(
            super::connection_share_text(&target, &rows).contains(&format!(
                "Bastion -> {}",
                rust_i18n::t!("savedConnections.jumpHostMissing")
            ))
        );

        target.id = "outer".into();
        let rows = super::connection_detail_rows(&target, &by_id);
        assert!(
            super::connection_share_text(&target, &rows).contains(&format!(
                "Bastion -> {}",
                rust_i18n::t!("savedConnections.jumpHostCycle")
            ))
        );
    }

    fn connection(
        id: &str,
        name: &str,
        group_id: Option<&str>,
        sort_order: i32,
    ) -> SavedConnection {
        SavedConnection {
            extensions: Default::default(),
            tags: Vec::new(),
            id: id.to_string(),
            name: name.to_string(),
            config: nyaterm_core::ConnectionType::LocalTerminal {
                shell_path: String::new(),
                shell_args: String::new(),
                working_dir: None,
                ai_execution_profile: nyaterm_core::AiExecutionProfile::Auto,
                encoding: String::new(),
                dynamic_tab_title: false,
            },
            group_id: group_id.map(ToOwned::to_owned),
            description: None,
            tree_order: None,
            sort_order,
            icon: None,
            icon_auto_detect: None,
            auth: None,
            recording: None,
            ssh_algorithms: None,
            allow_short_rsa_host_keys: false,
            ssh_compatibility_mode: false,
            ssh_profile: Default::default(),
            terminal_type: None,
            sftp: Default::default(),
            network: None,
            post_login: None,
            asset: None,
            created_at_ms: None,
            updated_at_ms: None,
            last_used_at_ms: None,
        }
    }
}
